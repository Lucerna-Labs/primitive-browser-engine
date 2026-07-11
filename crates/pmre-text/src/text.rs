//! Glyph rasterization: draw text runs onto any `Surface`.
//!
//! Two coverage tiers, picked automatically (see `crate::font`):
//! - **Vector**: anti-aliased TrueType glyph bitmaps from the system font, positioned on a
//!   real baseline with per-glyph bearings and advances. This is the polished path.
//! - **Bitmap**: the original 5×7 pixel font when no font file exists on the machine.
//!
//! A text run is `scan` (over glyph coverage cells) + `combine` (alpha-over) — its own
//! coverage generator, distinct from the SDF generator used for shapes. Mechanism only.

use crate::font;
use pmre_core::framebuffer::Surface;
use pmre_core::geom::Vec2;
use pmre_core::paint::{Bounds, Rgba};

/// Ascent/descent of the active tier at `size`, in px. The bitmap tier spans exactly
/// `size` above the baseline with no descender rows.
pub fn v_metrics(size: f32) -> (f32, f32) {
    v_metrics_styled(size, false)
}

/// `v_metrics` for a styled run — the bold face can have different metrics.
pub fn v_metrics_styled(size: f32, bold: bool) -> (f32, f32) {
    let face = if bold { font::bold() } else { font::regular() };
    match face {
        Some(f) => (f.ascent(size), f.descent(size)),
        None => (size, 0.0),
    }
}

/// Width in device pixels that `draw` will advance for `content` at font size `size`.
pub fn advance(content: &str, size: f32) -> f32 {
    advance_styled(content, size, false)
}

/// `advance` for a styled run (the bold face has its own metrics).
pub fn advance_styled(content: &str, size: f32, bold: bool) -> f32 {
    let face = if bold { font::bold() } else { font::regular() };
    match face {
        Some(f) => content.chars().map(|c| f.advance(c, size)).sum(),
        None => {
            let cs = (size / 7.0).max(1.0);
            content.chars().count() as f32 * 6.0 * cs
        }
    }
}

/// Greedy word-wrap of `content` into lines that each fit within `max_width` pixels.
/// O(n): each word is measured exactly once.
pub fn wrap(content: &str, size: f32, max_width: f32) -> Vec<String> {
    if max_width <= 0.0 {
        return vec![content.to_string()];
    }
    let space_w = advance(" ", size);
    let mut lines = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0f32;
    for word in content.split_whitespace() {
        let word_w = advance(word, size);
        let need = if cur.is_empty() {
            word_w
        } else {
            cur_w + space_w + word_w
        };
        if cur.is_empty() || need <= max_width {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
            cur_w = need;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur.push_str(word);
            cur_w = word_w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Render `content` with the top of its ascent box at `origin` (so the baseline sits at
/// `origin.y + ascent`). Pixels outside `clip` (when given) are skipped.
pub fn draw<S: Surface>(
    fb: &mut S,
    content: &str,
    origin: Vec2,
    size: f32,
    color: Rgba,
    clip: Option<Bounds>,
) {
    draw_styled(fb, content, origin, size, color, clip, false, false);
}

/// `draw` with style flags: `bold` selects the bold face, `underline` rules the run.
#[allow(clippy::too_many_arguments)]
pub fn draw_styled<S: Surface>(
    fb: &mut S,
    content: &str,
    origin: Vec2,
    size: f32,
    color: Rgba,
    clip: Option<Bounds>,
    bold: bool,
    underline: bool,
) {
    let face = if bold { font::bold() } else { font::regular() };
    match face {
        Some(f) => {
            let ascent = f.ascent(size);
            let baseline = origin.y + ascent;
            let mut pen = origin.x;
            for ch in content.chars() {
                let g = f.raster(ch, size);
                if g.w > 0 && g.h > 0 {
                    let gx = (pen + 0.5).floor() as i32 + g.left;
                    let gy = (baseline + 0.5).floor() as i32 + g.top;
                    blit(fb, &g, gx, gy, color, clip);
                }
                pen += f.advance(ch, size);
            }
            if underline && pen > origin.x {
                let y = (baseline + size * 0.09).round();
                let th = (size * 0.07).round().max(1.0);
                rule(fb, origin.x, pen, y, th, color, clip);
            }
        }
        None => {
            let cs = (size / 7.0).max(1.0);
            let mut pen_x = origin.x;
            for ch in content.chars() {
                let g = font::glyph(ch);
                for (r, &row) in g.iter().enumerate() {
                    for col in 0..5u32 {
                        if row & (1 << (4 - col)) != 0 {
                            fill_cell(
                                fb,
                                pen_x + col as f32 * cs,
                                origin.y + r as f32 * cs,
                                cs,
                                color,
                                clip,
                            );
                        }
                    }
                }
                pen_x += 6.0 * cs;
            }
            if underline && pen_x > origin.x {
                rule(
                    fb,
                    origin.x,
                    pen_x,
                    origin.y + 8.0 * cs,
                    cs.max(1.0),
                    color,
                    clip,
                );
            }
        }
    }
}

/// Composite one glyph coverage bitmap at `(gx, gy)` in `color`, honoring the clip
/// rectangle and the surface's accepted row band.
fn blit<S: Surface>(
    fb: &mut S,
    g: &font::RasterGlyph,
    gx: i32,
    gy: i32,
    color: Rgba,
    clip: Option<Bounds>,
) {
    let (rlo, rhi) = fb.row_range();
    let (cx0, cy0, cx1, cy1) = clip_box(fb, clip);
    let w = g.w as usize;
    for r in 0..g.h {
        let y = gy + r;
        if y < cy0 || y >= cy1 || (y as u32) < rlo || y as u32 >= rhi {
            continue;
        }
        let row = r as usize * w;
        for c in 0..g.w {
            let x = gx + c;
            if x < cx0 || x >= cx1 {
                continue;
            }
            let cov = g.cov[row + c as usize];
            if cov > 0 {
                let a = color.a * cov as f32 * (1.0 / 255.0);
                fb.blend_over(x as u32, y as u32, color.with_alpha(a));
            }
        }
    }
}

/// Fill the horizontal rule `[x0, x1) × [y, y+th)` (underlines).
fn rule<S: Surface>(
    fb: &mut S,
    x0: f32,
    x1: f32,
    y: f32,
    th: f32,
    color: Rgba,
    clip: Option<Bounds>,
) {
    let (cx0, cy0, cx1, cy1) = clip_box(fb, clip);
    let (rlo, rhi) = fb.row_range();
    let xa = (x0.round() as i32).max(cx0);
    let xb = (x1.round() as i32).min(cx1);
    let ya = (y as i32).max(cy0).max(rlo as i32);
    let yb = ((y + th) as i32).min(cy1).min(rhi as i32);
    for py in ya..yb {
        for px in xa..xb {
            fb.blend_over(px as u32, py as u32, color);
        }
    }
}

/// The effective integer clip box: surface bounds intersected with `clip`, using the
/// pixel-center rule (a pixel is inside when its center is), matching the SDF path.
fn clip_box<S: Surface>(fb: &S, clip: Option<Bounds>) -> (i32, i32, i32, i32) {
    let (mut x0, mut y0, mut x1, mut y1) = (0.0f32, 0.0f32, fb.width() as f32, fb.height() as f32);
    if let Some(c) = clip {
        x0 = x0.max(c.min.x);
        y0 = y0.max(c.min.y);
        x1 = x1.min(c.max.x);
        y1 = y1.min(c.max.y);
    }
    (
        (x0 - 0.5).ceil() as i32,
        (y0 - 0.5).ceil() as i32,
        (x1 - 0.5).ceil().max(0.0) as i32,
        (y1 - 0.5).ceil().max(0.0) as i32,
    )
}

fn fill_cell<S: Surface>(fb: &mut S, x: f32, y: f32, cs: f32, color: Rgba, clip: Option<Bounds>) {
    let x0 = x.round() as i32;
    let y0 = y.round() as i32;
    let x1 = (x + cs).round().max((x0 + 1) as f32) as i32;
    let y1 = (y + cs).round().max((y0 + 1) as f32) as i32;
    for py in y0..y1 {
        for px in x0..x1 {
            if px < 0 || py < 0 {
                continue;
            }
            if let Some(c) = clip {
                let (fx, fy) = (px as f32, py as f32);
                if fx < c.min.x || fx >= c.max.x || fy < c.min.y || fy >= c.max.y {
                    continue;
                }
            }
            fb.blend_over(px as u32, py as u32, color);
        }
    }
}
