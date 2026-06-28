//! Layout-aware painter: turn a styled DOM + computed layout into primitives at
//! their **real** on-screen positions.
//!
//! This replaces the kit's MVP `cap-paint`, which anchored every box at the
//! origin and ignored layout. Here we walk the DOM in document order (paint
//! order = z-order for non-positioned content) and, for each element that has a
//! laid-out box, emit a background fill and border edges at the box's actual
//! `pbe_layout` bounds. Text is handled by the text stage; this is the box layer.

use cap_geometry::{point, size, Bounds, Pixels};
use cap_primitives::{Fill, Primitive, RectShape, Rgba as PrimRgba, Shape, ShapePrimitive, Stroke};
use cap_style_cascade::{ComputedStyle, StyledDom};

use pbe_layout::LayoutResult;

fn to_prim_rgba(c: cap_color::Rgba) -> PrimRgba {
    PrimRgba {
        r: c.r,
        g: c.g,
        b: c.b,
        a: c.a,
    }
}

/// Paint a styled DOM using real layout geometry. Returns primitives in paint
/// order (document order; later = on top).
pub fn paint_with_layout(styled: &StyledDom, layout: &LayoutResult) -> Vec<Primitive> {
    let mut out = Vec::new();
    let mut order = 0u32;
    paint_node(styled, layout, styled.root(), &mut out, &mut order);
    out
}

fn paint_node(
    styled: &StyledDom,
    layout: &LayoutResult,
    id: cap_html_parse::NodeId,
    out: &mut Vec<Primitive>,
    order: &mut u32,
) {
    if let Some(node) = styled.node(id) {
        if node.is_element() {
            if let (Some(style), Some(bounds)) = (styled.style(id), layout.bounds(id)) {
                paint_box(style, &bounds, out, order);
            }
        }
        // Recurse in document order so children paint above their parent.
        for &child in node.children() {
            paint_node(styled, layout, child, out, order);
        }
    }
}

/// Emit background + borders for one element box at its real bounds.
fn paint_box(
    style: &ComputedStyle,
    bounds: &Bounds<Pixels>,
    out: &mut Vec<Primitive>,
    order: &mut u32,
) {
    // Background fill (skip fully transparent).
    let bg = style.visual.background_color;
    if bg.a > 0.0 {
        let next = *order;
        *order += 1;
        out.push(Primitive::Shape(ShapePrimitive {
            shape: Shape::Rect(RectShape {
                rect: *bounds,
                fill: Fill::Solid(to_prim_rgba(bg)),
                stroke: Stroke::default(),
            }),
            bounds: *bounds,
            order: next,
        }));
    }

    // Borders: draw each non-zero edge as a filled rect at the box perimeter.
    let bw = &style.visual.border_width;
    let bc = &style.visual.border_color;
    let (x, y) = (bounds.origin.x.0, bounds.origin.y.0);
    let (w, h) = (bounds.size.width.0, bounds.size.height.0);

    let edge = |ex: f32,
                ey: f32,
                ew: f32,
                eh: f32,
                color: cap_color::Rgba,
                out: &mut Vec<Primitive>,
                order: &mut u32| {
        if ew <= 0.0 || eh <= 0.0 || color.a <= 0.0 {
            return;
        }
        let b = Bounds::new(point(Pixels(ex), Pixels(ey)), size(Pixels(ew), Pixels(eh)));
        let next = *order;
        *order += 1;
        out.push(Primitive::Shape(ShapePrimitive {
            shape: Shape::Rect(RectShape {
                rect: b,
                fill: Fill::Solid(to_prim_rgba(color)),
                stroke: Stroke::default(),
            }),
            bounds: b,
            order: next,
        }));
    };

    edge(x, y, w, bw.top, bc.top, out, order); // top
    edge(x, y + h - bw.bottom, w, bw.bottom, bc.bottom, out, order); // bottom
    edge(x, y, bw.left, h, bc.left, out, order); // left
    edge(x + w - bw.right, y, bw.right, h, bc.right, out, order); // right
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_css_parse::Stylesheet;
    use cap_html_parse::parse_html;

    #[test]
    fn paints_background_at_laid_out_position() {
        // Two stacked divs; the second should paint below the first (real y).
        let dom = parse_html("<html><body><div class=a></div><div class=b></div></body></html>");
        let css = "html{display:block} body{display:block} div{display:block} \
                   .a{width:100px;height:50px;background-color:#ff0000} \
                   .b{width:100px;height:60px;background-color:#00ff00}";
        let styled = StyledDom::new(dom, &[Stylesheet::parse_author(css)]);
        let layout = pbe_layout::layout(&styled, 800.0, 600.0);
        let prims = paint_with_layout(&styled, &layout);

        // Two backgrounds painted.
        let rects: Vec<&ShapePrimitive> = prims
            .iter()
            .filter_map(|p| match p {
                Primitive::Shape(s) => Some(s),
                _ => None,
            })
            .collect();
        assert!(
            rects.len() >= 2,
            "expected >=2 painted rects, got {}",
            rects.len()
        );

        // Their y origins must differ (stacked, not both at 0/origin).
        let ys: Vec<f32> = rects.iter().map(|s| s.bounds.origin.y.0).collect();
        assert!(
            ys.iter().any(|&y| y > 1.0),
            "at least one box should be below the top, ys={ys:?}"
        );
        assert!(
            ys.windows(2).any(|w| (w[0] - w[1]).abs() > 1.0),
            "stacked boxes should have different y, ys={ys:?}"
        );
    }
}
