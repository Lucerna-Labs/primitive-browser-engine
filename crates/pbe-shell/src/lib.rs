//! The primitive browser: an address bar, Back/Forward/Reload buttons, and
//! the loaded page in a scrollable region — composed entirely from
//! `pmre-orchestrator`'s own interactive-UI system (`UiState`/`handle_event`/
//! `render_ui`, the same primitives its calculator/todo demo apps use) plus
//! `pmre-kit`'s HTML reducer for the loaded page's content.
//!
//! There is no custom scroll math here: `Style::scroll` already tracks
//! content height and clamps the offset (`UiState.scrolls`), so this crate
//! only supplies navigation (what HTML is currently loaded, and history) and
//! chrome (the buttons/input around it) — the two things `pmre-kit` doesn't
//! have an opinion on.

use pmre_kit::raster::{decode_bmp, decode_png, Image};
use pmre_kit::ux::{Align, Dim, Edges, Justify, Style, UxNode};
use pmre_kit::{Framebuffer, Rgba};
use pmre_orchestrator::UiState;
use std::collections::HashMap;
use std::sync::Arc;

pub use pmre_orchestrator::{handle_event, render_ui, render_ui_quality, Quality, UiEvent};

const ADDRESS_INPUT: u32 = 1;
const BACK_BTN: u32 = 2;
const FORWARD_BTN: u32 = 3;
const RELOAD_BTN: u32 = 4;
const PAGE_SCROLL: u32 = 99;

const CHROME_H: f32 = 44.0;
const CHROME_BG: Rgba = Rgba::new(0.11, 0.12, 0.14, 1.0);
const BTN_BG: Rgba = Rgba::new(0.27, 0.30, 0.36, 1.0);
const BTN_BG_DISABLED: Rgba = Rgba::new(0.16, 0.16, 0.20, 1.0);
const BTN_FG: Rgba = Rgba::new(0.93, 0.94, 0.97, 1.0);
const BTN_FG_DISABLED: Rgba = Rgba::new(0.43, 0.45, 0.48, 1.0);
const INPUT_BG: Rgba = Rgba::new(0.08, 0.09, 0.11, 1.0);
const INPUT_BORDER: Rgba = Rgba::new(0.19, 0.20, 0.26, 1.0);
const FOCUS_RING: Rgba = Rgba::new(0.38, 0.65, 0.98, 1.0);

/// Opaque white page background.
pub const PAGE_BG: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);

/// Where a loaded page came from, so `reload`/history can refetch it.
#[derive(Clone)]
enum Origin {
    File(String),
    Url(String),
    /// In-memory HTML, not backed by a file or URL — used by tests/examples.
    Html(String),
}

fn classify(address: &str) -> Origin {
    // The modern protocols the engine speaks: http(s), ws(s), and data:.
    // Each is routed through pbe_net (which dispatches to the matching
    // pbe-proto-* crate). Anything else is a local file path.
    if is_network_scheme(address) {
        Origin::Url(address.to_string())
    } else {
        Origin::File(address.to_string())
    }
}

/// Whether `address` begins with one of the modern fetch protocols the
/// modular protocol layer routes (http/https/ws/wss/data). Everything else
/// is treated as a local file path.
fn is_network_scheme(address: &str) -> bool {
    let a = address.to_ascii_lowercase();
    a.starts_with("http://")
        || a.starts_with("https://")
        || a.starts_with("ws://")
        || a.starts_with("wss://")
        || a.starts_with("data:")
}

fn load(origin: &Origin) -> (String, UxNode) {
    let (label, html) = match origin {
        Origin::File(path) => {
            let html = std::fs::read_to_string(path)
                .unwrap_or_else(|e| error_html(&format!("cannot read {path}: {e}")));
            (path.clone(), html)
        }
        Origin::Url(url) => match pbe_net::fetch(url) {
            Ok(page) => (page.final_url, page.body),
            Err(e) => (url.clone(), error_html(&format!("fetch failed: {e:?}"))),
        },
        Origin::Html(html) => ("about:blank".to_string(), html.clone()),
    };
    // Compose external stylesheets in from outside: scan the HTML source for
    // <link rel=stylesheet> tags (the atom `scan` specialised to HTML), fetch
    // each one (pbe_net for URLs, fs for local paths), fold the results into a
    // single <style> block prepended to the source. The kit's html::parse
    // already reads <style> blocks — this composition adds external
    // stylesheets with zero kit change, using only capabilities already there.
    let augmented = inject_external_stylesheets(&label, &html);
    // Compose external images the same way: scan the augmented HTML for
    // <img src="…">, fetch + decode each (BMP or PNG per magic bytes), hand
    // the pre-decoded map to the kit's parse_with_images. The kit itself
    // never fetches or decodes anything the browser hasn't already given it —
    // the composition boundary stays at the browser layer.
    let images = fetch_page_images(&label, &augmented);
    (
        label,
        pmre_kit::html::parse_with_images(&augmented, &images),
    )
}

/// Scan `html` for `<img src="…">` tags, fetch and decode each into an
/// `Arc<Image>` map keyed by the original src string (so the kit can look up
/// each `<img>` at parse time). URL srcs go through `pbe_net::fetch`; local
/// paths through `std::fs::read`. Per-image failures are non-fatal — an
/// undecodable or unreachable image just gets skipped, and `parse_with_images`
/// drops the corresponding `<img>` tag from the render, matching the real-
/// browser "broken image → missing" behaviour we already have for missing
/// stylesheets. `Arc` so a page with the same `src` repeated across many
/// `<img>` tags decodes once and reuses.
fn fetch_page_images(base: &str, html: &str) -> HashMap<String, Arc<Image>> {
    let srcs = find_img_srcs(html);
    let mut out: HashMap<String, Arc<Image>> = HashMap::new();
    for src in srcs {
        if out.contains_key(&src) {
            continue; // decoded already this page — reuse the Arc
        }
        let resolved = resolve_href(base, &src);
        let Some(bytes) = fetch_image_bytes(&resolved) else {
            continue;
        };
        let Some(image) = decode_image_bytes(&bytes) else {
            continue;
        };
        out.insert(src, Arc::new(image));
    }
    out
}

/// Fetch raw image bytes over the on-ramps the browser already uses:
/// `pbe_net::fetch_bytes` for URLs (binary-safe — never UTF-8-lossies the
/// response, so PNG/BMP/JPEG streams survive intact), `std::fs::read` for
/// local paths. `fetch_bytes` is the sibling of `fetch` added to pbe-net
/// specifically to close the "URL image round-trips through String and
/// gets its bytes replaced with U+FFFD" defect that the first cut of img
/// support shipped with.
fn fetch_image_bytes(target: &str) -> Option<Vec<u8>> {
    if is_network_scheme(target) {
        pbe_net::fetch_bytes(target).ok().map(|p| p.body)
    } else {
        std::fs::read(target).ok()
    }
}

/// Try BMP and PNG decoders in turn on the given bytes. Returns `None` if
/// neither format matches — future decoders (JPEG, WebP, GIF) would join the
/// same dispatch here.
fn decode_image_bytes(bytes: &[u8]) -> Option<Image> {
    if bytes.starts_with(b"BM") {
        decode_bmp(bytes)
    } else if bytes.len() >= 8 && bytes[0..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
        decode_png(bytes)
    } else {
        None
    }
}

/// Scan `html` for `<img src="…">` tags and return the src attribute values
/// in source order. Same primitive-composition pattern as
/// `find_stylesheet_hrefs`: the atom `scan` specialised to HTML, tolerant of
/// attribute order and single/double/unquoted values.
fn find_img_srcs(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = lower[i..].find("<img").map(|k| i + k) {
        let tag_start = rel;
        let Some(close_rel) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + close_rel;
        // Original-case slice for value extraction so URLs keep their case.
        let tag_slice = &html[tag_start..tag_end];
        if let Some(src) = attr_value(tag_slice, "src") {
            out.push(src);
        }
        i = tag_end + 1;
    }
    out
}

/// For each `<link rel="stylesheet" href="...">` in `html`, fetch the target
/// (relative to `base`) and inject its contents as a synthetic `<style>` block
/// at the top of the returned document. Failures per-link are non-fatal —
/// unreachable stylesheets are silently skipped, matching real-browser
/// behaviour (a missing sheet renders unstyled rather than aborting the page).
fn inject_external_stylesheets(base: &str, html: &str) -> String {
    let hrefs = find_stylesheet_hrefs(html);
    if hrefs.is_empty() {
        return html.to_string();
    }
    let mut combined = String::new();
    for href in &hrefs {
        let resolved = resolve_href(base, href);
        if let Some(text) = fetch_stylesheet_text(&resolved) {
            combined.push_str(&text);
            combined.push('\n');
        }
    }
    if combined.is_empty() {
        html.to_string()
    } else {
        format!("<style>{combined}</style>{html}")
    }
}

/// Fetch stylesheet text over the same on-ramps the browser already uses:
/// `pbe_net::fetch` for URLs, `std::fs` for local paths. Returns `None` on
/// any failure — the caller treats missing sheets as no styling, not fatal.
fn fetch_stylesheet_text(target: &str) -> Option<String> {
    if is_network_scheme(target) {
        pbe_net::fetch(target).ok().map(|p| p.body)
    } else {
        std::fs::read_to_string(target).ok()
    }
}

/// Scan HTML source for `<link rel="stylesheet" href="…">` tags and return
/// their hrefs in source order. Deliberately simple — attribute order is
/// free, single- or double-quoted values both work, `type="text/css"` and
/// other extra attributes are ignored, and a `<link>` without `rel=stylesheet`
/// is skipped. Not a full HTML tokenizer; the kit has that internally. This
/// is the atom `scan` specialised to enough of HTML to spot the tags external
/// CSS travels through, and no more.
fn find_stylesheet_hrefs(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut i = 0usize;
    let bytes = lower.as_bytes();
    while let Some(rel) = lower[i..].find("<link").map(|k| i + k) {
        let tag_start = rel;
        let Some(close_rel) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + close_rel;
        // Use the original-case slice for value extraction so hrefs preserve
        // case; use the lowercased view for the rel= keyword check.
        let tag_slice_lc = &lower[tag_start..tag_end];
        let tag_slice = &html[tag_start..tag_end];
        if attr_equals(tag_slice_lc, "rel", "stylesheet") {
            if let Some(href) = attr_value(tag_slice, "href") {
                out.push(href);
            }
        }
        i = tag_end + 1;
        if i >= bytes.len() {
            break;
        }
    }
    out
}

/// Case-insensitive `attr="value"` / `attr='value'` / `attr=value` check on
/// the interior of a single tag (already lowercased).
fn attr_equals(tag_lc: &str, name: &str, want_lc: &str) -> bool {
    attr_value(tag_lc, name)
        .map(|v| v.eq_ignore_ascii_case(want_lc))
        .unwrap_or(false)
}

/// Extract the value of `name=…` from the interior of a single tag. Accepts
/// double-quoted, single-quoted, or unquoted values; returns the original
/// substring (case preserved). `None` if the attribute is absent or malformed.
fn attr_value(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let mut search_from = 0;
    loop {
        let rel = lower[search_from..].find(name)?;
        let start = search_from + rel;
        // Reject partial matches inside another attribute name (e.g. `href`
        // matched inside `xhref`).
        let before_ok = start == 0
            || matches!(
                lower.as_bytes()[start - 1],
                b' ' | b'\t' | b'\n' | b'\r' | b'/'
            );
        let after = start + name.len();
        if !before_ok || after >= lower.len() {
            search_from = after;
            continue;
        }
        // Skip whitespace before `=`.
        let mut j = after;
        while j < lower.len() && matches!(lower.as_bytes()[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j >= lower.len() || lower.as_bytes()[j] != b'=' {
            search_from = after;
            continue;
        }
        j += 1;
        while j < lower.len() && matches!(lower.as_bytes()[j], b' ' | b'\t' | b'\n' | b'\r') {
            j += 1;
        }
        if j >= lower.len() {
            return None;
        }
        let quote = lower.as_bytes()[j];
        let (vstart, vend) = if quote == b'"' || quote == b'\'' {
            let vs = j + 1;
            let ve = tag[vs..].find(quote as char).map(|k| vs + k)?;
            (vs, ve)
        } else {
            let vs = j;
            let ve = tag[vs..]
                .find(|c: char| c.is_ascii_whitespace() || c == '/')
                .map(|k| vs + k)
                .unwrap_or(tag.len());
            (vs, ve)
        };
        return Some(tag[vstart..vend].to_string());
    }
}

/// A minimal inline-styled error document so failures are visible in-window.
/// `pmre-kit`'s HTML reducer only reads inline `style="..."` attributes.
fn error_html(msg: &str) -> String {
    format!(
        "<div style=\"background:#fff3f3\"><h1 style=\"color:#b00020\">Load error</h1><p style=\"color:#333333\">{msg}</p></div>"
    )
}

/// Resolve a clicked `<a href>` against the page it was clicked on. A
/// deliberately small, purpose-built resolver — not a full RFC 3986
/// implementation (no `..` segment collapsing, no query/fragment
/// special-casing) — handling the common cases real site navigation needs:
/// absolute URLs pass through untouched, a root-relative `href` (`/path`)
/// replaces the current URL's path, anything else joins against the current
/// page's own directory. `pbe-net` deliberately links no URL-parsing crate,
/// so this stays a plain string primitive rather than pulling one in for a
/// handful of cases.
fn resolve_href(base: &str, href: &str) -> String {
    // Absolute URLs of any modern protocol pass through untouched (http(s),
    // ws(s), data:). Relative hrefs resolve against the base below.
    if is_network_scheme(href) {
        return href.to_string();
    }
    if let Some(scheme_end) = base.find("://") {
        // base is a URL: resolve against its origin/directory.
        let after_scheme = scheme_end + 3;
        // Where the path starts — end of the string if there's no path at all
        // (e.g. `https://example.com`, a bare origin).
        let host_end = base[after_scheme..]
            .find('/')
            .map(|i| after_scheme + i)
            .unwrap_or(base.len());
        if href.starts_with('/') {
            return format!("{}{href}", &base[..host_end]);
        }
        // Same-directory join: search for the last '/' within the path
        // portion only (never inside "https://"'s own slashes).
        let path = &base[host_end..];
        let dir_end = host_end + path.rfind('/').map(|i| i + 1).unwrap_or(0);
        let dir = &base[..dir_end];
        return if dir.ends_with('/') {
            format!("{dir}{href}")
        } else {
            format!("{dir}/{href}")
        };
    }
    // base is a local file path: resolve relative to its directory, unless
    // href already looks absolute (root path or has its own scheme).
    if href.starts_with('/') || href.contains("://") {
        return href.to_string();
    }
    match std::path::Path::new(base).parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(href).to_string_lossy().to_string(),
        _ => href.to_string(),
    }
}

/// The browser: navigation state, the loaded page, and the interactive-UI
/// state driving the chrome + scroll region. Pure data — no window, no event
/// loop; `pbe-window`'s winit shell (and this crate's example) drive it.
pub struct Browser {
    page_root: UxNode,
    label: String,
    history: Vec<Origin>,
    history_pos: usize,
    /// `pub` so callers can set `.scale` on window creation / DPI change
    /// without a wrapper method — same fields `pmre-orchestrator`'s own
    /// examples mutate directly.
    pub ui: UiState,
}

impl Browser {
    /// Open a browser window on a starting address (a local path or an
    /// `http(s)://` URL).
    pub fn open(address: &str, width: u32, height: u32) -> Self {
        Self::from_origin(classify(address), width, height)
    }

    /// Open a browser window on an in-memory HTML string, not backed by a
    /// file or URL. For tests and examples that don't want a real fetch/file.
    pub fn open_html(html: &str, width: u32, height: u32) -> Self {
        Self::from_origin(Origin::Html(html.to_string()), width, height)
    }

    fn from_origin(origin: Origin, width: u32, height: u32) -> Self {
        let (label, page_root) = load(&origin);
        let mut ui = UiState::new(width, height);
        ui.inputs.insert(ADDRESS_INPUT, label.clone());
        Self {
            page_root,
            label,
            history: vec![origin],
            history_pos: 0,
            ui,
        }
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn can_go_back(&self) -> bool {
        self.history_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_pos + 1 < self.history.len()
    }

    pub fn go_back(&mut self) {
        if self.can_go_back() {
            self.history_pos -= 1;
            self.load_current();
        }
    }

    pub fn go_forward(&mut self) {
        if self.can_go_forward() {
            self.history_pos += 1;
            self.load_current();
        }
    }

    pub fn reload(&mut self) {
        self.load_current();
    }

    /// Navigate to a new address (typed into the bar or otherwise supplied).
    /// Truncates any forward history past the current point, like a real
    /// browser.
    pub fn navigate(&mut self, address: &str) {
        self.history.truncate(self.history_pos + 1);
        self.history.push(classify(address));
        self.history_pos = self.history.len() - 1;
        self.load_current();
    }

    fn load_current(&mut self) {
        let (label, root) = load(&self.history[self.history_pos]);
        self.label = label;
        self.page_root = root;
        self.ui.scrolls.remove(&PAGE_SCROLL);
        self.ui.inputs.insert(ADDRESS_INPUT, self.label.clone());
    }

    /// Feed one UI event through the chrome + page tree, then apply any
    /// resulting navigation (Back/Forward/Reload click, or the address bar
    /// submitted via Enter).
    pub fn dispatch(&mut self, ev: UiEvent) {
        let can_back = self.can_go_back();
        let can_fwd = self.can_go_forward();
        let was_address_focused = self.ui.focused == Some(ADDRESS_INPUT);
        // `handle_event` takes `ev` by value; capture what we need from it
        // up front since it won't be available after the call below.
        let pointer_up_at = match ev {
            UiEvent::PointerUp(x, y) => Some((x, y)),
            _ => None,
        };
        {
            let label = &self.label;
            let page_root = &self.page_root;
            let b = |s: &UiState| build_tree(s, label, can_back, can_fwd, page_root);
            handle_event(&mut self.ui, &b, ev);
        }
        // A fresh click into the address bar starts a clean edit, like a real
        // browser selecting the whole URL on focus — `UiEvent::Char` just
        // appends to whatever's already in `ui.inputs`, so without this the
        // first keystroke would concatenate onto the current URL instead of
        // replacing it.
        if self.ui.focused == Some(ADDRESS_INPUT) && !was_address_focused {
            self.ui.clear_input(ADDRESS_INPUT);
        }
        if let Some(id) = self.ui.take_click() {
            match id {
                BACK_BTN => self.go_back(),
                FORWARD_BTN => self.go_forward(),
                RELOAD_BTN => self.reload(),
                _ => {}
            }
        }
        if self.ui.take_submit() == Some(ADDRESS_INPUT) {
            let addr = self.ui.input_text(ADDRESS_INPUT).to_string();
            self.navigate(&addr);
        }
        // A completed click (mouse released) might have landed on a rendered
        // `<a href>` inside the page content — those aren't `Role`-tagged
        // boxes `handle_event` already hit-tests (a whole paragraph is one
        // box; only one wrapped word inside it is the link), so this is a
        // second, separate hit-test pass over the same tree specifically for
        // link text.
        if let Some((x, y)) = pointer_up_at {
            if let Some(href) = self.hit_test_link_at(x, y) {
                self.navigate(&resolve_href(&self.label, &href));
            }
        }
    }

    /// Hit-test the composed tree at a point for a rendered `<a href>`,
    /// returning its (unresolved) href if one covers that point.
    fn hit_test_link_at(&self, x: f32, y: f32) -> Option<String> {
        let can_back = self.can_go_back();
        let can_fwd = self.can_go_forward();
        let label = &self.label;
        let page_root = &self.page_root;
        let tree = build_tree(&self.ui, label, can_back, can_fwd, page_root);
        let viewport = pmre_kit::Bounds {
            min: pmre_kit::Vec2::new(0.0, 0.0),
            max: pmre_kit::Vec2::new(self.ui.width as f32, self.ui.height as f32),
        };
        let scroll_fn = |id: u32| self.ui.scroll_of(id);
        let boxes = pmre_kit::layout::solve(&tree, viewport, &scroll_fn);
        pmre_kit::layout::hit_test_link(&boxes, x, y)
    }

    /// Render the current chrome + page tree to a framebuffer at `ui.width` x
    /// `ui.height` (set via `dispatch(UiEvent::Resize(..))` or directly on
    /// `self.ui`).
    pub fn render(&self) -> Framebuffer {
        let can_back = self.can_go_back();
        let can_fwd = self.can_go_forward();
        let label = &self.label;
        let page_root = &self.page_root;
        let b = |s: &UiState| build_tree(s, label, can_back, can_fwd, page_root);
        render_ui(&b, &self.ui, CHROME_BG)
    }

    /// Alternate composition: render the same tree through the kit's
    /// `render_ui_quality` — an opt-in post-process tier (bloom via the
    /// cache-tiled CPU path benchmarked to beat the wgpu path 1.27x–1.73x on
    /// this hardware). `Quality::Fast` is byte-identical to `render()`; higher
    /// tiers apply additive Gaussian bloom. Kept as a separate method rather
    /// than a stored field so callers decide at each render call site — no
    /// rendering state on `Browser`.
    pub fn render_with_quality(&self, quality: Quality) -> Framebuffer {
        let can_back = self.can_go_back();
        let can_fwd = self.can_go_forward();
        let label = &self.label;
        let page_root = &self.page_root;
        let b = |s: &UiState| build_tree(s, label, can_back, can_fwd, page_root);
        render_ui_quality(&b, &self.ui, CHROME_BG, quality)
    }

    /// Center point of a chrome widget in window coordinates, if it's
    /// currently laid out. Lets tests/examples simulate clicks by id instead
    /// of hardcoding pixel positions — the same pattern
    /// `pmre-orchestrator`'s own `todo` example uses internally, promoted to
    /// public API here since this crate has no headless test harness of its
    /// own to hide it behind.
    fn widget_center(&self, id: u32) -> Option<(f32, f32)> {
        let can_back = self.can_go_back();
        let can_fwd = self.can_go_forward();
        let label = &self.label;
        let page_root = &self.page_root;
        let b = |s: &UiState| build_tree(s, label, can_back, can_fwd, page_root);
        pmre_orchestrator::widget_rect(&b, &self.ui, id)
            .map(|r| ((r.min.x + r.max.x) / 2.0, (r.min.y + r.max.y) / 2.0))
    }

    fn click_widget(&mut self, id: u32) {
        if let Some((x, y)) = self.widget_center(id) {
            self.dispatch(UiEvent::PointerDown(x, y));
            self.dispatch(UiEvent::PointerUp(x, y));
        }
    }

    /// Simulate clicking the address bar to focus it (tests/examples only —
    /// a real click reaches the same result through `dispatch`).
    pub fn focus_address_bar(&mut self) {
        self.click_widget(ADDRESS_INPUT);
    }

    /// Type text into the address bar (must be focused first). Does not
    /// press Enter — pair with `dispatch(UiEvent::Enter)` to navigate.
    pub fn type_address(&mut self, text: &str) {
        for c in text.chars() {
            self.dispatch(UiEvent::Char(c));
        }
    }

    pub fn click_back(&mut self) {
        self.click_widget(BACK_BTN);
    }

    pub fn click_forward(&mut self) {
        self.click_widget(FORWARD_BTN);
    }

    pub fn click_reload(&mut self) {
        self.click_widget(RELOAD_BTN);
    }

    /// Center point of the first rendered `<a href>` link on the current
    /// page, if any. Tests/examples only — links have no numeric id
    /// (`widget_center` can't find them), so this re-derives where a wrapped
    /// link piece landed the same way `hit_test_link_at` re-derives it to
    /// check a point, just inverted (piece → position instead of position →
    /// piece).
    pub fn first_link_center(&self) -> Option<(f32, f32)> {
        let can_back = self.can_go_back();
        let can_fwd = self.can_go_forward();
        let label = &self.label;
        let page_root = &self.page_root;
        let tree = build_tree(&self.ui, label, can_back, can_fwd, page_root);
        let viewport = pmre_kit::Bounds {
            min: pmre_kit::Vec2::new(0.0, 0.0),
            max: pmre_kit::Vec2::new(self.ui.width as f32, self.ui.height as f32),
        };
        let scroll_fn = |id: u32| self.ui.scroll_of(id);
        let boxes = pmre_kit::layout::solve(&tree, viewport, &scroll_fn);
        for b in &boxes {
            let pmre_kit::layout::Painted::Rich { spans, align } = &b.kind else {
                continue;
            };
            let w = b.rect.max.x - b.rect.min.x;
            let (lines, line_h) = pmre_kit::layout::rich_lines(spans, Some(w).filter(|w| *w > 0.0));
            for (i, line) in lines.iter().enumerate() {
                let line_x0 = match align {
                    Align::Start | Align::Stretch => 0.0,
                    Align::Center => (w - line.width) / 2.0,
                    Align::End => w - line.width,
                };
                if let Some(p) = line.pieces.iter().find(|p| p.href.is_some()) {
                    let cx = b.rect.min.x + line_x0 + p.x + p.width / 2.0;
                    let cy = b.rect.min.y + (i as f32 + 0.5) * line_h;
                    return Some((cx, cy));
                }
            }
        }
        None
    }

    /// Simulate clicking the first rendered link on the page (test/example
    /// only — a real click reaches the same result through `dispatch`).
    pub fn click_first_link(&mut self) -> bool {
        let Some((x, y)) = self.first_link_center() else {
            return false;
        };
        self.dispatch(UiEvent::PointerDown(x, y));
        self.dispatch(UiEvent::PointerUp(x, y));
        true
    }
}

fn nav_button(ui: &UiState, id: u32, label: &str, enabled: bool) -> UxNode {
    let (bg, fg) = if !enabled {
        (BTN_BG_DISABLED, BTN_FG_DISABLED)
    } else if ui.is_pressed(id) {
        (
            Rgba::new(BTN_BG.r * 0.7, BTN_BG.g * 0.7, BTN_BG.b * 0.7, 1.0),
            BTN_FG,
        )
    } else if ui.is_hover(id) {
        (
            Rgba::new(
                (BTN_BG.r * 1.25).min(1.0),
                (BTN_BG.g * 1.25).min(1.0),
                (BTN_BG.b * 1.25).min(1.0),
                1.0,
            ),
            BTN_FG,
        )
    } else {
        (BTN_BG, BTN_FG)
    };
    let mut style = Style::row()
        .w(Dim::Px(60.0))
        .h(Dim::Px(30.0))
        .radius(6.0)
        .bg(bg)
        .align(Align::Center)
        .justify(Justify::Center);
    if enabled {
        style = style.button(id);
    }
    UxNode::boxed(style, vec![UxNode::text(label, 13.0, fg)])
}

fn address_field(ui: &UiState, current_label: &str) -> UxNode {
    let focused = ui.is_focused(ADDRESS_INPUT);
    // Focused: show the live (possibly empty, mid-edit) buffer. Unfocused:
    // always show the real current URL, ignoring any abandoned partial edit —
    // matches a real browser reverting the bar if you click away without
    // pressing Enter.
    let display = if focused {
        ui.input_text(ADDRESS_INPUT).to_string()
    } else {
        current_label.to_string()
    };
    let mut children = vec![UxNode::text(display, 14.0, BTN_FG)];
    if focused {
        children.push(UxNode::boxed(
            Style::col().w(Dim::Px(2.0)).h(Dim::Px(18.0)).bg(FOCUS_RING),
            vec![],
        ));
    }
    UxNode::boxed(
        Style::row()
            .input(ADDRESS_INPUT)
            .w(Dim::Flex(1.0))
            .h(Dim::Px(30.0))
            .align(Align::Center)
            .pad(Edges::xy(10.0, 0.0))
            .radius(6.0)
            .bg(INPUT_BG)
            .border(1.0, if focused { FOCUS_RING } else { INPUT_BORDER }),
        children,
    )
}

/// A freshly-parsed page's root box has `width: Dim::Auto` (sized to its own
/// content), which is right for a top-level render but wrong once embedded as
/// a *child* of the scroll region below — it needs to stretch to the
/// viewport's width so text wraps at the right column, matching what
/// `render_html`'s direct top-level call would have produced. `html::parse`
/// never emits a `Layer`/`Clip`/etc. wrapper, only `Box`/`Text`/`Rich`, so a
/// `Box` is the only variant that has a `width` to force.
fn force_full_width(node: &mut UxNode) {
    if let UxNode::Box { style, .. } = node {
        style.width = Dim::Flex(1.0);
    }
}

fn build_tree(
    ui: &UiState,
    label: &str,
    can_back: bool,
    can_fwd: bool,
    page_root: &UxNode,
) -> UxNode {
    let chrome = UxNode::boxed(
        Style::row()
            .h(Dim::Px(CHROME_H))
            .align(Align::Center)
            .gap(8.0)
            .pad(Edges::xy(10.0, 6.0))
            .bg(CHROME_BG),
        vec![
            nav_button(ui, BACK_BTN, "Back", can_back),
            nav_button(ui, FORWARD_BTN, "Fwd", can_fwd),
            nav_button(ui, RELOAD_BTN, "Reload", true),
            address_field(ui, label),
        ],
    );
    let mut content = page_root.clone();
    force_full_width(&mut content);
    let scroll_area = UxNode::boxed(
        Style::col()
            .scroll(PAGE_SCROLL)
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .bg(PAGE_BG),
        vec![content],
    );
    UxNode::boxed(
        Style::col().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)),
        vec![chrome, scroll_area],
    )
}

#[cfg(test)]
mod image_scan_tests {
    use super::{decode_image_bytes, find_img_srcs};

    #[test]
    fn finds_double_quoted_img_src() {
        let html = r#"<div><img src="logo.png" width="32"></div>"#;
        assert_eq!(find_img_srcs(html), vec!["logo.png".to_string()]);
    }

    #[test]
    fn finds_single_quoted_img_src() {
        let html = "<img src='logo.bmp'>";
        assert_eq!(find_img_srcs(html), vec!["logo.bmp".to_string()]);
    }

    #[test]
    fn accepts_attribute_order_and_extras() {
        let html = r#"<img alt="hi" width="16" src="x.png" height="16">"#;
        assert_eq!(find_img_srcs(html), vec!["x.png".to_string()]);
    }

    #[test]
    fn preserves_source_order_of_multiple_imgs() {
        let html = r#"<img src="a.bmp"><p>text</p><img src="b.png">"#;
        assert_eq!(
            find_img_srcs(html),
            vec!["a.bmp".to_string(), "b.png".to_string()]
        );
    }

    #[test]
    fn unclosed_img_tag_does_not_hang() {
        let html = "<img src=\"broken.png";
        assert!(find_img_srcs(html).is_empty());
    }

    #[test]
    fn decode_dispatch_recognizes_bmp_magic() {
        // Not a full BMP — just the signature; the decoder should return None
        // (bytes too short) but the dispatch should route to decode_bmp not
        // decode_png (verified indirectly: no panic, None result).
        assert!(decode_image_bytes(b"BM\x00\x00").is_none());
    }

    #[test]
    fn decode_dispatch_recognizes_png_magic() {
        let signature = [137, 80, 78, 71, 13, 10, 26, 10];
        assert!(decode_image_bytes(&signature).is_none());
    }

    #[test]
    fn decode_dispatch_rejects_unknown_format() {
        assert!(decode_image_bytes(b"JPEG").is_none());
        assert!(decode_image_bytes(&[]).is_none());
    }
}

#[cfg(test)]
mod stylesheet_scan_tests {
    use super::{attr_value, find_stylesheet_hrefs, inject_external_stylesheets};

    #[test]
    fn finds_double_quoted_stylesheet_href() {
        let html = r#"<html><head><link rel="stylesheet" href="site.css"></head></html>"#;
        assert_eq!(find_stylesheet_hrefs(html), vec!["site.css".to_string()]);
    }

    #[test]
    fn finds_single_quoted_stylesheet_href() {
        let html = "<link rel='stylesheet' href='theme.css'>";
        assert_eq!(find_stylesheet_hrefs(html), vec!["theme.css".to_string()]);
    }

    #[test]
    fn accepts_attribute_order_and_extras() {
        let html = r#"<link type="text/css" href="a.css" rel="stylesheet" media="all">"#;
        assert_eq!(find_stylesheet_hrefs(html), vec!["a.css".to_string()]);
    }

    #[test]
    fn skips_non_stylesheet_link_and_wrong_rel() {
        let html = r#"<link rel="icon" href="/favicon.ico"><link rel="preconnect" href="x">"#;
        assert!(find_stylesheet_hrefs(html).is_empty());
    }

    #[test]
    fn preserves_source_order_of_multiple_sheets() {
        let html =
            r#"<link rel="stylesheet" href="base.css"><link rel="stylesheet" href="theme.css">"#;
        assert_eq!(
            find_stylesheet_hrefs(html),
            vec!["base.css".to_string(), "theme.css".to_string()]
        );
    }

    #[test]
    fn unclosed_link_tag_does_not_hang_or_panic() {
        let html = "<link rel=\"stylesheet\" href=\"broken.css";
        assert!(find_stylesheet_hrefs(html).is_empty());
    }

    #[test]
    fn injection_is_pass_through_when_no_links_present() {
        let html = "<div>hi</div>";
        assert_eq!(inject_external_stylesheets("about:blank", html), html);
    }

    #[test]
    fn attr_value_handles_unquoted() {
        let tag = "<link rel=stylesheet href=plain.css media=all";
        assert_eq!(attr_value(tag, "href"), Some("plain.css".to_string()));
        assert_eq!(attr_value(tag, "rel"), Some("stylesheet".to_string()));
    }

    #[test]
    fn attr_value_does_not_match_substring_of_another_attribute() {
        // `data-href` must not satisfy a lookup for `href`.
        let tag = r#"<link data-href="not-a-real-href" rel="stylesheet" href="real.css""#;
        assert_eq!(attr_value(tag, "href"), Some("real.css".to_string()));
    }
}

#[cfg(test)]
mod resolve_href_tests {
    use super::{classify, resolve_href, Origin};

    #[test]
    fn absolute_href_passes_through() {
        assert_eq!(
            resolve_href("https://example.com/a/b.html", "https://other.org/x"),
            "https://other.org/x"
        );
    }

    #[test]
    fn root_relative_href_replaces_path_on_a_url_base() {
        assert_eq!(
            resolve_href("https://example.com/a/b.html", "/about"),
            "https://example.com/about"
        );
    }

    #[test]
    fn same_directory_relative_href_joins_on_a_url_base() {
        assert_eq!(
            resolve_href("https://example.com/a/b.html", "c.html"),
            "https://example.com/a/c.html"
        );
    }

    #[test]
    fn url_base_with_no_path_joins_at_root() {
        assert_eq!(
            resolve_href("https://example.com", "about.html"),
            "https://example.com/about.html"
        );
    }

    #[test]
    fn relative_href_joins_against_local_file_directory() {
        let base = if cfg!(windows) {
            r"C:\pages\index.html"
        } else {
            "/pages/index.html"
        };
        let resolved = resolve_href(base, "about.html");
        assert!(resolved.ends_with("about.html"));
        assert!(resolved.contains("pages"));
    }

    #[test]
    fn classify_treats_modern_schemes_as_urls() {
        assert!(matches!(classify("https://example.com"), Origin::Url(_)));
        assert!(matches!(classify("http://example.com"), Origin::Url(_)));
        assert!(matches!(classify("ws://echo.example.com"), Origin::Url(_)));
        assert!(matches!(classify("wss://echo.example.com"), Origin::Url(_)));
        assert!(matches!(classify("data:,hello"), Origin::Url(_)));
    }

    #[test]
    fn classify_treats_bare_paths_as_files() {
        assert!(matches!(classify("page.html"), Origin::File(_)));
        assert!(matches!(classify("/abs/path/page.html"), Origin::File(_)));
    }

    #[test]
    fn classify_is_case_insensitive_on_scheme() {
        assert!(matches!(classify("HTTPS://Example.COM"), Origin::Url(_)));
        assert!(matches!(classify("WSS://echo.example.com"), Origin::Url(_)));
        assert!(matches!(classify("DATA:,hi"), Origin::Url(_)));
    }

    #[test]
    fn resolve_href_passes_through_ws_and_data_urls() {
        assert_eq!(resolve_href("https://x/page", "ws://echo/x"), "ws://echo/x");
        assert_eq!(
            resolve_href("https://x/page", "wss://echo/x"),
            "wss://echo/x"
        );
        assert_eq!(resolve_href("https://x/page", "data:,hello"), "data:,hello");
    }
}
