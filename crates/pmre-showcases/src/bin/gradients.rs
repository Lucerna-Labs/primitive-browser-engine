//! Gradient demo: a linear-gradient rounded rect, a radial-gradient circle, and a
//! gradient-filled star path. Two-stop gradients sampled per pixel by the rasterizer —
//! SDF shapes sample in local space (so the gradient moves with the shape), paths in device space.
//!
//! Run: cargo run -p pmre-showcases --bin gradients

use std::f32::consts::{PI, TAU};

use pmre_kit::{
    geom::Vec2,
    path::{self, PathCmd},
    Affine, DrawCmd, Paint, Rgba, Shape,
};
use pmre_orchestrator::{render, Scene};

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

fn main() {
    let bg = Rgba::rgb8(16, 17, 22);
    let mut scene = Scene::new(760, 340, bg);

    // Linear gradient across a rounded rect (local-space, corner to corner).
    scene.push(
        0.0,
        DrawCmd {
            shape: Shape::RoundedRect {
                half: Vec2::new(150.0, 95.0),
                radius: 26.0,
            },
            paint: Paint::Linear {
                from: Vec2::new(-150.0, -95.0),
                to: Vec2::new(150.0, 95.0),
                c0: Rgba::rgb8(80, 140, 250),
                c1: Rgba::rgb8(175, 80, 230),
            },
            transform: Affine::translate(190.0, 170.0),
            soft: 0.0,
        },
    );

    // Radial gradient on a circle (bright off-center core fading to a deep rim).
    scene.push(
        1.0,
        DrawCmd {
            shape: Shape::Circle { radius: 95.0 },
            paint: Paint::Radial {
                center: Vec2::new(-28.0, -28.0),
                radius: 150.0,
                c0: Rgba::rgb8(250, 250, 255),
                c1: Rgba::rgb8(40, 90, 200),
            },
            transform: Affine::translate(470.0, 170.0),
            soft: 0.0,
        },
    );

    let mut fb = render(&scene);

    // Gradient-filled star path (device-space gradient).
    let s = star(660.0, 170.0, 82.0, 36.0, 5);
    path::fill_cmds(
        &mut fb,
        &s,
        Paint::Linear {
            from: Vec2::new(600.0, 95.0),
            to: Vec2::new(720.0, 250.0),
            c0: Rgba::rgb8(255, 205, 70),
            c1: Rgba::rgb8(240, 85, 70),
        },
        None,
    );

    std::fs::write("gradients.bmp", fb.to_bmp(bg)).expect("write gradients.bmp");
    println!("wrote gradients.bmp ({}x{})", fb.width, fb.height);
}
