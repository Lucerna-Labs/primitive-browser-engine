//! # pbe — the primitive browser engine orchestrator
//!
//! All **policy**, no mechanism. This binary:
//!   1. registers the render payload types for bus fan-out,
//!   2. registers the render stages (dumb primitive wrappers) as strands,
//!   3. registers the `spider` orchestrator (crash/restart policy),
//!   4. publishes a `RenderRequest` and runs the bus until the render thread
//!      flows parse → cascade → paint → render and a `FrameReady` comes back,
//!   5. writes the engine's artifacts (display list + PPM raster) to `out/`.
//!
//! Nothing here knows *how* to parse, cascade, paint, or rasterize — that
//! mechanism lives in the sealed `cap-*` kit and `pbe-render`. The orchestrator
//! only decides what runs and reacts to what the fabric reports.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use spider::Spider;
use spiderweb::{Bus, BusHandle, Socket, Strand, StrandError, StrandSpec};

use pbe_protocol::{FrameReady, RenderRequest, SOCK_FRAME_READY, SOCK_RENDER_REQUEST};
use pbe_stages::{register_render_types, BuildStyledStage, PaintStage, RenderStage};

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

/// The finished-frame off-ramp: hands the completed frame back to `main` over a
/// channel so the binary can persist artifacts and exit.
struct FrameSink {
    tx: mpsc::Sender<FrameReady>,
}

impl Strand for FrameSink {
    fn name(&self) -> &str {
        "frame-sink"
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
            bus.log(&format!(
                "✅ {} frame complete: {} primitive(s), {}x{}",
                frame.label, frame.primitive_count, frame.width, frame.height
            ));
            let _ = self.tx.send(frame);
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

    let (tx, rx) = mpsc::channel::<FrameReady>();

    // 2. + 3. Build the web. Stages are restartable specs; the spider supervises.
    let mut bus = Bus::open();
    bus.register_spider(StrandSpec::new("spider", || Box::new(Spider::new())));
    bus.register_spec(StrandSpec::new("build-styled", || Box::new(BuildStyledStage)));
    bus.register_spec(StrandSpec::new("paint", || Box::new(PaintStage)));
    bus.register_spec(StrandSpec::new("render", || Box::new(RenderStage)));
    bus.register(FrameSink { tx });
    bus.register(RequestSource {
        request: Some(request),
    });

    // 4. Run until the render thread completes (or a safety timeout).
    println!("── primitive browser engine: composing cap-* kit over the spiderweb bus ──");
    bus.run_until(Some(Duration::from_secs(3)));

    let frame = match rx.try_recv() {
        Ok(frame) => frame,
        Err(_) => {
            eprintln!("\nERROR: no FrameReady came back — the render thread did not complete.");
            std::process::exit(1);
        }
    };

    if frame.primitive_count == 0 {
        eprintln!("\nwarning: 0 primitives — pipeline ran but painted nothing.");
        std::process::exit(1);
    }

    // 5. Persist the engine's artifacts.
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("out");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!("\nERROR: could not create out dir {}: {e}", out_dir.display());
        std::process::exit(1);
    }
    let dl_path = out_dir.join(format!("{}.display-list.txt", frame.label));
    let ppm_path = out_dir.join(format!("{}.ppm", frame.label));

    if let Err(e) = std::fs::write(&dl_path, &frame.display_list) {
        eprintln!("\nERROR: could not write {}: {e}", dl_path.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&ppm_path, frame.ppm.as_slice()) {
        eprintln!("\nERROR: could not write {}: {e}", ppm_path.display());
        std::process::exit(1);
    }

    println!("\n{}", frame.display_list);
    println!("RESULT: '{}' rendered {} primitive(s) via the bus.", frame.label, frame.primitive_count);
    println!("  display list → {}", dl_path.display());
    println!("  raster (PPM) → {} ({} bytes)", ppm_path.display(), frame.ppm.len());
}
