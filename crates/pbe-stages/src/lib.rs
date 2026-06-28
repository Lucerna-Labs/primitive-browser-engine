//! # pbe-stages
//!
//! Render stages, each a Spiderweb [`Strand`] that wraps a **dumb** primitive
//! function from the `cap-*` kit. These adapters are the only new code in the
//! render path — they translate bus messages into primitive calls and back.
//! They hold **no policy**: no retries, no placement, no decisions. The kit
//! keeps all mechanism; the orchestrator keeps all policy; these just bridge.
//!
//! ## Stages
//!
//! - [`BuildStyledStage`] — `parse_html` + `Stylesheet::parse_author` +
//!   `StyledDom::new`. These fuse because `StyledDom::new` *consumes* the
//!   `DomTree` by value (a real ownership wall in the current kit). Splitting
//!   parse and cascade into separate strands is a future additive change to
//!   `cap-style-cascade`, tracked in the project ROADMAP.
//! - [`PaintStage`] — `cap_paint::paint`, producing the primitive list.
//!
//! Neither stage names the other. `BuildStyledStage` listens for
//! [`RenderRequest`] and emits [`StyledReady`]; `PaintStage` listens for
//! [`StyledReady`] and emits [`PaintReady`]. The bus wires them by type.

use std::sync::Arc;

use spiderweb::{BusHandle, Socket, Strand, StrandError};

use pbe_protocol::{
    PaintReady, RenderRequest, StyledReady, SOCK_PAINT_READY, SOCK_RENDER_REQUEST,
    SOCK_STYLED_READY,
};

/// Stage 1: source → styled DOM. Wraps parse + cascade.
pub struct BuildStyledStage;

impl Strand for BuildStyledStage {
    fn name(&self) -> &str {
        "build-styled"
    }

    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<RenderRequest>(SOCK_RENDER_REQUEST);
        std::slice::from_ref(&S)
    }

    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<StyledReady>(SOCK_STYLED_READY);
        std::slice::from_ref(&S)
    }

    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for req in bus.recv::<RenderRequest>(SOCK_RENDER_REQUEST)? {
            // --- drive the dumb primitives, in order ---
            let dom = cap_html_parse::parse_html(&req.html);
            let sheet = cap_css_parse::Stylesheet::parse_author(&req.css);
            let styled = cap_style_cascade::StyledDom::new(dom, &[sheet]);

            bus.log(&format!(
                "{}: styled {} element(s)",
                req.label,
                styled.styled_elements().count()
            ));

            bus.publish_static(
                SOCK_STYLED_READY,
                StyledReady {
                    label: req.label,
                    styled: Arc::new(styled),
                },
            )?;
        }
        bus.sleep(std::time::Duration::from_millis(10));
        Ok(())
    }
}

/// Stage 2: styled DOM → primitive list. Wraps `cap_paint::paint`.
pub struct PaintStage;

impl Strand for PaintStage {
    fn name(&self) -> &str {
        "paint"
    }

    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<StyledReady>(SOCK_STYLED_READY);
        std::slice::from_ref(&S)
    }

    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<PaintReady>(SOCK_PAINT_READY);
        std::slice::from_ref(&S)
    }

    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for ready in bus.recv::<StyledReady>(SOCK_STYLED_READY)? {
            let primitives = cap_paint::paint(&ready.styled);
            bus.log(&format!(
                "{}: painted {} primitive(s)",
                ready.label,
                primitives.len()
            ));
            bus.publish_static(
                SOCK_PAINT_READY,
                PaintReady {
                    label: ready.label,
                    primitives: Arc::new(primitives),
                },
            )?;
        }
        bus.sleep(std::time::Duration::from_millis(10));
        Ok(())
    }
}

/// Register the render payload types for bus fan-out. Must be called once at
/// boot before the bus runs — the kernel needs a clone fn per custom type.
pub fn register_render_types() {
    spiderweb::register_clone_type::<RenderRequest>();
    spiderweb::register_clone_type::<StyledReady>();
    spiderweb::register_clone_type::<PaintReady>();
}
