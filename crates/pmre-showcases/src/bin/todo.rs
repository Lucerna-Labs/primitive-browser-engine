//! A real app on the engine: a todo list. Text input (type a task), an Add button, a
//! scrollable list with a checkbox and a delete button per row — all driven through the
//! engine's event loop (focus, typed chars, clicks).
//!
//! Self-verifying: it types three tasks, adds each, checks the first off, and asserts the
//! resulting list before rendering, so a green run proves typing -> add -> toggle actually
//! work through the engine. The same `build` drops into the winit `app` shell for live use.
//!
//! Run: cargo run -p pmre-showcases --bin todo

use pmre_kit::{
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::{handle_event, render_ui, widget_rect, UiEvent, UiState};

const BG: Rgba = Rgba::new(0.075, 0.082, 0.106, 1.0);
const NEW_INPUT: u32 = 1;
const ADD: u32 = 2;
const LIST: u32 = 99;
const CHECK_BASE: u32 = 1000;
const DEL_BASE: u32 = 2000;

fn white() -> Rgba {
    Rgba::rgb8(236, 240, 248)
}
fn muted() -> Rgba {
    Rgba::rgb8(140, 148, 164)
}
fn accent() -> Rgba {
    Rgba::rgb8(86, 150, 252)
}

struct Todo {
    text: String,
    done: bool,
}

fn caret() -> UxNode {
    UxNode::boxed(
        Style::col().w(Dim::Px(2.0)).h(Dim::Px(22.0)).bg(white()),
        vec![],
    )
}

fn input_field(ui: &UiState) -> UxNode {
    let txt = ui.input_text(NEW_INPUT);
    let focused = ui.is_focused(NEW_INPUT);
    let placeholder = txt.is_empty() && !focused;
    let label = if placeholder { "add a task..." } else { txt };
    let mut children = vec![UxNode::text(
        label,
        16.0,
        if placeholder { muted() } else { white() },
    )];
    if focused {
        children.push(caret());
    }
    UxNode::boxed(
        Style::row()
            .input(NEW_INPUT)
            .w(Dim::Flex(1.0))
            .h(Dim::Px(42.0))
            .align(Align::Center)
            .pad(Edges::xy(12.0, 0.0))
            .radius(8.0)
            .bg(Rgba::rgb8(26, 29, 38))
            .border(
                1.0,
                if focused {
                    accent()
                } else {
                    Rgba::rgb8(48, 52, 66)
                },
            ),
        children,
    )
}

fn add_button(s: &UiState) -> UxNode {
    let base = accent();
    let bg = if s.is_pressed(ADD) {
        Rgba::new(base.r * 0.7, base.g * 0.7, base.b * 0.7, 1.0)
    } else if s.is_hover(ADD) {
        Rgba::new(base.r * 1.2, base.g * 1.2, base.b * 1.2, 1.0)
    } else {
        base
    };
    UxNode::boxed(
        Style::row()
            .button(ADD)
            .w(Dim::Px(72.0))
            .h(Dim::Px(42.0))
            .radius(8.0)
            .bg(bg)
            .align(Align::Center)
            .justify(Justify::Center),
        vec![UxNode::text("ADD", 14.0, white())],
    )
}

fn todo_row(i: usize, todo: &Todo) -> UxNode {
    let check = UxNode::boxed(
        Style::row()
            .button(CHECK_BASE + i as u32)
            .w(Dim::Px(26.0))
            .h(Dim::Px(26.0))
            .radius(6.0)
            .bg(if todo.done {
                Rgba::rgb8(52, 199, 130)
            } else {
                Rgba::rgb8(32, 36, 46)
            })
            .border(1.0, Rgba::rgb8(70, 76, 92)),
        vec![],
    );
    let label_color = if todo.done { muted() } else { white() };
    let spacer = UxNode::boxed(Style::row().w(Dim::Flex(1.0)).h(Dim::Px(1.0)), vec![]);
    let del = UxNode::boxed(
        Style::row()
            .button(DEL_BASE + i as u32)
            .w(Dim::Px(26.0))
            .h(Dim::Px(26.0))
            .radius(6.0)
            .bg(Rgba::rgb8(86, 56, 64))
            .align(Align::Center)
            .justify(Justify::Center),
        vec![UxNode::text("x", 14.0, Rgba::rgb8(240, 200, 205))],
    );
    UxNode::boxed(
        Style::row()
            .h(Dim::Px(44.0))
            .align(Align::Center)
            .gap(10.0)
            .pad(Edges::xy(8.0, 0.0))
            .radius(8.0)
            .bg(Rgba::rgb8(30, 33, 43)),
        vec![
            check,
            UxNode::text(todo.text.clone(), 15.0, label_color),
            spacer,
            del,
        ],
    )
}

fn build(todos: &[Todo], s: &UiState) -> UxNode {
    let header = UxNode::boxed(
        Style::row().h(Dim::Px(34.0)).align(Align::Center),
        vec![UxNode::text("MY TASKS", 22.0, white())],
    );
    let input_row = UxNode::boxed(
        Style::row().h(Dim::Px(42.0)).gap(10.0),
        vec![input_field(s), add_button(s)],
    );
    let rows: Vec<UxNode> = todos
        .iter()
        .enumerate()
        .map(|(i, t)| todo_row(i, t))
        .collect();
    let list = UxNode::boxed(
        Style::col()
            .scroll(LIST)
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .gap(8.0)
            .pad(Edges::all(8.0))
            .radius(10.0)
            .bg(Rgba::rgb8(20, 22, 30)),
        rows,
    );
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(16.0))
            .gap(12.0)
            .bg(BG),
        vec![header, input_row, list],
    )
}

fn center(todos: &[Todo], ui: &UiState, id: u32) -> Option<(f32, f32)> {
    let b = |s: &UiState| build(todos, s);
    widget_rect(&b, ui, id).map(|r| ((r.min.x + r.max.x) / 2.0, (r.min.y + r.max.y) / 2.0))
}

fn focus(todos: &[Todo], ui: &mut UiState, id: u32) {
    if let Some((x, y)) = center(todos, ui, id) {
        let b = |s: &UiState| build(todos, s);
        handle_event(ui, &b, UiEvent::PointerDown(x, y));
    }
}

fn type_text(todos: &[Todo], ui: &mut UiState, text: &str) {
    let b = |s: &UiState| build(todos, s);
    for c in text.chars() {
        handle_event(ui, &b, UiEvent::Char(c));
    }
}

fn click(todos: &[Todo], ui: &mut UiState, id: u32) {
    if let Some((x, y)) = center(todos, ui, id) {
        let b = |s: &UiState| build(todos, s);
        handle_event(ui, &b, UiEvent::PointerDown(x, y));
        handle_event(ui, &b, UiEvent::PointerUp(x, y));
    }
}

fn main() {
    let mut todos: Vec<Todo> = Vec::new();
    let mut ui = UiState::new(400, 540);

    // Add three tasks by focusing the field, typing, and clicking Add.
    for text in ["Buy milk", "Walk the dog", "Ship pmre 0.2"] {
        focus(&todos, &mut ui, NEW_INPUT);
        type_text(&todos, &mut ui, text);
        click(&todos, &mut ui, ADD);
        if ui.take_click() == Some(ADD) {
            todos.push(Todo {
                text: ui.input_text(NEW_INPUT).to_string(),
                done: false,
            });
            ui.clear_input(NEW_INPUT);
        }
    }

    // Check the first task off.
    click(&todos, &mut ui, CHECK_BASE);
    if let Some(cid) = ui.take_click() {
        if (CHECK_BASE..DEL_BASE).contains(&cid) {
            let i = (cid - CHECK_BASE) as usize;
            if let Some(t) = todos.get_mut(i) {
                t.done = !t.done;
            }
        }
    }

    assert_eq!(
        todos.len(),
        3,
        "three tasks should have been added by typing"
    );
    assert!(todos[0].done, "first task should be checked off");
    assert_eq!(todos[0].text, "Buy milk");
    assert_eq!(todos[2].text, "Ship pmre 0.2");

    let fb = {
        let b = |s: &UiState| build(&todos, s);
        render_ui(&b, &ui, BG)
    };
    std::fs::write("todo.bmp", fb.to_bmp(BG)).expect("write todo.bmp");
    println!(
        "todo: added {} tasks by typing through the engine; first checked off; wrote todo.bmp",
        todos.len()
    );
}
