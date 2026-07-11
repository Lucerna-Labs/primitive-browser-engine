//! Stroke demo: an outlined star (closed), a thick open zig-zag polyline with round caps and
//! joins, and a gradient-stroked Bézier curve. Strokes are a union of segment quads + vertex
//! discs merged by the path rasterizer's nonzero winding.
//!
//! Run: cargo run -p pmre-showcases --bin stroke

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
    v
}

fn main() {
    let bg = Rgba::rgb8(18, 18, 26);
    let mut fb = Framebuffer::new(760, 340, bg);

    // Outlined star (closed stroke).
    let s = star(150.0, 170.0, 92.0, 40.0, 5);
    path::stroke_cmds(
        &mut fb,
        &s,
        8.0,
        Paint::Solid(Rgba::rgb8(250, 200, 90)),
        None,
        true,
    );

    // Thick open zig-zag with round caps + joins.
    let zig = vec![
        PathCmd::MoveTo(Vec2::new(320.0, 110.0)),
        PathCmd::LineTo(Vec2::new(380.0, 230.0)),
        PathCmd::LineTo(Vec2::new(440.0, 110.0)),
        PathCmd::LineTo(Vec2::new(500.0, 230.0)),
    ];
    path::stroke_cmds(
        &mut fb,
        &zig,
        14.0,
        Paint::Solid(Rgba::rgb8(96, 165, 250)),
        None,
        false,
    );

    // Gradient-stroked Bézier curve.
    let curve = vec![
        PathCmd::MoveTo(Vec2::new(560.0, 235.0)),
        PathCmd::Cubic(
            Vec2::new(600.0, 75.0),
            Vec2::new(700.0, 75.0),
            Vec2::new(725.0, 235.0),
        ),
    ];
    path::stroke_cmds(
        &mut fb,
        &curve,
        11.0,
        Paint::Linear {
            from: Vec2::new(560.0, 235.0),
            to: Vec2::new(725.0, 235.0),
            c0: Rgba::rgb8(255, 200, 70),
            c1: Rgba::rgb8(240, 90, 70),
        },
        None,
        false,
    );

    std::fs::write("stroke.bmp", fb.to_bmp(bg)).expect("write stroke.bmp");
    println!("wrote stroke.bmp ({}x{})", fb.width, fb.height);
}
