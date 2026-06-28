//! # pbe-window — the windowed browser shell
//!
//! Presents the engine's rendered frames in a real OS window. This is the step
//! from "writes a PNG" to "a browser you can look at and scroll": it loads a
//! page (local file or `--url` over the sealed-curl fetch), renders it with the
//! same pipeline the bus stages use (`pbe_stages::render_to_rgba`), and blits
//! the CPU framebuffer to the window via softbuffer. Resize re-renders at the
//! new size; the mouse wheel scrolls; `R` reloads the source.
//!
//! No GPU dependency — the software rasterizer already produces RGBA; the GPU
//! backend (ordo-ux-vello) is a later swap of the render step, not needed here.

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

/// Where the page came from, so `R` can reload it.
enum PageSource {
    File(String),
    Url(String),
}

impl PageSource {
    /// Load (or reload) the page's HTML. Returns (html, label).
    fn load(&self) -> (String, String) {
        match self {
            PageSource::File(path) => {
                let html = std::fs::read_to_string(path)
                    .unwrap_or_else(|e| error_page(&format!("cannot read {path}: {e}")));
                (html, path.clone())
            }
            PageSource::Url(url) => match pbe_net::fetch(url) {
                Ok(page) => (page.body, page.final_url),
                Err(e) => (error_page(&format!("fetch failed: {e:?}")), url.clone()),
            },
        }
    }
}

/// A minimal styled error document so failures are visible in-window.
fn error_page(msg: &str) -> String {
    format!(
        "<html><body><style>body{{background-color:#fff3f3}} \
         h1{{color:#b00020}} p{{color:#333}}</style>\
         <h1>Load error</h1><p>{msg}</p></body></html>"
    )
}

struct App {
    source: PageSource,
    html: String,
    label: String,
    scroll_y: f32,
    content_height: f32,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
}

impl App {
    fn new(source: PageSource) -> Self {
        let (html, label) = source.load();
        Self {
            source,
            html,
            label,
            scroll_y: 0.0,
            content_height: 0.0,
            window: None,
            surface: None,
        }
    }

    fn reload(&mut self) {
        let (html, label) = self.source.load();
        self.html = html;
        self.label = label;
        self.scroll_y = 0.0;
        if let Some(w) = &self.window {
            w.set_title(&format!("Aegis pbe — {}", self.label));
            w.request_redraw();
        }
    }

    /// Render the current page at the window's size and blit to the surface.
    fn render(&mut self) {
        let (Some(window), Some(surface)) = (self.window.as_ref(), self.surface.as_mut()) else {
            return;
        };
        let size = window.inner_size();
        let (w, h) = (size.width.max(1), size.height.max(1));

        // Clamp scroll to content.
        let max_scroll = (self.content_height - h as f32).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_scroll);

        let (rgba, content_height) =
            pbe_stages::render_to_rgba(&self.html, "", w, h, self.scroll_y);
        self.content_height = content_height;

        surface
            .resize(NonZeroU32::new(w).unwrap(), NonZeroU32::new(h).unwrap())
            .expect("surface resize");
        let mut buffer = surface.buffer_mut().expect("surface buffer");

        // softbuffer wants 0RGB u32 per pixel; our raster is RGBA8 bytes.
        for (i, px) in buffer.iter_mut().enumerate() {
            let o = i * 4;
            if o + 2 < rgba.len() {
                let r = rgba[o] as u32;
                let g = rgba[o + 1] as u32;
                let b = rgba[o + 2] as u32;
                *px = (r << 16) | (g << 8) | b;
            }
        }
        buffer.present().expect("present");
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(format!("Aegis pbe — {}", self.label))
            .with_inner_size(LogicalSize::new(800.0, 600.0));
        let window = Rc::new(event_loop.create_window(attrs).expect("create window"));
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.window = Some(window);
        self.surface = Some(surface);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => self.render(),
            WindowEvent::Resized(_) => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y * 48.0,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                self.scroll_y -= dy;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state.is_pressed() => {
                match event.logical_key {
                    Key::Character(ref c) if c.eq_ignore_ascii_case("r") => self.reload(),
                    Key::Named(NamedKey::Escape) => event_loop.exit(),
                    Key::Named(NamedKey::PageDown) => {
                        self.scroll_y += 400.0;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    Key::Named(NamedKey::PageUp) => {
                        self.scroll_y -= 400.0;
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let source = match args.as_slice() {
        [flag, url, ..] if flag == "--url" => PageSource::Url(url.clone()),
        [path, ..] => PageSource::File(path.clone()),
        [] => {
            eprintln!("usage: pbe-window <html-file> | pbe-window --url <URL>");
            eprintln!("keys: scroll = wheel/PageUp/PageDown, R = reload, Esc = quit");
            std::process::exit(2);
        }
    };

    let event_loop = EventLoop::new().expect("event loop");
    let mut app = App::new(source);
    event_loop.run_app(&mut app).expect("run app");
}
