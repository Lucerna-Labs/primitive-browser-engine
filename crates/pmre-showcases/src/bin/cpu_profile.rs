//! CPU frame profiler — decomposes a frame into layout / rasterize / bloom so we can see
//! where the time actually goes and what the lane (bus) model buys per stage. Differential
//! timing, single scene, release build.
//!
//! Run: cargo run -p pmre-showcases --bin cpu_profile --release

use pmre_kit::bloom_sweep::{bloom_with, Arith, Dispatch, Strategy, Structure};
use pmre_kit::{
    layout,
    paint::Bounds,
    post,
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Framebuffer, Rgba, Vec2,
};
use pmre_orchestrator::{render_uxi, render_uxi_serial};
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
    UxNode::boxed(
        Style::row()
            .w(Dim::Px(size))
            .h(Dim::Px(size))
            .radius(size / 2.0)
            .bg(col),
        vec![],
    )
}

fn panel(col: Rgba, label: &str) -> UxNode {
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(130.0))
            .pad(Edges::all(16.0))
            .gap(10.0)
            .radius(14.0)
            .bg(Rgba::rgb8(12, 12, 20))
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
                ],
            ),
        ],
    )
}

fn time<F: FnMut()>(n: u32, mut f: F) -> f64 {
    f(); // warm
    let t0 = Instant::now();
    for _ in 0..n {
        f();
    }
    t0.elapsed().as_secs_f64() * 1000.0 / n as f64
}

fn main() {
    let (w, h) = (860u32, 380u32);
    let n = 80u32;
    let root = build_scene();
    let viewport = Bounds {
        min: Vec2::new(0.0, 0.0),
        max: Vec2::new(w as f32, h as f32),
    };
    let n_cpu = std::thread::available_parallelism()
        .map(|c| c.get())
        .unwrap_or(1);

    // Pre-rendered framebuffer (no post) for the bloom-only measurements.
    let base = render_uxi(&root, w, h, bg());
    let snapshot = base.pixels().to_vec();
    let bus = Strategy::new(Dispatch::Band, Structure::TiledFused, Arith::Simd);

    let t_layout = time(n, || {
        let boxes = layout::solve(&root, viewport, &|_| 0.0);
        std::hint::black_box(&boxes);
    });
    let t_render_serial = time(n, || drop(render_uxi_serial(&root, w, h, bg())));
    let t_render_bus = time(n, || drop(render_uxi(&root, w, h, bg())));
    let t_bloom_serial = time(n, || {
        let mut fb = Framebuffer::new(w, h, bg());
        fb.pixels_mut().copy_from_slice(&snapshot);
        post::bloom(&mut fb, 0.45, 3.0, 6);
    });
    let t_bloom_bus = time(n, || {
        let mut fb = Framebuffer::new(w, h, bg());
        fb.pixels_mut().copy_from_slice(&snapshot);
        bloom_with(&mut fb, 0.45, 3.0, 6, bus);
    });

    let paint_serial = t_render_serial - t_layout;
    let paint_bus = t_render_bus - t_layout;

    println!("PMRE CPU frame profile  ({w}x{h}, {n} iters, release, {n_cpu} cores)\n");
    println!("  stage                    serial      lanes     speedup");
    println!(
        "  layout solve           {:7.2}ms        —           —",
        t_layout
    );
    println!(
        "  rasterize (paint)      {:7.2}ms  {:7.2}ms     {:4.2}x",
        paint_serial,
        paint_bus,
        paint_serial / paint_bus.max(1e-6)
    );
    println!(
        "  bloom σ=3 r=6          {:7.2}ms  {:7.2}ms     {:4.2}x",
        t_bloom_serial,
        t_bloom_bus,
        t_bloom_serial / t_bloom_bus.max(1e-6)
    );
    println!("  ─────────────────────────────────────────────────────");
    println!(
        "  full frame             {:7.2}ms  {:7.2}ms     {:4.2}x",
        t_layout + paint_serial + t_bloom_serial,
        t_render_bus + t_bloom_bus,
        (t_layout + paint_serial + t_bloom_serial) / (t_render_bus + t_bloom_bus).max(1e-6)
    );
    println!(
        "\n  layout is {:.0}% of the lane frame; rasterize {:.0}%; bloom {:.0}%",
        t_layout / (t_render_bus + t_bloom_bus) * 100.0,
        paint_bus / (t_render_bus + t_bloom_bus) * 100.0,
        t_bloom_bus / (t_render_bus + t_bloom_bus) * 100.0
    );
}
