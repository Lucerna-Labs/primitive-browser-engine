//! UXI demo: an app shell (title bar + sidebar + content cards) described as intent only —
//! no coordinates anywhere — solved by the reduced flex/box layout and rendered by the kit.
//!
//! Run: cargo run -p pmre-showcases --bin uxi

use pmre_kit::{
    ux::{Align, Dim, Edges, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::render_uxi;

fn text(s: &str, size: f32, c: Rgba) -> UxNode {
    UxNode::text(s, size, c)
}

fn nav_item(label: &str) -> UxNode {
    UxNode::boxed(
        Style::row()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(34.0))
            .align(Align::Center)
            .pad(Edges::xy(10.0, 0.0))
            .radius(8.0)
            .bg(Rgba::rgb8(46, 50, 62)),
        vec![text(label, 14.0, Rgba::rgb8(206, 212, 224))],
    )
}

fn card(title: &str, accent: Rgba) -> UxNode {
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(14.0))
            .gap(10.0)
            .radius(12.0)
            .bg(Rgba::rgb8(38, 42, 53))
            .border(1.0, Rgba::rgb8(58, 63, 78)),
        vec![
            UxNode::boxed(
                Style::row()
                    .w(Dim::Px(46.0))
                    .h(Dim::Px(46.0))
                    .radius(10.0)
                    .bg(accent),
                vec![],
            ),
            text(title, 15.0, Rgba::rgb8(232, 236, 244)),
            text("metric trending up", 12.0, Rgba::rgb8(150, 158, 174)),
        ],
    )
}

fn main() {
    let bg = Rgba::rgb8(22, 24, 30);
    let white = Rgba::rgb8(236, 240, 248);
    let muted = Rgba::rgb8(168, 176, 192);

    let title_bar = UxNode::boxed(
        Style::row()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(52.0))
            .align(Align::Center)
            .gap(22.0)
            .pad(Edges::xy(18.0, 0.0))
            .bg(Rgba::rgb8(30, 33, 41)),
        vec![
            text("Dashboard", 18.0, white),
            text("File", 14.0, muted),
            text("Edit", 14.0, muted),
            text("View", 14.0, muted),
        ],
    );

    let sidebar = UxNode::boxed(
        Style::col()
            .w(Dim::Px(210.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(12.0))
            .gap(8.0)
            .bg(Rgba::rgb8(28, 31, 39)),
        vec![
            text("NAVIGATION", 11.0, Rgba::rgb8(120, 128, 144)),
            nav_item("Home"),
            nav_item("Projects"),
            nav_item("Reports"),
            nav_item("Settings"),
        ],
    );

    let cards_row = UxNode::boxed(
        Style::row().w(Dim::Flex(1.0)).h(Dim::Px(150.0)).gap(14.0),
        vec![
            card("Revenue", Rgba::rgb8(96, 165, 250)),
            card("Sessions", Rgba::rgb8(52, 211, 153)),
            card("Latency", Rgba::rgb8(251, 191, 96)),
        ],
    );

    let big_panel = UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(16.0))
            .gap(12.0)
            .radius(12.0)
            .bg(Rgba::rgb8(34, 38, 49))
            .border(1.0, Rgba::rgb8(54, 59, 74)),
        vec![
            text("Activity", 16.0, white),
            text("rows of recent events render here", 12.0, muted),
        ],
    );

    let content = UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(16.0))
            .gap(16.0),
        vec![text("Overview", 22.0, white), cards_row, big_panel],
    );

    let body = UxNode::boxed(
        Style::row().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)),
        vec![sidebar, content],
    );

    let root = UxNode::boxed(
        Style::col().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)).bg(bg),
        vec![title_bar, body],
    );

    let (w, h) = (960u32, 600u32);
    let fb = render_uxi(&root, w, h, bg);
    let bmp = fb.to_bmp(bg);
    let path = r"uxi.bmp";
    std::fs::write(path, bmp).expect("write uxi.bmp");
    println!("wrote {path} ({w}x{h})");
}
