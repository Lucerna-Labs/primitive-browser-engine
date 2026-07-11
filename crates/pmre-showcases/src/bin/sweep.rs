//! Bloom strategy sweep — benchmarks every combination of the kernel-borrowed
//! primitives (atomic cursor, tiling+fusion, SIMD, persistent pool) against the
//! `FairQueue` baseline, and validates each against the reference `post::bloom`.
//!
//! Run: cargo run -p pmre-showcases --bin sweep --release

use pmre_kit::bloom_sweep::{bloom_with, Arith, Dispatch, Strategy, Structure};
use pmre_kit::{
    framebuffer::Framebuffer,
    post,
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::{gpu_bloom, render_uxi};
use std::time::Instant;

// ── Scene (identical to examples/bench.rs) ─────────────────────────────────────

fn bg() -> Rgba {
    Rgba::rgb8(6, 6, 10)
}
fn teal() -> Rgba {
    Rgba::rgb8(0, 220, 180)
}
fn amber() -> Rgba {
    Rgba::rgb8(255, 178, 36)
}
fn rose() -> Rgba {
    Rgba::rgb8(255, 80, 120)
}
fn violet() -> Rgba {
    Rgba::rgb8(160, 80, 255)
}
fn lime() -> Rgba {
    Rgba::rgb8(80, 220, 80)
}

fn glow_dot(col: Rgba, size: f32) -> UxNode {
    let r = size / 2.0;
    UxNode::boxed(
        Style::row()
            .w(Dim::Px(size))
            .h(Dim::Px(size))
            .radius(r)
            .bg(col),
        vec![],
    )
}

fn panel(col: Rgba, label: &str) -> UxNode {
    let card_bg = Rgba::rgb8(12, 12, 20);
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(130.0))
            .pad(Edges::all(16.0))
            .gap(10.0)
            .radius(14.0)
            .bg(card_bg)
            .border(2.0, col),
        vec![
            UxNode::boxed(
                Style::row().align(Align::Center).gap(8.0),
                vec![glow_dot(col, 10.0), UxNode::text("SIGNAL", 10.0, col)],
            ),
            UxNode::text(label, 22.0, col),
            UxNode::text("nominal", 10.0, Rgba::rgb8(60, 60, 80)),
        ],
    )
}

fn dot_row() -> UxNode {
    UxNode::boxed(
        Style::row()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(60.0))
            .align(Align::Center)
            .justify(Justify::Center)
            .gap(18.0),
        vec![
            glow_dot(teal(), 36.0),
            glow_dot(amber(), 24.0),
            glow_dot(rose(), 44.0),
            glow_dot(violet(), 30.0),
            glow_dot(lime(), 20.0),
            glow_dot(amber(), 38.0),
            glow_dot(teal(), 26.0),
            glow_dot(rose(), 18.0),
        ],
    )
}

fn build_scene() -> UxNode {
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .bg(bg())
            .pad(Edges::all(28.0))
            .gap(20.0),
        vec![
            UxNode::text("BLOOM DEMO", 20.0, Rgba::rgb8(180, 180, 200)),
            UxNode::boxed(
                Style::row().w(Dim::Flex(1.0)).gap(16.0),
                vec![
                    panel(teal(), "ACTIVE"),
                    panel(amber(), "QUEUED"),
                    panel(rose(), "STALL"),
                    panel(violet(), "RELAY"),
                ],
            ),
            dot_row(),
        ],
    )
}

// ── Measurement helpers ────────────────────────────────────────────────────────

fn max_diff(a: &[Rgba], b: &[Rgba]) -> f32 {
    let mut m = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        m = m
            .max((x.r - y.r).abs())
            .max((x.g - y.g).abs())
            .max((x.b - y.b).abs())
            .max((x.a - y.a).abs());
    }
    m
}

fn dispatch_name(d: Dispatch) -> &'static str {
    match d {
        Dispatch::Serial => "Serial",
        Dispatch::FairQueue => "FairQ ",
        Dispatch::Atomic => "Atomic",
        Dispatch::Band => "Band  ",
        Dispatch::AtomicPool => "Pool  ",
    }
}

fn structure_name(s: Structure) -> &'static str {
    match s {
        Structure::Separable => "Separable",
        Structure::TiledFused => "Tiled    ",
    }
}

fn arith_name(a: Arith) -> &'static str {
    match a {
        Arith::Scalar => "scalar",
        Arith::Simd => "simd",
    }
}

struct Row {
    label: String,
    ms: f64,
    diff: f32,
    is_baseline: bool,
}

#[allow(clippy::too_many_arguments)]
fn time_strategy(
    snapshot: &[Rgba],
    w: u32,
    h: u32,
    threshold: f32,
    sigma: f32,
    radius: usize,
    strat: Strategy,
    iters: u32,
) -> f64 {
    let mut fb = Framebuffer::new(w, h, bg());
    let mut total = 0.0;
    for _ in 0..iters {
        fb.pixels_mut().copy_from_slice(snapshot);
        let t0 = Instant::now();
        bloom_with(&mut fb, threshold, sigma, radius, strat);
        total += t0.elapsed().as_secs_f64();
    }
    total * 1000.0 / iters as f64
}

fn run_radius(snapshot: &[Rgba], w: u32, h: u32, sigma: f32, radius: usize, iters: u32) {
    let threshold = 0.45;

    // Reference: the shipped serial `post::bloom`.
    let mut rfb = Framebuffer::new(w, h, bg());
    rfb.pixels_mut().copy_from_slice(snapshot);
    post::bloom(&mut rfb, threshold, sigma, radius);
    let reference = rfb.pixels().to_vec();

    let dispatches = [
        Dispatch::Serial,
        Dispatch::FairQueue,
        Dispatch::Atomic,
        Dispatch::Band,
        Dispatch::AtomicPool,
    ];
    let structures = [Structure::Separable, Structure::TiledFused];
    let ariths = [Arith::Scalar, Arith::Simd];

    let mut rows: Vec<Row> = Vec::new();
    for d in dispatches {
        for s in structures {
            for a in ariths {
                let strat = Strategy::new(d, s, a);

                // Validate one run against the reference.
                let mut vfb = Framebuffer::new(w, h, bg());
                vfb.pixels_mut().copy_from_slice(snapshot);
                bloom_with(&mut vfb, threshold, sigma, radius, strat);
                let diff = max_diff(vfb.pixels(), &reference);

                let ms = time_strategy(snapshot, w, h, threshold, sigma, radius, strat, iters);
                let is_baseline =
                    d == Dispatch::Serial && s == Structure::Separable && a == Arith::Scalar;
                rows.push(Row {
                    label: format!(
                        "{}  {}  {}",
                        dispatch_name(d),
                        structure_name(s),
                        arith_name(a)
                    ),
                    ms,
                    diff,
                    is_baseline,
                });
            }
        }
    }

    // GPU reference time (bloom only, on the same snapshot).
    let mut gfb = Framebuffer::new(w, h, bg());
    gfb.pixels_mut().copy_from_slice(snapshot);
    gpu_bloom::gpu_bloom(&mut gfb, threshold, sigma, radius); // warm
    let mut gpu_total = 0.0;
    for _ in 0..iters {
        gfb.pixels_mut().copy_from_slice(snapshot);
        let t0 = Instant::now();
        gpu_bloom::gpu_bloom(&mut gfb, threshold, sigma, radius);
        gpu_total += t0.elapsed().as_secs_f64();
    }
    let gpu_ms = gpu_total * 1000.0 / iters as f64;

    let baseline_ms = rows
        .iter()
        .find(|r| r.is_baseline)
        .map(|r| r.ms)
        .unwrap_or(0.0);

    rows.sort_by(|a, b| a.ms.partial_cmp(&b.ms).unwrap());

    println!("\n── bloom σ={sigma}  r={radius}  ({w}x{h}, {iters} iters/strategy) ──");
    println!(
        "   {:<26}  {:>9}  {:>7}  {:>9}  {:>6}",
        "strategy", "ms/frame", "fps", "vs base", "valid"
    );
    for r in &rows {
        let speedup = if r.ms > 0.0 { baseline_ms / r.ms } else { 0.0 };
        let valid = if r.diff < 1.0e-3 { "ok" } else { "DRIFT" };
        let tag = if r.is_baseline { " (baseline)" } else { "" };
        println!(
            "   {:<26}  {:>7.2}    {:>5.0}  {:>7.2}x  {:>6}{}",
            r.label,
            r.ms,
            1000.0 / r.ms,
            speedup,
            valid,
            tag
        );
    }
    println!(
        "   {:<26}  {:>7.2}    {:>5.0}  {:>7.2}x  {:>6}",
        "GPU  wgpu compute",
        gpu_ms,
        1000.0 / gpu_ms,
        baseline_ms / gpu_ms,
        "ok"
    );

    // Winner among CPU strategies that stayed faithful to the reference.
    if let Some(best) = rows
        .iter()
        .filter(|r| r.diff < 1.0e-3)
        .min_by(|a, b| a.ms.partial_cmp(&b.ms).unwrap())
    {
        println!(
            "   → fastest valid CPU: {}  at {:.2} ms ({:.2}x over baseline)",
            best.label.trim(),
            best.ms,
            baseline_ms / best.ms
        );
    }
}

fn main() {
    let (w, h) = (860u32, 380u32);
    let iters = 30u32;

    // Render the scene once with no post, then snapshot the pixels.
    let base = render_uxi(&build_scene(), w, h, bg());
    let snapshot = base.pixels().to_vec();

    let n_cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    println!("PMRE bloom strategy sweep  ({w}x{h}, release build)");
    println!("GPU backend: {}", gpu_bloom::gpu_backend_name());
    println!("CPU threads available: {n_cpu}");
    println!("Axes: Dispatch{{Serial,FairQ,Atomic,Pool}} x Structure{{Separable,Tiled}} x Arith{{scalar,simd}}");

    run_radius(&snapshot, w, h, 3.0, 6, iters);
    run_radius(&snapshot, w, h, 5.0, 12, iters);
}
