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

use pbe_protocol::{
    FetchRequest, FrameReady, RenderRequest, SOCK_FETCH_REQUEST, SOCK_FRAME_READY,
    SOCK_RENDER_REQUEST,
};
use pbe_stages::{register_render_types, BuildStyledStage, FetchStage, PaintStage, RenderStage};

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

/// A one-shot source strand that publishes a single [`FetchRequest`] (the
/// network on-ramp), then detaches. Used for `--url` mode.
struct UrlSource {
    request: Option<FetchRequest>,
}

impl Strand for UrlSource {
    fn name(&self) -> &str {
        "url-source"
    }
    fn inputs(&self) -> &[Socket] {
        &[]
    }
    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<FetchRequest>(SOCK_FETCH_REQUEST);
        std::slice::from_ref(&S)
    }
    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        match self.request.take() {
            Some(req) => {
                bus.log(&format!("dispatching fetch: {}", req.url));
                bus.publish_static(SOCK_FETCH_REQUEST, req)?;
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

/// The built-in demo page, used when no input files are given.
fn demo_request() -> RenderRequest {
    RenderRequest {
        label: "demo".into(),
        html: "<html><body><div><p>Hello from the primitive browser engine</p></div></body></html>"
            .into(),
        css: "div { background-color: #1e2430; width: 640px; height: 200px; } \
              p { color: #e6e9f0; }"
            .into(),
    }
}

/// What kind of on-ramp the CLI selected.
enum Source {
    /// Render local source directly (built-in demo or files).
    Local(RenderRequest),
    /// Fetch a live URL first (network on-ramp), then render it.
    Url(FetchRequest),
}

/// Build the on-ramp from CLI args.
///
/// - `pbe`                        → the built-in demo page
/// - `pbe --url <URL> [<css>]`    → fetch + render a live page (optional CSS file)
/// - `pbe <html> [<css>]`         → render local file(s)
///
/// Returns an error string on unreadable input.
fn source_from_args(args: &[String]) -> Result<Source, String> {
    match args {
        [] => Ok(Source::Local(demo_request())),
        [flag, url, rest @ ..] if flag == "--url" => {
            let css = match rest.first() {
                Some(css_path) => std::fs::read_to_string(css_path)
                    .map_err(|e| format!("cannot read CSS '{css_path}': {e}"))?,
                None => String::new(),
            };
            Ok(Source::Url(FetchRequest {
                url: url.clone(),
                css,
            }))
        }
        [flag] if flag == "--url" => Err("--url requires a URL argument".into()),
        [html_path, rest @ ..] => {
            let html = std::fs::read_to_string(html_path)
                .map_err(|e| format!("cannot read HTML '{html_path}': {e}"))?;
            let css = match rest.first() {
                Some(css_path) => std::fs::read_to_string(css_path)
                    .map_err(|e| format!("cannot read CSS '{css_path}': {e}"))?,
                None => String::new(),
            };
            let label = std::path::Path::new(html_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("page")
                .to_string();
            Ok(Source::Local(RenderRequest { label, html, css }))
        }
    }
}

fn main() {
    // 1. Fan-out clone fns for our custom payloads.
    register_render_types();

    // Resolve the on-ramp from CLI args (built-in demo if none).
    let cli: Vec<String> = std::env::args().skip(1).collect();
    let source = match source_from_args(&cli) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: {e}");
            eprintln!("usage: pbe [<html-file> [<css-file>]] | pbe --url <URL> [<css-file>]");
            std::process::exit(2);
        }
    };
    // A network fetch needs longer than a local render.
    let is_url = matches!(source, Source::Url(_));
    let budget = if is_url {
        Duration::from_secs(30)
    } else {
        Duration::from_secs(3)
    };

    let (tx, rx) = mpsc::channel::<FrameReady>();

    // 2. + 3. Build the web. Stages are restartable specs; the spider supervises.
    // The fetch stage (network on-ramp) is always present; it only does work
    // when a FetchRequest flows, so it is harmless for local renders.
    let mut bus = Bus::open();
    bus.register_spider(StrandSpec::new("spider", || Box::new(Spider::new())));
    bus.register_spec(StrandSpec::new("fetch", || Box::new(FetchStage)));
    bus.register_spec(StrandSpec::new("build-styled", || {
        Box::new(BuildStyledStage)
    }));
    bus.register_spec(StrandSpec::new("paint", || Box::new(PaintStage)));
    bus.register_spec(StrandSpec::new("render", || Box::new(RenderStage)));
    bus.register(FrameSink { tx });
    match source {
        Source::Local(request) => {
            bus.register(RequestSource {
                request: Some(request),
            });
        }
        Source::Url(request) => {
            bus.register(UrlSource {
                request: Some(request),
            });
        }
    }

    // 4. Run until the render thread completes (or a safety timeout).
    println!("── primitive browser engine: composing cap-* kit over the spiderweb bus ──");
    bus.run_until(Some(budget));

    let frame = match rx.try_recv() {
        Ok(frame) => frame,
        Err(_) => {
            eprintln!("\nERROR: no FrameReady came back — the render thread did not complete.");
            std::process::exit(1);
        }
    };

    // A valid frame with 0 primitives is NOT a failure — it means the page had
    // nothing the current (MVP) `cap-paint` knows how to draw. cap-paint only
    // emits primitives for elements with a non-transparent background or border;
    // text and real box layout are not yet implemented in the kit, so a
    // text-only page (e.g. example.com) legitimately paints nothing. Report it
    // honestly and still persist the (blank-but-valid) frame.
    if frame.primitive_count == 0 {
        eprintln!(
            "\nNOTE: 0 paint primitives — the page parsed and rendered a valid {}x{} frame, \
             but cap-paint (MVP) only draws backgrounds/borders; text + layout are not yet \
             implemented in the kit, so a text-only page appears blank.",
            frame.width, frame.height
        );
    }

    // 5. Persist the engine's artifacts.
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("out");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!(
            "\nERROR: could not create out dir {}: {e}",
            out_dir.display()
        );
        std::process::exit(1);
    }
    // The label may be a URL (slashes, colons) — sanitize for a filename.
    let safe_label: String = frame
        .label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe_label = safe_label.trim_matches('_').to_string();
    let safe_label = if safe_label.is_empty() {
        "page".to_string()
    } else {
        safe_label
    };
    let dl_path = out_dir.join(format!("{safe_label}.display-list.txt"));
    let ppm_path = out_dir.join(format!("{safe_label}.ppm"));

    if let Err(e) = std::fs::write(&dl_path, &frame.display_list) {
        eprintln!("\nERROR: could not write {}: {e}", dl_path.display());
        std::process::exit(1);
    }
    if let Err(e) = std::fs::write(&ppm_path, frame.ppm.as_slice()) {
        eprintln!("\nERROR: could not write {}: {e}", ppm_path.display());
        std::process::exit(1);
    }

    println!("\n{}", frame.display_list);
    println!(
        "RESULT: '{}' rendered {} primitive(s) via the bus.",
        frame.label, frame.primitive_count
    );
    println!("  display list → {}", dl_path.display());
    println!(
        "  raster (PPM) → {} ({} bytes)",
        ppm_path.display(),
        frame.ppm.len()
    );
}
