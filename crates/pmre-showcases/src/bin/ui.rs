//! Interactive UI demo, driven headlessly. Builds a state-aware UI (buttons, toggles, a
//! scrollable list), feeds it synthetic pointer/wheel/resize events through the same engine
//! a live window uses, and renders the resulting states to images — proving hover, press,
//! click-to-toggle, scroll-with-clipping + scrollbar, and auto-resize reflow.
//!
//! Run: cargo run -p pmre-showcases --bin ui

use pmre_kit::{
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::{handle_event, render_ui, widget_rect, UiEvent, UiState};

const BG: Rgba = Rgba::new(0.078, 0.086, 0.110, 1.0);
const PANEL: Rgba = Rgba::new(0.118, 0.129, 0.161, 1.0);

fn white() -> Rgba {
    Rgba::rgb8(235, 239, 247)
}
fn muted() -> Rgba {
    Rgba::rgb8(150, 158, 174)
}

// ids
const TOG_DARK: u32 = 1;
const BTN_SAVE: u32 = 2;
const BTN_CANCEL: u32 = 3;
const TOG_OPT: u32 = 4;
const LIST: u32 = 10;

fn button(s: &UiState, id: u32, label: &str, base: Rgba) -> UxNode {
    let bg = if s.is_pressed(id) {
        Rgba::rgb8(40, 44, 56)
    } else if s.is_hover(id) {
        Rgba::new(base.r * 1.25, base.g * 1.25, base.b * 1.25, 1.0)
    } else {
        base
    };
    UxNode::boxed(
        Style::row()
            .button(id)
            .w(Dim::Flex(1.0))
            .h(Dim::Px(40.0))
            .radius(8.0)
            .bg(bg)
            .align(Align::Center)
            .justify(Justify::Center),
        vec![UxNode::text(label, 14.0, white())],
    )
}

fn toggle(s: &UiState, id: u32) -> UxNode {
    let on = s.toggle_on(id);
    let track = if on {
        Rgba::rgb8(52, 199, 130)
    } else {
        Rgba::rgb8(70, 74, 90)
    };
    let knob = UxNode::boxed(
        Style::col()
            .w(Dim::Px(22.0))
            .h(Dim::Px(22.0))
            .radius(11.0)
            .bg(white()),
        vec![],
    );
    UxNode::boxed(
        Style::row()
            .toggle(id)
            .w(Dim::Px(52.0))
            .h(Dim::Px(28.0))
            .radius(14.0)
            .bg(track)
            .align(Align::Center)
            .pad(Edges::xy(3.0, 0.0))
            .justify(if on { Justify::End } else { Justify::Start }),
        vec![knob],
    )
}

fn row(i: u32) -> UxNode {
    let shade = if i.is_multiple_of(2) { 30 } else { 36 };
    UxNode::boxed(
        Style::row()
            .h(Dim::Px(40.0))
            .radius(8.0)
            .bg(Rgba::rgb8(shade, shade + 3, shade + 10))
            .align(Align::Center)
            .pad(Edges::xy(12.0, 0.0)),
        vec![UxNode::text(
            format!("ITEM {i:02} - SCROLLABLE ROW CONTENT"),
            13.0,
            muted(),
        )],
    )
}

fn build(s: &UiState) -> UxNode {
    let spacer = UxNode::boxed(Style::row().w(Dim::Flex(1.0)).h(Dim::Px(1.0)), vec![]);
    let header = UxNode::boxed(
        Style::row()
            .h(Dim::Px(56.0))
            .bg(PANEL)
            .align(Align::Center)
            .pad(Edges::xy(18.0, 0.0))
            .gap(14.0),
        vec![
            UxNode::text("CONTROLS", 18.0, white()),
            spacer,
            UxNode::text("DARK MODE", 13.0, muted()),
            toggle(s, TOG_DARK),
        ],
    );

    let sidebar = UxNode::boxed(
        Style::col()
            .w(Dim::Px(190.0))
            .h(Dim::Flex(1.0))
            .bg(PANEL)
            .pad(Edges::all(16.0))
            .gap(12.0),
        vec![
            UxNode::text("ACTIONS", 12.0, muted()),
            button(s, BTN_SAVE, "SAVE", Rgba::rgb8(48, 110, 210)),
            button(s, BTN_CANCEL, "CANCEL", Rgba::rgb8(70, 74, 90)),
            UxNode::text("OPTION", 12.0, muted()),
            UxNode::boxed(
                Style::row().align(Align::Center).gap(10.0).h(Dim::Px(28.0)),
                vec![toggle(s, TOG_OPT), UxNode::text("ENABLED", 13.0, muted())],
            ),
        ],
    );

    let list = UxNode::boxed(
        Style::col()
            .scroll(LIST)
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .bg(Rgba::rgb8(24, 26, 33))
            .pad(Edges::all(12.0))
            .gap(8.0),
        (0..16).map(row).collect(),
    );

    let body = UxNode::boxed(
        Style::row().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)),
        vec![sidebar, list],
    );

    UxNode::boxed(
        Style::col().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)).bg(BG),
        vec![header, body],
    )
}

fn center(b: pmre_kit::paint::Bounds) -> (f32, f32) {
    ((b.min.x + b.max.x) / 2.0, (b.min.y + b.max.y) / 2.0)
}

fn save(frame: &pmre_kit::Framebuffer, name: &str) {
    let path = format!(r"{name}.bmp");
    std::fs::write(&path, frame.to_bmp(BG)).expect("write bmp");
    println!("wrote {path} ({}x{})", frame.width, frame.height);
}

fn main() {
    let b: &dyn Fn(&UiState) -> UxNode = &build;
    let mut s = UiState::new(900, 560);

    // Frame 1 — initial.
    save(&render_ui(b, &s, BG), "ui_initial");

    // Turn both toggles on (down+up on each).
    for id in [TOG_DARK, TOG_OPT] {
        if let Some(r) = widget_rect(b, &s, id) {
            let (x, y) = center(r);
            handle_event(&mut s, b, UiEvent::PointerDown(x, y));
            handle_event(&mut s, b, UiEvent::PointerUp(x, y));
        }
    }
    // Hover SAVE.
    if let Some(r) = widget_rect(b, &s, BTN_SAVE) {
        let (x, y) = center(r);
        handle_event(&mut s, b, UiEvent::PointerMove(x, y));
    }
    // Press-and-hold CANCEL.
    if let Some(r) = widget_rect(b, &s, BTN_CANCEL) {
        let (x, y) = center(r);
        handle_event(&mut s, b, UiEvent::PointerDown(x, y));
    }
    // Scroll the list down.
    if let Some(r) = widget_rect(b, &s, LIST) {
        let (x, y) = center(r);
        for _ in 0..3 {
            handle_event(&mut s, b, UiEvent::Wheel(x, y, 90.0));
        }
    }
    // Frame 2 — interacted: toggles on, SAVE hovered, CANCEL pressed, list scrolled.
    save(&render_ui(b, &s, BG), "ui_active");

    // Frame 3 — auto-resize to a smaller window; same tree reflows.
    handle_event(&mut s, b, UiEvent::Resize(700, 460));
    save(&render_ui(b, &s, BG), "ui_resized");
}
