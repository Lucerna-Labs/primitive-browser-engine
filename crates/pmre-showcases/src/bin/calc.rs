//! A real app built on the engine: a working calculator. A button grid + a display, driven
//! through the engine's own click path (hit-test -> clicked -> apply), with real arithmetic.
//!
//! This example is self-verifying: it presses `7 * 6 =` through synthetic pointer events and
//! asserts the display reads `42` before rendering, so a green run proves the click ->
//! state -> re-render loop actually works. The same `build`/`Calc` pair drops into the winit
//! `app` shell for live use.
//!
//! Run: cargo run -p pmre-showcases --bin calc

use pmre_kit::{
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::{handle_event, render_ui, widget_rect, UiEvent, UiState};

const BG: Rgba = Rgba::new(0.071, 0.078, 0.098, 1.0);

/// Button grid, row-major. Flattened index == widget id.
const ROWS: [&[&str]; 5] = [
    &["C", "<", "/", "*"],
    &["7", "8", "9", "-"],
    &["4", "5", "6", "+"],
    &["1", "2", "3", "="],
    &["0", "."],
];

fn keys() -> Vec<&'static str> {
    ROWS.iter().flat_map(|r| r.iter().copied()).collect()
}
fn id_of(key: &str) -> u32 {
    keys().iter().position(|k| *k == key).unwrap() as u32
}
fn key_of(id: u32) -> &'static str {
    keys()[id as usize]
}

struct Calc {
    display: String,
    acc: f64,
    pending: Option<char>,
    fresh: bool,
}

impl Calc {
    fn new() -> Self {
        Self {
            display: "0".into(),
            acc: 0.0,
            pending: None,
            fresh: true,
        }
    }
    fn value(&self) -> f64 {
        self.display.parse().unwrap_or(0.0)
    }
    fn compute(&mut self) {
        if let Some(op) = self.pending {
            let rhs = self.value();
            let r = match op {
                '+' => self.acc + rhs,
                '-' => self.acc - rhs,
                '*' => self.acc * rhs,
                '/' => {
                    if rhs != 0.0 {
                        self.acc / rhs
                    } else {
                        0.0
                    }
                }
                _ => rhs,
            };
            self.display = fmt_num(r);
        }
    }
    fn press(&mut self, key: &str) {
        match key {
            "C" => *self = Calc::new(),
            "<" => {
                if !self.fresh {
                    self.display.pop();
                    if self.display.is_empty() || self.display == "-" {
                        self.display = "0".into();
                        self.fresh = true;
                    }
                }
            }
            "." => {
                if self.fresh {
                    self.display = "0.".into();
                    self.fresh = false;
                } else if !self.display.contains('.') {
                    self.display.push('.');
                }
            }
            "+" | "-" | "*" | "/" => {
                self.compute();
                self.acc = self.value();
                self.pending = Some(key.chars().next().unwrap());
                self.fresh = true;
            }
            "=" => {
                self.compute();
                self.pending = None;
                self.fresh = true;
            }
            d if d.len() == 1 && d.chars().next().unwrap().is_ascii_digit() => {
                if self.fresh || self.display == "0" {
                    self.display = d.into();
                    self.fresh = false;
                } else {
                    self.display.push_str(d);
                }
            }
            _ => {}
        }
    }
}

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        (v as i64).to_string()
    } else {
        format!("{v:.6}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    }
}

fn button(s: &UiState, id: u32, label: &str) -> UxNode {
    let op = matches!(label, "/" | "*" | "-" | "+" | "=");
    let base = if matches!(label, "C" | "<") {
        Rgba::rgb8(90, 70, 80)
    } else if op {
        Rgba::rgb8(232, 140, 60)
    } else {
        Rgba::rgb8(54, 58, 72)
    };
    let bg = if s.is_pressed(id) {
        Rgba::new(base.r * 0.7, base.g * 0.7, base.b * 0.7, 1.0)
    } else if s.is_hover(id) {
        Rgba::new(base.r * 1.2, base.g * 1.2, base.b * 1.2, 1.0)
    } else {
        base
    };
    UxNode::boxed(
        Style::row()
            .button(id)
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .radius(10.0)
            .bg(bg)
            .align(Align::Center)
            .justify(Justify::Center),
        vec![UxNode::text(label, 22.0, Rgba::rgb8(240, 244, 250))],
    )
}

fn build(calc: &Calc, s: &UiState) -> UxNode {
    let display = UxNode::boxed(
        Style::row()
            .h(Dim::Px(84.0))
            .align(Align::Center)
            .justify(Justify::End)
            .pad(Edges::xy(18.0, 0.0))
            .radius(12.0)
            .bg(Rgba::rgb8(18, 20, 28)),
        vec![UxNode::text(
            calc.display.clone(),
            34.0,
            Rgba::rgb8(240, 244, 250),
        )],
    );

    let mut children = vec![display];
    let mut id = 0u32;
    for row in ROWS {
        let mut btns = Vec::new();
        for &label in row {
            btns.push(button(s, id, label));
            id += 1;
        }
        children.push(UxNode::boxed(
            Style::row().w(Dim::Flex(1.0)).h(Dim::Flex(1.0)).gap(10.0),
            btns,
        ));
    }

    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(14.0))
            .gap(10.0)
            .bg(BG),
        children,
    )
}

fn main() {
    let mut calc = Calc::new();
    let mut ui = UiState::new(300, 460);

    // Drive a real calculation through the engine's click path: 7 * 6 = 42.
    for key in ["7", "*", "6", "="] {
        let id = id_of(key);
        let center = {
            let b = |s: &UiState| build(&calc, s);
            widget_rect(&b, &ui, id).map(|r| ((r.min.x + r.max.x) / 2.0, (r.min.y + r.max.y) / 2.0))
        };
        if let Some((x, y)) = center {
            {
                let b = |s: &UiState| build(&calc, s);
                handle_event(&mut ui, &b, UiEvent::PointerDown(x, y));
                handle_event(&mut ui, &b, UiEvent::PointerUp(x, y));
            }
            if let Some(clicked) = ui.take_click() {
                calc.press(key_of(clicked));
            }
        }
    }

    assert_eq!(
        calc.display, "42",
        "7 * 6 should compute to 42 through the click path"
    );

    let fb = {
        let b = |s: &UiState| build(&calc, s);
        render_ui(&b, &ui, BG)
    };
    std::fs::write("calc.bmp", fb.to_bmp(BG)).expect("write calc.bmp");
    println!(
        "calc: 7 * 6 = {} (via the engine's click path); wrote calc.bmp",
        calc.display
    );
}
