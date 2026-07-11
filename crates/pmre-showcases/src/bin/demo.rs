//! Demo scene: pure-math SDF shapes, anti-aliased and composited by the kit + orchestrator,
//! written to a BMP. No Vello, no GPU, no external crates.
//!
//! Run: cargo run -p pmre-showcases --bin demo

use pmre_kit::{geom::Vec2, Affine, DrawCmd, Paint, Rgba, Shape};
use pmre_orchestrator::{render, Scene};

fn main() {
    let mut scene = Scene::new(640, 360, Rgba::rgb8(18, 18, 26));

    // A solid rounded-rect "card".
    scene.push(
        0.0,
        DrawCmd {
            shape: Shape::RoundedRect {
                half: Vec2::new(150.0, 90.0),
                radius: 28.0,
            },
            paint: Paint::Solid(Rgba::rgb8(60, 120, 220)),
            transform: Affine::translate(230.0, 180.0),
            soft: 0.0,
        },
    );

    // A semi-transparent circle, overlapping the card (tests alpha-over compositing).
    scene.push(
        1.0,
        DrawCmd {
            shape: Shape::Circle { radius: 70.0 },
            paint: Paint::Solid(Rgba::new(1.0, 0.42, 0.30, 0.85)),
            transform: Affine::translate(410.0, 140.0),
            soft: 0.0,
        },
    );

    // A thick line / bar on top (tests the segment SDF and painter order).
    scene.push(
        2.0,
        DrawCmd {
            shape: Shape::Line {
                a: Vec2::new(-160.0, 0.0),
                b: Vec2::new(160.0, 0.0),
                width: 10.0,
            },
            paint: Paint::Solid(Rgba::rgb8(245, 210, 90)),
            transform: Affine::translate(320.0, 285.0),
            soft: 0.0,
        },
    );

    let fb = render(&scene);
    let bmp = fb.to_bmp(scene.clear);
    let path = r"demo.bmp";
    std::fs::write(path, bmp).expect("write demo.bmp");
    println!("wrote {path} ({}x{})", scene.width, scene.height);
}
