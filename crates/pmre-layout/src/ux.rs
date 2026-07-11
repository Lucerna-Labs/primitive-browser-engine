//! UXI intent vocabulary: a tree of styled boxes and text, with NO coordinates.
//! Position is derived by the layout solver (`crate::layout`), never authored here.
//! HTML/CSS reduces onto this same vocabulary (a box tree + a property subset).
//!
//! Interaction is carried as data too: a box may declare an `id` and a `Role`
//! (Button / Toggle / Scroll). The kit provides hit-testing and scroll/clip mechanism;
//! all widget *policy* (hover/press visuals, toggle flips, scroll offsets) lives in the
//! orchestrator and the app's state-driven `build` function.

use pmre_core::paint::Rgba;
use pmre_raster::raster::Image;
use std::sync::Arc;

/// Main-axis direction of a box's children (the flex axis).
#[derive(Clone, Copy, Debug)]
pub enum Dir {
    Row,
    Column,
}

/// A size along one axis. `Flex` grows to share leftover main-axis space by weight.
/// `Pct` is a percentage of the parent's content extent on that axis.
#[derive(Clone, Copy, Debug)]
pub enum Dim {
    Auto,
    Px(f32),
    Flex(f32),
    Pct(f32),
}

/// Cross-axis alignment of children.
#[derive(Clone, Copy, Debug)]
pub enum Align {
    Start,
    Center,
    End,
    Stretch,
}

/// Main-axis distribution of children.
#[derive(Clone, Copy, Debug)]
pub enum Justify {
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Interactive role of a box. `None` is inert; the others are hit-testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    None,
    Button,
    Toggle,
    Scroll,
    Input,
}

/// Per-side lengths (padding / border insets).
#[derive(Clone, Copy, Debug, Default)]
pub struct Edges {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl Edges {
    pub fn all(v: f32) -> Self {
        Self {
            l: v,
            t: v,
            r: v,
            b: v,
        }
    }
    pub fn xy(x: f32, y: f32) -> Self {
        Self {
            l: x,
            t: y,
            r: x,
            b: y,
        }
    }
}

/// A soft drop shadow behind a box: offset, blur radius, and color.
#[derive(Clone, Copy, Debug)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub blur: f32,
    pub color: Rgba,
}

/// The reduced style subset shared by UXI and HTML/CSS, plus interaction metadata.
#[derive(Clone, Copy, Debug)]
pub struct Style {
    pub dir: Dir,
    pub width: Dim,
    pub height: Dim,
    pub padding: Edges,
    pub margin: Edges,
    pub gap: f32,
    pub align: Align,
    pub justify: Justify,
    pub background: Option<Rgba>,
    pub radius: f32,
    pub border: Option<(f32, Rgba)>,
    pub shadow: Option<Shadow>,
    pub id: Option<u32>,
    pub role: Role,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            dir: Dir::Column,
            width: Dim::Auto,
            height: Dim::Auto,
            padding: Edges::default(),
            margin: Edges::default(),
            gap: 0.0,
            align: Align::Stretch, // matches CSS flexbox `align-items: stretch`
            justify: Justify::Start,
            background: None,
            radius: 0.0,
            border: None,
            shadow: None,
            id: None,
            role: Role::None,
        }
    }
}

impl Style {
    pub fn row() -> Self {
        Self {
            dir: Dir::Row,
            ..Self::default()
        }
    }
    pub fn col() -> Self {
        Self {
            dir: Dir::Column,
            ..Self::default()
        }
    }
    pub fn w(mut self, d: Dim) -> Self {
        self.width = d;
        self
    }
    pub fn h(mut self, d: Dim) -> Self {
        self.height = d;
        self
    }
    pub fn pad(mut self, e: Edges) -> Self {
        self.padding = e;
        self
    }
    pub fn margin(mut self, e: Edges) -> Self {
        self.margin = e;
        self
    }
    pub fn shadow(mut self, dx: f32, dy: f32, blur: f32, color: Rgba) -> Self {
        self.shadow = Some(Shadow {
            dx,
            dy,
            blur,
            color,
        });
        self
    }
    pub fn gap(mut self, g: f32) -> Self {
        self.gap = g;
        self
    }
    pub fn align(mut self, a: Align) -> Self {
        self.align = a;
        self
    }
    pub fn justify(mut self, j: Justify) -> Self {
        self.justify = j;
        self
    }
    pub fn bg(mut self, c: Rgba) -> Self {
        self.background = Some(c);
        self
    }
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = r;
        self
    }
    pub fn border(mut self, w: f32, c: Rgba) -> Self {
        self.border = Some((w, c));
        self
    }
    /// Make this box hit-testable with the given id and role.
    pub fn interactive(mut self, id: u32, role: Role) -> Self {
        self.id = Some(id);
        self.role = role;
        self
    }
    pub fn button(self, id: u32) -> Self {
        self.interactive(id, Role::Button)
    }
    pub fn toggle(self, id: u32) -> Self {
        self.interactive(id, Role::Toggle)
    }
    pub fn scroll(self, id: u32) -> Self {
        self.interactive(id, Role::Scroll)
    }
    pub fn input(self, id: u32) -> Self {
        self.interactive(id, Role::Input)
    }
}

/// One styled run inside a rich-text flow. Spans wrap together as a single paragraph,
/// so bold/linked/colored fragments flow inline the way HTML text does.
///
/// `href` carries an `<a href="...">`'s target, if this span is (or is nested inside)
/// a link — plain data, no navigation policy here. `layout::hit_test_link` finds which
/// wrapped piece of text a point falls on and returns its href; what to do with that
/// href (resolve it, navigate, ignore it) is entirely the embedding app's call.
#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub size: f32,
    pub color: Rgba,
    pub bold: bool,
    pub underline: bool,
    pub href: Option<String>,
}

impl Span {
    pub fn new(text: impl Into<String>, size: f32, color: Rgba) -> Span {
        Span {
            text: text.into(),
            size,
            color,
            bold: false,
            underline: false,
            href: None,
        }
    }
    pub fn bold(mut self) -> Span {
        self.bold = true;
        self
    }
    pub fn underline(mut self) -> Span {
        self.underline = true;
        self
    }
    pub fn link(mut self, href: impl Into<String>) -> Span {
        self.href = Some(href.into());
        self
    }
}

/// A UXI node: a styled box with children, a run of plain text, a rich inline
/// flow, or a bitmap image blit.
#[derive(Clone, Debug)]
pub enum UxNode {
    Box {
        style: Style,
        children: Vec<UxNode>,
    },
    Text {
        content: String,
        size: f32,
        color: Rgba,
    },
    /// Inline flow of styled spans that word-wrap together; `align` places each
    /// wrapped line horizontally within the node's rect.
    Rich {
        spans: Vec<Span>,
        align: Align,
    },
    /// A bitmap image blitted at the box's solved rect. `style` carries the
    /// width/height (typically `Dim::Px` matching the HTML img `width`/`height`
    /// attributes). `Arc` so the same decoded image can be shared across
    /// multiple `<img>` tags with the same `src` without a re-decode.
    Image {
        style: Style,
        image: Arc<Image>,
    },
}

impl UxNode {
    pub fn boxed(style: Style, children: Vec<UxNode>) -> UxNode {
        UxNode::Box { style, children }
    }
    pub fn text(content: impl Into<String>, size: f32, color: Rgba) -> UxNode {
        UxNode::Text {
            content: content.into(),
            size,
            color,
        }
    }
    pub fn rich(spans: Vec<Span>) -> UxNode {
        UxNode::Rich {
            spans,
            align: Align::Start,
        }
    }
    /// A bitmap image at the solved rect defined by `style` (width/height).
    /// Callers holding the `Arc` avoid re-decoding when the same image
    /// appears multiple times.
    pub fn image(style: Style, image: Arc<Image>) -> UxNode {
        UxNode::Image { style, image }
    }
}
