//! # pbe-layout
//!
//! Real box layout, composed over the kit. This crate mirrors a [`StyledDom`]
//! into a `cap_layout::LayoutTree` (taffy), computes layout at a viewport size,
//! and reports the **absolute** box geometry of every DOM node. It replaces the
//! origin-anchored placeholder bounds the MVP `cap-paint` used, so elements
//! actually stack and flow like a page.
//!
//! Doctrine: no layout math lives here. cap-layout (taffy) is the sealed
//! mechanism; this crate only *maps* CSS computed style → `LayoutStyle` and
//! walks the DOM to build the parallel tree. The one piece of policy is the
//! standard CSS-block → column-flex equivalence (cap-layout always emits taffy
//! `Display::Flex`, so block flow is modeled as a column flex container).

use std::collections::HashMap;

use cap_geometry::{px, size, AbsoluteLength, Bounds, Edges, Length, Pixels};
use cap_html_parse::{DomNode, NodeId};
use cap_layout::{
    AlignItems, AvailableSpace, FlexDirection, LayoutId, LayoutStyle, LayoutTree,
    Position as LayoutPosition,
};
use cap_style_cascade::{
    ComputedStyle, Display, Edges as StyleEdges, Length as StyleLength, Position as StylePosition,
    StyledDom,
};

/// The output of layout: the absolute on-screen box of every laid-out node,
/// keyed by DOM [`NodeId`]. Positions are window-space (origin top-left).
#[derive(Clone, Debug, Default)]
pub struct LayoutResult {
    boxes: HashMap<usize, Bounds<Pixels>>,
}

impl LayoutResult {
    /// The absolute bounds computed for a DOM node, if it took part in layout.
    pub fn bounds(&self, id: NodeId) -> Option<Bounds<Pixels>> {
        self.boxes.get(&id.0).copied()
    }

    /// Number of laid-out boxes.
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }
}

/// Approximate average glyph advance as a fraction of font size, used to give
/// text nodes an intrinsic width so they occupy space in layout. Real shaping
/// (pbe text stage) refines this; this keeps layout honest without a font here.
const AVG_GLYPH_W_RATIO: f32 = 0.5;

/// Elements whose subtree is metadata/script, not rendered page content. A real
/// browser's UA sheet sets these to `display:none`; the kit ships no UA sheet,
/// so we drop them here (also prevents `<style>`/`<script>` text from painting).
fn is_non_rendered(tag: &str) -> bool {
    matches!(
        tag,
        "head" | "style" | "script" | "title" | "meta" | "link" | "base" | "noscript" | "template"
    )
}

/// Compute layout for a styled DOM at the given viewport size.
///
/// Returns the absolute box of every element (and text node) reachable from the
/// root. The viewport width/height seed the available space; block content can
/// extend past the viewport height (taffy returns the content size).
pub fn layout(styled: &StyledDom, viewport_w: f32, viewport_h: f32) -> LayoutResult {
    let mut tree = LayoutTree::new();
    let rem = px(16.0);
    let scale = 1.0;

    // Build the layout tree bottom-up so children exist before parents.
    let root_dom = styled.root();
    let Some(root_layout) = build_node(styled, root_dom, &mut tree, rem, scale) else {
        return LayoutResult::default();
    };

    tree.compute_layout(
        root_layout.id,
        size(
            AvailableSpace::Definite(px(viewport_w)),
            AvailableSpace::Definite(px(viewport_h)),
        ),
    );

    // Walk the mapping and pull absolute bounds for each DOM node.
    let mut boxes = HashMap::new();
    collect_bounds(&tree, &root_layout, &mut boxes);
    LayoutResult { boxes }
}

/// A DOM node paired with its layout id and children (so we can resolve
/// absolute bounds after compute without re-querying taffy parentage).
struct Mapped {
    dom_id: NodeId,
    id: LayoutId,
    children: Vec<Mapped>,
}

/// Recursively build a `cap-layout` node for a DOM node and its children.
/// Returns `None` for nodes that don't participate (e.g. `display:none`,
/// comments).
fn build_node(
    styled: &StyledDom,
    dom_id: NodeId,
    tree: &mut LayoutTree,
    rem: Pixels,
    scale: f32,
) -> Option<Mapped> {
    let node = styled.node(dom_id)?;

    // Text node: a leaf sized by its (estimated) intrinsic text box.
    if let DomNode::Text { text, .. } = node {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        // Default font size; the text-shaping stage refines text geometry later.
        let style = text_leaf_style(trimmed, 16.0);
        let id = tree.add_leaf(&style, rem, scale);
        return Some(Mapped {
            dom_id,
            id,
            children: Vec::new(),
        });
    }

    // Non-rendered elements: their contents never produce boxes or visible text
    // (CSS gives them `display:none` in the UA sheet, but the kit ships none, so
    // skip them structurally here). This also prevents <style>/<script> text
    // from being laid out and painted as page content.
    if let Some(tag) = node.tag() {
        if is_non_rendered(tag) {
            return None;
        }
    }

    // Elements and the document: container nodes.
    let computed = styled.style(dom_id);
    if let Some(c) = computed {
        if c.layout.display == Display::None {
            return None;
        }
    }

    // Build children first.
    let mut child_maps: Vec<Mapped> = Vec::new();
    for &child in node.children() {
        if let Some(m) = build_node(styled, child, tree, rem, scale) {
            child_maps.push(m);
        }
    }

    let style = match computed {
        Some(c) => element_style(c),
        None => block_container_style(),
    };
    let child_ids: Vec<LayoutId> = child_maps.iter().map(|m| m.id).collect();
    let id = tree.add_node(&style, rem, scale, &child_ids);

    Some(Mapped {
        dom_id,
        id,
        children: child_maps,
    })
}

/// Pull absolute bounds for a mapped subtree into the result map.
fn collect_bounds(tree: &LayoutTree, m: &Mapped, out: &mut HashMap<usize, Bounds<Pixels>>) {
    out.insert(m.dom_id.0, tree.layout_absolute_bounds(m.id));
    for child in &m.children {
        collect_bounds(tree, child, out);
    }
}

// ── style mapping ──────────────────────────────────────────────────────────

fn to_len(l: &StyleLength) -> Length {
    match l {
        StyleLength::Px(p) => Length::from(px(*p)),
        // Auto and None both map to layout-determined sizing.
        StyleLength::Auto | StyleLength::None => Length::Auto,
    }
}

fn to_edges_abs(e: &StyleEdges) -> Edges<AbsoluteLength> {
    Edges {
        top: AbsoluteLength::Pixels(px(e.top)),
        right: AbsoluteLength::Pixels(px(e.right)),
        bottom: AbsoluteLength::Pixels(px(e.bottom)),
        left: AbsoluteLength::Pixels(px(e.left)),
    }
}

fn to_edges_len(e: &StyleEdges) -> Edges<Length> {
    Edges {
        top: Length::from(px(e.top)),
        right: Length::from(px(e.right)),
        bottom: Length::from(px(e.bottom)),
        left: Length::from(px(e.left)),
    }
}

/// Map a computed element style onto a cap-layout `LayoutStyle`.
///
/// CSS `display` mapping: cap-layout always produces taffy Flex, so block-level
/// boxes become **column** flex containers (children stack vertically, the CSS
/// block default); `flex`/`inline-flex` use the computed flex-direction; inline
/// boxes are approximated as row flex so siblings sit beside each other.
fn element_style(c: &ComputedStyle) -> LayoutStyle {
    let l = &c.layout;
    let direction = match l.display {
        Display::Flex | Display::InlineFlex => match l.flex_direction {
            cap_style_cascade::FlexDirection::Row => FlexDirection::Row,
            cap_style_cascade::FlexDirection::RowReverse => FlexDirection::RowReverse,
            cap_style_cascade::FlexDirection::Column => FlexDirection::Column,
            cap_style_cascade::FlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
        },
        Display::Inline | Display::InlineBlock => FlexDirection::Row,
        // Block/Grid/etc.: stack children vertically like CSS block flow.
        _ => FlexDirection::Column,
    };

    LayoutStyle {
        width: to_len(&l.width),
        height: to_len(&l.height),
        min_width: to_len(&l.min_width),
        min_height: to_len(&l.min_height),
        max_width: to_len(&l.max_width),
        max_height: to_len(&l.max_height),
        flex_direction: direction,
        flex_grow: l.flex_grow,
        flex_shrink: l.flex_shrink,
        // Stretch on the cross axis matches CSS block flow (children fill the
        // inline axis) and is the sensible flex default here too.
        align_items: AlignItems::Stretch,
        padding: to_edges_abs(&l.padding),
        margin: to_edges_len(&l.margin),
        position: match l.position {
            StylePosition::Absolute => LayoutPosition::Absolute,
            // cap-layout models only Relative/Absolute; Fixed/Sticky fall back
            // to Relative (a correct, if non-pinning, approximation for now).
            _ => LayoutPosition::Relative,
        },
        ..LayoutStyle::default()
    }
}

/// A plain block container (used for the document root / unstyled nodes):
/// auto width (stretches to fill the parent's cross axis), stacks children in a
/// column, and grows to take available main-axis space.
fn block_container_style() -> LayoutStyle {
    LayoutStyle {
        width: Length::Auto,
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Stretch,
        flex_grow: 1.0,
        ..LayoutStyle::default()
    }
}

/// Intrinsic-ish box for a text leaf: width ≈ chars × avg advance, height ≈
/// one line. Gives text real space until the shaping stage refines it.
fn text_leaf_style(text: &str, font_size: f32) -> LayoutStyle {
    let w = (text.chars().count() as f32) * font_size * AVG_GLYPH_W_RATIO;
    let h = font_size * 1.3;
    LayoutStyle::sized(px(w), px(h))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cap_css_parse::Stylesheet;
    use cap_html_parse::parse_html;

    // Minimal UA defaults so structural elements are block-level, matching how
    // the engine configures the cascade in the build-styled stage. NOTE: the
    // kit's selector parser has no comma-list support, so one rule per selector.
    const UA: &str =
        "html{display:block} body{display:block;margin:8px} div{display:block} p{display:block}";

    fn styled(html: &str, css: &str) -> StyledDom {
        let dom = parse_html(html);
        let sheet = Stylesheet::parse_author(&format!("{UA}\n{css}"));
        StyledDom::new(dom, &[sheet])
    }

    #[test]
    fn two_blocks_stack_vertically() {
        // Two fixed-size divs in block flow should not both sit at y=0.
        let s = styled(
            "<html><body><div class=a></div><div class=b></div></body></html>",
            ".a{width:100px;height:50px} .b{width:100px;height:60px}",
        );
        let result = layout(&s, 800.0, 600.0);

        // Find the two divs by tag and collect their y origins.
        let mut ys: Vec<f32> = Vec::new();
        for (id, node) in s.tree.iter() {
            if node.tag() == Some("div") {
                if let Some(b) = result.bounds(id) {
                    ys.push(b.origin.y.0);
                }
            }
        }
        assert_eq!(ys.len(), 2, "expected two divs laid out");
        // They must be at different vertical positions (stacked), not both 0.
        assert!(
            (ys[0] - ys[1]).abs() > 1.0,
            "blocks should stack vertically, got ys={ys:?}"
        );
    }

    #[test]
    fn fixed_size_is_respected() {
        let s = styled(
            "<html><body><div class=a></div></body></html>",
            ".a{width:120px;height:40px}",
        );
        let result = layout(&s, 800.0, 600.0);
        let mut found = false;
        for (id, node) in s.tree.iter() {
            if node.tag() == Some("div") {
                let b = result.bounds(id).expect("div laid out");
                assert!(
                    (b.size.width.0 - 120.0).abs() < 1.0,
                    "width {:?}",
                    b.size.width
                );
                assert!(
                    (b.size.height.0 - 40.0).abs() < 1.0,
                    "height {:?}",
                    b.size.height
                );
                found = true;
            }
        }
        assert!(found, "div not found");
    }
}
