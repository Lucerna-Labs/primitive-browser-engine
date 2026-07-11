//! Ordo dashboard — Ordo's deep-navy/teal design language rendered through the
//! Primitive Math Rendering Engine. No Electron, no Tauri, no browser — pure math.
//!
//! Run: cargo run -p pmre-showcases --bin ordo

use pmre_kit::{
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::render_uxi;

// ── Ordo colour palette (from ordo-control/src/dashboard.html) ─────────────────
fn bg() -> Rgba {
    Rgba::rgb8(15, 23, 32)
} // #0f1720
fn panel() -> Rgba {
    Rgba::rgb8(18, 29, 44)
} // slightly lighter navy
fn panel_deep() -> Rgba {
    Rgba::rgb8(12, 21, 34)
} // #0c1522 — cards
fn ink() -> Rgba {
    Rgba::rgb8(236, 244, 239)
} // #ecf4ef — near-white with green cast
fn muted() -> Rgba {
    Rgba::rgb8(158, 180, 176)
} // #9eb4b0
fn accent() -> Rgba {
    Rgba::rgb8(111, 209, 180)
} // #6fd1b4 — teal
fn accent_dim() -> Rgba {
    Rgba::rgb8(47, 143, 122)
} // #2f8f7a — darker teal for borders
fn warm() -> Rgba {
    Rgba::rgb8(212, 162, 88)
} // #d4a258 — amber
fn danger() -> Rgba {
    Rgba::rgb8(216, 106, 99)
} // #d86a63 — red-ish
fn edge() -> Rgba {
    Rgba::rgb8(38, 53, 58)
} // edge: rgba(teal, 18%) blended onto bg

// ── Reusable atoms ─────────────────────────────────────────────────────────────

fn spacer_h() -> UxNode {
    UxNode::boxed(Style::row().w(Dim::Flex(1.0)), vec![])
}

fn dot(col: Rgba, r: f32) -> UxNode {
    let d = r * 2.0;
    UxNode::boxed(
        Style::row().w(Dim::Px(d)).h(Dim::Px(d)).radius(r).bg(col),
        vec![],
    )
}

fn pill(label: &str, fg: Rgba, border_col: Rgba) -> UxNode {
    UxNode::boxed(
        Style::row()
            .h(Dim::Px(20.0))
            .align(Align::Center)
            .justify(Justify::Center)
            .pad(Edges::xy(8.0, 0.0))
            .radius(10.0)
            .border(1.0, border_col),
        vec![UxNode::text(label, 9.0, fg)],
    )
}

fn tag_chip(label: &str, bg_col: Rgba) -> UxNode {
    let text_col = Rgba::rgb8(10, 18, 28); // dark on coloured bg
    UxNode::boxed(
        Style::row()
            .w(Dim::Px(52.0))
            .h(Dim::Px(18.0))
            .align(Align::Center)
            .justify(Justify::Center)
            .radius(4.0)
            .bg(bg_col),
        vec![UxNode::text(label, 9.0, text_col)],
    )
}

// ── Top navigation bar ─────────────────────────────────────────────────────────

fn topbar() -> UxNode {
    UxNode::boxed(
        Style::row()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(50.0))
            .align(Align::Center)
            .pad(Edges::xy(20.0, 0.0))
            .gap(22.0)
            .bg(panel())
            .border(1.0, edge()),
        vec![
            UxNode::text("ORDO", 18.0, accent()),
            spacer_h(),
            UxNode::text("RUNTIME", 12.0, muted()),
            UxNode::text("AGENTS", 12.0, muted()),
            UxNode::text("TASKS", 12.0, muted()),
            UxNode::text("LOGS", 12.0, muted()),
            spacer_h(),
            pill("ONLINE", accent(), accent_dim()),
        ],
    )
}

// ── Sidebar ────────────────────────────────────────────────────────────────────

fn nav_item(label: &str, active: bool) -> UxNode {
    let (bg_col, fg_col) = if active {
        (Some(Rgba::rgb8(20, 42, 38)), accent())
    } else {
        (None, muted())
    };
    let mut s = Style::row()
        .h(Dim::Px(32.0))
        .align(Align::Center)
        .pad(Edges::xy(10.0, 0.0))
        .radius(8.0);
    if let Some(c) = bg_col {
        s = s.bg(c);
    }
    UxNode::boxed(s, vec![UxNode::text(label, 12.0, fg_col)])
}

fn section_label(text: &str) -> UxNode {
    UxNode::boxed(
        Style::row()
            .h(Dim::Px(22.0))
            .align(Align::Center)
            .pad(Edges::xy(10.0, 0.0)),
        vec![UxNode::text(text, 10.0, Rgba::rgb8(82, 108, 104))],
    )
}

fn sidebar() -> UxNode {
    UxNode::boxed(
        Style::col()
            .w(Dim::Px(186.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(10.0))
            .gap(3.0)
            .bg(panel_deep())
            .border(1.0, edge()),
        vec![
            section_label("SYSTEM"),
            nav_item("RUNTIME", true),
            nav_item("AGENTS", false),
            nav_item("TASKS", false),
            nav_item("NETWORK", false),
            section_label("TOOLS"),
            nav_item("LOGS", false),
            nav_item("DIAGNOSTICS", false),
        ],
    )
}

// ── Hero card ─────────────────────────────────────────────────────────────────

fn hero_card() -> UxNode {
    UxNode::boxed(
        Style::row().w(Dim::Flex(1.0)).h(Dim::Px(106.0)).gap(14.0),
        vec![
            // Left: title + description
            UxNode::boxed(
                Style::col()
                    .w(Dim::Flex(2.0))
                    .h(Dim::Flex(1.0))
                    .pad(Edges::all(16.0))
                    .gap(8.0)
                    .radius(14.0)
                    .bg(panel_deep())
                    .border(1.0, edge()),
                vec![
                    UxNode::boxed(
                        Style::row().align(Align::Center).gap(8.0),
                        vec![
                            pill("RUNTIME", accent(), accent_dim()),
                            UxNode::text("v0.3.0", 10.0, muted()),
                        ],
                    ),
                    UxNode::text("ORDO CONTROL", 20.0, ink()),
                    UxNode::text("primitive math rendering engine", 11.0, muted()),
                ],
            ),
            // Right: status card
            UxNode::boxed(
                Style::col()
                    .w(Dim::Flex(1.0))
                    .h(Dim::Flex(1.0))
                    .pad(Edges::all(16.0))
                    .gap(10.0)
                    .radius(14.0)
                    .bg(panel_deep())
                    .border(1.0, edge()),
                vec![
                    UxNode::text("SYSTEM STATUS", 10.0, muted()),
                    UxNode::boxed(
                        Style::row().align(Align::Center).gap(8.0),
                        vec![dot(accent(), 5.0), UxNode::text("ONLINE", 14.0, accent())],
                    ),
                    UxNode::text("all services nominal", 10.0, muted()),
                ],
            ),
        ],
    )
}

// ── Metric cards ──────────────────────────────────────────────────────────────

fn metric_card(label: &str, value: &str, sub: &str, col: Rgba) -> UxNode {
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Px(108.0))
            .pad(Edges::all(16.0))
            .gap(8.0)
            .radius(12.0)
            .bg(panel_deep())
            .border(1.0, edge()),
        vec![
            UxNode::boxed(
                Style::row().align(Align::Center).gap(8.0),
                vec![dot(col, 5.0), UxNode::text(label, 10.0, muted())],
            ),
            UxNode::text(value, 22.0, ink()),
            UxNode::text(sub, 10.0, muted()),
        ],
    )
}

// ── Activity log ──────────────────────────────────────────────────────────────

fn log_row(time: &str, tag: &str, msg: &str, tag_col: Rgba) -> UxNode {
    UxNode::boxed(
        Style::row()
            .h(Dim::Px(32.0))
            .align(Align::Center)
            .gap(10.0)
            .pad(Edges::xy(10.0, 0.0))
            .radius(6.0)
            .bg(panel()),
        vec![
            UxNode::text(time, 10.0, Rgba::rgb8(82, 108, 104)),
            tag_chip(tag, tag_col),
            UxNode::text(msg, 11.0, ink()),
        ],
    )
}

fn activity_log() -> UxNode {
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(14.0))
            .gap(6.0)
            .radius(12.0)
            .bg(panel_deep())
            .border(1.0, edge()),
        vec![
            UxNode::boxed(
                Style::row().h(Dim::Px(26.0)).align(Align::Center).gap(10.0),
                vec![
                    UxNode::text("ACTIVITY LOG", 12.0, ink()),
                    spacer_h(),
                    UxNode::text("live", 10.0, accent()),
                    dot(accent(), 4.0),
                ],
            ),
            log_row(
                "12:23:01",
                "AGENT",
                "agent-7 completed task analysis",
                accent(),
            ),
            log_row("12:22:48", "TASK", "task-42 queued for execution", warm()),
            log_row("12:22:31", "NET", "peer handshake 192.168.1.4", muted()),
            log_row("12:22:10", "AGENT", "agent-3 spawned from pool", accent()),
            log_row("12:21:55", "ERR", "connection timeout retrying", danger()),
            log_row("12:21:32", "TASK", "task-38 dispatched to agent-5", warm()),
        ],
    )
}

// ── Main scene ────────────────────────────────────────────────────────────────

fn main() {
    let content = UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(16.0))
            .gap(14.0)
            .bg(bg()),
        vec![
            hero_card(),
            UxNode::boxed(
                Style::row().h(Dim::Px(108.0)).gap(14.0),
                vec![
                    metric_card("AGENTS", "12", "ACTIVE", accent()),
                    metric_card("TASKS", "47", "QUEUED", warm()),
                    metric_card("MEMORY", "8.2 GB", "ALLOCATED", danger()),
                ],
            ),
            activity_log(),
        ],
    );

    let body = UxNode::boxed(
        Style::row().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)),
        vec![sidebar(), content],
    );

    let root = UxNode::boxed(
        Style::col().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)).bg(bg()),
        vec![topbar(), body],
    );

    let (w, h) = (1100u32, 680u32);
    let fb = render_uxi(&root, w, h, bg());
    let bmp = fb.to_bmp(bg());
    std::fs::write("ordo.bmp", &bmp).expect("write ordo.bmp");
    println!("wrote ordo.bmp ({w}x{h})");
}
