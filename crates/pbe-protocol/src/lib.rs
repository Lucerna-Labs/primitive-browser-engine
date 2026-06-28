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

/// Socket: a request to render a page. Carries the raw HTML + CSS source.
pub const SOCK_RENDER_REQUEST: &str = "render.request";
/// Socket: a styled DOM is ready (parse + cascade complete).
pub const SOCK_STYLED_READY: &str = "render.styled";
/// Socket: paint is complete — a primitive list is ready for the renderer.
pub const SOCK_PAINT_READY: &str = "render.paint";

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

/// The paint result: an ordered list of renderer-neutral primitives, ready to
/// hand to a sealed rasterizer (`ordo-ux-vello` → `vello::Scene`). Wrapped in an
/// [`Arc`] for the same cheap-fan-out reason.
#[derive(Clone, Debug)]
pub struct PaintReady {
    pub label: String,
    pub primitives: Arc<Vec<Primitive>>,
}

/// A tiny helper so [`DomTree`] is part of the public surface here — the
/// protocol crate is where the engine's type vocabulary is documented, and a
/// fetch/parse split (future) will publish a `Arc<DomTree>` between stages.
pub type SharedDom = Arc<DomTree>;
