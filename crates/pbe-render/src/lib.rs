//! # pbe-render
//!
//! The render **off-ramp**: it turns a paint primitive list
//! (`Vec<cap_primitives::Primitive>`) into two concrete, inspectable engine
//! outputs, with **zero GPU dependency**:
//!
//! 1. [`display_list`] — a deterministic text dump of the painted geometry
//!    (kind, z-order, bounds, fill). This is the engine's verifiable artifact:
//!    same input always yields the same string, so it is golden-testable.
//! 2. [`rasterize`] + [`Raster::to_ppm`] — a tiny software rasterizer that
//!    composites the shapes into an RGBA framebuffer and serializes it as a
//!    binary PPM (P6) image. Real pixels, no `wgpu`/`vello`/window needed.
//!
//! This keeps with the doctrine: we drive the sealed primitive vocabulary from
//! outside and compose a renderer out of it. A GPU backend (`ordo-ux-vello`)
//! is a *swap* of this stage later, not a prerequisite for a working engine.

use cap_geometry::{Bounds, Pixels};
use cap_primitives::{Fill, Primitive, Rgba, Shape};

// ---------------------------------------------------------------------------
// Display list — the deterministic, golden-testable artifact
// ---------------------------------------------------------------------------

/// A flat, ordered description of one painted item.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayItem {
    /// Primitive kind: "rect", "rounded-rect", "circle", "text", etc.
    pub kind: &'static str,
    /// Paint order (lower = behind).
    pub order: u32,
    /// Axis-aligned bounds as (x, y, w, h) in CSS pixels.
    pub rect: (f32, f32, f32, f32),
    /// Solid fill as 0xRRGGBBAA, if the item has one.
    pub fill: Option<u32>,
}

/// Build a deterministic display list from a paint primitive list. Pure: the
/// same primitives always produce the same items, in the same order.
pub fn display_items(primitives: &[Primitive]) -> Vec<DisplayItem> {
    primitives.iter().map(describe).collect()
}

/// Render the display list to a stable, human-readable string — the engine's
/// golden artifact. One line per item.
pub fn display_list(primitives: &[Primitive]) -> String {
    let mut out = String::new();
    out.push_str(&format!("display-list: {} item(s)\n", primitives.len()));
    for item in display_items(primitives) {
        let (x, y, w, h) = item.rect;
        let fill = match item.fill {
            Some(hex) => format!("#{hex:08x}"),
            None => "—".to_string(),
        };
        out.push_str(&format!(
            "  [{:>3}] {:<13} rect=({:>6.1},{:>6.1} {:>6.1}x{:>6.1}) fill={}\n",
            item.order, item.kind, x, y, w, h, fill
        ));
    }
    out
}

fn bounds_tuple(b: &Bounds<Pixels>) -> (f32, f32, f32, f32) {
    (b.origin.x.0, b.origin.y.0, b.size.width.0, b.size.height.0)
}

fn fill_hex(fill: &Fill) -> Option<u32> {
    match fill {
        Fill::Solid(c) => Some(c.to_hex()),
        _ => None,
    }
}

fn describe(p: &Primitive) -> DisplayItem {
    let rect = p
        .bounds()
        .map(bounds_tuple)
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    match p {
        Primitive::Shape(s) => {
            let (kind, fill) = match &s.shape {
                Shape::Rect(r) => ("rect", fill_hex(&r.fill)),
                Shape::RoundedRect(r) => ("rounded-rect", fill_hex(&r.fill)),
                Shape::Circle(c) => ("circle", fill_hex(&c.fill)),
                Shape::Ellipse(e) => ("ellipse", fill_hex(&e.fill)),
                Shape::Line(_) => ("line", None),
                Shape::Path(p) => ("path", fill_hex(&p.fill)),
            };
            DisplayItem { kind, order: s.order, rect, fill }
        }
        Primitive::Text(t) => DisplayItem { kind: "text", order: t.order, rect, fill: None },
        Primitive::Image(i) => DisplayItem { kind: "image", order: i.order, rect, fill: None },
        Primitive::Shadow(s) => DisplayItem {
            kind: "shadow",
            order: s.order,
            rect,
            fill: Some(s.color.to_hex()),
        },
        Primitive::Clip(_) => DisplayItem { kind: "clip", order: 0, rect, fill: None },
        Primitive::Layer(_) => DisplayItem { kind: "layer", order: 0, rect, fill: None },
        Primitive::Transform(_) => DisplayItem { kind: "transform", order: 0, rect, fill: None },
    }
}

// ---------------------------------------------------------------------------
// Software rasterizer — real pixels, no GPU
// ---------------------------------------------------------------------------

/// An RGBA8 framebuffer.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// Row-major RGBA, 4 bytes per pixel.
    pub pixels: Vec<u8>,
}

impl Raster {
    /// A new framebuffer cleared to an opaque background color.
    pub fn new(width: u32, height: u32, bg: Rgba) -> Self {
        let [r, g, b, _] = rgba_bytes(bg);
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..(width * height) {
            pixels.extend_from_slice(&[r, g, b, 255]);
        }
        Self { width, height, pixels }
    }

    /// Alpha-blend a solid color over an axis-aligned rect (clipped to bounds).
    fn fill_rect(&mut self, rect: (f32, f32, f32, f32), color: Rgba) {
        if color.a <= 0.0 {
            return;
        }
        let (rx, ry, rw, rh) = rect;
        let x0 = rx.floor().max(0.0) as u32;
        let y0 = ry.floor().max(0.0) as u32;
        let x1 = ((rx + rw).ceil() as i64).clamp(0, self.width as i64) as u32;
        let y1 = ((ry + rh).ceil() as i64).clamp(0, self.height as i64) as u32;
        let [sr, sg, sb, _] = rgba_bytes(color);
        let a = color.a.clamp(0.0, 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * self.width + x) * 4) as usize;
                self.pixels[i] = blend(self.pixels[i], sr, a);
                self.pixels[i + 1] = blend(self.pixels[i + 1], sg, a);
                self.pixels[i + 2] = blend(self.pixels[i + 2], sb, a);
                self.pixels[i + 3] = 255;
            }
        }
    }

    /// Serialize as a binary PPM (P6) image — readable by most image viewers.
    pub fn to_ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.width, self.height).into_bytes();
        out.reserve((self.width * self.height * 3) as usize);
        for px in self.pixels.chunks_exact(4) {
            out.extend_from_slice(&px[0..3]); // drop alpha for PPM
        }
        out
    }
}

fn rgba_bytes(c: Rgba) -> [u8; 4] {
    [
        (c.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.b.clamp(0.0, 1.0) * 255.0).round() as u8,
        (c.a.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

fn blend(dst: u8, src: u8, alpha: f32) -> u8 {
    (dst as f32 * (1.0 - alpha) + src as f32 * alpha).round() as u8
}

/// Rasterize a paint primitive list into a framebuffer. Shapes paint in
/// ascending `order` (painter's algorithm). Only solid-filled rectangles are
/// drawn today (what the paint stage emits); other shapes contribute to the
/// display list and are skipped here until their rasterizers land.
pub fn rasterize(primitives: &[Primitive], width: u32, height: u32, bg: Rgba) -> Raster {
    let mut raster = Raster::new(width, height, bg);

    // Paint back-to-front by order.
    let mut ordered: Vec<&Primitive> = primitives.iter().collect();
    ordered.sort_by_key(|p| match p {
        Primitive::Shape(s) => s.order,
        Primitive::Shadow(s) => s.order,
        Primitive::Text(t) => t.order,
        Primitive::Image(i) => i.order,
        _ => 0,
    });

    for p in ordered {
        if let Primitive::Shape(s) = p {
            let (fill, rect) = match &s.shape {
                Shape::Rect(r) => (&r.fill, r.rect),
                Shape::RoundedRect(r) => (&r.fill, r.rect),
                _ => continue,
            };
            if let Fill::Solid(c) = fill {
                raster.fill_rect(bounds_tuple(&rect), *c);
            }
        }
    }
    raster
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_geometry::{point, size, Pixels};
    use cap_primitives::{RectShape, ShapePrimitive, Stroke};

    fn red_rect(order: u32) -> Primitive {
        let rect = Bounds::new(
            point(Pixels(10.0), Pixels(20.0)),
            size(Pixels(30.0), Pixels(40.0)),
        );
        Primitive::Shape(ShapePrimitive {
            shape: Shape::Rect(RectShape {
                rect,
                fill: Fill::solid_hex(0xFF0000FF),
                stroke: Stroke::default(),
            }),
            bounds: rect,
            order,
        })
    }

    #[test]
    fn display_list_is_deterministic() {
        let prims = vec![red_rect(0)];
        let a = display_list(&prims);
        let b = display_list(&prims);
        assert_eq!(a, b);
        assert!(a.contains("rect=("));
        assert!(a.contains("#ff0000ff"));
    }

    #[test]
    fn display_items_carry_order_and_fill() {
        let items = display_items(&[red_rect(7)]);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "rect");
        assert_eq!(items[0].order, 7);
        assert_eq!(items[0].rect, (10.0, 20.0, 30.0, 40.0));
        assert_eq!(items[0].fill, Some(0xFF0000FF));
    }

    #[test]
    fn rasterize_fills_pixels_inside_and_leaves_bg_outside() {
        let bg = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let raster = rasterize(&[red_rect(0)], 64, 64, bg);
        assert_eq!(raster.pixels.len(), 64 * 64 * 4);

        // A pixel inside the rect (10..40, 20..60) must be red.
        let inside = ((30 * 64 + 20) * 4) as usize;
        assert_eq!(&raster.pixels[inside..inside + 3], &[255, 0, 0]);

        // A pixel outside the rect must remain background (black).
        let outside = ((5 * 64 + 5) * 4) as usize;
        assert_eq!(&raster.pixels[outside..outside + 3], &[0, 0, 0]);
    }

    #[test]
    fn ppm_header_and_size_are_correct() {
        let bg = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let ppm = rasterize(&[], 8, 4, bg).to_ppm();
        assert!(ppm.starts_with(b"P6\n8 4\n255\n"));
        // header + 8*4 pixels * 3 bytes
        let header_len = b"P6\n8 4\n255\n".len();
        assert_eq!(ppm.len(), header_len + 8 * 4 * 3);
    }
}
