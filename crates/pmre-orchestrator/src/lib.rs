//! pmre-orchestrator — the single orchestrator. ALL policy, no mechanism.
//!
//! It owns draw order, the empty-slot check, the interaction state machine (hover / press /
//! click / toggle / scroll), and resize. It drives `pmre-kit`; it never rasterizes a pixel
//! itself. Two render paths sit on the same kit: `render`/`render_uxi`/`render_html` for
//! static frames, and the stateful `render_ui` + `handle_event` for interactive UIs.

use std::collections::HashMap;

pub mod gpu_bloom;

use pmre_kit::{
    atoms,
    bloom_sweep::{bloom_with, Arith, Dispatch, Strategy, Structure},
    framebuffer::{BandView, Framebuffer, Surface},
    geom::{Affine, Vec2},
    html,
    layout::{self, LaidBox, Painted},
    paint::{Bounds, Paint, Rgba, Shape},
    post, raster, text,
    ux::{Role, UxNode},
    DrawCmd,
};

// ----------------------------------------------------------------------------
// Static shape scene (used by the SDF demo)
// ----------------------------------------------------------------------------

/// A draw command plus its painter depth (`z`): larger `z` is nearer / drawn on top.
pub struct Item {
    pub z: f32,
    pub cmd: DrawCmd,
}

/// An ordered set of draw commands plus the surface to render them onto.
pub struct Scene {
    pub width: u32,
    pub height: u32,
    pub clear: Rgba,
    pub items: Vec<Item>,
}

impl Scene {
    pub fn new(width: u32, height: u32, clear: Rgba) -> Self {
        Self {
            width,
            height,
            clear,
            items: Vec::new(),
        }
    }
    pub fn push(&mut self, z: f32, cmd: DrawCmd) {
        self.items.push(Item { z, cmd });
    }
}

/// Render the scene with the painter's algorithm (the `order` atom, back-to-front).
pub fn render(scene: &Scene) -> Framebuffer {
    let mut fb = Framebuffer::new(scene.width, scene.height, scene.clear);
    for i in atoms::order(&scene.items, |it| -it.z) {
        let item = &scene.items[i];
        if item.cmd.shape.is_degenerate() {
            continue;
        }
        raster::scan_convert(&item.cmd, &mut fb, None);
    }
    fb
}

// ----------------------------------------------------------------------------
// Shared box-tree painting
// ----------------------------------------------------------------------------

/// Paint one laid-out box (its shape commands, or its wrapped text) into `surf`, in
/// absolute device coordinates. Generic over the sink so it targets a full framebuffer or
/// a single row-band of one.
fn paint_one_box<S: Surface>(surf: &mut S, laid: &LaidBox) {
    match &laid.kind {
        Painted::Box { .. } => {
            let mut cmds: Vec<DrawCmd> = Vec::new();
            layout::cmds_for(laid, &mut cmds);
            for cmd in &cmds {
                if !cmd.shape.is_degenerate() {
                    raster::scan_convert(cmd, surf, laid.clip);
                }
            }
        }
        Painted::Text {
            content,
            size,
            color,
        } => {
            let max_w = laid.rect.max.x - laid.rect.min.x;
            let line_h = *size * 1.3;
            let (asc, desc) = text::v_metrics(*size);
            let mut y = laid.rect.min.y;
            for line in text::wrap(content, *size, max_w) {
                // center the font's ascent+descent box inside the line box
                let origin = Vec2::new(laid.rect.min.x, y + (line_h - (asc + desc)) * 0.5);
                text::draw(surf, &line, origin, *size, *color, laid.clip);
                y += line_h;
            }
        }
        Painted::Image { image } => {
            let clip = laid.clip.unwrap_or(pmre_kit::Bounds {
                min: pmre_kit::Vec2::new(0.0, 0.0),
                max: pmre_kit::Vec2::new(surf.width() as f32, surf.height() as f32),
            });
            raster::blit_image(surf, laid.rect, image, clip);
        }
        Painted::Rich { spans, align } => {
            let max_w = laid.rect.max.x - laid.rect.min.x;
            let (lines, line_h) = layout::rich_lines(spans, Some(max_w));
            let mut y = laid.rect.min.y;
            for line in &lines {
                let x0 = match align {
                    pmre_kit::ux::Align::Center => laid.rect.min.x + (max_w - line.width) * 0.5,
                    pmre_kit::ux::Align::End => laid.rect.max.x - line.width,
                    _ => laid.rect.min.x,
                };
                // All pieces on a line share ONE baseline (set by the tallest piece);
                // mixed sizes must sit on it, not center independently.
                let max_size = line.pieces.iter().map(|p| p.size).fold(1.0f32, f32::max);
                let (asc_l, desc_l) = text::v_metrics(max_size);
                let baseline = y + (line_h - (asc_l + desc_l)) * 0.5 + asc_l;
                for p in &line.pieces {
                    let (asc_p, _) = text::v_metrics_styled(p.size, p.bold);
                    let origin = Vec2::new(x0 + p.x, baseline - asc_p);
                    text::draw_styled(
                        surf,
                        &p.text,
                        origin,
                        p.size,
                        p.color,
                        laid.clip,
                        p.bold,
                        p.underline,
                    );
                }
                y += line_h;
            }
        }
    }
}

fn paint_boxes<S: Surface>(surf: &mut S, boxes: &[LaidBox]) {
    for laid in boxes {
        paint_one_box(surf, laid);
    }
}

fn cpu_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// Coverage anti-aliasing margin (px): a box edge bleeds at most ~2px past its rect, so a
/// lane must rasterize boxes within this margin of its rows to avoid a seam.
const BAND_PAD: f32 = 3.0;

/// The vertical device rows a box can actually touch: its rect, expanded by shadow bleed
/// and by wrapped-text overflow, then narrowed by its clip. Lanes use this (plus
/// `BAND_PAD`) to decide which boxes reach their band — a shadow blurring 14px past a
/// card must still be painted by the lane below the card, or the frame seams.
fn paint_y_extent(b: &LaidBox) -> (f32, f32) {
    let (mut lo, mut hi) = (b.rect.min.y, b.rect.max.y);
    match &b.kind {
        Painted::Box { shadow, .. } => {
            if let Some(sh) = shadow {
                let bleed = sh.blur + sh.dy.abs() + 2.0;
                lo -= bleed;
                hi += bleed;
            }
        }
        Painted::Text { content, size, .. } => {
            let max_w = b.rect.max.x - b.rect.min.x;
            let lines = text::wrap(content, *size, max_w).len().max(1);
            hi = hi.max(b.rect.min.y + lines as f32 * *size * 1.3);
        }
        Painted::Rich { spans, .. } => {
            let max_w = b.rect.max.x - b.rect.min.x;
            let (lines, line_h) = layout::rich_lines(spans, Some(max_w));
            hi = hi.max(b.rect.min.y + lines.len() as f32 * line_h);
        }
        // An image blit paints inside its own rect only; no shadow or wrap
        // bleed, so the base rect y-range is the exact extent.
        Painted::Image { .. } => {}
    }
    if let Some(c) = b.clip {
        lo = lo.max(c.min.y);
        hi = hi.min(c.max.y);
    }
    (lo, hi)
}

/// Rasterize `boxes` into a fresh framebuffer, parallelized with the MM3E "lane / bus"
/// model: the frame's pixel buffer is split into contiguous row-bands (one per lane), and
/// each lane rasterizes — in absolute device coordinates — straight into its own band slice
/// via a [`BandView`]. No per-band temp buffer, no stitch, no coordinate translation; bands
/// are disjoint so lanes never alias. Output is bit-identical to the serial render and
/// independent of thread count. No locks, no atomics, no `unsafe`.
fn paint_boxes_banded(w: u32, h: u32, clear: Rgba, boxes: &[LaidBox]) -> Framebuffer {
    let n = cpu_threads();
    let mut fb = Framebuffer::new(w, h, clear);
    if n <= 1 || w == 0 || h < 2 * n as u32 {
        paint_boxes(&mut fb, boxes);
        return fb;
    }

    let band = (h as usize).div_ceil(n);
    let wsz = w as usize;
    let extents: Vec<(f32, f32)> = boxes.iter().map(paint_y_extent).collect();
    let extents = &extents;
    std::thread::scope(|s| {
        for (ti, chunk) in fb.pixels_mut().chunks_mut(band * wsz).enumerate() {
            let y0 = (ti * band) as u32;
            let band_h = (chunk.len() / wsz) as u32;
            // A lane only rasterizes boxes whose paint extent (plus AA bleed) reaches it.
            let (lo, hi) = (y0 as f32 - BAND_PAD, (y0 + band_h) as f32 + BAND_PAD);
            s.spawn(move || {
                let mut view = BandView::new(chunk, w, y0, h);
                for (b, &(elo, ehi)) in boxes.iter().zip(extents) {
                    if ehi >= lo && elo <= hi {
                        paint_one_box(&mut view, b);
                    }
                }
            });
        }
    });
    fb
}

fn viewport(w: u32, h: u32) -> Bounds {
    Bounds {
        min: Vec2::new(0.0, 0.0),
        max: Vec2::new(w as f32, h as f32),
    }
}

/// Render a UXI tree (no interaction). Reduced layout → identical raster path.
pub fn render_uxi(root: &UxNode, width: u32, height: u32, clear: Rgba) -> Framebuffer {
    let boxes = layout::solve(root, viewport(width, height), &|_| 0.0);
    paint_boxes_banded(width, height, clear, &boxes)
}

/// Single-threaded variant of [`render_uxi`] — for profiling/baselining the lane render.
/// Output is bit-identical to [`render_uxi`] (a test enforces this).
pub fn render_uxi_serial(root: &UxNode, width: u32, height: u32, clear: Rgba) -> Framebuffer {
    let mut fb = Framebuffer::new(width, height, clear);
    let boxes = layout::solve(root, viewport(width, height), &|_| 0.0);
    paint_boxes(&mut fb, &boxes);
    fb
}

/// Render an HTML/CSS document: the reduced front-end parses it into the same box tree.
pub fn render_html(src: &str, width: u32, height: u32, clear: Rgba) -> Framebuffer {
    render_uxi(&html::parse(src), width, height, clear)
}

/// Post-processing quality tier.
#[derive(Clone, Copy)]
pub enum Quality {
    /// Pure SDF rasterization, no post-processing.
    Fast,
    /// CPU additive Gaussian bloom at σ=3, radius=6.
    Balanced,
    /// CPU additive Gaussian bloom at σ=5, radius=12.
    Full,
    /// GPU-accelerated bloom at σ=3, radius=6 (falls back to CPU if no adapter).
    GpuBalanced,
    /// GPU-accelerated bloom at σ=5, radius=12 (falls back to CPU if no adapter).
    GpuFull,
    /// Parallel CPU bloom at σ=3, radius=6 using FairQueue across all CPU threads.
    ParallelBalanced,
    /// Parallel CPU bloom at σ=5, radius=12 using FairQueue across all CPU threads.
    ParallelFull,
    /// Cache-tiled fused CPU bloom at σ=3, radius=6 (FairQueue over tiles). Fastest CPU tier.
    TiledBalanced,
    /// Cache-tiled fused CPU bloom at σ=5, radius=12 (FairQueue over tiles). Fastest CPU tier.
    TiledFull,
}

/// Render a UXI tree then apply the post-processing pipeline for `quality`.
pub fn render_uxi_quality(
    root: &UxNode,
    width: u32,
    height: u32,
    clear: Rgba,
    quality: Quality,
) -> Framebuffer {
    let mut fb = render_uxi(root, width, height, clear);
    apply_quality(&mut fb, quality);
    fb
}

/// Render an interactive UI tree then apply the post-processing pipeline for `quality`.
pub fn render_ui_quality(
    build: &dyn Fn(&UiState) -> UxNode,
    state: &UiState,
    clear: Rgba,
    quality: Quality,
) -> Framebuffer {
    let mut fb = render_ui(build, state, clear);
    apply_quality(&mut fb, quality);
    fb
}

fn apply_quality(fb: &mut Framebuffer, quality: Quality) {
    match quality {
        Quality::Fast => {}
        Quality::Balanced => post::bloom(fb, 0.45, 3.0, 6),
        Quality::Full => post::bloom(fb, 0.45, 5.0, 12),
        Quality::GpuBalanced => gpu_bloom::gpu_bloom(fb, 0.45, 3.0, 6),
        Quality::GpuFull => gpu_bloom::gpu_bloom(fb, 0.45, 5.0, 12),
        Quality::ParallelBalanced => post::bloom_parallel(fb, 0.45, 3.0, 6),
        Quality::ParallelFull => post::bloom_parallel(fb, 0.45, 5.0, 12),
        Quality::TiledBalanced => bloom_with(fb, 0.45, 3.0, 6, tiled_strategy()),
        Quality::TiledFull => bloom_with(fb, 0.45, 5.0, 12, tiled_strategy()),
    }
}

/// The MM3E "lane / bus" model applied to bloom: each thread owns one contiguous
/// strip of fused cache tiles, writing its own region — no locks, no atomics, no
/// `unsafe` aliasing, deterministic in thread count. Within ~6–12% of the fastest
/// (FairQueue) dispatch in `examples/sweep`, and the only fast path that is fully
/// lock-free and order-independent. SIMD inner loop edges out scalar for this dispatch.
fn tiled_strategy() -> Strategy {
    Strategy::new(Dispatch::Band, Structure::TiledFused, Arith::Simd)
}

// ----------------------------------------------------------------------------
// Interactive UI: state, events, stateful render
// ----------------------------------------------------------------------------

/// All UI interaction state. The app's `build(&UiState) -> UxNode` reads this to style
/// widgets (hover/press/toggle) so the tree always reflects current state.
///
/// `width`/`height` are **physical** pixels; `scale` is the DPI factor (1.0 = 96 dpi).
/// Layout solves in logical units (`width / scale`) and painting multiplies back up,
/// so the same tree renders crisply on any monitor. Events are fed in logical units.
pub struct UiState {
    pub width: u32,
    pub height: u32,
    /// Device-pixel ratio: physical px per logical px. Always ≥ a small epsilon.
    pub scale: f32,
    pub hover: Option<u32>,
    pub pressed: Option<u32>,
    pub clicked: Option<u32>,
    pub toggles: HashMap<u32, bool>,
    pub scrolls: HashMap<u32, f32>,
    /// Scroll region whose scrollbar thumb is currently being dragged.
    pub drag: Option<u32>,
    /// Pointer offset from the thumb's top edge at grab time, so the thumb doesn't
    /// jump to center itself under the cursor.
    pub drag_grab: f32,
    /// The focused text input, if any.
    pub focused: Option<u32>,
    /// Text contents per input field id.
    pub inputs: HashMap<u32, String>,
    /// Input field that received Enter since it was last polled.
    pub submit: Option<u32>,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            width: 0,
            height: 0,
            scale: 1.0,
            hover: None,
            pressed: None,
            clicked: None,
            toggles: HashMap::new(),
            scrolls: HashMap::new(),
            drag: None,
            drag_grab: 0.0,
            focused: None,
            inputs: HashMap::new(),
            submit: None,
        }
    }
}

impl UiState {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            ..Self::default()
        }
    }
    pub fn is_hover(&self, id: u32) -> bool {
        self.hover == Some(id)
    }
    pub fn is_pressed(&self, id: u32) -> bool {
        self.pressed == Some(id)
    }
    pub fn toggle_on(&self, id: u32) -> bool {
        self.toggles.get(&id).copied().unwrap_or(false)
    }
    pub fn scroll_of(&self, id: u32) -> f32 {
        self.scrolls.get(&id).copied().unwrap_or(0.0)
    }
    /// True exactly once for the widget clicked on the most recent PointerUp.
    pub fn take_click(&mut self) -> Option<u32> {
        self.clicked.take()
    }
    pub fn is_focused(&self, id: u32) -> bool {
        self.focused == Some(id)
    }
    pub fn input_text(&self, id: u32) -> &str {
        self.inputs.get(&id).map(String::as_str).unwrap_or("")
    }
    pub fn clear_input(&mut self, id: u32) {
        self.inputs.remove(&id);
    }
    /// The input field that received Enter since the last poll.
    pub fn take_submit(&mut self) -> Option<u32> {
        self.submit.take()
    }
}

/// Pointer / window events fed to `handle_event`.
pub enum UiEvent {
    Resize(u32, u32),
    PointerMove(f32, f32),
    PointerDown(f32, f32),
    PointerUp(f32, f32),
    /// Vertical wheel: cursor position and a positive-down delta in pixels.
    Wheel(f32, f32, f32),
    /// A typed character routed to the focused input.
    Char(char),
    /// Delete the last character of the focused input.
    Backspace,
    /// Enter pressed; marks the focused input as submitted.
    Enter,
}

/// Solve layout in **logical** units (physical size divided by the DPI scale).
fn solve_for(build: &dyn Fn(&UiState) -> UxNode, state: &UiState) -> Vec<LaidBox> {
    let s = state.scale.max(0.1);
    let tree = build(state);
    let vp = Bounds {
        min: Vec2::new(0.0, 0.0),
        max: Vec2::new(state.width as f32 / s, state.height as f32 / s),
    };
    layout::solve(&tree, vp, &|id| state.scroll_of(id))
}

fn scale_bounds(b: Bounds, s: f32) -> Bounds {
    Bounds {
        min: Vec2::new(b.min.x * s, b.min.y * s),
        max: Vec2::new(b.max.x * s, b.max.y * s),
    }
}

/// Multiply solved logical boxes up to physical pixels for painting.
fn scale_boxes(boxes: &mut [LaidBox], s: f32) {
    if (s - 1.0).abs() < 1e-6 {
        return;
    }
    for b in boxes {
        b.rect = scale_bounds(b.rect, s);
        b.clip = b.clip.map(|c| scale_bounds(c, s));
        b.content_len *= s;
        match &mut b.kind {
            Painted::Box {
                radius,
                border,
                shadow,
                ..
            } => {
                *radius *= s;
                if let Some((w, _)) = border {
                    *w *= s;
                }
                if let Some(sh) = shadow {
                    sh.dx *= s;
                    sh.dy *= s;
                    sh.blur *= s;
                }
            }
            Painted::Text { size, .. } => *size *= s,
            Painted::Rich { spans, .. } => {
                for sp in spans {
                    sp.size *= s;
                }
            }
            // Image pixel data doesn't scale — the rect above already scaled.
            // blit_image resamples on the fly against the (possibly rescaled)
            // dst rect, so no per-image mutation is needed.
            Painted::Image { .. } => {}
        }
    }
}

fn rect_contains(b: Bounds, x: f32, y: f32) -> bool {
    x >= b.min.x && x < b.max.x && y >= b.min.y && y < b.max.y
}

/// Scrollbar track + thumb geometry for a scroll region: `(bar_x, track_top, track_h,
/// thumb_y, thumb_h, max_scroll)`. `None` when there is nothing to scroll. `s` is the
/// unit scale of `b` (1.0 for logical boxes, the DPI factor for painted boxes) so the
/// fixed insets stay proportional and the two spaces agree exactly.
fn scrollbar_geom(b: &LaidBox, scroll: f32, s: f32) -> Option<(f32, f32, f32, f32, f32, f32)> {
    if b.role != Role::Scroll {
        return None;
    }
    let view_h = b.rect.max.y - b.rect.min.y;
    let max = (b.content_len - view_h).max(0.0);
    if max <= 0.0 {
        return None;
    }
    let track_top = b.rect.min.y + 4.0 * s;
    let track_h = (view_h - 8.0 * s).max(1.0);
    let bar_x = b.rect.max.x - 7.0 * s;
    let thumb_h = (view_h / b.content_len * track_h).clamp((24.0 * s).min(track_h), track_h);
    let t = (scroll / max).clamp(0.0, 1.0);
    let thumb_y = track_top + t * (track_h - thumb_h);
    Some((bar_x, track_top, track_h, thumb_y, thumb_h, max))
}

/// The solved rectangle of the box with the given id under the current state.
/// Useful for placing synthetic events and for tests.
pub fn widget_rect(build: &dyn Fn(&UiState) -> UxNode, state: &UiState, id: u32) -> Option<Bounds> {
    solve_for(build, state)
        .into_iter()
        .find(|b| b.id == Some(id))
        .map(|b| b.rect)
}

/// Advance the interaction state machine by one event. `build` produces the current tree.
pub fn handle_event(state: &mut UiState, build: &dyn Fn(&UiState) -> UxNode, ev: UiEvent) {
    match ev {
        UiEvent::Resize(w, h) => {
            state.width = w;
            state.height = h;
        }
        UiEvent::PointerMove(x, y) => {
            if let Some(id) = state.drag {
                let boxes = solve_for(build, state);
                if let Some(b) = boxes.iter().find(|b| b.id == Some(id)) {
                    if let Some((_bx, track_top, track_h, _ty, thumb_h, max)) =
                        scrollbar_geom(b, state.scroll_of(id), 1.0)
                    {
                        let denom = (track_h - thumb_h).max(1e-3);
                        let t = ((y - track_top - state.drag_grab) / denom).clamp(0.0, 1.0);
                        state.scrolls.insert(id, t * max);
                    }
                }
                return;
            }
            let boxes = solve_for(build, state);
            state.hover = layout::hit_test(&boxes, x, y).map(|(id, _)| id);
        }
        UiEvent::PointerDown(x, y) => {
            let boxes = solve_for(build, state);
            state.drag = None;
            for b in &boxes {
                let Some(id) = b.id else { continue };
                if let Some((bar_x, _tt, _th, thumb_y, thumb_h, _max)) =
                    scrollbar_geom(b, state.scroll_of(id), 1.0)
                {
                    if x >= bar_x - 4.0
                        && x <= bar_x + 8.0
                        && y >= thumb_y
                        && y <= thumb_y + thumb_h
                    {
                        state.drag = Some(id);
                        state.drag_grab = y - thumb_y;
                    }
                }
            }
            let hit = layout::hit_test(&boxes, x, y);
            // Clicking a text input focuses it; clicking anything else clears focus.
            state.focused = match hit {
                Some((id, Role::Input)) => Some(id),
                _ => None,
            };
            if state.drag.is_some() {
                state.pressed = None;
            } else {
                state.pressed = hit.map(|(id, _)| id);
            }
        }
        UiEvent::PointerUp(x, y) => {
            if state.drag.is_some() {
                state.drag = None;
                state.pressed = None;
                return;
            }
            let boxes = solve_for(build, state);
            state.clicked = None;
            if let (Some((up_id, role)), Some(pressed)) =
                (layout::hit_test(&boxes, x, y), state.pressed)
            {
                if up_id == pressed {
                    state.clicked = Some(up_id);
                    if role == Role::Toggle {
                        let now = state.toggle_on(up_id);
                        state.toggles.insert(up_id, !now);
                    }
                }
            }
            state.pressed = None;
        }
        UiEvent::Wheel(x, y, delta) => {
            let boxes = solve_for(build, state);
            // Topmost scroll region under the cursor.
            let mut target: Option<(u32, f32, f32)> = None;
            for b in &boxes {
                if b.role == Role::Scroll && rect_contains(b.rect, x, y) {
                    if let Some(id) = b.id {
                        target = Some((id, b.rect.max.y - b.rect.min.y, b.content_len));
                    }
                }
            }
            if let Some((id, view_h, content_len)) = target {
                let max = (content_len - view_h).max(0.0);
                let next = (state.scroll_of(id) + delta).clamp(0.0, max);
                state.scrolls.insert(id, next);
            }
        }
        UiEvent::Char(c) => {
            if let Some(id) = state.focused {
                if !c.is_control() {
                    state.inputs.entry(id).or_default().push(c);
                }
            }
        }
        UiEvent::Backspace => {
            if let Some(id) = state.focused {
                state.inputs.entry(id).or_default().pop();
            }
        }
        UiEvent::Enter => {
            state.submit = state.focused;
        }
    }
}

/// Render the interactive UI for the current state, including scrollbars. Layout is
/// solved in logical units and painted at `state.scale` physical pixels per unit.
pub fn render_ui(build: &dyn Fn(&UiState) -> UxNode, state: &UiState, clear: Rgba) -> Framebuffer {
    let s = state.scale.max(0.1);
    let mut boxes = solve_for(build, state);
    scale_boxes(&mut boxes, s);
    let mut fb = paint_boxes_banded(state.width, state.height, clear, &boxes);
    draw_scrollbars(&mut fb, &boxes, state, s);
    fb
}

/// `boxes` are already in physical pixels here; scroll offsets are logical, so they are
/// scaled up to match before the thumb geometry is computed.
fn draw_scrollbars(fb: &mut Framebuffer, boxes: &[LaidBox], state: &UiState, s: f32) {
    for b in boxes {
        let Some(id) = b.id else { continue };
        if let Some((bar_x, track_top, track_h, thumb_y, thumb_h, _max)) =
            scrollbar_geom(b, state.scroll_of(id) * s, s)
        {
            let thumb_col = if state.drag == Some(id) {
                Rgba::rgb8(113, 113, 122) // zinc-500 — brighter while dragging
            } else {
                Rgba::rgb8(82, 82, 91) // zinc-600 — subtle at rest
            };
            fill_rect(
                fb,
                bar_x,
                track_top,
                4.0 * s,
                track_h,
                Rgba::rgb8(39, 39, 42), // zinc-800 — nearly invisible track
                2.0 * s,
            );
            fill_rect(fb, bar_x, thumb_y, 4.0 * s, thumb_h, thumb_col, 2.0 * s);
        }
    }
}

fn fill_rect(fb: &mut Framebuffer, x: f32, y: f32, w: f32, h: f32, color: Rgba, radius: f32) {
    let cmd = DrawCmd::new(
        Shape::RoundedRect {
            half: Vec2::new(w / 2.0, h / 2.0),
            radius,
        },
        Paint::Solid(color),
        Affine::translate(x + w / 2.0, y + h / 2.0),
    );
    raster::scan_convert(&cmd, fb, None);
}

#[cfg(test)]
mod banded_render_tests {
    use super::*;
    use pmre_kit::ux::{Dim, Edges, Style, UxNode};

    fn probe_scene() -> UxNode {
        // shadows bleed far past their rect and lowercase glyphs cross band edges —
        // both must land in the right lanes or the banded render seams.
        let panel = |c: Rgba, label: &str| {
            UxNode::boxed(
                Style::col()
                    .w(Dim::Flex(1.0))
                    .h(Dim::Px(70.0))
                    .pad(Edges::all(10.0))
                    .gap(6.0)
                    .radius(12.0)
                    .bg(Rgba::rgb8(20, 20, 28))
                    .border(2.0, c)
                    .shadow(0.0, 6.0, 14.0, Rgba::new(0.0, 0.0, 0.0, 0.5)),
                vec![UxNode::text(label, 18.0, c)],
            )
        };
        UxNode::boxed(
            Style::col()
                .w(Dim::Flex(1.0))
                .h(Dim::Flex(1.0))
                .pad(Edges::all(14.0))
                .gap(12.0)
                .bg(Rgba::rgb8(8, 8, 12)),
            vec![
                UxNode::text("lane seam probe — gjpqy", 16.0, Rgba::rgb8(200, 200, 220)),
                panel(Rgba::rgb8(0, 220, 180), "alpha jumping quickly"),
                panel(Rgba::rgb8(255, 120, 80), "bravo gyrating deeply"),
                panel(Rgba::rgb8(160, 90, 255), "charlie playing jazz"),
            ],
        )
    }

    /// The lane/bus render must be bit-identical to the single-threaded render — each
    /// pixel is produced by exactly one lane, so band boundaries leave no seam.
    #[test]
    fn banded_render_matches_serial() {
        let (w, h) = (260u32, 320u32);
        let clear = Rgba::rgb8(8, 8, 12);
        let boxes = layout::solve(&probe_scene(), viewport(w, h), &|_| 0.0);

        let mut serial = Framebuffer::new(w, h, clear);
        paint_boxes(&mut serial, &boxes);
        let banded = paint_boxes_banded(w, h, clear, &boxes);

        let mut maxd = 0f32;
        let mut worst = (0u32, 0u32);
        for i in 0..(w * h) as usize {
            let (a, b) = (serial.pixels()[i], banded.pixels()[i]);
            let d = (a.r - b.r)
                .abs()
                .max((a.g - b.g).abs())
                .max((a.b - b.b).abs())
                .max((a.a - b.a).abs());
            if d > maxd {
                maxd = d;
                worst = (i as u32 % w, i as u32 / w);
            }
        }
        assert!(
            maxd < 1e-6,
            "lane render diverged from serial at {worst:?}: max diff {maxd}"
        );
    }
}
