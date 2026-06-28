//! # pbe — the primitive browser engine orchestrator
//!
//! All **policy**, no mechanism. This binary:
//!   1. registers the render payload types for bus fan-out,
//!   2. registers the render stages (dumb primitive wrappers) as strands,
//!   3. registers the `spider` orchestrator (crash/restart policy),
//!   4. publishes a `RenderRequest` and runs the bus until the render thread
//!      flows parse → cascade → paint and a `PaintReady` comes back.
//!
//! Nothing here knows *how* to parse, cascade, or paint — that mechanism lives
//! in the sealed `cap-*` primitive kit. The orchestrator only decides what runs
//! and reacts to what the fabric reports.

use std::sync::mpsc;
use std::time::Duration;

use spiderweb::{Bus, BusHandle, Socket, Strand, StrandError, StrandSpec};
use spider::Spider;

use pbe_protocol::{PaintReady, RenderRequest, SOCK_PAINT_READY, SOCK_RENDER_REQUEST};
use pbe_stages::{register_render_types, BuildStyledStage, PaintStage};

/// A one-shot source strand: publishes a single [`RenderRequest`] on its first
/// tick, then detaches. This is the on-ramp that starts the render thread.
struct RequestSource {
    request: Option<RenderRequest>,
}

impl Strand for RequestSource {
    fn name(&self) -> &str {
        "request-source"
    }
    fn inputs(&self) -> &[Socket] {
        &[]
    }
    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<RenderRequest>(SOCK_RENDER_REQUEST);
        std::slice::from_ref(&S)
    }
    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        match self.request.take() {
            Some(req) => {
                bus.log(&format!("dispatching render: {}", req.label));
                bus.publish_static(SOCK_RENDER_REQUEST, req)?;
                Err(StrandError::Detach) // one-shot
            }
            None => Err(StrandError::Detach),
        }
    }
}

/// A sink strand that reports finished renders back to `main` over a channel,
/// so the binary can print a result and exit. The channel sender is the
/// orchestrator's off-ramp out of the bus.
struct PaintSink {
    tx: mpsc::Sender<(String, usize)>,
}

impl Strand for PaintSink {
    fn name(&self) -> &str {
        "paint-sink"
    }
    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<PaintReady>(SOCK_PAINT_READY);
        std::slice::from_ref(&S)
    }
    fn outputs(&self) -> &[Socket] {
        &[]
    }
    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for done in bus.recv::<PaintReady>(SOCK_PAINT_READY)? {
            let n = done.primitives.len();
            bus.log(&format!("✅ {} render complete: {} primitive(s)", done.label, n));
            let _ = self.tx.send((done.label, n));
        }
        bus.sleep(Duration::from_millis(10));
        Ok(())
    }
}

fn main() {
    // 1. Fan-out clone fns for our custom payloads.
    register_render_types();

    // The demo page: source the orchestrator hands to the engine.
    let request = RenderRequest {
        label: "demo".into(),
        html: "<html><body><div><p>Hello from the primitive browser engine</p></div></body></html>"
            .into(),
        css: "div { background-color: #1e2430; width: 640px; height: 200px; } \
              p { color: #e6e9f0; }"
            .into(),
    };

    let (tx, rx) = mpsc::channel::<(String, usize)>();

    // 2. + 3. Build the web. Stages are restartable specs; the spider supervises.
    let mut bus = Bus::open();
    bus.register_spider(StrandSpec::new("spider", || Box::new(Spider::new())));
    bus.register_spec(StrandSpec::new("build-styled", || Box::new(BuildStyledStage)));
    bus.register_spec(StrandSpec::new("paint", || Box::new(PaintStage)));
    bus.register(PaintSink { tx });
    bus.register(RequestSource {
        request: Some(request),
    });

    // 4. Run until the render thread completes (or a safety timeout).
    println!("── primitive browser engine: composing cap-* kit over the spiderweb bus ──");
    bus.run_until(Some(Duration::from_secs(3)));

    match rx.try_recv() {
        Ok((label, n)) => {
            println!("\nRESULT: '{label}' produced {n} render primitive(s) via the bus.");
            if n == 0 {
                eprintln!("warning: 0 primitives — pipeline ran but painted nothing.");
                std::process::exit(1);
            }
        }
        Err(_) => {
            eprintln!("\nERROR: no PaintReady came back — the render thread did not complete.");
            std::process::exit(1);
        }
    }
}
