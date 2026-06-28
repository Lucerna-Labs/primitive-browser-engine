//! # pbe-protocol
//!
//! The **message vocabulary** for the primitive browser engine. This crate is
//! pure contracts: the typed payloads that render stages publish and subscribe
//! to on the Spiderweb bus. No mechanism, no policy — just the shapes that flow.
//!
//! ## Why these types, and why `Arc`
//!
//! The bus fans out by **type**: a stage declares `Socket::new::<T>(name)` and
//! the kernel routes every `T` to it. Fan-out requires `T: Clone + Send`. The
//! render payloads (`DomTree`, `StyledDom`) are large and deliberately **not**
//! `Clone` — cloning a whole DOM per hop would be the invisible cost the
//! composition doctrine forbids. So heavy payloads ride wrapped in [`Arc`]:
//! cloning an `Arc` bumps a refcount, the tree itself never moves. Cost stays
//! explicit and near-zero. (`cap_primitives::Primitive` *is* `Clone`, but the
//! paint output is a whole `Vec`, so it rides in an `Arc` too.)
//!
//! ## The pipeline as a thread
//!
//! ```text
//!  RenderRequest ──▶ [build_styled] ──▶ StyledReady ──▶ [paint] ──▶ PaintReady
//! ```
//!
//! Each arrow is a typed socket. No stage names another stage; the fabric wires
//! them by type, and the render becomes an emergent **thread** through the web.

use std::sync::Arc;

use cap_html_parse::DomTree;
use cap_primitives::Primitive;
use cap_style_cascade::StyledDom;

/// Socket: a request to fetch a URL from the network (the on-ramp for live web).
pub const SOCK_FETCH_REQUEST: &str = "fetch.request";
/// Socket: a request to render a page. Carries the raw HTML + CSS source.
pub const SOCK_RENDER_REQUEST: &str = "render.request";
/// Socket: a styled DOM is ready (parse + cascade complete).
pub const SOCK_STYLED_READY: &str = "render.styled";
/// Socket: paint is complete — a primitive list is ready for the renderer.
pub const SOCK_PAINT_READY: &str = "render.paint";
/// Socket: the render off-ramp is done — display list + raster are ready.
pub const SOCK_FRAME_READY: &str = "render.frame";

/// A request to fetch a live URL over the network. The `fetch` stage loads it
/// and publishes a [`RenderRequest`] with the page's HTML, so the same render
/// pipeline serves both local files and the live web. Optional `css` lets the
/// caller supply an author/override stylesheet alongside the fetched HTML.
#[derive(Clone, Debug)]
pub struct FetchRequest {
    pub url: String,
    /// Optional CSS to apply on top of the fetched page (often empty).
    pub css: String,
}

/// A request to render a page from source. This is the on-ramp payload an
/// orchestrator (or a fetch stage) publishes to kick off a render.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    /// A label for this render (URL, test name, etc.) — flows through so later
    /// stages and the fabric can identify which page a result belongs to.
    pub label: String,
    /// Raw HTML source.
    pub html: String,
    /// Raw CSS source (author stylesheet).
    pub css: String,
}

/// A styled DOM, ready for paint. Wraps the non-`Clone` [`StyledDom`] in an
/// [`Arc`] so it fans out across the bus as a cheap refcount bump, never a deep
/// copy. The owned [`DomTree`] lives inside the `StyledDom` (cascade consumed
/// it), so this single handle carries the whole styled tree.
#[derive(Clone)]
pub struct StyledReady {
    pub label: String,
    pub styled: Arc<StyledDom>,
}

impl std::fmt::Debug for StyledReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyledReady")
            .field("label", &self.label)
            .field("elements", &self.styled.styled_elements().count())
            .finish()
    }
}

/// One run of text to draw, produced by paint from a text node + its inherited
/// typography, positioned by real layout. The render stage shapes + rasterizes
/// it. Plain data so it rides the bus inside [`PaintReady`].
#[derive(Clone, Debug)]
pub struct TextDraw {
    pub text: String,
    /// Left edge of the text box (logical px, window space).
    pub x: f32,
    /// Top edge of the text box; the renderer adds the font ascent to get the
    /// baseline, so callers don't need font metrics.
    pub top_y: f32,
    pub font_size: f32,
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Text color (0..1 per channel).
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// The paint result: renderer-neutral box primitives plus the text runs to
/// draw, ready for the render off-ramp. Wrapped in [`Arc`] for cheap fan-out.
#[derive(Clone, Debug)]
pub struct PaintReady {
    pub label: String,
    pub primitives: Arc<Vec<Primitive>>,
    pub texts: Arc<Vec<TextDraw>>,
}

/// The finished frame: the engine's concrete outputs for one render — a
/// deterministic display list (text) and a rasterized image (binary PPM bytes),
/// plus the painted-primitive count. This is what the render off-ramp emits and
/// the orchestrator writes to disk / hands onward.
#[derive(Clone)]
pub struct FrameReady {
    pub label: String,
    pub primitive_count: usize,
    pub display_list: String,
    pub width: u32,
    pub height: u32,
    /// Rasterized frame as binary PPM (P6) bytes.
    pub ppm: Arc<Vec<u8>>,
}

impl std::fmt::Debug for FrameReady {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameReady")
            .field("label", &self.label)
            .field("primitive_count", &self.primitive_count)
            .field("size", &(self.width, self.height))
            .field("ppm_bytes", &self.ppm.len())
            .finish()
    }
}

/// A tiny helper so [`DomTree`] is part of the public surface here — the
/// protocol crate is where the engine's type vocabulary is documented, and a
/// fetch/parse split (future) will publish a `Arc<DomTree>` between stages.
pub type SharedDom = Arc<DomTree>;
