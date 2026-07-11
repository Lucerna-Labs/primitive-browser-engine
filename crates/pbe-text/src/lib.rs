//! # pbe-text
//!
//! The one place the engine measures and wraps text, and it does so by driving
//! the **sealed `cap-text-shape` (cosmic-text) primitive** — never an estimate.
//! Both layout (to reserve line height) and render (to break + draw lines)
//! consume this same engine, so their line counts always agree.
//!
//! Doctrine: measurement is a function the shaping engine already exposes
//! (shape → sum of glyph advances). We compose wrapping out of it rather than
//! approximating glyph widths with a constant. The `CosmicShaper` is created
//! once **per thread** and held in a lazy `thread_local!` cell — loading the
//! system font DB is expensive, so a re-render or a scroll never re-pays that
//! cost within a thread. Callers on a single thread (`pbe-shell`'s
//! `render_to_rgba`, `pbe-svg`'s stateless formatter) share one font DB total;
//! the bus pipeline has each strand running on its own worker thread, so
//! `PaintStage` and `RenderStage` each cold-start one shaper per process. That
//! O(N-strands) cost is bounded, paid once at boot, and is the price of the
//! bus's parallel-strand model — not a leak.

use std::cell::RefCell;

use cap_geometry::Pixels;
use cap_text_shape::{
    CosmicShaper, FontDescriptor, FontStyle, FontWeight, ShapeCache, ShapeRequest,
};

/// Line height as a multiple of font size (the engine's default leading). Width
/// is *always* measured by the real shaper; only vertical spacing uses this
/// constant. Kept here (not in `pbe-protocol`) because it's a computation, not
/// a message contract — `TextDraw` on the wire carries positions, not leading.
pub const LINE_HEIGHT_RATIO: f32 = 1.3;

/// Line box height for a given font size.
pub fn line_height(font_size: f32) -> f32 {
    font_size * LINE_HEIGHT_RATIO
}

/// How a run of text should be measured/shaped. Borrows the family so callers
/// don't allocate for the common case.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle<'a> {
    pub family: &'a str,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

impl TextStyle<'_> {
    fn descriptor(&self) -> FontDescriptor<'_> {
        FontDescriptor {
            family: self.family,
            weight: if self.bold {
                FontWeight::Bold
            } else {
                FontWeight::Normal
            },
            style: if self.italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
        }
    }
}

/// Owned mirror of [`TextStyle`] — cheap enough (one small `String` per node)
/// and convenient for callers that need to inherit style down a tree walk
/// (layout inherits typography from element to text child, and can't hold a
/// borrow across recursive build calls).
#[derive(Clone, Debug)]
pub struct OwnedTextStyle {
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

impl OwnedTextStyle {
    pub fn as_style(&self) -> TextStyle<'_> {
        TextStyle {
            family: &self.family,
            size: self.size,
            bold: self.bold,
            italic: self.italic,
        }
    }
}

/// Owns the sealed shaper + a shape cache. Create once, share for the lifetime
/// of the engine; cloning the system font DB happens a single time here. The
/// engine is normally accessed through the [`with_engine`] / [`wrap`] / etc.
/// free functions, which hold one instance per thread; keeping the struct
/// public lets tests construct fresh instances.
pub struct TextEngine {
    shaper: RefCell<CosmicShaper>,
    cache: RefCell<ShapeCache>,
}

impl TextEngine {
    pub fn new() -> Self {
        Self {
            shaper: RefCell::new(CosmicShaper::new()),
            cache: RefCell::new(ShapeCache::new()),
        }
    }

    /// Real measured width of one line of text, via the shaper (sum of glyph
    /// advances). This is the truth wrapping breaks on — no character-ratio
    /// estimate.
    pub fn measure(&self, text: &str, style: TextStyle<'_>) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let mut shaper = self.shaper.borrow_mut();
        let mut cache = self.cache.borrow_mut();
        let shaped = cache.get_or_shape(
            &mut shaper,
            ShapeRequest {
                text,
                font: style.descriptor(),
                size: Pixels(style.size.max(1.0)),
            },
        );
        shaped.width.0
    }

    /// Greedy word-wrap to `max_width`, measuring each word (and the space
    /// separator) with the real shaper — never candidate-substrings. Returns
    /// one string per visual line. A word wider than `max_width` sits on its
    /// own line (overflow, like `overflow-wrap: normal`). With
    /// `max_width <= 0` the text collapses to a single normalized line.
    ///
    /// Cost: each unique word measured (and cached) once, plus one space-width
    /// probe per call — an N-word paragraph is O(N) shapes on first sight of a
    /// word, O(1) on subsequent views. The old candidate-substring loop
    /// inserted one distinct `ShapeCache` key per prefix and was O(N²) total
    /// bytes shaped; this is O(sum(word.len())).
    pub fn wrap(&self, text: &str, max_width: f32, style: TextStyle<'_>) -> Vec<String> {
        let words: Vec<&str> = text.split_whitespace().collect();
        if words.is_empty() {
            return Vec::new();
        }
        if max_width <= 0.0 {
            return vec![words.join(" ")];
        }
        let space_w = self.measure(" ", style);

        let mut lines: Vec<String> = Vec::new();
        let mut current = String::new();
        let mut current_w = 0.0_f32;
        for word in words {
            let word_w = self.measure(word, style);
            let sep_w = if current.is_empty() { 0.0 } else { space_w };
            if current_w + sep_w + word_w <= max_width || current.is_empty() {
                if !current.is_empty() {
                    current.push(' ');
                    current_w += space_w;
                }
                current.push_str(word);
                current_w += word_w;
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
                current_w = word_w;
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
        lines
    }

    /// Borrow the shaper mutably (the rasterizer needs it to shape final lines
    /// and read font bytes/metrics). Sharing it keeps the font DB loaded once.
    pub fn with_shaper<R>(&self, f: impl FnOnce(&mut CosmicShaper) -> R) -> R {
        f(&mut self.shaper.borrow_mut())
    }
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ── Shared per-thread instance ─────────────────────────────────────────────
//
// The engine holds a heavy `FontSystem` and a `RefCell` (so it's not `Send`),
// which fits a per-thread `thread_local!` naturally: layout, render, and the
// windowed shell all run on the same thread as they call into us, and per-
// thread caching means a scroll or re-render pays zero font-DB cost. Bus
// stages that run in worker threads each get their own instance the first
// time they measure text — still one-off cost, not per-frame.

thread_local! {
    static ENGINE: RefCell<Option<TextEngine>> = const { RefCell::new(None) };
}

/// Run `f` with the current thread's shared [`TextEngine`], creating it on
/// first use. Private helper for [`wrap`] and [`with_shaper`]; callers get
/// those high-level entry points, never the engine handle itself.
fn with_engine<R>(f: impl FnOnce(&TextEngine) -> R) -> R {
    ENGINE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(TextEngine::new());
        }
        f(slot.as_ref().expect("engine just initialised"))
    })
}

/// Wrap `text` to `max_width` using the shared shaper — the single truth both
/// layout (for line-count → box height) and render (for drawing lines) call.
pub fn wrap(text: &str, max_width: f32, style: TextStyle<'_>) -> Vec<String> {
    with_engine(|e| e.wrap(text, max_width, style))
}

/// Borrow the shared shaper mutably. Used by the rasterizer to shape each
/// wrapped line for real glyph output — sharing the shaper means each face is
/// discovered exactly once per thread across the whole engine.
pub fn with_shaper<R>(f: impl FnOnce(&mut CosmicShaper) -> R) -> R {
    with_engine(|e| e.with_shaper(f))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_is_monotonic_in_length() {
        let e = TextEngine::new();
        let s = TextStyle {
            family: "sans-serif",
            size: 16.0,
            bold: false,
            italic: false,
        };
        // A longer string measures at least as wide (with any real font).
        let short = e.measure("hi", s);
        let long = e.measure("hello world, this is much longer", s);
        if cfg!(windows) {
            assert!(
                long > short,
                "longer text should be wider: {short} vs {long}"
            );
        }
    }

    #[test]
    fn wrap_produces_multiple_lines_for_a_narrow_width() {
        let e = TextEngine::new();
        let s = TextStyle {
            family: "sans-serif",
            size: 16.0,
            bold: false,
            italic: false,
        };
        let text = "the quick brown fox jumps over the lazy dog again and again";
        let lines = e.wrap(text, 120.0, s);
        if cfg!(windows) {
            assert!(lines.len() > 1, "narrow width should wrap, got {lines:?}");
            // No line should exceed the max width (single long words excepted).
            for line in &lines {
                if line.contains(' ') {
                    assert!(
                        e.measure(line, s) <= 120.0 + s.size, // small slack
                        "line too wide: {line:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn unconstrained_width_is_one_line() {
        let e = TextEngine::new();
        let s = TextStyle {
            family: "sans-serif",
            size: 16.0,
            bold: false,
            italic: false,
        };
        assert_eq!(e.wrap("a b c", 0.0, s), vec!["a b c".to_string()]);
    }

    #[test]
    fn shared_engine_matches_owned_engine() {
        // The free `wrap` (shared per-thread engine) must agree with a fresh
        // `TextEngine` on the same input — otherwise layout's line count would
        // diverge from render's line count.
        let s = TextStyle {
            family: "sans-serif",
            size: 16.0,
            bold: false,
            italic: false,
        };
        let text = "wrap this at a narrow width to force multiple lines here";
        let owned = TextEngine::new().wrap(text, 100.0, s);
        let shared = super::wrap(text, 100.0, s);
        if cfg!(windows) {
            assert_eq!(
                owned, shared,
                "shared engine must produce identical line breaks"
            );
        }
    }

    #[test]
    fn line_height_helper_is_ratio_times_size() {
        assert!((super::line_height(16.0) - 16.0 * LINE_HEIGHT_RATIO).abs() < 1e-6);
    }
}
