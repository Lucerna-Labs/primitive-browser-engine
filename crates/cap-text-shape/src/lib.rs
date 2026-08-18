//! # cap-text-shape
//!
//! Renderer-neutral text shaping.
//!
//! Wraps [cosmic-text](https://crates.io/crates/cosmic-text) — the same
//! shaping engine Zed uses inside `gpui_wgpu` — and re-exposes its
//! output as **renderer-neutral glyph runs**: a list of `(font_id,
//! glyph_id, x_offset, y_offset, x_advance)` tuples. No `peniko`, no
//! `vello`, no `wgpu` dependencies — those bindings live in the
//! `ordo-ux-text` crate that consumes these runs.
//!
//! ## Why this split
//!
//! Shaping (text → glyph positions) and rasterisation (glyph → pixels)
//! are two separate problems. cosmic-text + harfbuzz handle the first;
//! vello, swash, and various atlas implementations handle the second.
//! This crate stops at the first boundary so any renderer can plug in
//! its own rasteriser.
//!
//! ## Architecture
//!
//! ```text
//! &str + FontDescriptor + size_px
//!         │
//!         ▼
//!    CosmicShaper (this crate)
//!         │
//!         ▼  Vec<ShapedGlyph>  — renderer-neutral
//!         │
//!         ▼
//!    ordo-ux-text → vello::Scene::draw_glyphs
//! ```
//!
//! ## Status
//!
//! Initial extraction. Covers shaping, font discovery, font-byte
//! retrieval, and glyph-metrics readback. Does NOT cover:
//!
//! - Bidirectional text (cosmic-text supports it, the API is a TODO)
//! - Line-breaking / wrapping (caller passes single-line input today)
//! - Subpixel positioning quantisation (the renderer side decides)
//! - Font fallback policy (cosmic-text picks; we expose what it picks)
//!
//! These are reachable with the same cosmic-text instance — just not
//! plumbed yet.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use cap_geometry::Pixels;
use cosmic_text::{Attrs, AttrsList, Family, FontSystem, ShapeBuffer, ShapeLine};

// ─── Public types ──────────────────────────────────────────────

/// Identifier for a loaded font, stable for the lifetime of the
/// [`CosmicShaper`] that issued it. Crates downstream (e.g.
/// `ordo-ux-text`) use this id to look up the underlying font bytes
/// once and cache the renderer-side handle (e.g. a `peniko::Font`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontId(pub usize);

/// Identifier for a glyph within a font. cosmic-text + harfbuzz
/// produce u16 glyph ids; we widen to u32 to leave headroom for
/// fonts with > 65k glyphs (rare, but possible with CJK + emoji).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphId(pub u32);

/// One positioned glyph in a shaped run. Coordinates are in
/// **logical pixels relative to the run origin**. The renderer
/// translates the run into world space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    /// Which font this glyph came from. Multiple fonts can appear in
    /// one run when the shaper falls through to fallbacks.
    pub font_id: FontId,
    /// Glyph index inside `font_id`'s glyph table.
    pub glyph_id: GlyphId,
    /// Horizontal pen advance after drawing this glyph, in pixels.
    /// Use this to compute the next glyph's x position.
    pub x_advance: Pixels,
    /// Per-glyph x offset from the pen position, in pixels. Usually
    /// zero; non-zero for diacritics / mark positioning.
    pub x_offset: Pixels,
    /// Per-glyph y offset from the baseline, in pixels. Negative is
    /// up (toward ascender). Usually zero; non-zero for marks.
    pub y_offset: Pixels,
    /// Byte index into the original `&str` this glyph cluster maps
    /// from. Useful for hit-testing back to source positions.
    pub source_byte_index: usize,
}

/// One contiguous run of glyphs sharing the same font. cosmic-text
/// can emit several runs for a single line when fallback fonts
/// kick in (e.g. emoji inside Latin text).
#[derive(Debug, Clone)]
pub struct ShapedRun {
    pub font_id: FontId,
    pub glyphs: Vec<ShapedGlyph>,
}

/// A single line of shaped text — what cosmic-text's `ShapeLine`
/// produces, normalised into our types.
#[derive(Debug, Clone)]
pub struct ShapedLine {
    /// One or more runs, in visual left-to-right order.
    pub runs: Vec<ShapedRun>,
    /// Total advance width of the line, in pixels. Sum of every
    /// glyph's `x_advance`.
    pub width: Pixels,
}

/// Caller-supplied request describing what to shape.
#[derive(Debug, Clone)]
pub struct ShapeRequest<'a> {
    pub text: &'a str,
    pub font: FontDescriptor<'a>,
    /// Font size in logical pixels (em-equivalent).
    pub size: Pixels,
}

/// What to ask cosmic-text to find. Loose match — cosmic-text's
/// font-system picks the best face given (family, weight, style).
#[derive(Debug, Clone)]
pub struct FontDescriptor<'a> {
    pub family: &'a str,
    pub weight: FontWeight,
    pub style: FontStyle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontWeight {
    Thin,
    Light,
    Normal,
    Medium,
    SemiBold,
    Bold,
    Black,
    /// Custom numeric weight (1..=1000).
    Custom(u16),
}

impl FontWeight {
    fn as_cosmic(self) -> cosmic_text::Weight {
        match self {
            FontWeight::Thin => cosmic_text::Weight::THIN,
            FontWeight::Light => cosmic_text::Weight::LIGHT,
            FontWeight::Normal => cosmic_text::Weight::NORMAL,
            FontWeight::Medium => cosmic_text::Weight::MEDIUM,
            FontWeight::SemiBold => cosmic_text::Weight::SEMIBOLD,
            FontWeight::Bold => cosmic_text::Weight::BOLD,
            FontWeight::Black => cosmic_text::Weight::BLACK,
            FontWeight::Custom(n) => cosmic_text::Weight(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}

impl FontStyle {
    fn as_cosmic(self) -> cosmic_text::Style {
        match self {
            FontStyle::Normal => cosmic_text::Style::Normal,
            FontStyle::Italic => cosmic_text::Style::Italic,
            FontStyle::Oblique => cosmic_text::Style::Oblique,
        }
    }
}

/// Coarse font metrics for a `FontId`, in pixels at the size the
/// shaper was queried with. `ordo-ux-text` uses these to position
/// the baseline and bound the line box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    /// Distance from baseline to the top of the tallest glyph,
    /// positive value.
    pub ascent: Pixels,
    /// Distance from baseline to the bottom of the deepest glyph,
    /// positive value (note: cosmic-text returns descent as
    /// positive; we keep that convention).
    pub descent: Pixels,
    /// Recommended extra line spacing on top of `ascent + descent`.
    pub line_gap: Pixels,
    /// Nominal x-height (lowercase letter height), in pixels.
    pub x_height: Pixels,
}

impl FontMetrics {
    /// Total line height: `ascent + descent + line_gap`.
    pub fn line_height(&self) -> Pixels {
        Pixels(self.ascent.0 + self.descent.0 + self.line_gap.0)
    }
}

// ─── Shaper ────────────────────────────────────────────────────

/// Shaping engine backed by cosmic-text. One instance owns a
/// `FontSystem` (the font database + loaded faces) and a reusable
/// `ShapeBuffer` (cosmic-text's scratch space). Cheap to keep around
/// for the lifetime of an app; expensive to recreate per frame.
///
/// Not `Send` because cosmic-text's `FontSystem` isn't `Send` on all
/// platforms. Wrap in your own mutex if cross-thread access is
/// needed (matching Zed's `RwLock<CosmicTextSystemState>` pattern).
pub struct CosmicShaper {
    font_system: FontSystem,
    scratch: ShapeBuffer,
    /// Stable mapping from cosmic-text's internal font db id to our
    /// `FontId`. Allocated lazily on first encounter.
    font_id_map: Vec<cosmic_text::fontdb::ID>,
}

impl CosmicShaper {
    /// Build a shaper with system fonts auto-discovered.
    pub fn new() -> Self {
        Self {
            font_system: FontSystem::new(),
            scratch: ShapeBuffer::default(),
            font_id_map: Vec::new(),
        }
    }

    /// Build a shaper with **no** system fonts. Use [`Self::add_font`]
    /// to load font byte buffers manually. Useful for deterministic
    /// tests + headless / locked-down environments.
    pub fn new_without_system_fonts() -> Self {
        Self {
            font_system: FontSystem::new_with_locale_and_db(
                "en-US".to_string(),
                cosmic_text::fontdb::Database::new(),
            ),
            scratch: ShapeBuffer::default(),
            font_id_map: Vec::new(),
        }
    }

    /// Register a font from raw bytes (TTF / OTF / etc).
    /// Returns the cosmic-text id; call [`Self::shape`] with a
    /// matching family name to use it.
    pub fn add_font(&mut self, bytes: Arc<Vec<u8>>) {
        self.font_system
            .db_mut()
            .load_font_source(cosmic_text::fontdb::Source::Binary(bytes));
    }

    /// Shape a single line of text against the given font + size.
    /// Multi-line input is currently treated as one logical line —
    /// caller is expected to pre-split on `\n`.
    pub fn shape(&mut self, request: ShapeRequest<'_>) -> ShapedLine {
        let attrs = Attrs::new()
            .family(Family::Name(request.font.family))
            .weight(request.font.weight.as_cosmic())
            .style(request.font.style.as_cosmic());
        let attrs_list = AttrsList::new(&attrs);

        // ShapeLine handles harfbuzz shaping + bidi + fallback.
        let shape_line = ShapeLine::new(
            &mut self.font_system,
            request.text,
            &attrs_list,
            cosmic_text::Shaping::Advanced,
            4, // tab width — irrelevant for single-line shaping
        );

        // Walk shaped spans → produce runs.
        let scale = request.size.0;
        let mut runs: Vec<ShapedRun> = Vec::new();
        let mut total_advance = 0.0_f32;

        for span in &shape_line.spans {
            for word in &span.words {
                for glyph in &word.glyphs {
                    let font_id = self.intern_font_id(glyph.font_id);
                    let shaped = ShapedGlyph {
                        font_id,
                        glyph_id: GlyphId(glyph.glyph_id as u32),
                        x_advance: Pixels(glyph.x_advance * scale),
                        x_offset: Pixels(glyph.x_offset * scale),
                        y_offset: Pixels(glyph.y_offset * scale),
                        source_byte_index: glyph.start,
                    };
                    total_advance += shaped.x_advance.0;

                    // Group consecutive glyphs sharing a font into one run.
                    if let Some(last) = runs.last_mut() {
                        if last.font_id == font_id {
                            last.glyphs.push(shaped);
                            continue;
                        }
                    }
                    runs.push(ShapedRun {
                        font_id,
                        glyphs: vec![shaped],
                    });
                }
            }
        }

        // Touch the scratch buffer so the field is acknowledged
        // even when cosmic-text doesn't drain it on this path.
        let _ = &mut self.scratch;

        ShapedLine {
            runs,
            width: Pixels(total_advance),
        }
    }

    /// Return font metrics for `font_id` at `size_px`. Reads them
    /// from swash (cosmic-text 0.17 exposes the face via
    /// `Font::as_swash()`) and scales them by
    /// `size_px / units_per_em`.
    ///
    /// `&mut self` because cosmic-text 0.17's `FontSystem::get_font`
    /// inserts into a font cache as a side effect.
    pub fn font_metrics(&mut self, font_id: FontId, size_px: Pixels) -> Option<FontMetrics> {
        let cosmic_id = *self.font_id_map.get(font_id.0)?;
        // Weight is part of the cache key. Metrics don't change with
        // weight (synthetic emboldening doesn't move the metrics); we
        // pass NORMAL so we share a cache slot regardless of the
        // weight the run was originally shaped with.
        let font = self
            .font_system
            .get_font(cosmic_id, cosmic_text::fontdb::Weight::NORMAL)?;
        let face = font.as_swash();
        let metrics = face.metrics(&[]);
        let units_per_em = metrics.units_per_em as f32;
        if units_per_em <= 0.0 {
            return None;
        }
        let scale = size_px.0 / units_per_em;
        Some(FontMetrics {
            ascent: Pixels(metrics.ascent * scale),
            descent: Pixels(metrics.descent * scale),
            line_gap: Pixels(metrics.leading * scale),
            x_height: Pixels(metrics.x_height * scale),
        })
    }

    /// Get the underlying font bytes for a `FontId` as an owned
    /// `Vec<u8>`. Renderers use this once per font to construct
    /// their own font handles (e.g. `peniko::Font::new(blob, ix)`)
    /// and then cache them — font byte buffers are typically
    /// 100 KB – 2 MB so the one-time clone is acceptable.
    ///
    /// Returns `None` if the font id is unknown.
    ///
    /// `&mut self` because cosmic-text 0.17's `get_font` writes to
    /// an internal cache; the data slice it returns borrows from
    /// the cached `Arc<Font>` (hence the clone — we can't return a
    /// reference safely without exposing the `Arc` in the API).
    pub fn font_bytes(&mut self, font_id: FontId) -> Option<Vec<u8>> {
        let cosmic_id = *self.font_id_map.get(font_id.0)?;
        let font = self
            .font_system
            .get_font(cosmic_id, cosmic_text::fontdb::Weight::NORMAL)?;
        Some(font.data().to_vec())
    }

    fn intern_font_id(&mut self, cosmic_id: cosmic_text::fontdb::ID) -> FontId {
        if let Some(idx) = self.font_id_map.iter().position(|&id| id == cosmic_id) {
            FontId(idx)
        } else {
            let idx = self.font_id_map.len();
            self.font_id_map.push(cosmic_id);
            FontId(idx)
        }
    }
}

impl Default for CosmicShaper {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Shape cache ───────────────────────────────────────────────

/// Cache key for [`ShapeCache`]. The (text, family, size, weight,
/// style) tuple uniquely identifies a shaping result for a given
/// `CosmicShaper` — assuming the shaper's font set hasn't changed
/// (we don't yet invalidate on `add_font`; callers wanting that
/// invalidation should call [`ShapeCache::clear`] after font load).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheKey {
    text: String,
    family: String,
    /// Size in pixels stored as raw f32 bits so the type stays
    /// `Hash + Eq`. Caller-supplied sizes are deterministic
    /// (composition computes them from style constants), so two
    /// shape requests with the same logical size produce the
    /// same bit pattern.
    size_bits: u32,
    weight: FontWeight,
    style: FontStyle,
}

/// Bounded cache of shaped lines, keyed by request inputs.
///
/// cosmic-text's shaper is fast but not free — each `shape()` call
/// runs harfbuzz + bidi + font fallback resolution. For an Ordo
/// shell that re-renders 60 frames/sec with mostly-identical text
/// (the conversation panel doesn't change every frame), the same
/// `(text, font, size)` shapes get re-computed many times per
/// second. This cache returns the previously-computed `ShapedLine`
/// when the inputs match.
///
/// **Bounded** via FIFO eviction — when the cache reaches
/// capacity, the oldest entry gets dropped. FIFO is simpler than
/// true LRU and good enough at the workloads we see (most
/// recently accessed strings are the actively-rendering ones).
///
/// Cache is per-shaper-instance — sharing across shapers with
/// different font sets would return stale glyph IDs. Each
/// `CosmicShaper` should have its own cache.
pub struct ShapeCache {
    entries: HashMap<CacheKey, ShapedLine>,
    fifo: VecDeque<CacheKey>,
    capacity: usize,
}

impl ShapeCache {
    /// Default cache size — 256 entries. At ~200 bytes per
    /// `ShapedLine` (small string + a handful of glyphs), that's
    /// ~50 KB resident. Tunable via [`Self::with_capacity`].
    pub const DEFAULT_CAPACITY: usize = 256;

    pub fn new() -> Self {
        Self::with_capacity(Self::DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity.min(64)),
            fifo: VecDeque::with_capacity(capacity.min(64)),
            capacity: capacity.max(1),
        }
    }

    /// Number of shaped lines currently cached.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Drop every cached entry. Call after font additions / theme
    /// changes that might affect shaping.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.fifo.clear();
    }

    /// Get-or-shape: return the cached `ShapedLine` for the given
    /// request, shaping fresh and inserting into the cache on a
    /// miss. Borrow lives for the cache's lifetime.
    pub fn get_or_shape(
        &mut self,
        shaper: &mut CosmicShaper,
        request: ShapeRequest<'_>,
    ) -> &ShapedLine {
        let key = CacheKey {
            text: request.text.to_string(),
            family: request.font.family.to_string(),
            size_bits: request.size.0.to_bits(),
            weight: request.font.weight,
            style: request.font.style,
        };
        if !self.entries.contains_key(&key) {
            let shaped = shaper.shape(request);
            self.insert(key.clone(), shaped);
        }
        self.entries.get(&key).expect("just inserted")
    }

    fn insert(&mut self, key: CacheKey, value: ShapedLine) {
        if self.entries.len() >= self.capacity {
            // Evict oldest. FIFO eviction — first inserted goes
            // first. For cache-warming patterns this is fine; if
            // we ever hit a workload where it's wasteful (e.g. a
            // big batch of shapes at startup that pushes out the
            // hot ones), bump capacity or switch to LRU.
            if let Some(old_key) = self.fifo.pop_front() {
                self.entries.remove(&old_key);
            }
        }
        self.fifo.push_back(key.clone());
        self.entries.insert(key, value);
    }
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ShapeCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShapeCache")
            .field("entries", &self.entries.len())
            .field("capacity", &self.capacity)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    // Tuffy is a public-domain TrueType font (Thatcher Ulrich et al., see
    // tests/fonts/Tuffy-LICENSE.txt) vendored here so the shaping tests are
    // deterministic and never depend on the host having system fonts
    // installed. cosmic-text panics with "no default font found" inside
    // ShapeLine when its FontSystem has zero registered faces, which is
    // exactly what a fresh CI / sandbox container provides.
    // `new_without_system_fonts()` + `add_font()` is the documented path for
    // this; see the headless constructor's rustdoc above.
    const TUFFY_TTF: &[u8] = include_bytes!("../tests/fonts/Tuffy.ttf");
    const TUFFY_FAMILY: &str = "Tuffy";

    fn test_shaper() -> CosmicShaper {
        let mut shaper = CosmicShaper::new_without_system_fonts();
        shaper.add_font(Arc::new(TUFFY_TTF.to_vec()));
        shaper
    }

    /// Default constructor produces a usable shaper (system fonts
    /// available on the host).
    #[test]
    fn shaper_constructs() {
        let _ = CosmicShaper::new();
    }

    /// Headless constructor exists. We do NOT shape against it in a
    /// unit test — cosmic-text panics if no fonts are registered at
    /// all, so a valid headless shaper requires `add_font` first.
    /// Test just verifies construction.
    #[test]
    fn headless_shaper_constructs() {
        let _ = CosmicShaper::new_without_system_fonts();
    }

    /// Shape a simple ASCII string against the vendored Tuffy face.
    /// Uses the headless shaper so the result is identical on every
    /// host (CI container or desktop). "hi" shapes to exactly two
    /// glyphs; the total advance equals the sum of glyph advances.
    #[test]
    fn shape_simple_string_against_system_fonts() {
        let mut shaper = test_shaper();
        let line = shaper.shape(ShapeRequest {
            text: "hi",
            font: FontDescriptor {
                family: TUFFY_FAMILY,
                weight: FontWeight::Normal,
                style: FontStyle::Normal,
            },
            size: Pixels(16.0),
        });
        let total: f32 = line
            .runs
            .iter()
            .flat_map(|r| r.glyphs.iter())
            .map(|g| g.x_advance.0)
            .sum();
        assert!((total - line.width.0).abs() < 0.001);
        // "hi" is two code points, both present in Tuffy -> two glyphs,
        // one run (single font, no fallback needed).
        let glyphs: Vec<_> = line.runs.iter().flat_map(|r| r.glyphs.iter()).collect();
        assert_eq!(glyphs.len(), 2, "expected exactly two shaped glyphs");
        assert!(!line.runs.is_empty(), "expected at least one shaped run");
    }

    /// FontMetrics::line_height adds ascent + descent + gap.
    #[test]
    fn line_height_is_sum_of_components() {
        let m = FontMetrics {
            ascent: Pixels(10.0),
            descent: Pixels(3.0),
            line_gap: Pixels(2.0),
            x_height: Pixels(7.0),
        };
        assert_eq!(m.line_height().0, 15.0);
    }

    /// FontWeight maps to cosmic-text's named weights without panic.
    #[test]
    fn font_weight_maps_to_cosmic() {
        for w in [
            FontWeight::Thin,
            FontWeight::Light,
            FontWeight::Normal,
            FontWeight::Medium,
            FontWeight::SemiBold,
            FontWeight::Bold,
            FontWeight::Black,
            FontWeight::Custom(450),
        ] {
            let _ = w.as_cosmic();
        }
    }

    /// FontStyle maps to cosmic-text's styles without panic.
    #[test]
    fn font_style_maps_to_cosmic() {
        for s in [FontStyle::Normal, FontStyle::Italic, FontStyle::Oblique] {
            let _ = s.as_cosmic();
        }
    }

    /// Empty shape cache reports empty.
    #[test]
    fn shape_cache_starts_empty() {
        let c = ShapeCache::new();
        assert!(c.is_empty());
        assert_eq!(c.len(), 0);
    }

    /// Cache hit: a second `get_or_shape` for the same request
    /// doesn't grow the entry count.
    #[test]
    fn shape_cache_dedups_repeated_requests() {
        let mut shaper = test_shaper();
        let mut cache = ShapeCache::new();
        let request = || ShapeRequest {
            text: "hi",
            font: FontDescriptor {
                family: TUFFY_FAMILY,
                weight: FontWeight::Normal,
                style: FontStyle::Normal,
            },
            size: Pixels(16.0),
        };
        cache.get_or_shape(&mut shaper, request());
        let after_first = cache.len();
        cache.get_or_shape(&mut shaper, request());
        cache.get_or_shape(&mut shaper, request());
        let after_three = cache.len();
        assert_eq!(after_first, 1);
        assert_eq!(after_three, 1, "repeated identical requests should hit");
    }

    /// Cache distinguishes by all key fields: text, family, size,
    /// weight, style.
    #[test]
    fn shape_cache_keys_on_all_request_fields() {
        let mut shaper = test_shaper();
        let mut cache = ShapeCache::new();
        cache.get_or_shape(
            &mut shaper,
            ShapeRequest {
                text: "a",
                font: FontDescriptor {
                    family: TUFFY_FAMILY,
                    weight: FontWeight::Normal,
                    style: FontStyle::Normal,
                },
                size: Pixels(14.0),
            },
        );
        cache.get_or_shape(
            &mut shaper,
            ShapeRequest {
                text: "b", // different text
                font: FontDescriptor {
                    family: TUFFY_FAMILY,
                    weight: FontWeight::Normal,
                    style: FontStyle::Normal,
                },
                size: Pixels(14.0),
            },
        );
        cache.get_or_shape(
            &mut shaper,
            ShapeRequest {
                text: "a",
                font: FontDescriptor {
                    family: TUFFY_FAMILY,
                    weight: FontWeight::Bold, // different weight
                    style: FontStyle::Normal,
                },
                size: Pixels(14.0),
            },
        );
        cache.get_or_shape(
            &mut shaper,
            ShapeRequest {
                text: "a",
                font: FontDescriptor {
                    family: TUFFY_FAMILY,
                    weight: FontWeight::Normal,
                    style: FontStyle::Normal,
                },
                size: Pixels(18.0), // different size
            },
        );
        assert_eq!(cache.len(), 4);
    }

    /// Cache evicts oldest entries past capacity.
    #[test]
    fn shape_cache_evicts_at_capacity() {
        let mut shaper = test_shaper();
        let mut cache = ShapeCache::with_capacity(3);
        for n in 0..5 {
            let s = format!("text-{n}");
            cache.get_or_shape(
                &mut shaper,
                ShapeRequest {
                    text: &s,
                    font: FontDescriptor {
                        family: TUFFY_FAMILY,
                        weight: FontWeight::Normal,
                        style: FontStyle::Normal,
                    },
                    size: Pixels(14.0),
                },
            );
        }
        // 5 inserts, capacity 3 → exactly 3 entries remain.
        assert_eq!(cache.len(), 3);
    }

    /// Cache `clear()` empties everything.
    #[test]
    fn shape_cache_clear_resets_state() {
        let mut shaper = test_shaper();
        let mut cache = ShapeCache::new();
        cache.get_or_shape(
            &mut shaper,
            ShapeRequest {
                text: "anything",
                font: FontDescriptor {
                    family: TUFFY_FAMILY,
                    weight: FontWeight::Normal,
                    style: FontStyle::Normal,
                },
                size: Pixels(14.0),
            },
        );
        assert!(!cache.is_empty());
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }
}
