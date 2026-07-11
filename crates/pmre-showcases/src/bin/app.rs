//! A live, interactive **todo app** with ZERO external crates. The engine renders pure math
//! into a CPU framebuffer; this runner drives a real OS window directly via raw Win32/GDI FFI
//! â€” no winit, no softbuffer, no dependencies at all. Type a task and press Enter (or click
//! ADD) to add it, click the circle to check it off, x to delete, wheel or drag the bar to scroll.
//!
//! Windows-only (it uses the Win32 API directly). The engine itself renders on every platform
//! with no dependencies â€” the other examples write images with no window.
//!
//! Run: cargo run -p pmre-showcases --bin app

#![allow(non_snake_case)]
#![allow(clippy::upper_case_acronyms)] // FFI type aliases mirror the Win32 names

use pmre_kit::{
    ux::{Align, Dim, Edges, Justify, Style, UxNode},
    Rgba,
};
use pmre_orchestrator::UiState;

// â”€â”€ Vercel / Next.js dark colour palette â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
const BG: Rgba = Rgba::new(0.035, 0.035, 0.043, 1.0); // zinc-950 #09090b

fn surface() -> Rgba {
    Rgba::rgb8(24, 24, 27)
} // zinc-900 #18181b â€” list container
fn elevated() -> Rgba {
    Rgba::rgb8(30, 30, 36)
} // between zinc-900/800 â€” row cards
fn border_subtle() -> Rgba {
    Rgba::rgb8(39, 39, 42)
} // zinc-800 #27272a
fn border_default() -> Rgba {
    Rgba::rgb8(63, 63, 70)
} // zinc-700 #3f3f46
fn text_hi() -> Rgba {
    Rgba::rgb8(250, 250, 250)
} // zinc-50
fn text_mid() -> Rgba {
    Rgba::rgb8(161, 161, 170)
} // zinc-400
fn text_lo() -> Rgba {
    Rgba::rgb8(113, 113, 122)
} // zinc-500
fn blue() -> Rgba {
    Rgba::rgb8(59, 130, 246)
} // blue-500 â€” focus ring, caret
fn emerald() -> Rgba {
    Rgba::rgb8(16, 185, 129)
} // emerald-500 â€” done state
fn red() -> Rgba {
    Rgba::rgb8(248, 113, 113)
} // red-400 â€” delete hover

const NEW_INPUT: u32 = 1;
const ADD: u32 = 2;
const LIST: u32 = 99;
const CHECK_BASE: u32 = 1000;
const DEL_BASE: u32 = 2000;

pub struct Todo {
    pub text: String,
    pub done: bool,
}

// â”€â”€ Header: title + count badge + subtitle â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn header(todos: &[Todo]) -> UxNode {
    let total = todos.len();
    let done = todos.iter().filter(|t| t.done).count();
    let sub = if total == 0 {
        "no tasks yet".to_string()
    } else if done == total {
        "all done".to_string()
    } else {
        format!("{} of {} done", done, total)
    };

    UxNode::boxed(
        Style::col().gap(6.0),
        vec![
            UxNode::boxed(
                Style::row().align(Align::Center).gap(10.0),
                vec![
                    UxNode::text("TASKS", 24.0, text_hi()),
                    // pill badge
                    UxNode::boxed(
                        Style::row()
                            .h(Dim::Px(22.0))
                            .align(Align::Center)
                            .justify(Justify::Center)
                            .pad(Edges::xy(8.0, 0.0))
                            .radius(11.0)
                            .bg(border_subtle()),
                        vec![UxNode::text(total.to_string(), 12.0, text_mid())],
                    ),
                ],
            ),
            UxNode::text(sub, 12.0, text_lo()),
        ],
    )
}

// â”€â”€ Input field â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn input_field(ui: &UiState) -> UxNode {
    let txt = ui.input_text(NEW_INPUT);
    let focused = ui.is_focused(NEW_INPUT);
    let placeholder = txt.is_empty() && !focused;
    let label = if placeholder {
        "add a new task..."
    } else {
        txt
    };
    let mut children = vec![UxNode::text(
        label,
        14.0,
        if placeholder { text_lo() } else { text_hi() },
    )];
    if focused {
        children.push(UxNode::boxed(
            Style::col().w(Dim::Px(2.0)).h(Dim::Px(18.0)).bg(blue()),
            vec![],
        ));
    }
    UxNode::boxed(
        Style::row()
            .input(NEW_INPUT)
            .w(Dim::Flex(1.0))
            .h(Dim::Px(44.0))
            .align(Align::Center)
            .pad(Edges::xy(14.0, 0.0))
            .radius(10.0)
            .bg(surface())
            .border(1.0, if focused { blue() } else { border_default() }),
        children,
    )
}

// â”€â”€ Add button â€” white bg, black text (Vercel primary style) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn add_button(s: &UiState) -> UxNode {
    let (bg, fg) = if s.is_pressed(ADD) {
        (Rgba::rgb8(200, 200, 200), BG)
    } else if s.is_hover(ADD) {
        (Rgba::rgb8(240, 240, 245), BG)
    } else {
        (text_hi(), BG)
    };
    UxNode::boxed(
        Style::row()
            .button(ADD)
            .w(Dim::Px(76.0))
            .h(Dim::Px(44.0))
            .radius(10.0)
            .bg(bg)
            .align(Align::Center)
            .justify(Justify::Center),
        vec![UxNode::text("ADD", 13.0, fg)],
    )
}

// â”€â”€ Checkbox â€” circle, filled emerald + white dot when done â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn checkbox(i: usize, todo: &Todo, s: &UiState) -> UxNode {
    let id = CHECK_BASE + i as u32;
    let (bg, border_col) = if todo.done {
        (emerald(), emerald()) // solid green circle
    } else if s.is_hover(id) {
        (elevated(), border_default()) // slightly brighter on hover
    } else {
        (elevated(), border_subtle()) // subtle outline
    };
    let inner: Vec<UxNode> = if todo.done {
        // small white dot centred inside â€” signals "checked"
        vec![UxNode::boxed(
            Style::row()
                .w(Dim::Px(8.0))
                .h(Dim::Px(8.0))
                .radius(4.0)
                .bg(Rgba::rgb8(255, 255, 255)),
            vec![],
        )]
    } else {
        vec![]
    };
    UxNode::boxed(
        Style::row()
            .button(id)
            .w(Dim::Px(22.0))
            .h(Dim::Px(22.0))
            .radius(11.0)
            .align(Align::Center)
            .justify(Justify::Center)
            .bg(bg)
            .border(1.5, border_col),
        inner,
    )
}

// â”€â”€ Delete button â€” dim Ã— that goes red on hover â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn del_button(i: usize, s: &UiState) -> UxNode {
    let id = DEL_BASE + i as u32;
    let col = if s.is_hover(id) { red() } else { text_lo() };
    UxNode::boxed(
        Style::row()
            .button(id)
            .w(Dim::Px(28.0))
            .h(Dim::Px(28.0))
            .radius(8.0)
            .align(Align::Center)
            .justify(Justify::Center),
        vec![UxNode::text("\u{00d7}", 15.0, col)],
    )
}

// â”€â”€ Todo row â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn todo_row(i: usize, todo: &Todo, s: &UiState) -> UxNode {
    let spacer = UxNode::boxed(Style::row().w(Dim::Flex(1.0)).h(Dim::Px(1.0)), vec![]);
    let label_col = if todo.done { text_lo() } else { text_hi() };
    UxNode::boxed(
        Style::row()
            .h(Dim::Px(52.0))
            .align(Align::Center)
            .gap(12.0)
            .pad(Edges::xy(14.0, 0.0))
            .radius(10.0)
            .bg(elevated())
            .border(1.0, border_subtle()),
        vec![
            checkbox(i, todo, s),
            UxNode::text(todo.text.clone(), 14.0, label_col),
            spacer,
            del_button(i, s),
        ],
    )
}

// â”€â”€ Scene builder â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn build(todos: &[Todo], s: &UiState) -> UxNode {
    let rows: Vec<UxNode> = todos
        .iter()
        .enumerate()
        .map(|(i, t)| todo_row(i, t, s))
        .collect();
    let list = UxNode::boxed(
        Style::col()
            .scroll(LIST)
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .gap(8.0)
            .pad(Edges::all(12.0))
            .radius(12.0)
            .bg(surface())
            .border(1.0, border_subtle())
            .shadow(0.0, 4.0, 14.0, Rgba::new(0.0, 0.0, 0.0, 0.35)),
        rows,
    );
    UxNode::boxed(
        Style::col()
            .w(Dim::Flex(1.0))
            .h(Dim::Flex(1.0))
            .pad(Edges::all(20.0))
            .gap(16.0)
            .bg(BG),
        vec![
            header(todos),
            UxNode::boxed(
                Style::row().h(Dim::Px(44.0)).gap(10.0),
                vec![input_field(s), add_button(s)],
            ),
            list,
        ],
    )
}

// â”€â”€ Business logic â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn add_todo(todos: &mut Vec<Todo>, ui: &mut UiState) {
    let text = ui.input_text(NEW_INPUT).trim().to_string();
    if !text.is_empty() {
        todos.push(Todo { text, done: false });
    }
    ui.clear_input(NEW_INPUT);
    ui.focused = Some(NEW_INPUT); // keep typing the next task
}

fn apply_click(todos: &mut Vec<Todo>, ui: &mut UiState, id: u32) {
    if id == ADD {
        add_todo(todos, ui);
    } else if (CHECK_BASE..DEL_BASE).contains(&id) {
        let i = (id - CHECK_BASE) as usize;
        if let Some(t) = todos.get_mut(i) {
            t.done = !t.done;
        }
    } else if id >= DEL_BASE {
        let i = (id - DEL_BASE) as usize;
        if i < todos.len() {
            todos.remove(i);
        }
    }
}

// â”€â”€ Entry points â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[cfg(windows)]
fn main() {
    win::run();
}

#[cfg(not(windows))]
fn main() {
    println!(
        "The live todo window uses the Win32 API and runs on Windows. The engine itself renders \
         on every platform with zero dependencies â€” run the headless examples (todo, calc, ui, \
         demo, paths, stroke, gradients, uxi, html) to see it draw to images."
    );
}

/// Direct OS windowing via raw FFI â€” no winit, no softbuffer, no crates.
#[cfg(windows)]
mod win {
    use super::{add_todo, apply_click, build, Todo, BG};
    use core::ffi::c_void;
    use pmre_kit::ux::UxNode;
    use pmre_orchestrator::{handle_event, render_ui_quality, Quality, UiEvent, UiState};
    use std::cell::RefCell;

    type HWND = *mut c_void;
    type HINSTANCE = *mut c_void;
    type HMENU = *mut c_void;
    type HDC = *mut c_void;
    type HICON = *mut c_void;
    type HCURSOR = *mut c_void;
    type HBRUSH = *mut c_void;
    type WPARAM = usize;
    type LPARAM = isize;
    type LRESULT = isize;
    type WndProc = Option<unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        proc_: WndProc,
        cls_extra: i32,
        wnd_extra: i32,
        instance: HINSTANCE,
        icon: HICON,
        cursor: HCURSOR,
        background: HBRUSH,
        menu_name: *const u16,
        class_name: *const u16,
    }
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct Rect {
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
    }
    #[repr(C)]
    struct Msg {
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        time: u32,
        pt: Point,
    }
    #[repr(C)]
    struct PaintStruct {
        hdc: HDC,
        erase: i32,
        paint: Rect,
        restore: i32,
        inc_update: i32,
        reserved: [u8; 32],
    }
    #[repr(C)]
    struct BitmapInfoHeader {
        size: u32,
        width: i32,
        height: i32,
        planes: u16,
        bit_count: u16,
        compression: u32,
        size_image: u32,
        x_ppm: i32,
        y_ppm: i32,
        clr_used: u32,
        clr_important: u32,
    }
    #[repr(C)]
    struct BitmapInfo {
        header: BitmapInfoHeader,
        colors: [u32; 1],
    }
    #[repr(C)]
    struct TrackMouseEventOpts {
        size: u32,
        flags: u32,
        hwnd: HWND,
        hover_time: u32,
    }

    #[link(name = "user32")]
    extern "system" {
        fn SetWindowTextW(hwnd: HWND, text: *const u16) -> i32;
        fn SetProcessDpiAwarenessContext(ctx: isize) -> i32;
        fn GetDpiForWindow(hwnd: HWND) -> u32;
        fn SetWindowPos(
            hwnd: HWND,
            after: HWND,
            x: i32,
            y: i32,
            cx: i32,
            cy: i32,
            flags: u32,
        ) -> i32;
        fn SetCapture(hwnd: HWND) -> HWND;
        fn ReleaseCapture() -> i32;
        fn TrackMouseEvent(t: *mut TrackMouseEventOpts) -> i32;
        fn RegisterClassW(c: *const WndClassW) -> u16;
        fn CreateWindowExW(
            ex: u32,
            class: *const u16,
            name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            w: i32,
            h: i32,
            parent: HWND,
            menu: HMENU,
            inst: HINSTANCE,
            param: *mut c_void,
        ) -> HWND;
        fn DefWindowProcW(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT;
        fn GetMessageW(msg: *mut Msg, hwnd: HWND, min: u32, max: u32) -> i32;
        fn TranslateMessage(msg: *const Msg) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> LRESULT;
        fn PostQuitMessage(code: i32);
        fn InvalidateRect(hwnd: HWND, rect: *const Rect, erase: i32) -> i32;
        fn BeginPaint(hwnd: HWND, ps: *mut PaintStruct) -> HDC;
        fn EndPaint(hwnd: HWND, ps: *const PaintStruct) -> i32;
        fn LoadCursorW(inst: HINSTANCE, name: *const u16) -> HCURSOR;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn StretchDIBits(
            hdc: HDC,
            xd: i32,
            yd: i32,
            wd: i32,
            hd: i32,
            xs: i32,
            ys: i32,
            ws: i32,
            hs: i32,
            bits: *const c_void,
            info: *const BitmapInfo,
            usage: u32,
            rop: u32,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> HINSTANCE;
    }

    const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const CW_USEDEFAULT: i32 = 0x8000_0000u32 as i32;
    const CS_HREDRAW: u32 = 0x0002;
    const CS_VREDRAW: u32 = 0x0001;
    const WM_DESTROY: u32 = 0x0002;
    const WM_PAINT: u32 = 0x000F;
    const WM_SIZE: u32 = 0x0005;
    const WM_KEYDOWN: u32 = 0x0100;
    const WM_MOUSEMOVE: u32 = 0x0200;
    const WM_LBUTTONDOWN: u32 = 0x0201;
    const WM_LBUTTONUP: u32 = 0x0202;
    const WM_MOUSEWHEEL: u32 = 0x020A;
    const WM_CHAR: u32 = 0x0102;
    const WM_DPICHANGED: u32 = 0x02E0;
    const WM_MOUSELEAVE: u32 = 0x02A3;
    const TME_LEAVE: u32 = 0x0000_0002;
    const SWP_NOZORDER: u32 = 0x0004;
    const SWP_NOACTIVATE: u32 = 0x0010;
    const SWP_NOMOVE: u32 = 0x0002;
    /// DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 — crisp pixels on any monitor.
    const DPI_PER_MONITOR_V2: isize = -4;
    const BI_RGB: u32 = 0;
    const DIB_RGB_COLORS: u32 = 0;
    const SRCCOPY: u32 = 0x00CC_0020;
    const IDC_ARROW: usize = 32512;

    struct App {
        width: u32,
        height: u32,
        ui: UiState,
        cursor: (f32, f32),
        todos: Vec<Todo>,
        quality: Quality,
        /// Whether a WM_MOUSELEAVE notification is currently armed.
        tracking_leave: bool,
    }

    thread_local! {
        static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn lo(l: LPARAM) -> f32 {
        ((l & 0xFFFF) as i16) as f32
    }
    fn hi(l: LPARAM) -> f32 {
        (((l >> 16) & 0xFFFF) as i16) as f32
    }

    /// Feed one event to the engine, then apply any resulting click / submit to the task
    /// list. Returns true when visible state changed (so pure mouse travel over inert
    /// pixels doesn't force a re-render).
    fn dispatch(ev: UiEvent) -> bool {
        APP.with(|cell| {
            if let Some(app) = cell.borrow_mut().as_mut() {
                let was_move = matches!(ev, UiEvent::PointerMove(..));
                let before = (app.ui.hover, app.ui.pressed, app.ui.drag);
                {
                    let b = |s: &UiState| build(&app.todos, s);
                    handle_event(&mut app.ui, &b, ev);
                }
                let mut dirty = !was_move || before != (app.ui.hover, app.ui.pressed, app.ui.drag);
                if app.ui.drag.is_some() {
                    dirty = true; // dragging the scrollbar moves content every event
                }
                if let Some(id) = app.ui.take_click() {
                    apply_click(&mut app.todos, &mut app.ui, id);
                    dirty = true;
                }
                if app.ui.take_submit().is_some() {
                    add_todo(&mut app.todos, &mut app.ui);
                    dirty = true;
                }
                dirty
            } else {
                false
            }
        })
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        match msg {
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_SIZE => {
                let w = (lp & 0xFFFF) as u32;
                let h = ((lp >> 16) & 0xFFFF) as u32;
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        app.width = w.max(1);
                        app.height = h.max(1);
                    }
                });
                dispatch(UiEvent::Resize(w.max(1), h.max(1)));
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
                // capture the mouse during a press so drags releasing outside the
                // window still deliver PointerUp (no stuck drag/pressed state)
                if msg == WM_LBUTTONDOWN {
                    SetCapture(hwnd);
                } else if msg == WM_LBUTTONUP {
                    ReleaseCapture();
                }
                // events are dispatched in logical units: physical / DPI scale
                let s = APP
                    .with(|c| c.borrow().as_ref().map(|a| a.ui.scale).unwrap_or(1.0))
                    .max(0.1);
                let (x, y) = (lo(lp) / s, hi(lp) / s);
                let arm_leave = APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        app.cursor = (x, y);
                        if !app.tracking_leave {
                            app.tracking_leave = true;
                            return true;
                        }
                    }
                    false
                });
                if arm_leave {
                    // ask for one WM_MOUSELEAVE so hover clears when the cursor exits
                    let mut tme = TrackMouseEventOpts {
                        size: std::mem::size_of::<TrackMouseEventOpts>() as u32,
                        flags: TME_LEAVE,
                        hwnd,
                        hover_time: 0,
                    };
                    TrackMouseEvent(&mut tme);
                }
                let dirty = dispatch(match msg {
                    WM_LBUTTONDOWN => UiEvent::PointerDown(x, y),
                    WM_LBUTTONUP => UiEvent::PointerUp(x, y),
                    _ => UiEvent::PointerMove(x, y),
                });
                if dirty {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_MOUSELEAVE => {
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        app.tracking_leave = false;
                    }
                });
                // a far-offscreen move hits nothing, clearing any hover highlight
                if dispatch(UiEvent::PointerMove(-1e6, -1e6)) {
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_MOUSEWHEEL => {
                let delta = (((wp >> 16) & 0xFFFF) as i16) as f32 / 120.0;
                let cursor = APP.with(|cell| {
                    cell.borrow()
                        .as_ref()
                        .map(|a| a.cursor)
                        .unwrap_or((0.0, 0.0))
                });
                dispatch(UiEvent::Wheel(cursor.0, cursor.1, -delta * 48.0));
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_DPICHANGED => {
                let dpi = (wp & 0xFFFF) as f32;
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        app.ui.scale = (dpi / 96.0).max(0.1);
                    }
                });
                // adopt the OS-suggested window rectangle for the new monitor
                let r = lp as *const Rect;
                if !r.is_null() {
                    let r = &*r;
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            WM_CHAR => {
                let code = wp as u32;
                let ev = match code {
                    8 => Some(UiEvent::Backspace),
                    13 => Some(UiEvent::Enter),
                    _ => char::from_u32(code)
                        .filter(|c| !c.is_control())
                        .map(UiEvent::Char),
                };
                if let Some(ev) = ev {
                    dispatch(ev);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                }
                0
            }
            WM_PAINT => {
                let mut ps: PaintStruct = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        let t0 = std::time::Instant::now();
                        let b: &dyn Fn(&UiState) -> UxNode = &|s| build(&app.todos, s);
                        let fb = render_ui_quality(b, &app.ui, BG, app.quality);
                        let ms = t0.elapsed().as_secs_f64() * 1000.0;
                        let px = fb.to_u32(BG);
                        let (w, h) = (app.width as i32, app.height as i32);
                        let bmi = BitmapInfo {
                            header: BitmapInfoHeader {
                                size: std::mem::size_of::<BitmapInfoHeader>() as u32,
                                width: w,
                                height: -h, // top-down
                                planes: 1,
                                bit_count: 32,
                                compression: BI_RGB,
                                size_image: 0,
                                x_ppm: 0,
                                y_ppm: 0,
                                clr_used: 0,
                                clr_important: 0,
                            },
                            colors: [0],
                        };
                        StretchDIBits(
                            hdc,
                            0,
                            0,
                            w,
                            h,
                            0,
                            0,
                            w,
                            h,
                            px.as_ptr() as *const c_void,
                            &bmi,
                            DIB_RGB_COLORS,
                            SRCCOPY,
                        );
                        let q = match app.quality {
                            Quality::Fast => "1:Fast",
                            Quality::Balanced => "2:Balanced",
                            Quality::Full => "3:Full",
                            Quality::GpuBalanced => "4:GpuBalanced",
                            Quality::GpuFull => "5:GpuFull",
                            Quality::ParallelBalanced => "6:ParBloom",
                            Quality::ParallelFull => "7:ParBigBloom",
                            Quality::TiledBalanced => "8:BusBloom",
                            Quality::TiledFull => "9:BusBigBloom",
                        };
                        let title = format!(
                            "Tasks  [{q}  {ms:.1}ms  {:.0}fps]  (1-9 to switch)",
                            1000.0 / ms
                        );
                        let wide: Vec<u16> =
                            title.encode_utf16().chain(std::iter::once(0)).collect();
                        SetWindowTextW(hwnd, wide.as_ptr());
                    }
                });
                EndPaint(hwnd, &ps);
                0
            }
            WM_KEYDOWN => {
                APP.with(|cell| {
                    if let Some(app) = cell.borrow_mut().as_mut() {
                        if app.ui.focused.is_some() {
                            return; // typing digits into an input must not switch tiers
                        }
                        app.quality = match wp {
                            0x31 => Quality::Fast,
                            0x32 => Quality::Balanced,
                            0x33 => Quality::Full,
                            0x34 => Quality::GpuBalanced,
                            0x35 => Quality::GpuFull,
                            0x36 => Quality::ParallelBalanced,
                            0x37 => Quality::ParallelFull,
                            0x38 => Quality::TiledBalanced,
                            0x39 => Quality::TiledFull,
                            _ => app.quality,
                        };
                    }
                });
                InvalidateRect(hwnd, std::ptr::null(), 0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    pub fn run() {
        unsafe {
            // Per-monitor DPI awareness: we render at native pixels, so Windows must not
            // bitmap-stretch us (that is what made the old window look blurry on hi-DPI).
            SetProcessDpiAwarenessContext(DPI_PER_MONITOR_V2);
            let class_name = wide("pmre_window");
            let title = wide("Tasks \u{2014} Primitive Math Rendering Engine");
            let hinst = GetModuleHandleW(std::ptr::null());
            let wc = WndClassW {
                style: CS_HREDRAW | CS_VREDRAW,
                proc_: Some(wndproc),
                cls_extra: 0,
                wnd_extra: 0,
                instance: hinst,
                icon: std::ptr::null_mut(),
                cursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16),
                background: std::ptr::null_mut(),
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
            };
            RegisterClassW(&wc);

            APP.with(|cell| {
                *cell.borrow_mut() = Some(App {
                    width: 480,
                    height: 640,
                    ui: UiState::new(480, 640),
                    cursor: (0.0, 0.0),
                    todos: Vec::new(),
                    quality: Quality::Fast,
                    tracking_leave: false,
                });
            });

            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                500, // outer width (client area + chrome â‰ˆ 480px)
                700, // outer height (client area + chrome â‰ˆ 640px)
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinst,
                std::ptr::null_mut(),
            );
            if hwnd.is_null() {
                eprintln!("CreateWindowExW failed");
                return;
            }
            // size the window for the monitor it actually opened on
            let dpi = GetDpiForWindow(hwnd);
            let s = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    app.ui.scale = s;
                }
            });
            if (s - 1.0).abs() > 1e-3 {
                SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    (500.0 * s) as i32,
                    (700.0 * s) as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE,
                );
            }
            InvalidateRect(hwnd, std::ptr::null(), 0);

            let mut msg: Msg = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}
