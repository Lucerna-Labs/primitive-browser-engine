//! # pbe-window — the primitive browser
//!
//! A real, navigable browser window: an address bar, Back/Forward/Reload
//! buttons, and the loaded page in a scrollable region — all composed from
//! `pmre-orchestrator`'s own interactive-UI system by [`pbe_shell::Browser`].
//! This binary only translates winit events into `UiEvent`s and presents the
//! resulting framebuffer via softbuffer; the browser itself owns all
//! navigation/chrome/scroll behavior.

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use pbe_shell::{Browser, Quality, UiEvent, PAGE_BG};

const START_W: u32 = 900;
const START_H: u32 = 650;

struct App {
    browser: Browser,
    quality: Quality,
    cursor: (f32, f32),
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl App {
    fn new(address: String, quality: Quality) -> Self {
        Self {
            browser: Browser::open(&address, START_W, START_H),
            quality,
            cursor: (0.0, 0.0),
            window: None,
            surface: None,
        }
    }

    fn render(&mut self) {
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .expect("surface resize");

        let fb = match self.quality {
            Quality::Fast => self.browser.render(),
            q => self.browser.render_with_quality(q),
        };
        let mut buffer = surface.buffer_mut().expect("surface buffer");
        buffer.copy_from_slice(&fb.to_u32(PAGE_BG));
        buffer.present().expect("present");

        window.set_title(&format!("Aegis pbe — {}", self.browser.label()));
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("Aegis pbe — {}", self.browser.label()))
            .with_inner_size(LogicalSize::new(START_W as f64, START_H as f64));
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));
        self.browser.ui.scale = window.scale_factor() as f32;
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(window) = self.window.clone() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::Resized(size) => {
                let logical = size.to_logical::<f32>(window.scale_factor());
                self.browser
                    .dispatch(UiEvent::Resize(logical.width as u32, logical.height as u32));
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.browser.ui.scale = scale_factor as f32;
                window.request_redraw();
            }
            WindowEvent::CursorMoved { position, .. } => {
                let logical = position.to_logical::<f32>(window.scale_factor());
                self.cursor = (logical.x, logical.y);
                self.browser
                    .dispatch(UiEvent::PointerMove(logical.x, logical.y));
                window.request_redraw();
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let ev = match state {
                    ElementState::Pressed => UiEvent::PointerDown(self.cursor.0, self.cursor.1),
                    ElementState::Released => UiEvent::PointerUp(self.cursor.0, self.cursor.1),
                };
                self.browser.dispatch(ev);
                window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 48.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32),
                };
                self.browser
                    .dispatch(UiEvent::Wheel(self.cursor.0, self.cursor.1, dy));
                window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match event.logical_key {
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::Backspace) => {
                        self.browser.dispatch(UiEvent::Backspace);
                        window.request_redraw();
                    }
                    Key::Named(NamedKey::Enter) => {
                        self.browser.dispatch(UiEvent::Enter);
                        window.request_redraw();
                    }
                    _ => {
                        if let Some(text) = &event.text {
                            for c in text.chars().filter(|c| !c.is_control()) {
                                self.browser.dispatch(UiEvent::Char(c));
                            }
                            window.request_redraw();
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_quality(s: &str) -> Option<Quality> {
    match s {
        "fast" => Some(Quality::Fast),
        "balanced" => Some(Quality::Balanced),
        "full" => Some(Quality::Full),
        "tiled-balanced" => Some(Quality::TiledBalanced),
        "tiled-full" => Some(Quality::TiledFull),
        "parallel-balanced" => Some(Quality::ParallelBalanced),
        "parallel-full" => Some(Quality::ParallelFull),
        "gpu-balanced" => Some(Quality::GpuBalanced),
        "gpu-full" => Some(Quality::GpuFull),
        _ => None,
    }
}

fn main() {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut address: Option<String> = None;
    let mut quality = Quality::Fast;
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--url" => address = it.next().cloned(),
            "--quality" => match it.next().and_then(|s| parse_quality(s)) {
                Some(q) => quality = q,
                None => {
                    eprintln!(
                        "--quality expects one of: fast | balanced | full | tiled-balanced | \
                         tiled-full | parallel-balanced | parallel-full | gpu-balanced | gpu-full"
                    );
                    std::process::exit(2);
                }
            },
            other if address.is_none() => address = Some(other.to_string()),
            _ => {}
        }
    }
    let Some(address) = address else {
        eprintln!(
            "usage: pbe-window <html-file> | pbe-window --url <URL> \
             [--quality fast|balanced|full|tiled-full|...]"
        );
        eprintln!("click the address bar to type a new URL/path, Enter to navigate");
        std::process::exit(2);
    };

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new(address, quality);
    event_loop.run_app(&mut app).expect("run app");
}
