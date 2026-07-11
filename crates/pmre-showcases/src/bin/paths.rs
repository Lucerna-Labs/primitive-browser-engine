//! Path rasterizer demo: a star (concave polygon), a donut (hole via opposite winding), and
//! a closed Bézier blob — all filled by the scanline path rasterizer with anti-aliasing.
//! No SDF; arbitrary geometry straight from the points.
//!
//! Run: cargo run -p pmre-showcases --bin paths

use std::f32::consts::{PI, TAU};

use pmre_kit::{
    geom::Vec2,
    path::{self, PathCmd},
    Framebuffer, Paint, Rgba,
};

fn star(cx: f32, cy: f32, outer: f32, inner: f32, points: usize) -> Vec<PathCmd> {
    let mut v = Vec::new();
    let n = points * 2;
    for i in 0..n {
        let r = if i.is_multiple_of(2) { outer } else { inner };
        let a = -PI / 2.0 + i as f32 / n as f32 * TAU;
        let p = Vec2::new(cx + r * a.cos(), cy + r * a.sin());
        v.push(if i == 0 {
            PathCmd::MoveTo(p)
        } else {
            PathCmd::LineTo(p)
        });
    }
    v.push(PathCmd::Close);
    v
}

fn circle(cx: f32, cy: f32, r: f32, ccw: bool, out: &mut Vec<PathCmd>) {
    let n = 64;
    for i in 0..n {
        let k = if ccw { i } else { n - i };
        let a = k as f32 / n as f32 * TAU;
        let p = Vec2::new(cx + r * a.cos(), cy + r * a.sin());
        out.push(if i == 0 {
            PathCmd::MoveTo(p)
        } else {
            PathCmd::LineTo(p)
        });
    }
    out.push(PathCmd::Close);
}

fn main() {
    let bg = Rgba::rgb8(18, 18, 26);
    let mut fb = Framebuffer::new(720, 420, bg);

    // Star — a concave outline; nonzero winding fills it correctly.
    path::fill_cmds(
        &mut fb,
        &star(150.0, 210.0, 95.0, 42.0, 5),
        Paint::Solid(Rgba::rgb8(250, 200, 90)),
        None,
    );

    // Donut — outer ring CCW + inner ring CW cuts a clean hole.
    let mut donut = Vec::new();
    circle(370.0, 210.0, 95.0, true, &mut donut);
    circle(370.0, 210.0, 48.0, false, &mut donut);
    path::fill_cmds(
        &mut fb,
        &donut,
        Paint::Solid(Rgba::rgb8(96, 165, 250)),
        None,
    );

    // Bézier blob — closed cubic curves.
    let blob = vec![
        PathCmd::MoveTo(Vec2::new(560.0, 130.0)),
        PathCmd::Cubic(
            Vec2::new(685.0, 135.0),
            Vec2::new(690.0, 255.0),
            Vec2::new(600.0, 300.0),
        ),
        PathCmd::Cubic(
            Vec2::new(560.0, 322.0),
            Vec2::new(498.0, 290.0),
            Vec2::new(512.0, 228.0),
        ),
        PathCmd::Cubic(
            Vec2::new(523.0, 180.0),
            Vec2::new(520.0, 150.0),
            Vec2::new(560.0, 130.0),
        ),
        PathCmd::Close,
    ];
    path::fill_cmds(&mut fb, &blob, Paint::Solid(Rgba::rgb8(240, 110, 90)), None);

    std::fs::write("paths.bmp", fb.to_bmp(bg)).expect("write paths.bmp");
    println!("wrote paths.bmp ({}x{})", fb.width, fb.height);
}
