//! Reduced HTML/CSS front-end. Parses an HTML subset with inline `style` attributes
//! *and* a bounded `<style>`-block selector engine (`crate::css`) into the shared UXI
//! box tree (`crate::ux`), which then flows through the same layout + raster path as
//! native UXI. This is the "reduce": the load-bearing core is the box model,
//! block/flex layout, inline text flow, and a CSS property subset; the *full* cascade
//! (combinators, pseudo-classes, `@media`, specificity edge cases beyond id/class/type)
//! is the expansion, not the foundation — see `crate::css`'s doc comment for exactly
//! where that line is drawn.
//!
//! Supported structure: comments, doctype, `<script>` skipping (`<style>` content is
//! captured, not skipped — see below), entities, block elements (`div p h1-h4 ul ol li
//! hr section header footer main nav article`), and inline elements (`b strong i em u a
//! span small code mark`) that coalesce into a single word-wrapping rich flow —
//! `<b>bold</b> and plain` stays on one line.
//!
//! Supported CSS: inline `style="..."` attributes, **and** `<style>` blocks with simple/
//! compound selectors (`div`, `.card`, `#hero`, `div.card#hero`) — see `crate::css` for
//! exactly what selector syntax is (and isn't) supported. Cascade order: type < class <
//! id < inline, later rules of equal specificity win, matching the real cascade's
//! source-order tiebreak. Properties: display(flex|block|none), flex-direction, flex,
//! flex-grow, width/height (px/%/auto), padding/margin (+ per-side, 1-4 value shorthand),
//! gap, background(-color), border, border-radius, box-shadow, color, font-size,
//! font-weight, text-align, text-decoration, align-items, justify-content, opacity.
//! Colors: #rgb/#rrggbb/#rrggbbaa, rgb()/rgba(), hsl(), ~40 named colors.

use crate::css::{self, Rule};
use pmre_core::paint::Rgba;
use pmre_layout::ux::{Align, Dim, Dir, Edges, Justify, Shadow, Span, Style, UxNode};
use pmre_raster::raster::Image;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;

/// Numeric widget ids allocated to HTML form controls (`<input>`, `<button>`,
/// `<textarea>`, `<select>`) so the orchestrator's `UiState` can track them
/// interactively. Starts at `FORM_BASE` — well above the browser chrome's
/// reserved range (1–99 in `pbe-shell`) so a page's controls never collide
/// with the address bar / nav buttons.
const FORM_BASE: u32 = 1000;

/// Per-parse monotonic id allocator for form controls. Cheap `Cell<u32>`;
/// one counter per `parse_with_images` call, so ids are stable within a
/// single render but don't leak across pages.
struct IdAlloc {
    next: Cell<u32>,
}

impl IdAlloc {
    fn new() -> Self {
        IdAlloc {
            next: Cell::new(FORM_BASE),
        }
    }
    fn alloc(&self) -> u32 {
        let id = self.next.get();
        self.next.set(id + 1);
        id
    }
}

// ─── DOM ─────────────────────────────────────────────────────────────────────

/// Attributes specific to `<img>`. Only ever populated when `tag == "img"`;
/// wrapped in `Option` on Dom::Elem/Tok::Open so the common case (every non-
/// image element) costs one null pointer, not four `Option<String>` fields.
#[derive(Clone, Debug, Default)]
pub struct ImgAttrs {
    pub src: Option<String>,
    pub alt: Option<String>,
    pub width: Option<String>,
    pub height: Option<String>,
}

#[allow(clippy::large_enum_variant)]
enum Dom {
    Elem {
        tag: String,
        style_attr: Option<String>,
        /// An `<a href="...">`'s target. Only ever populated for `tag == "a"`;
        /// carried on every element (not a dedicated `<a>` variant) for the
        /// same reason `style_attr` is — one uniform shape, no special-casing
        /// through the tokenizer/parser.
        href_attr: Option<String>,
        /// Space-separated `class="..."` list, already split.
        classes: Vec<String>,
        id_attr: Option<String>,
        /// `<img>`-specific attrs (src/alt/width/height). `None` on every
        /// other element.
        img_attrs: Option<ImgAttrs>,
        /// The `type="..."` attribute for `<input>`/`<button>` (text,
        /// checkbox, submit, ...). `None` on every other element.
        form_type: Option<String>,
        kids: Vec<Dom>,
    },
    Text(String),
}

#[allow(clippy::large_enum_variant)] // Open carries the full attr set; Tok is transient
enum Tok {
    Open {
        tag: String,
        style_attr: Option<String>,
        href_attr: Option<String>,
        classes: Vec<String>,
        id_attr: Option<String>,
        img_attrs: Option<ImgAttrs>,
        form_type: Option<String>,
        self_close: bool,
    },
    Close(String),
    Text(String),
}

/// Inherited text properties (a minimal stand-in for CSS inheritance).
#[derive(Clone, Copy)]
struct Inherited {
    color: Rgba,
    font_size: f32,
    bold: bool,
    underline: bool,
    text_align: Align,
    opacity: f32,
    text_transform: TextTransform,
}

/// CSS `text-transform` — applied to every text span as it flows into a
/// `Rich`/`Text` node. Threaded through `Inherited` so nested inline
/// elements inherit their ancestor's transform unless overridden.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum TextTransform {
    /// Leave the text as authored.
    #[default]
    None,
    /// ASCII uppercase every character (matches the majority of real
    /// stylesheets that use `text-transform: uppercase` on nav/header labels).
    Uppercase,
    /// ASCII lowercase every character.
    Lowercase,
    /// Uppercase the first character of every whitespace-delimited word;
    /// leave the rest unchanged (CSS spec §2.1 — real UAs do more with
    /// Unicode word breaks, but that's outside this kit's Unicode scope).
    Capitalize,
}

impl TextTransform {
    fn apply(self, s: &str) -> String {
        match self {
            TextTransform::None => s.to_string(),
            TextTransform::Uppercase => s.to_ascii_uppercase(),
            TextTransform::Lowercase => s.to_ascii_lowercase(),
            TextTransform::Capitalize => {
                let mut out = String::with_capacity(s.len());
                let mut at_word_start = true;
                for ch in s.chars() {
                    if ch.is_whitespace() {
                        out.push(ch);
                        at_word_start = true;
                    } else if at_word_start {
                        // ASCII upper only; matches Uppercase's scope. Chars
                        // that don't case-shift stay themselves.
                        for c in ch.to_uppercase() {
                            out.push(c);
                        }
                        at_word_start = false;
                    } else {
                        out.push(ch);
                    }
                }
                out
            }
        }
    }
}

impl Default for Inherited {
    fn default() -> Self {
        Inherited {
            color: Rgba::rgb8(228, 232, 240),
            font_size: 14.0,
            bold: false,
            underline: false,
            text_align: Align::Start,
            opacity: 1.0,
            text_transform: TextTransform::None,
        }
    }
}

/// Parse an HTML document fragment into a single UXI root node. `<style>`
/// blocks are collected from anywhere in the document and cascaded (type <
/// class < id < inline `style="..."` precedence, source order breaking ties)
/// against every element — the same one-pass reduce as always, just with a
/// stylesheet computed up front instead of only reading inline styles.
///
/// `<img>` tags are dropped (rendered as nothing) — this entry point has no
/// image data. To render images, pre-fetch their bytes at the caller layer,
/// decode via `raster::decode_bmp`/`decode_png`, and pass the resulting map to
/// `parse_with_images`.
pub fn parse(src: &str) -> UxNode {
    parse_with_images(src, &HashMap::new())
}

/// Parse an HTML document fragment, painting `<img>` tags whose `src` is
/// present in `images` as `UxNode::Image`. Any `<img>` whose src is missing
/// from the map (fetch failed, undecodable bytes, or the map is empty) is
/// dropped, matching the doctrine boundary: the kit does not fetch — it only
/// renders what it's given.
pub fn parse_with_images(src: &str, images: &HashMap<String, Arc<Image>>) -> UxNode {
    let toks = tokenize(src);
    let mut pos = 0usize;
    let roots = parse_nodes(&toks, &mut pos, None, 0);
    let mut style_text = String::new();
    collect_style_text(&roots, &mut style_text);
    let sheet = css::parse_stylesheet(&style_text);
    let mut ancestors: Vec<AncestorStackFrame> = Vec::new();
    let ids = IdAlloc::new();
    let kids = children_to_ux(
        &roots,
        Inherited::default(),
        None,
        false,
        &sheet,
        images,
        ParentList::None,
        &mut ancestors,
        &ids,
    );
    if kids.len() == 1 {
        kids.into_iter().next().unwrap()
    } else {
        UxNode::Box {
            style: Style::col(),
            children: kids,
        }
    }
}

/// Walk the DOM collecting every `<style>` element's text content into one
/// combined stylesheet source (in document order, so rule source-order
/// ties resolve the same way a real cascade's does).
fn collect_style_text(nodes: &[Dom], out: &mut String) {
    for n in nodes {
        let Dom::Elem { tag, kids, .. } = n else {
            continue;
        };
        if tag == "style" {
            for k in kids {
                if let Dom::Text(t) = k {
                    out.push_str(t);
                    out.push('\n');
                }
            }
        } else {
            collect_style_text(kids, out);
        }
    }
}

fn is_void(tag: &str) -> bool {
    matches!(tag, "br" | "img" | "hr" | "input" | "meta" | "link")
}

fn is_inline(tag: &str) -> bool {
    matches!(
        tag,
        "b" | "strong" | "i" | "em" | "u" | "a" | "span" | "small" | "code" | "mark" | "br"
    )
}

fn is_dropped(tag: &str) -> bool {
    matches!(tag, "img" | "meta" | "link" | "head" | "title" | "style")
}

// ─── Tokenizer ───────────────────────────────────────────────────────────────

/// Collapse whitespace runs to single spaces, **preserving** boundary spaces so inline
/// runs keep their word gaps (`<b>bold</b> text` needs the space before "text").
fn collapse_ws(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

/// Decode the common HTML entities in already-collapsed text.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let b: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    while i < b.len() {
        if b[i] == '&' {
            // an entity name is short — bound the scan so '&' runs stay O(n)
            let window = &b[i + 1..(i + 33).min(b.len())];
            if let Some(semi) = window.iter().position(|&c| c == ';') {
                let name: String = b[i + 1..i + 1 + semi].iter().collect();
                let decoded = match name.as_str() {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    "middot" => Some('·'),
                    "bull" => Some('•'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "hellip" => Some('…'),
                    "copy" => Some('©'),
                    "times" => Some('×'),
                    "eacute" => Some('é'),
                    "egrave" => Some('è'),
                    "agrave" => Some('à'),
                    "uuml" => Some('ü'),
                    "ouml" => Some('ö'),
                    "auml" => Some('ä'),
                    "deg" => Some('°'),
                    "rarr" => Some('→'),
                    "larr" => Some('←'),
                    _ => {
                        if let Some(num) = name.strip_prefix("#x").or(name.strip_prefix("#X")) {
                            u32::from_str_radix(num, 16).ok().and_then(char::from_u32)
                        } else if let Some(num) = name.strip_prefix('#') {
                            num.parse::<u32>().ok().and_then(char::from_u32)
                        } else {
                            None
                        }
                    }
                };
                if let Some(c) = decoded {
                    out.push(c);
                    i += semi + 2;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < b.len() {
        if b[i] == b'<' {
            // comments and doctype
            if src[i..].starts_with("<!--") {
                i = match src[i + 4..].find("-->") {
                    Some(end) => i + 4 + end + 3,
                    None => b.len(),
                };
                continue;
            }
            if src[i..].starts_with("<!") || src[i..].starts_with("<?") {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'>' {
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            let mut j = i + 1;
            let mut quote: u8 = 0;
            while j < b.len() {
                let c = b[j];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                j += 1;
            }
            let inner = src[i + 1..j.min(b.len())].trim();
            if let Some(rest) = inner.strip_prefix('/') {
                out.push(Tok::Close(rest.trim().to_ascii_lowercase()));
            } else {
                let self_close = inner.ends_with('/');
                let inner = inner.trim_end_matches('/').trim();
                let (tag, style_attr, href_attr, classes, id_attr, img_attrs, form_type) =
                    parse_open(inner);
                // Raw-content element: skip everything until the matching
                // close tag, scanning in place (no copy/lowercase of the
                // whole remainder). Only <script> — its JS isn't executed,
                // so there's nothing to gain from tokenizing it, and
                // treating it as text risks a stray '<'/'>' inside a string
                // literal confusing the tokenizer. <style> content, by
                // contrast, is real markup this engine now reads (see
                // `crate::css`), so it is *not* skipped — it flows through
                // the normal Open/Text/Close path like any other element and
                // gets dropped from the render tree later (`is_dropped`),
                // the same way `<head>`/`<title>` already are.
                if tag == "script" {
                    let close = format!("</{tag}");
                    i = match find_ascii_ci(b, j + 1, close.as_bytes()) {
                        Some(after) => match src[after..].find('>') {
                            Some(g) => after + g + 1,
                            None => b.len(),
                        },
                        None => b.len(),
                    };
                    continue;
                }
                let self_close = self_close || is_void(&tag);
                out.push(Tok::Open {
                    tag,
                    style_attr,
                    href_attr,
                    classes,
                    id_attr,
                    img_attrs,
                    form_type,
                    self_close,
                });
            }
            i = j + 1;
        } else {
            let start = i;
            while i < b.len() && b[i] != b'<' {
                i += 1;
            }
            let text = decode_entities(&collapse_ws(&src[start..i]));
            if !text.is_empty() {
                out.push(Tok::Text(text));
            }
        }
    }
    out
}

/// Case-insensitive ASCII substring search over bytes starting at `from`;
/// returns the index of the first match.
fn find_ascii_ci(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (from..=hay.len() - needle.len())
        .find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Pull the tag name and the `style`/`href`/`class`/`id`/img attribute values
/// out of an opening-tag body, scanning attributes properly (quoted values
/// may contain spaces and `=`).
#[allow(clippy::type_complexity)]
fn parse_open(
    inner: &str,
) -> (
    String,
    Option<String>,
    Option<String>,
    Vec<String>,
    Option<String>,
    Option<ImgAttrs>,
    Option<String>,
) {
    let mut it = inner.splitn(2, char::is_whitespace);
    let tag = it.next().unwrap_or("").to_ascii_lowercase();
    let attrs = it.next().unwrap_or("");
    let href = if tag == "a" {
        find_attr(attrs, "href")
    } else {
        None
    };
    let classes = find_attr(attrs, "class")
        .map(|c| c.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let id_attr = find_attr(attrs, "id");
    let img_attrs = if tag == "img" {
        Some(ImgAttrs {
            src: find_attr(attrs, "src"),
            alt: find_attr(attrs, "alt"),
            width: find_attr(attrs, "width"),
            height: find_attr(attrs, "height"),
        })
    } else {
        None
    };
    let form_type = if tag == "input" || tag == "button" {
        find_attr(attrs, "type")
    } else {
        None
    };
    (
        tag,
        find_attr(attrs, "style"),
        href,
        classes,
        id_attr,
        img_attrs,
        form_type,
    )
}

fn find_attr(attrs: &str, want: &str) -> Option<String> {
    let b: Vec<char> = attrs.chars().collect();
    let mut i = 0usize;
    while i < b.len() {
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        // attribute name
        let name_start = i;
        while i < b.len() && b[i] != '=' && !b[i].is_whitespace() {
            i += 1;
        }
        let name: String = b[name_start..i]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase();
        while i < b.len() && b[i].is_whitespace() {
            i += 1;
        }
        let mut value = String::new();
        if i < b.len() && b[i] == '=' {
            i += 1;
            while i < b.len() && b[i].is_whitespace() {
                i += 1;
            }
            if i < b.len() && (b[i] == '"' || b[i] == '\'') {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    value.push(b[i]);
                    i += 1;
                }
                i += 1; // closing quote
            } else {
                while i < b.len() && !b[i].is_whitespace() {
                    value.push(b[i]);
                    i += 1;
                }
            }
        }
        if name == want {
            return Some(value);
        }
        if name.is_empty() {
            i += 1; // guard against pathological input
        }
    }
    None
}

/// Recursion guard: DOM nesting past this depth is flattened (children parsed as
/// siblings) instead of overflowing the stack on adversarial input.
const MAX_DOM_DEPTH: usize = 192;

fn parse_nodes(toks: &[Tok], pos: &mut usize, stop: Option<&str>, depth: usize) -> Vec<Dom> {
    let mut nodes = Vec::new();
    while *pos < toks.len() {
        match &toks[*pos] {
            Tok::Close(name) => {
                if Some(name.as_str()) == stop {
                    *pos += 1;
                    return nodes;
                }
                *pos += 1; // stray close: skip
            }
            Tok::Text(t) => {
                nodes.push(Dom::Text(t.clone()));
                *pos += 1;
            }
            Tok::Open {
                tag,
                style_attr,
                href_attr,
                classes,
                id_attr,
                img_attrs,
                form_type,
                self_close,
            } => {
                let tag = tag.clone();
                let style_attr = style_attr.clone();
                let href_attr = href_attr.clone();
                let classes = classes.clone();
                let id_attr = id_attr.clone();
                let img_attrs = img_attrs.clone();
                let form_type = form_type.clone();
                let self_close = *self_close || depth >= MAX_DOM_DEPTH;
                *pos += 1;
                let kids = if self_close {
                    Vec::new()
                } else {
                    parse_nodes(toks, pos, Some(&tag), depth + 1)
                };
                nodes.push(Dom::Elem {
                    tag,
                    style_attr,
                    href_attr,
                    classes,
                    id_attr,
                    img_attrs,
                    form_type,
                    kids,
                });
            }
        }
    }
    nodes
}

// ─── DOM → UXI ───────────────────────────────────────────────────────────────

fn tag_font(tag: &str, base: f32) -> f32 {
    match tag {
        "h1" => 30.0,
        "h2" => 24.0,
        "h3" => 18.0,
        "h4" => 16.0,
        "small" => (base * 0.85).max(8.0),
        "code" => (base * 0.95).max(8.0),
        _ => base,
    }
}

fn tag_default_style(tag: &str) -> Style {
    let mut s = Style::col();
    match tag {
        "p" => s.margin = Edges::xy(0.0, 6.0),
        "h1" => s.margin = Edges::xy(0.0, 10.0),
        "h2" => s.margin = Edges::xy(0.0, 8.0),
        "h3" | "h4" => s.margin = Edges::xy(0.0, 6.0),
        "ul" | "ol" => {
            s.margin = Edges::xy(0.0, 6.0);
            s.padding = Edges {
                l: 8.0,
                t: 0.0,
                r: 0.0,
                b: 0.0,
            };
            s.gap = 4.0;
        }
        "li" => s.gap = 2.0,
        // Semantic block-quote: indented + a left border rail. Matches the
        // most common real-world browser default for `<blockquote>` — real
        // stylesheets override via `.quote { … }` or similar; this ensures
        // unstyled blockquotes still visually stand out.
        "blockquote" => {
            s.margin = Edges {
                l: 24.0,
                r: 24.0,
                t: 8.0,
                b: 8.0,
            };
            s.padding = Edges {
                l: 12.0,
                r: 12.0,
                t: 4.0,
                b: 4.0,
            };
            s.border = Some((3.0, Rgba::rgb8(120, 120, 140)));
        }
        // `<hr>` — a 1px horizontal rule with a small vertical margin.
        // Moved into tag_default_style from a special-cased short-circuit in
        // `elem_to_ux` so CSS class/id overrides actually apply (before, the
        // short-circuit returned before the cascade ran).
        "hr" => {
            s.height = Dim::Px(1.0);
            s.background = Some(Rgba::rgb8(63, 63, 70));
            s.margin = Edges::xy(0.0, 8.0);
        }
        _ => {}
    }
    s
}

/// One ancestor of the currently-matched element, owned so it can live in a
/// mutable stack threaded through the recursive DOM walk without lifetime
/// contortions. `classes` is a plain owned `Vec<String>`; the cascade
/// converts to `&[&str]` at match time.
type AncestorStackFrame = (String, Option<String>, Vec<String>);

/// Convert an owned ancestor stack into the borrowed form
/// `Rule::specificity_if_matches` expects. Called once per element cascade,
/// so a single small allocation is amortised over every matched rule.
fn borrow_ancestors<'a>(
    stack: &'a [AncestorStackFrame],
    class_scratch: &'a mut Vec<Vec<&'a str>>,
) -> Vec<(&'a str, Option<&'a str>, &'a [&'a str])> {
    class_scratch.clear();
    class_scratch.reserve(stack.len());
    for (_, _, cs) in stack {
        class_scratch.push(cs.iter().map(String::as_str).collect());
    }
    stack
        .iter()
        .zip(class_scratch.iter())
        .map(|((t, i, _), cs)| (t.as_str(), i.as_deref(), cs.as_slice()))
        .collect()
}

/// Every stylesheet rule that matches this element, sorted lowest-to-highest
/// precedence (specificity first, source order breaking ties — `sheet` is
/// already in source order and `sort_by_key` is stable, so no explicit
/// `Rule::order` comparison is needed to get that tiebreak right). Applying
/// them in this order and letting each later `apply_css` call overwrite the
/// same `Style`/`Inherited` fields *is* the cascade.
fn matched_rules<'a>(
    sheet: &'a [Rule],
    ancestors: &[AncestorStackFrame],
    tag: &str,
    id: Option<&str>,
    classes: &[&str],
) -> Vec<&'a Rule> {
    let mut scratch: Vec<Vec<&str>> = Vec::with_capacity(ancestors.len());
    let borrowed = borrow_ancestors(ancestors, &mut scratch);
    let mut matched: Vec<(&Rule, (u32, u32, u32))> = sheet
        .iter()
        .filter_map(|r| {
            r.specificity_if_matches(&borrowed, tag, id, classes)
                .map(|sp| (r, sp))
        })
        .collect();
    matched.sort_by_key(|(_, sp)| *sp);
    matched.into_iter().map(|(r, _)| r).collect()
}

/// Whether a declaration block mentions `display`, and if so, whether it's
/// `none`. `apply_css`'s own bool return only reflects one declaration block
/// in isolation (fine for a single `style="..."` attribute), which isn't
/// enough once several cascaded rules of different specificity can each
/// either set or *not mention* `display` on the same element — a later rule
/// that doesn't mention `display` at all must not un-hide an earlier
/// `display:none`, and a later, more specific rule that says `display:block`
/// must. This tracks that independent of `apply_css`'s per-call return.
fn declares_display_none(decls: &str) -> Option<bool> {
    for decl in decls.split(';') {
        let mut kv = decl.splitn(2, ':');
        let key = kv.next().unwrap_or("").trim().to_ascii_lowercase();
        if key == "display" {
            let val = kv.next().unwrap_or("").trim().to_ascii_lowercase();
            return Some(val == "none");
        }
    }
    None
}

/// Apply every matched stylesheet rule (lowest to highest precedence), then
/// the inline `style="..."` (always highest precedence, applied last).
/// Returns whether the element ends up visible.
#[allow(clippy::too_many_arguments)]
fn apply_cascade(
    style: &mut Style,
    inh: &mut Inherited,
    sheet: &[Rule],
    ancestors: &[AncestorStackFrame],
    tag: &str,
    id: Option<&str>,
    classes: &[&str],
    inline_style: Option<&str>,
) -> bool {
    let mut hidden = false;
    for r in matched_rules(sheet, ancestors, tag, id, classes) {
        apply_css(style, inh, &r.declarations);
        if let Some(is_none) = declares_display_none(&r.declarations) {
            hidden = is_none;
        }
    }
    if let Some(css) = inline_style {
        apply_css(style, inh, css);
        if let Some(is_none) = declares_display_none(css) {
            hidden = is_none;
        }
    }
    !hidden
}

/// Convert a list of sibling DOM nodes, coalescing text and inline elements into
/// shared `Rich` flows so mixed-style words wrap on the same lines. Inside a
/// `display:flex` row (`flex_row`), CSS makes every child its own flex item instead,
/// so inline elements stay separate boxes and the container's `gap` applies.
/// The kind of list an `<li>` is a child of. Passed down into `children_to_ux`
/// so its `<li>` prefix can be a "• " bullet for `<ul>`, an "N. " numeral for
/// `<ol>`, or nothing at all for a stray `<li>` outside any list container.
#[derive(Clone, Copy)]
enum ParentList {
    None,
    Unordered,
    Ordered,
}

#[allow(clippy::too_many_arguments)]
fn children_to_ux(
    kids: &[Dom],
    inh: Inherited,
    li_prefix: Option<&str>,
    flex_row: bool,
    sheet: &[Rule],
    images: &HashMap<String, Arc<Image>>,
    parent_list: ParentList,
    ancestors: &mut Vec<AncestorStackFrame>,
    ids: &IdAlloc,
) -> Vec<UxNode> {
    let mut out: Vec<UxNode> = Vec::new();
    let mut run: Vec<Span> = Vec::new();
    let mut first_flush = true;
    // Sequential index of the current `<li>` sibling under this list — only
    // used when `parent_list` is `Ordered`; increments as each `<li>` is
    // dispatched so the numeric prefix reflects source order.
    let mut li_index: usize = 0;

    let flush = |run: &mut Vec<Span>, out: &mut Vec<UxNode>, first: &mut bool| {
        let has_content = run.iter().any(|s| !s.text.trim().is_empty());
        if has_content {
            let mut spans = std::mem::take(run);
            if *first {
                if let Some(prefix) = li_prefix {
                    let mut bullet = Span::new(prefix, inh.font_size, inh.color);
                    bullet.bold = false;
                    spans.insert(0, bullet);
                }
            }
            *first = false;
            out.push(UxNode::Rich {
                spans,
                align: inh.text_align,
            });
        } else {
            run.clear();
        }
    };

    for k in kids {
        match k {
            Dom::Text(t) => {
                // Plain text directly inside a block (not wrapped in an <a>)
                // carries no link target.
                run.push(make_span(t, inh, None));
                if flex_row {
                    flush(&mut run, &mut out, &mut first_flush);
                }
            }
            Dom::Elem { tag, .. } if tag == "br" => {
                flush(&mut run, &mut out, &mut first_flush);
            }
            Dom::Elem {
                tag,
                style_attr,
                href_attr,
                classes,
                id_attr,
                kids: inner,
                ..
            } if is_inline(tag) => {
                // `href_attr` seeds the link context here — `None` for every
                // inline tag except `<a>`, which is where it's ever populated.
                inline_spans(
                    tag,
                    id_attr.as_deref(),
                    classes,
                    style_attr.as_deref(),
                    inner,
                    inh,
                    href_attr.as_deref(),
                    sheet,
                    &mut run,
                    ancestors,
                );
                if flex_row {
                    flush(&mut run, &mut out, &mut first_flush);
                }
            }
            Dom::Elem {
                tag,
                style_attr,
                classes,
                id_attr,
                img_attrs,
                form_type,
                kids: inner,
                ..
            } => {
                flush(&mut run, &mut out, &mut first_flush);
                let prefix = if tag == "li" {
                    match parent_list {
                        ParentList::Ordered => {
                            li_index += 1;
                            Some(format!("{li_index}. "))
                        }
                        ParentList::Unordered => Some("• ".to_string()),
                        // A stray <li> without a <ul>/<ol> parent stays
                        // bullet-prefixed to match the existing behaviour;
                        // real HTML almost never has this, but changing it
                        // would silently regress any page that relied on it.
                        ParentList::None => Some("• ".to_string()),
                    }
                } else {
                    None
                };
                if let Some(node) = elem_to_ux(
                    tag,
                    id_attr.as_deref(),
                    classes,
                    style_attr.as_deref(),
                    img_attrs.as_ref(),
                    form_type.as_deref(),
                    inner,
                    inh,
                    prefix,
                    sheet,
                    images,
                    ancestors,
                    ids,
                ) {
                    out.push(node);
                }
            }
        }
    }
    flush(&mut run, &mut out, &mut first_flush);
    out
}

fn make_span(text: &str, inh: Inherited, href: Option<&str>) -> Span {
    Span {
        text: inh.text_transform.apply(text),
        size: inh.font_size,
        color: inh.color.with_alpha(inh.color.a * inh.opacity),
        bold: inh.bold,
        underline: inh.underline,
        href: href.map(str::to_string),
    }
}

/// Flatten an inline element (possibly nested) into styled spans appended to
/// `run`. `href` is the link target currently in scope (`None` outside any
/// `<a>`) — threaded as a plain parameter rather than folded into `Inherited`
/// so `Inherited` keeps its `Copy` bound; passed explicitly through the
/// recursive call below the same way `li_prefix` is threaded through
/// `children_to_ux`, not stored on shared state.
#[allow(clippy::too_many_arguments)]
fn inline_spans(
    tag: &str,
    id: Option<&str>,
    classes: &[String],
    style_attr: Option<&str>,
    kids: &[Dom],
    inh: Inherited,
    href: Option<&str>,
    sheet: &[Rule],
    run: &mut Vec<Span>,
    ancestors: &mut Vec<AncestorStackFrame>,
) {
    let mut inh = inh;
    inh.font_size = tag_font(tag, inh.font_size);
    match tag {
        "b" | "strong" => inh.bold = true,
        "u" => inh.underline = true,
        "a" => {
            inh.underline = true;
            inh.color = Rgba::rgb8(96, 165, 250); // blue-400 — link affordance
        }
        "code" | "mark" => {
            inh.color = Rgba::rgb8(251, 191, 96); // amber — stands in for a code face
        }
        _ => {}
    }
    let class_refs: Vec<&str> = classes.iter().map(String::as_str).collect();
    let mut scratch = Style::col();
    apply_cascade(
        &mut scratch,
        &mut inh,
        sheet,
        ancestors,
        tag,
        id,
        &class_refs,
        style_attr,
    );
    // Push self before recursing into inline children so their cascade sees
    // this inline element in their ancestor chain (span > a > text case).
    ancestors.push((tag.to_string(), id.map(str::to_string), classes.to_vec()));
    for k in kids {
        match k {
            Dom::Text(t) => run.push(make_span(t, inh, href)),
            Dom::Elem {
                tag,
                style_attr,
                href_attr,
                classes,
                id_attr,
                kids,
                ..
            } if is_inline(tag) && tag != "br" => {
                let inner_href = if tag == "a" {
                    href_attr.as_deref()
                } else {
                    href
                };
                inline_spans(
                    tag,
                    id_attr.as_deref(),
                    classes,
                    style_attr.as_deref(),
                    kids,
                    inh,
                    inner_href,
                    sheet,
                    run,
                    ancestors,
                )
            }
            _ => {} // block inside inline: out of subset, dropped
        }
    }
    ancestors.pop();
}

#[allow(clippy::too_many_arguments)]
fn elem_to_ux(
    tag: &str,
    id: Option<&str>,
    classes: &[String],
    style_attr: Option<&str>,
    img_attrs: Option<&ImgAttrs>,
    form_type: Option<&str>,
    kids: &[Dom],
    inh: Inherited,
    li_prefix: Option<String>,
    sheet: &[Rule],
    images: &HashMap<String, Arc<Image>>,
    ancestors: &mut Vec<AncestorStackFrame>,
    ids: &IdAlloc,
) -> Option<UxNode> {
    // <img>: emit a UxNode::Image if the src is in the pre-fetched map, drop
    // otherwise. This runs *before* `is_dropped(tag)` so an img with a hit
    // reaches the render tree; a miss still hits is_dropped below and gets
    // dropped, matching the existing "no image => nothing" behaviour.
    if tag == "img" {
        if let Some(attrs) = img_attrs {
            if let Some(src) = attrs.src.as_deref() {
                if let Some(image) = images.get(src) {
                    let mut style = Style::col();
                    // Width/height from HTML attrs are unitless CSS pixels
                    // per the HTML spec (`width="200"` = 200px). Fall through
                    // to Dim::Auto (natural image size) when absent.
                    if let Some(w) = attrs.width.as_deref().and_then(parse_f32) {
                        style.width = Dim::Px(w);
                    }
                    if let Some(h) = attrs.height.as_deref().and_then(parse_f32) {
                        style.height = Dim::Px(h);
                    }
                    return Some(UxNode::image(style, image.clone()));
                }
            }
        }
    }
    // <input>/<button>/<textarea>/<select>: interactive form controls. The
    // kit's layout/paint already supports interactive roles (Style::input /
    // Style::button, hit-tested by the orchestrator's UiState); the gap was
    // only that the HTML reducer dropped these tags. Each gets a stable
    // numeric widget id (allocated from `ids`, base 1000+) so the browser
    // shell can track focus/value across renders. <form> itself is just a
    // container Box (already handled by the generic path below).
    if matches!(tag, "input" | "button" | "textarea" | "select") {
        let wid = ids.alloc();
        let mut style = tag_default_style(tag);
        let mut inh2 = inh;
        let class_refs: Vec<&str> = classes.iter().map(String::as_str).collect();
        let _ = apply_cascade(
            &mut style,
            &mut inh2,
            sheet,
            ancestors,
            tag,
            id,
            &class_refs,
            style_attr,
        );
        let style = match tag {
            "button" | "select" => style.button(wid),
            _ => style.input(wid),
        };
        // A <button>/<select> with text children renders its label inline;
        // an <input> is a void control with no children.
        let children = if matches!(tag, "button" | "select") {
            let child_ids = IdAlloc::new();
            children_to_ux(
                kids,
                inh2,
                None,
                false,
                sheet,
                images,
                ParentList::None,
                ancestors,
                &child_ids,
            )
        } else {
            Vec::new()
        };
        let _ = form_type; // captured for future type-specific styling
        return Some(UxNode::Box { style, children });
    }

    if is_dropped(tag) {
        return None;
    }
    let mut style = tag_default_style(tag);
    let mut inh2 = inh;
    inh2.font_size = tag_font(tag, inh.font_size);
    if matches!(tag, "h1" | "h2" | "h3" | "h4") {
        inh2.bold = true;
    }
    let class_refs: Vec<&str> = classes.iter().map(String::as_str).collect();
    let visible = apply_cascade(
        &mut style,
        &mut inh2,
        sheet,
        ancestors,
        tag,
        id,
        &class_refs,
        style_attr,
    );
    if !visible {
        return None;
    }
    let flex_row = matches!(style.dir, Dir::Row);
    // Establish this element's list context for its own children: `<ul>` →
    // Unordered (bullet), `<ol>` → Ordered (numbered), everything else →
    // None so nested content doesn't accidentally inherit a stale list mode.
    let this_list = match tag {
        "ul" => ParentList::Unordered,
        "ol" => ParentList::Ordered,
        _ => ParentList::None,
    };
    // Push self into the ancestor chain before descending, pop after — every
    // element becomes an ancestor for its own subtree's cascade lookups.
    ancestors.push((tag.to_string(), id.map(str::to_string), classes.to_vec()));
    let children = children_to_ux(
        kids,
        inh2,
        li_prefix.as_deref(),
        flex_row,
        sheet,
        images,
        this_list,
        ancestors,
        ids,
    );
    ancestors.pop();
    Some(UxNode::Box { style, children })
}

// ─── CSS ─────────────────────────────────────────────────────────────────────

/// Apply inline declarations to `style`/`inh`. Returns `false` for `display:none`.
fn apply_css(style: &mut Style, inh: &mut Inherited, css: &str) -> bool {
    let mut visible = true;
    for decl in css.split(';') {
        let mut kv = decl.splitn(2, ':');
        let key = kv.next().unwrap_or("").trim().to_ascii_lowercase();
        let val = match kv.next() {
            Some(v) => v.trim(),
            None => continue,
        };
        let lval = val.to_ascii_lowercase();
        match key.as_str() {
            "display" => match lval.as_str() {
                "none" => visible = false,
                "flex" => style.dir = Dir::Row,
                _ => style.dir = Dir::Column,
            },
            "flex-direction" => {
                style.dir = if lval.starts_with("row") {
                    Dir::Row
                } else {
                    Dir::Column
                };
            }
            "flex" | "flex-grow" => {
                if lval == "none" {
                    style.width = Dim::Auto;
                    style.height = Dim::Auto;
                } else if let Some(n) = val
                    .split_whitespace()
                    .next()
                    .and_then(parse_f32)
                    .or((lval == "auto").then_some(1.0))
                {
                    style.width = Dim::Flex(n);
                    style.height = Dim::Flex(n);
                }
                // any other non-numeric value: leave sizing untouched
            }
            "width" => style.width = parse_dim(val),
            "height" => style.height = parse_dim(val),
            "padding" => style.padding = parse_edges(val).unwrap_or(style.padding),
            "padding-left" => set_edge(&mut style.padding, val, 'l'),
            "padding-top" => set_edge(&mut style.padding, val, 't'),
            "padding-right" => set_edge(&mut style.padding, val, 'r'),
            "padding-bottom" => set_edge(&mut style.padding, val, 'b'),
            "margin" => style.margin = parse_edges(val).unwrap_or(style.margin),
            "margin-left" => set_edge(&mut style.margin, val, 'l'),
            "margin-top" => set_edge(&mut style.margin, val, 't'),
            "margin-right" => set_edge(&mut style.margin, val, 'r'),
            "margin-bottom" => set_edge(&mut style.margin, val, 'b'),
            "gap" => {
                if let Some(p) = parse_px(val) {
                    style.gap = p;
                }
            }
            "background" | "background-color" => {
                if let Some(c) = parse_color(val) {
                    style.background = Some(c);
                }
            }
            "border-radius" => {
                if let Some(p) = parse_px(val) {
                    style.radius = p;
                }
            }
            "border" => {
                if lval == "none" {
                    style.border = None;
                } else {
                    let parts: Vec<&str> = val.split_whitespace().collect();
                    if parts.len() >= 3 {
                        if let (Some(w), Some(c)) =
                            (parse_px(parts[0]), parse_color(&parts[2..].join(" ")))
                        {
                            style.border = Some((w, c));
                        }
                    }
                }
            }
            "box-shadow" => {
                if lval == "none" {
                    style.shadow = None;
                } else if let Some(sh) = parse_shadow(val) {
                    style.shadow = Some(sh);
                }
            }
            "color" => {
                if let Some(c) = parse_color(val) {
                    inh.color = c;
                }
            }
            "font-size" => {
                if let Some(p) = parse_px(val) {
                    inh.font_size = p;
                }
            }
            "font-weight" => {
                inh.bold = lval == "bold"
                    || lval == "bolder"
                    || lval.parse::<f32>().map(|n| n >= 600.0).unwrap_or(false);
            }
            "text-decoration" | "text-decoration-line" => {
                inh.underline = lval.contains("underline");
            }
            "text-align" => {
                inh.text_align = match lval.as_str() {
                    "center" => Align::Center,
                    "right" | "end" => Align::End,
                    _ => Align::Start,
                };
            }
            "text-transform" => {
                inh.text_transform = match lval.as_str() {
                    "uppercase" => TextTransform::Uppercase,
                    "lowercase" => TextTransform::Lowercase,
                    "capitalize" => TextTransform::Capitalize,
                    // "none" and everything unrecognised → default (no
                    // transform); a bad value doesn't silently corrupt the
                    // element's text.
                    _ => TextTransform::None,
                };
            }
            "opacity" => {
                if let Some(o) = parse_f32(val) {
                    inh.opacity = o.clamp(0.0, 1.0);
                }
            }
            "align-items" => {
                style.align = match lval.as_str() {
                    "center" => Align::Center,
                    "flex-end" | "end" => Align::End,
                    "flex-start" | "start" => Align::Start,
                    _ => Align::Stretch,
                };
            }
            "justify-content" => {
                style.justify = match lval.as_str() {
                    "center" => Justify::Center,
                    "flex-end" | "end" => Justify::End,
                    "space-between" => Justify::SpaceBetween,
                    _ => Justify::Start,
                };
            }
            _ => {}
        }
    }
    // opacity dims the box's own paint as well as inherited text color
    if inh.opacity < 1.0 {
        if let Some(bg) = &mut style.background {
            bg.a *= inh.opacity;
        }
        if let Some((_, c)) = &mut style.border {
            c.a *= inh.opacity;
        }
        if let Some(sh) = &mut style.shadow {
            sh.color.a *= inh.opacity;
        }
    }
    visible
}

fn parse_f32(s: &str) -> Option<f32> {
    s.trim().parse().ok()
}

fn parse_px(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = s
        .strip_suffix("px")
        .or_else(|| s.strip_suffix("pt"))
        .unwrap_or(s)
        .trim();
    s.parse().ok()
}

fn parse_dim(s: &str) -> Dim {
    let s = s.trim();
    if s == "auto" {
        Dim::Auto
    } else if let Some(p) = s.strip_suffix('%').and_then(|v| v.trim().parse().ok()) {
        Dim::Pct(p)
    } else if let Some(p) = parse_px(s) {
        Dim::Px(p)
    } else {
        Dim::Auto
    }
}

/// CSS 1-4 value shorthand → TRBL edges.
fn parse_edges(s: &str) -> Option<Edges> {
    let v: Vec<f32> = s.split_whitespace().filter_map(parse_px).collect();
    match v.len() {
        1 => Some(Edges::all(v[0])),
        2 => Some(Edges::xy(v[1], v[0])),
        3 => Some(Edges {
            t: v[0],
            r: v[1],
            b: v[2],
            l: v[1],
        }),
        4 => Some(Edges {
            t: v[0],
            r: v[1],
            b: v[2],
            l: v[3],
        }),
        _ => None,
    }
}

fn set_edge(e: &mut Edges, val: &str, side: char) {
    if let Some(p) = parse_px(val) {
        match side {
            'l' => e.l = p,
            't' => e.t = p,
            'r' => e.r = p,
            _ => e.b = p,
        }
    }
}

/// `box-shadow: dx dy [blur [spread]] color` — the color may come first or last.
/// A parenthesized function color (`rgba(...)`, `hsl(...)`) is extracted whole so its
/// internal spaces and commas can't be mistaken for lengths.
fn parse_shadow(s: &str) -> Option<Shadow> {
    let mut rest = s.trim().to_string();
    let mut color_str = String::new();
    // pull out a functional color first, wherever it sits
    if let (Some(open), Some(close)) = (rest.find('('), rest.rfind(')')) {
        if close > open {
            let start = rest[..open]
                .rfind(char::is_whitespace)
                .map(|i| i + 1)
                .unwrap_or(0);
            color_str = rest[start..=close].to_string();
            rest.replace_range(start..=close, " ");
        }
    }
    let mut nums: Vec<f32> = Vec::new();
    for p in rest.split_whitespace() {
        let numeric_start = p
            .chars()
            .next()
            .map(|c| c.is_ascii_digit() || c == '-' || c == '.')
            .unwrap_or(false);
        match parse_px(p) {
            Some(v) if numeric_start => nums.push(v),
            _ => {
                // a bare keyword/hex color token, before or after the lengths
                if !color_str.is_empty() {
                    color_str.push(' ');
                }
                color_str.push_str(p);
            }
        }
    }
    if nums.len() < 2 {
        return None;
    }
    let color = parse_color(color_str.trim()).unwrap_or(Rgba::new(0.0, 0.0, 0.0, 0.35));
    Some(Shadow {
        dx: nums[0],
        dy: nums[1],
        blur: nums.get(2).copied().unwrap_or(0.0).max(0.5),
        color,
    })
}

fn parse_color(s: &str) -> Option<Rgba> {
    // CSS colors are case-insensitive; normalize once so RGB(...) and #ABC work
    let s = s.trim().to_ascii_lowercase();
    let s = s.as_str();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.is_ascii() {
            return None; // byte-indexed below; multibyte input must not slice mid-char
        }
        let dup = |i: usize| u8::from_str_radix(&hex[i..i + 1].repeat(2), 16).ok();
        let two = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
        return match hex.len() {
            3 => Some(Rgba::rgb8(dup(0)?, dup(1)?, dup(2)?)),
            4 => Some(Rgba::rgb8(dup(0)?, dup(1)?, dup(2)?).with_alpha(dup(3)? as f32 / 255.0)),
            6 => Some(Rgba::rgb8(two(0)?, two(2)?, two(4)?)),
            8 => Some(Rgba::rgb8(two(0)?, two(2)?, two(4)?).with_alpha(two(6)? as f32 / 255.0)),
            _ => None,
        };
    }
    let inner = |prefix: &str| -> Option<Vec<String>> {
        s.strip_prefix(prefix)
            .and_then(|x| x.strip_suffix(')'))
            .map(|x| {
                x.split([',', '/'])
                    .flat_map(|p| p.split_whitespace())
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
    };
    if let Some(parts) = inner("rgba(").or_else(|| inner("rgb(")) {
        if parts.len() >= 3 {
            let ch = |p: &str| -> Option<f32> {
                if let Some(pc) = p.strip_suffix('%') {
                    pc.parse::<f32>().ok().map(|v| v / 100.0 * 255.0)
                } else {
                    p.parse::<f32>().ok()
                }
            };
            let r = ch(&parts[0])?;
            let g = ch(&parts[1])?;
            let b = ch(&parts[2])?;
            let a = match parts.get(3) {
                Some(p) if p.ends_with('%') => p.trim_end_matches('%').parse::<f32>().ok()? / 100.0,
                Some(p) => p.parse::<f32>().ok()?,
                None => 1.0,
            };
            return Some(Rgba::new(
                (r / 255.0).clamp(0.0, 1.0),
                (g / 255.0).clamp(0.0, 1.0),
                (b / 255.0).clamp(0.0, 1.0),
                a.clamp(0.0, 1.0),
            ));
        }
    }
    if let Some(parts) = inner("hsla(").or_else(|| inner("hsl(")) {
        if parts.len() >= 3 {
            let h: f32 = parts[0].trim_end_matches("deg").parse().ok()?;
            let sa: f32 = parts[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let l: f32 = parts[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let a = parts
                .get(3)
                .and_then(|p| p.trim_end_matches('%').parse::<f32>().ok())
                .map(|v| {
                    if parts[3].ends_with('%') {
                        v / 100.0
                    } else {
                        v
                    }
                })
                .unwrap_or(1.0);
            return Some(hsl_to_rgba(h, sa, l, a));
        }
    }
    named_color(s)
}

fn hsl_to_rgba(h: f32, s: f32, l: f32, a: f32) -> Rgba {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c * 0.5;
    Rgba::new(r + m, g + m, b + m, a)
}

fn named_color(s: &str) -> Option<Rgba> {
    let (r, g, b, a) = match s {
        "transparent" => (0, 0, 0, 0u8),
        "white" => (255, 255, 255, 255),
        "black" => (0, 0, 0, 255),
        "gray" | "grey" => (128, 128, 128, 255),
        "silver" => (192, 192, 192, 255),
        "lightgray" | "lightgrey" => (211, 211, 211, 255),
        "darkgray" | "darkgrey" => (169, 169, 169, 255),
        "dimgray" | "dimgrey" => (105, 105, 105, 255),
        "slategray" | "slategrey" => (112, 128, 144, 255),
        "whitesmoke" => (245, 245, 245, 255),
        "red" => (220, 70, 70, 255),
        "darkred" => (139, 0, 0, 255),
        "crimson" => (220, 20, 60, 255),
        "salmon" => (250, 128, 114, 255),
        "coral" => (255, 127, 80, 255),
        "orange" => (255, 165, 0, 255),
        "gold" => (255, 215, 0, 255),
        "yellow" => (250, 204, 21, 255),
        "khaki" => (240, 230, 140, 255),
        "green" => (60, 190, 120, 255),
        "darkgreen" => (0, 100, 0, 255),
        "lime" => (132, 204, 22, 255),
        "olive" => (128, 128, 0, 255),
        "teal" => (20, 184, 166, 255),
        "cyan" | "aqua" => (34, 211, 238, 255),
        "turquoise" => (64, 224, 208, 255),
        "blue" => (80, 140, 230, 255),
        "navy" => (0, 0, 128, 255),
        "royalblue" => (65, 105, 225, 255),
        "skyblue" => (135, 206, 235, 255),
        "steelblue" => (70, 130, 180, 255),
        "indigo" => (99, 102, 241, 255),
        "purple" => (168, 85, 247, 255),
        "violet" => (238, 130, 238, 255),
        "magenta" | "fuchsia" => (232, 121, 249, 255),
        "pink" => (244, 114, 182, 255),
        "orchid" => (218, 112, 214, 255),
        "plum" => (221, 160, 221, 255),
        "brown" => (165, 42, 42, 255),
        "maroon" => (128, 0, 0, 255),
        "tan" => (210, 180, 140, 255),
        "beige" => (245, 245, 220, 255),
        "ivory" => (255, 255, 240, 255),
        "rebeccapurple" => (102, 51, 153, 255),
        _ => return None,
    };
    Some(Rgba::new(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        a as f32 / 255.0,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmre_layout::ux::UxNode;

    fn count_rich(node: &UxNode) -> usize {
        match node {
            UxNode::Rich { .. } => 1,
            UxNode::Text { .. } | UxNode::Image { .. } => 0,
            UxNode::Box { children, .. } => children.iter().map(count_rich).sum(),
        }
    }

    #[test]
    fn inline_elements_coalesce_into_one_flow() {
        let doc = "<p>plain <b>bold</b> and <a>linked</a> words</p>";
        let root = parse(doc);
        assert_eq!(count_rich(&root), 1, "one paragraph = one rich flow");
        // find the flow and check the span styles
        fn find(node: &UxNode) -> Option<&Vec<pmre_layout::ux::Span>> {
            match node {
                UxNode::Rich { spans, .. } => Some(spans),
                UxNode::Box { children, .. } => children.iter().find_map(find),
                _ => None,
            }
        }
        let spans = find(&root).expect("rich flow exists");
        assert!(spans.iter().any(|s| s.bold && s.text.contains("bold")));
        assert!(spans
            .iter()
            .any(|s| s.underline && s.text.contains("linked")));
        assert!(spans.iter().any(|s| !s.bold && s.text.contains("plain")));
    }

    #[test]
    fn anchor_href_is_captured_on_its_spans() {
        let doc = r#"<p>see <a href="https://example.com/page">this link</a> now</p>"#;
        let root = parse(doc);
        fn find(node: &UxNode) -> Option<&Vec<pmre_layout::ux::Span>> {
            match node {
                UxNode::Rich { spans, .. } => Some(spans),
                UxNode::Box { children, .. } => children.iter().find_map(find),
                _ => None,
            }
        }
        let spans = find(&root).expect("rich flow exists");
        let link_span = spans
            .iter()
            .find(|s| s.text.contains("this link"))
            .expect("link span exists");
        assert_eq!(link_span.href.as_deref(), Some("https://example.com/page"));
        // Surrounding plain text is not a link.
        assert!(spans
            .iter()
            .any(|s| s.text.contains("see") && s.href.is_none()));
        assert!(spans
            .iter()
            .any(|s| s.text.contains("now") && s.href.is_none()));
    }

    #[test]
    fn nested_inline_element_inside_a_link_inherits_its_href() {
        let doc = r#"<a href="/about"><b>bold link text</b></a>"#;
        let root = parse(doc);
        fn find(node: &UxNode) -> Option<&Vec<pmre_layout::ux::Span>> {
            match node {
                UxNode::Rich { spans, .. } => Some(spans),
                UxNode::Box { children, .. } => children.iter().find_map(find),
                _ => None,
            }
        }
        let spans = find(&root).expect("rich flow exists");
        assert!(spans
            .iter()
            .any(|s| s.bold && s.href.as_deref() == Some("/about")));
    }

    #[test]
    fn anchor_without_href_produces_no_link() {
        let doc = r#"<a name="top">not a link</a>"#;
        let root = parse(doc);
        fn find(node: &UxNode) -> Option<&Vec<pmre_layout::ux::Span>> {
            match node {
                UxNode::Rich { spans, .. } => Some(spans),
                UxNode::Box { children, .. } => children.iter().find_map(find),
                _ => None,
            }
        }
        let spans = find(&root).expect("rich flow exists");
        assert!(spans.iter().all(|s| s.href.is_none()));
    }

    #[test]
    fn comments_scripts_and_entities() {
        let doc = "<!-- c --><div><script>var x = '<div>';</script>a &amp; b &lt;ok&gt;</div>";
        let root = parse(doc);
        fn text_of(node: &UxNode, out: &mut String) {
            match node {
                UxNode::Rich { spans, .. } => {
                    for s in spans {
                        out.push_str(&s.text);
                    }
                }
                UxNode::Text { content, .. } => out.push_str(content),
                UxNode::Box { children, .. } => children.iter().for_each(|c| text_of(c, out)),
                UxNode::Image { .. } => {}
            }
        }
        let mut t = String::new();
        text_of(&root, &mut t);
        assert!(t.contains("a & b <ok>"), "got {t:?}");
        assert!(!t.contains("var x"), "script content must be dropped");
    }

    #[test]
    fn colors_and_shadows_parse() {
        assert!(parse_color("#abc").is_some());
        assert!(parse_color("#aabbccdd").map(|c| c.a < 1.0).unwrap_or(false));
        let c = parse_color("rgba(255, 0, 0, 0.5)").unwrap();
        assert!(c.r > 0.99 && (c.a - 0.5).abs() < 0.01);
        assert!(parse_color("hsl(120, 50%, 50%)")
            .map(|c| c.g > c.r)
            .unwrap_or(false));
        assert!(parse_color("rebeccapurple").is_some());
        let sh = parse_shadow("0 4px 12px rgba(0,0,0,0.4)").unwrap();
        assert!((sh.dy - 4.0).abs() < 0.01 && (sh.blur - 12.0).abs() < 0.01);
    }

    #[test]
    fn margins_percent_and_display_none() {
        let doc = r#"<div>
            <div style="width:50%; margin:10px 20px"></div>
            <div style="display:none">hidden</div>
        </div>"#;
        let root = parse(doc);
        let UxNode::Box { children, .. } = &root else {
            panic!("root is a box")
        };
        assert_eq!(children.len(), 1, "display:none child dropped");
        let UxNode::Box { style, .. } = &children[0] else {
            panic!("child is a box")
        };
        assert!(matches!(style.width, Dim::Pct(p) if (p - 50.0).abs() < 0.01));
        assert!((style.margin.l - 20.0).abs() < 0.01 && (style.margin.t - 10.0).abs() < 0.01);
    }

    #[test]
    fn style_block_class_selector_applies() {
        let doc = r#"<style>.card { width: 200px; }</style><div class="card"></div>"#;
        let root = parse(doc);
        let UxNode::Box { style, .. } = &root else {
            panic!("root is a box")
        };
        assert!(matches!(style.width, Dim::Px(w) if (w - 200.0).abs() < 0.01));
    }

    #[test]
    fn style_block_id_selector_applies() {
        let doc = r#"<style>#hero { width: 300px; }</style><div id="hero"></div>"#;
        let root = parse(doc);
        let UxNode::Box { style, .. } = &root else {
            panic!("root is a box")
        };
        assert!(matches!(style.width, Dim::Px(w) if (w - 300.0).abs() < 0.01));
    }

    #[test]
    fn id_beats_class_beats_type_in_the_cascade() {
        let doc = r#"<style>
            div { width: 100px; }
            .card { width: 200px; }
            #hero { width: 300px; }
        </style><div class="card" id="hero"></div>"#;
        let root = parse(doc);
        let UxNode::Box { style, .. } = &root else {
            panic!("root is a box")
        };
        assert!(
            matches!(style.width, Dim::Px(w) if (w - 300.0).abs() < 0.01),
            "id selector should win over class and type, regardless of source order"
        );
    }

    #[test]
    fn later_rule_of_equal_specificity_wins() {
        let doc = r#"<style>
            .a { width: 100px; }
            .b { width: 200px; }
        </style><div class="a b"></div>"#;
        let root = parse(doc);
        let UxNode::Box { style, .. } = &root else {
            panic!("root is a box")
        };
        assert!(
            matches!(style.width, Dim::Px(w) if (w - 200.0).abs() < 0.01),
            "with equal specificity, the later rule in source order should win"
        );
    }

    #[test]
    fn inline_style_beats_the_stylesheet() {
        let doc =
            r#"<style>#hero { width: 300px; }</style><div id="hero" style="width:50px"></div>"#;
        let root = parse(doc);
        let UxNode::Box { style, .. } = &root else {
            panic!("root is a box")
        };
        assert!(
            matches!(style.width, Dim::Px(w) if (w - 50.0).abs() < 0.01),
            "inline style must always win over any stylesheet rule"
        );
    }

    #[test]
    fn style_block_display_none_hides_the_element() {
        let doc = r#"<style>.hidden { display: none; }</style><div><div class="hidden">a</div><div>b</div></div>"#;
        let root = parse(doc);
        let UxNode::Box { children, .. } = &root else {
            panic!("root is a box")
        };
        assert_eq!(children.len(), 1, "the .hidden div should be dropped");
    }

    #[test]
    fn multiple_style_blocks_combine() {
        let doc = r#"<style>.a { width: 10px; }</style><p>text</p><style>.b { height: 20px; }</style><div class="a b"></div>"#;
        let root = parse(doc);
        let UxNode::Box { children, .. } = &root else {
            panic!("root is a box")
        };
        let target = children
            .iter()
            .find_map(|c| match c {
                UxNode::Box { style, .. } if matches!(style.width, Dim::Px(_)) => Some(style),
                _ => None,
            })
            .expect("the div.a.b should exist with width set");
        assert!(matches!(target.width, Dim::Px(w) if (w - 10.0).abs() < 0.01));
        assert!(matches!(target.height, Dim::Px(h) if (h - 20.0).abs() < 0.01));
    }

    #[test]
    fn style_tag_content_does_not_render_as_text() {
        let doc = r#"<style>.card { color: red; }</style><p>visible text</p>"#;
        let root = parse(doc);
        fn text_of(node: &UxNode, out: &mut String) {
            match node {
                UxNode::Rich { spans, .. } => {
                    for s in spans {
                        out.push_str(&s.text);
                    }
                }
                UxNode::Text { content, .. } => out.push_str(content),
                UxNode::Box { children, .. } => children.iter().for_each(|c| text_of(c, out)),
                UxNode::Image { .. } => {}
            }
        }
        let mut t = String::new();
        text_of(&root, &mut t);
        assert!(t.contains("visible text"));
        assert!(
            !t.contains("color"),
            "CSS text must not leak into rendered content"
        );
    }

    #[test]
    fn descendant_combinator_applies_to_matching_descendants() {
        // `div p` now supported: styles the <p> inside a <div>, but a <p>
        // outside the <div> gets nothing.
        let doc =
            r#"<style>div p { width: 999px; } p { height: 5px; }</style><div><p></p></div><p></p>"#;
        let root = parse(doc);
        // Collect all <p> boxes' styles (identified by having height: 5px
        // — the flat-p rule — and see how many have width: 999px too).
        fn collect_ps<'a>(node: &'a UxNode, out: &mut Vec<&'a Style>) {
            if let UxNode::Box { style, children } = node {
                if matches!(style.height, Dim::Px(h) if (h - 5.0).abs() < 0.01) {
                    out.push(style);
                }
                for c in children {
                    collect_ps(c, out);
                }
            }
        }
        let mut ps = Vec::new();
        collect_ps(&root, &mut ps);
        assert_eq!(ps.len(), 2, "expected two <p> elements in the tree");
        let with_width = ps
            .iter()
            .filter(|s| matches!(s.width, Dim::Px(w) if (w - 999.0).abs() < 0.01))
            .count();
        assert_eq!(
            with_width, 1,
            "exactly one <p> (the one inside the <div>) should have the descendant rule applied"
        );
    }

    #[test]
    fn child_combinator_still_dropped() {
        // `>` isn't supported — the rule matches nothing.
        let doc =
            r#"<style>div > p { width: 999px; } p { height: 5px; }</style><div><p></p></div>"#;
        let root = parse(doc);
        fn find_p_width(node: &UxNode) -> Option<f32> {
            if let UxNode::Box { style, children } = node {
                if let Dim::Px(w) = style.width {
                    if (w - 999.0).abs() < 0.01 {
                        return Some(w);
                    }
                }
                for c in children {
                    if let Some(w) = find_p_width(c) {
                        return Some(w);
                    }
                }
            }
            None
        }
        assert!(
            find_p_width(&root).is_none(),
            "child combinator `>` must not silently degrade to descendant"
        );
    }

    #[test]
    fn img_with_matched_src_emits_a_ux_image_node() {
        let img = Arc::new(Image {
            width: 2,
            height: 2,
            pixels: vec![
                Rgba::rgb8(255, 0, 0),
                Rgba::rgb8(0, 255, 0),
                Rgba::rgb8(0, 0, 255),
                Rgba::rgb8(255, 255, 255),
            ],
        });
        let mut map = HashMap::new();
        map.insert("logo.bmp".to_string(), img);
        let doc = r#"<div><img src="logo.bmp" width="32" height="32"></div>"#;
        let root = parse_with_images(doc, &map);
        // Expect: root Box → children include exactly one UxNode::Image with
        // Dim::Px(32) width/height.
        fn find_image(node: &UxNode) -> Option<&Style> {
            match node {
                UxNode::Image { style, .. } => Some(style),
                UxNode::Box { children, .. } => children.iter().find_map(find_image),
                _ => None,
            }
        }
        let s = find_image(&root).expect("expected a UxNode::Image in the tree");
        assert!(matches!(s.width, Dim::Px(v) if (v - 32.0).abs() < 0.01));
        assert!(matches!(s.height, Dim::Px(v) if (v - 32.0).abs() < 0.01));
    }

    #[test]
    fn img_with_no_matched_src_is_dropped() {
        // Empty images map: parse must not panic and must not emit any Image node.
        let doc = r#"<div><img src="missing.bmp" width="32" height="32"></div>"#;
        let root = parse_with_images(doc, &HashMap::new());
        fn has_image(node: &UxNode) -> bool {
            match node {
                UxNode::Image { .. } => true,
                UxNode::Box { children, .. } => children.iter().any(has_image),
                _ => false,
            }
        }
        assert!(
            !has_image(&root),
            "img without a matching src in the images map must be dropped"
        );
    }

    fn collect_text(node: &UxNode, out: &mut String) {
        match node {
            UxNode::Rich { spans, .. } => {
                for s in spans {
                    out.push_str(&s.text);
                }
                out.push('\n');
            }
            UxNode::Text { content, .. } => {
                out.push_str(content);
                out.push('\n');
            }
            UxNode::Box { children, .. } => {
                for c in children {
                    collect_text(c, out);
                }
            }
            UxNode::Image { .. } => {}
        }
    }

    #[test]
    fn unordered_list_prefixes_each_li_with_a_bullet() {
        let root = parse("<ul><li>alpha</li><li>beta</li></ul>");
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(
            t.contains("• alpha"),
            "expected first li to start with '• ' (got {t:?})"
        );
        assert!(
            t.contains("• beta"),
            "expected second li to start with '• ' (got {t:?})"
        );
    }

    #[test]
    fn ordered_list_prefixes_each_li_with_a_source_order_number() {
        let root = parse("<ol><li>alpha</li><li>beta</li><li>gamma</li></ol>");
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(
            t.contains("1. alpha"),
            "expected first ol item to start with '1. ' (got {t:?})"
        );
        assert!(
            t.contains("2. beta"),
            "expected second ol item to start with '2. ' (got {t:?})"
        );
        assert!(
            t.contains("3. gamma"),
            "expected third ol item to start with '3. ' (got {t:?})"
        );
        // No bullets should appear for a numbered list.
        assert!(
            !t.contains("• alpha") && !t.contains("• beta") && !t.contains("• gamma"),
            "ordered list must not use bullets (got {t:?})"
        );
    }

    #[test]
    fn nested_ol_inside_ul_uses_its_own_numbering() {
        let root = parse(
            "<ul><li>outer<ol><li>inner-one</li><li>inner-two</li></ol></li><li>bottom</li></ul>",
        );
        let mut t = String::new();
        collect_text(&root, &mut t);
        // Outer <ul> children stay bulleted.
        assert!(t.contains("• outer"));
        assert!(t.contains("• bottom"));
        // Nested <ol> children numbered from 1, independent of outer state.
        assert!(t.contains("1. inner-one"), "got {t:?}");
        assert!(t.contains("2. inner-two"), "got {t:?}");
    }

    #[test]
    fn text_transform_uppercase_applies_via_css() {
        let doc = r#"<style>.shout { text-transform: uppercase; }</style><p class="shout">hello world</p>"#;
        let root = parse(doc);
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(
            t.contains("HELLO WORLD"),
            "expected uppercase transform to have applied (got {t:?})"
        );
    }

    #[test]
    fn text_transform_lowercase_applies_via_css() {
        let doc = r#"<style>.quiet { text-transform: lowercase; }</style><p class="quiet">SHOUTING TEXT</p>"#;
        let root = parse(doc);
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(
            t.contains("shouting text"),
            "expected lowercase transform to have applied (got {t:?})"
        );
    }

    #[test]
    fn text_transform_capitalize_upcases_each_word_start() {
        let doc = r#"<style>.title { text-transform: capitalize; }</style><p class="title">hello world of css</p>"#;
        let root = parse(doc);
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(
            t.contains("Hello World Of Css"),
            "expected each word's first char capitalised (got {t:?})"
        );
    }

    #[test]
    fn text_transform_none_leaves_text_authored() {
        let doc =
            r#"<style>.plain { text-transform: none; }</style><p class="plain">Mixed Case</p>"#;
        let root = parse(doc);
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(
            t.contains("Mixed Case"),
            "expected default text unchanged (got {t:?})"
        );
    }

    #[test]
    fn text_transform_unrecognized_value_falls_back_to_none() {
        // A future CSS value we don't yet support (e.g. `full-width`) must not
        // silently corrupt or panic — it falls back to `none`, matching the
        // fail-closed policy elsewhere in this engine.
        let doc = r#"<style>.p { text-transform: full-width; }</style><p class="p">unchanged</p>"#;
        let root = parse(doc);
        let mut t = String::new();
        collect_text(&root, &mut t);
        assert!(t.contains("unchanged"), "got {t:?}");
    }

    #[test]
    fn blockquote_gets_default_indent_padding_and_left_border() {
        let root = parse("<blockquote>a quoted line</blockquote>");
        fn find_bq(node: &UxNode) -> Option<&Style> {
            match node {
                UxNode::Box { style, children } => {
                    // Blockquote has all three: non-zero margin, non-zero
                    // padding, and a border. That triple is unique in the kit's
                    // tag_default_style so a positive match is unambiguous.
                    if style.border.is_some() && style.margin.l > 0.0 && style.padding.l > 0.0 {
                        return Some(style);
                    }
                    children.iter().find_map(find_bq)
                }
                _ => None,
            }
        }
        let s = find_bq(&root).expect("expected a blockquote in the tree");
        // Confirm the specific default numbers we ship (change the test if
        // the defaults ever change intentionally).
        assert!(s.margin.l >= 24.0);
        assert!(s.padding.l >= 12.0);
        let (bw, _) = s.border.expect("border");
        assert!(bw >= 3.0);
    }

    #[test]
    fn hr_default_height_is_one_pixel_and_has_background() {
        let root = parse("<hr>");
        fn find_hr(node: &UxNode) -> Option<&Style> {
            match node {
                UxNode::Box { style, children } => {
                    if matches!(style.height, Dim::Px(h) if (h - 1.0).abs() < 0.01)
                        && style.background.is_some()
                    {
                        return Some(style);
                    }
                    children.iter().find_map(find_hr)
                }
                _ => None,
            }
        }
        assert!(
            find_hr(&root).is_some(),
            "hr should render with height:1px and a background"
        );
    }

    #[test]
    fn hr_respects_class_selector_from_style_block() {
        // Before the refactor, hr short-circuited past apply_cascade and CSS
        // rules for hr never applied. Now the class rule wins over the
        // default height.
        let doc = r#"<style>hr.thick { height: 5px; }</style><hr class="thick">"#;
        let root = parse(doc);
        fn find_hr_height(node: &UxNode) -> Option<f32> {
            match node {
                UxNode::Box { style, children } => {
                    if style.background.is_some() {
                        if let Dim::Px(h) = style.height {
                            return Some(h);
                        }
                    }
                    children.iter().find_map(find_hr_height)
                }
                _ => None,
            }
        }
        let h = find_hr_height(&root).expect("expected the hr in the tree");
        assert!(
            (h - 5.0).abs() < 0.01,
            "expected hr.thick to have CSS-overridden height=5, got {h}"
        );
    }

    #[test]
    fn parse_without_images_still_drops_img_tags_silently() {
        // Backwards compatibility: the old `parse(src)` API must render an
        // <img> as nothing, matching existing test expectations.
        let doc = r#"<div><img src="anything.png"></div>"#;
        let root = parse(doc);
        fn has_image(node: &UxNode) -> bool {
            match node {
                UxNode::Image { .. } => true,
                UxNode::Box { children, .. } => children.iter().any(has_image),
                _ => false,
            }
        }
        assert!(!has_image(&root));
    }

    #[test]
    fn input_control_renders_as_interactive_box_not_dropped() {
        let root = parse(r#"<form><input type="text"></form>"#);
        // The <input> must reach the render tree as an interactive Box
        // (not be dropped as it was before form-control support).
        fn has_input(node: &UxNode) -> bool {
            if let UxNode::Box { style, children } = node {
                style.role == pmre_layout::ux::Role::Input || children.iter().any(has_input)
            } else {
                false
            }
        }
        assert!(has_input(&root), "expected an <input> to render");
    }

    #[test]
    fn button_control_renders_with_label_text() {
        let root = parse(r#"<button>Click me</button>"#);
        fn has_button_with_text(node: &UxNode) -> bool {
            if let UxNode::Box { style, children } = node {
                (style.role == pmre_layout::ux::Role::Button
                    && children
                        .iter()
                        .any(|c| matches!(c, UxNode::Rich { .. } | UxNode::Text { .. })))
                    || children.iter().any(has_button_with_text)
            } else {
                false
            }
        }
        assert!(
            has_button_with_text(&root),
            "expected a <button> with its label text"
        );
    }

    #[test]
    fn multiple_inputs_get_distinct_widget_ids() {
        // Two <input> controls in the same parse must each get a unique
        // numeric widget id so the orchestrator's UiState tracks them
        // independently (no id collision with the chrome's 1-99 range).
        let root = parse(r#"<div><input><input></div>"#);
        let mut ids = Vec::new();
        fn collect_input_ids(node: &UxNode, ids: &mut Vec<u32>) {
            if let UxNode::Box { style, children } = node {
                if style.role == pmre_layout::ux::Role::Input {
                    if let Some(id) = style.id {
                        ids.push(id);
                    }
                }
                for c in children {
                    collect_input_ids(c, ids);
                }
            }
        }
        collect_input_ids(&root, &mut ids);
        assert_eq!(ids.len(), 2, "expected two inputs");
        assert_ne!(ids[0], ids[1], "inputs must have distinct ids");
        assert!(
            ids.iter().all(|&i| i >= 1000),
            "ids must be >= 1000 (FORM_BASE)"
        );
    }
}
