//! Render-time benchmark: measures frame time at Fast / Balanced / Full quality.
//! Scene matches the bloom demo (860×380, 4 panels + dot row).
//!
//! Run: cargo run -p pmre-showcases --bin bench --release

use pmre_kit::{
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::{render_uxi_quality, Quality};
use std::time::Instant;

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

fn measure(root: &UxNode, w: u32, h: u32, q: Quality, n: u32) -> f64 {
    let t0 = Instant::now();
    for _ in 0..n {
        drop(render_uxi_quality(root, w, h, bg(), q));
    }
    t0.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn main() {
    let root = build_scene();
    let (w, h) = (860u32, 380u32);
    let n = 40u32;

    println!("PMRE render benchmark  ({w}x{h}, {n} frames each, release build)");
    println!(
        "GPU backend: {}\n",
        pmre_orchestrator::gpu_bloom::gpu_backend_name()
    );

    // Warm up the GPU path (first call initialises the wgpu device + compiles shaders)
    drop(render_uxi_quality(&root, w, h, bg(), Quality::GpuFull));

    let n_cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    println!("CPU threads available: {n_cpu}\n");

    let tiers: &[(&str, Quality)] = &[
        ("CPU  Fast           — no post           ", Quality::Fast),
        ("CPU  Balanced       — bloom σ=3  r=6   ", Quality::Balanced),
        ("CPU  Full           — bloom σ=5  r=12  ", Quality::Full),
        (
            "PAR  Balanced (FQ)  — bloom σ=3  r=6   ",
            Quality::ParallelBalanced,
        ),
        (
            "PAR  Full     (FQ)  — bloom σ=5  r=12  ",
            Quality::ParallelFull,
        ),
        (
            "BUS  Balanced (lanes)— bloom σ=3  r=6  ",
            Quality::TiledBalanced,
        ),
        (
            "BUS  Full     (lanes)— bloom σ=5  r=12 ",
            Quality::TiledFull,
        ),
        (
            "GPU  Balanced       — bloom σ=3  r=6   ",
            Quality::GpuBalanced,
        ),
        ("GPU  Full           — bloom σ=5  r=12  ", Quality::GpuFull),
    ];

    for &(label, q) in tiers {
        let ms = measure(&root, w, h, q, n);
        let fps = 1000.0 / ms;
        println!("  {label}  {:6.1} ms/frame  ({:5.0} fps)", ms, fps);
    }
}
