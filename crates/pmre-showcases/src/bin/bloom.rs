//! Bloom demo — vivid neon shapes on a near-black background; Gaussian bloom applied
//! as a post-pass so bright accents glow against the dark field.
//!
//! Algorithm ported from `mm3e-orchestrator/src/post.rs`:
//!   bright-pass → horizontal Gaussian (σ=5, r=12) → vertical Gaussian → additive composite.
//!
//! Run: cargo run -p pmre-showcases --bin bloom

use pmre_kit::{
    post::bloom,
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::render_uxi;

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

fn main() {
    let root = UxNode::boxed(
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
    );

    let (w, h) = (860u32, 380u32);
    let mut fb = render_uxi(&root, w, h, bg());

    // Bloom: extract luma > 0.45, blur with σ=5 radius=12, add back.
    bloom(&mut fb, 0.45, 5.0, 12);

    let bmp = fb.to_bmp(bg());
    std::fs::write("bloom.bmp", &bmp).expect("write bloom.bmp");
    println!("wrote bloom.bmp ({w}x{h})");
}
