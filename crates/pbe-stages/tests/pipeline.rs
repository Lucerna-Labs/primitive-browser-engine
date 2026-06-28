//! End-to-end pipeline test: drive the real Spiderweb bus with the real stages
//! and assert the engine renders a page from source to pixels.
//!
//! This exercises the *composition* — `build-styled → paint → render` wired by
//! type over the bus, supervised by the spider — not just the isolated render
//! functions (those are unit-tested in `pbe-render`). A test source strand
//! publishes a `RenderRequest`; a test sink captures the `FrameReady` and hands
//! it back over a channel.

use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use spiderweb::{Bus, BusHandle, Socket, Strand, StrandError, StrandSpec};

use pbe_protocol::{FrameReady, RenderRequest, SOCK_FRAME_READY, SOCK_RENDER_REQUEST};
use pbe_stages::{register_render_types, BuildStyledStage, PaintStage, RenderStage};

struct OneShotSource {
    req: Option<RenderRequest>,
}
impl Strand for OneShotSource {
    fn name(&self) -> &str {
        "test-source"
    }
    fn inputs(&self) -> &[Socket] {
        &[]
    }
    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<RenderRequest>(SOCK_RENDER_REQUEST);
        std::slice::from_ref(&S)
    }
    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        if let Some(req) = self.req.take() {
            bus.publish_static(SOCK_RENDER_REQUEST, req)?;
        }
        Err(StrandError::Detach)
    }
}

struct CaptureSink {
    tx: Sender<FrameReady>,
}
impl Strand for CaptureSink {
    fn name(&self) -> &str {
        "test-sink"
    }
    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<FrameReady>(SOCK_FRAME_READY);
        std::slice::from_ref(&S)
    }
    fn outputs(&self) -> &[Socket] {
        &[]
    }
    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for frame in bus.recv::<FrameReady>(SOCK_FRAME_READY)? {
            let _ = self.tx.send(frame);
        }
        bus.sleep(Duration::from_millis(5));
        Ok(())
    }
}

/// Run one render through the full bus and return the captured frame.
fn render_via_bus(html: &str, css: &str) -> FrameReady {
    register_render_types();
    let (tx, rx) = mpsc::channel::<FrameReady>();

    let mut bus = Bus::open();
    bus.register_spec(StrandSpec::new("build-styled", || {
        Box::new(BuildStyledStage)
    }));
    bus.register_spec(StrandSpec::new("paint", || Box::new(PaintStage)));
    bus.register_spec(StrandSpec::new("render", || Box::new(RenderStage)));
    bus.register(CaptureSink { tx });
    bus.register(OneShotSource {
        req: Some(RenderRequest {
            label: "test".into(),
            html: html.into(),
            css: css.into(),
        }),
    });

    bus.run_until(Some(Duration::from_secs(5)));
    rx.try_recv()
        .expect("pipeline did not produce a FrameReady within the timeout")
}

#[test]
fn renders_a_styled_box_to_pixels_over_the_bus() {
    let frame = render_via_bus(
        "<html><body><div></div></body></html>",
        "div { background-color: #ff0000; width: 100px; height: 50px; }",
    );

    // The frame round-tripped through the whole fabric.
    assert_eq!(frame.label, "test");
    assert_eq!(frame.width, 800);
    assert_eq!(frame.height, 600);
    assert!(
        frame.primitive_count >= 1,
        "expected at least one painted primitive"
    );

    // The display list names the painted rect and its fill.
    assert!(
        frame.display_list.contains("rect"),
        "display list should describe a rect, got:\n{}",
        frame.display_list
    );
    assert!(
        frame.display_list.contains("#ff0000ff"),
        "display list should carry the red fill, got:\n{}",
        frame.display_list
    );

    // The PPM is a valid 800x600 P6 image of the right byte length.
    assert!(frame.ppm.starts_with(b"P6\n800 600\n255\n"));
    let header_len = b"P6\n800 600\n255\n".len();
    assert_eq!(frame.ppm.len(), header_len + 800 * 600 * 3);

    // A pixel inside the 100x50 box at the origin must be red.
    let body = &frame.ppm[header_len..];
    let px = |x: usize, y: usize| {
        let i = (y * 800 + x) * 3;
        (body[i], body[i + 1], body[i + 2])
    };
    assert_eq!(px(10, 10), (255, 0, 0), "inside the box should be red");
    // A pixel well outside the box stays at the white page background.
    assert_eq!(
        px(400, 400),
        (255, 255, 255),
        "outside the box should be white"
    );
}

#[test]
fn empty_page_still_produces_a_valid_frame() {
    let frame = render_via_bus("<html><body></body></html>", "");
    // No painted boxes, but a real, correctly-sized white frame.
    assert_eq!(frame.width, 800);
    assert_eq!(frame.height, 600);
    let header_len = b"P6\n800 600\n255\n".len();
    assert_eq!(frame.ppm.len(), header_len + 800 * 600 * 3);
}
