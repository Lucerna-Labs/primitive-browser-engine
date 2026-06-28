//! # pbe-stages
//!
//! Render stages, each a Spiderweb [`Strand`] that wraps a **dumb** primitive
//! function from the `cap-*` kit. These adapters are the only new code in the
//! render path — they translate bus messages into primitive calls and back.
//! They hold **no policy**: no retries, no placement, no decisions. The kit
//! keeps all mechanism; the orchestrator keeps all policy; these just bridge.
//!
//! ## Stages
//!
//! - [`BuildStyledStage`] — `parse_html` + `Stylesheet::parse_author` +
//!   `StyledDom::new`. These fuse because `StyledDom::new` *consumes* the
//!   `DomTree` by value (a real ownership wall in the current kit). Splitting
//!   parse and cascade into separate strands is a future additive change to
//!   `cap-style-cascade`, tracked in the project ROADMAP.
//! - [`PaintStage`] — `cap_paint::paint`, producing the primitive list.
//!
//! Neither stage names the other. `BuildStyledStage` listens for
//! [`RenderRequest`] and emits [`StyledReady`]; `PaintStage` listens for
//! [`StyledReady`] and emits [`PaintReady`]. The bus wires them by type.

use std::sync::Arc;

use spiderweb::{BusHandle, Socket, Strand, StrandError};

use pbe_protocol::{
    FetchRequest, FrameReady, PaintReady, RenderRequest, StyledReady, SOCK_FETCH_REQUEST,
    SOCK_FRAME_READY, SOCK_PAINT_READY, SOCK_RENDER_REQUEST, SOCK_STYLED_READY,
};

/// Minimal user-agent stylesheet. The kit's cascade uses the CSS *initial*
/// value `display:inline` for any element with no matching rule (spec-correct),
/// but real browsers ship a UA sheet that makes structural elements block-level
/// and gives headings/margins their familiar look. Without this, every element
/// lays out inline and a page collapses to one row. We supply it by composition
/// (prepended to author CSS) rather than modifying the kit. Lowest precedence,
/// so author CSS always wins.
///
/// NOTE: the kit's MVP selector parser does NOT support comma selector lists
/// (it treats `,` as a descendant combinator), so every rule here uses a single
/// type selector. One rule per element — verbose but correct against the kit.
const USER_AGENT_CSS: &str = "\
html { display: block; } body { display: block; margin: 8px; } \
div { display: block; } p { display: block; margin: 16px 0; } \
section { display: block; } article { display: block; } \
header { display: block; } footer { display: block; } \
main { display: block; } nav { display: block; } aside { display: block; } \
figure { display: block; } figcaption { display: block; } \
blockquote { display: block; margin: 16px 40px; } \
ul { display: block; margin: 16px 0; padding: 0 0 0 40px; } \
ol { display: block; margin: 16px 0; padding: 0 0 0 40px; } \
li { display: block; } dl { display: block; } dt { display: block; } \
dd { display: block; } table { display: block; } form { display: block; } \
fieldset { display: block; } pre { display: block; } \
address { display: block; } hr { display: block; } \
h1 { display: block; font-size: 32px; margin: 21px 0; } \
h2 { display: block; font-size: 24px; margin: 20px 0; } \
h3 { display: block; font-size: 19px; margin: 18px 0; } \
h4 { display: block; font-size: 16px; margin: 21px 0; } \
h5 { display: block; font-size: 13px; margin: 22px 0; } \
h6 { display: block; font-size: 11px; margin: 24px 0; } \
a { color: #0000ee; } strong { font-weight: 700; } b { font-weight: 700; } \
";

/// Default frame size for the render off-ramp (CSS px == device px for now).
const FRAME_W: u32 = 800;
const FRAME_H: u32 = 600;
/// Opaque white page background.
const PAGE_BG: cap_primitives::Rgba = cap_primitives::Rgba {
    r: 1.0,
    g: 1.0,
    b: 1.0,
    a: 1.0,
};

/// Stage 0 (network on-ramp): URL → fetched page → `RenderRequest`. Wraps the
/// sealed-`curl` fetch primitive in `pbe-net`. This is what lets the engine
/// render the live web; for local files the orchestrator skips it and publishes
/// a `RenderRequest` directly. No mechanism beyond driving the dumb fetch.
pub struct FetchStage;

impl Strand for FetchStage {
    fn name(&self) -> &str {
        "fetch"
    }

    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<FetchRequest>(SOCK_FETCH_REQUEST);
        std::slice::from_ref(&S)
    }

    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<RenderRequest>(SOCK_RENDER_REQUEST);
        std::slice::from_ref(&S)
    }

    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for req in bus.recv::<FetchRequest>(SOCK_FETCH_REQUEST)? {
            match pbe_net::fetch(&req.url) {
                Ok(page) => {
                    bus.log(&format!(
                        "fetched {} → HTTP {} ({}, {} bytes html)",
                        page.final_url,
                        page.status,
                        page.content_type.as_deref().unwrap_or("no content-type"),
                        page.body.len()
                    ));
                    bus.publish_static(
                        SOCK_RENDER_REQUEST,
                        RenderRequest {
                            label: page.final_url,
                            html: page.body,
                            css: req.css,
                        },
                    )?;
                }
                Err(e) => {
                    // Mechanism failure is reported; the orchestrator/spider owns
                    // any retry policy. We do not crash the stage on a bad URL.
                    bus.log(&format!("fetch failed for {}: {e:?}", req.url));
                }
            }
        }
        bus.sleep(std::time::Duration::from_millis(10));
        Ok(())
    }
}

/// Stage 1: source → styled DOM. Wraps parse + cascade.
pub struct BuildStyledStage;

impl Strand for BuildStyledStage {
    fn name(&self) -> &str {
        "build-styled"
    }

    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<RenderRequest>(SOCK_RENDER_REQUEST);
        std::slice::from_ref(&S)
    }

    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<StyledReady>(SOCK_STYLED_READY);
        std::slice::from_ref(&S)
    }

    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for req in bus.recv::<RenderRequest>(SOCK_RENDER_REQUEST)? {
            // --- drive the dumb primitives, in order ---
            let dom = cap_html_parse::parse_html(&req.html);

            // A real document carries its CSS inside <style> blocks. The kit's
            // MVP parser keeps <style> text as a DOM text node but does not
            // surface it as a stylesheet, so we extract it here (pure string
            // composition over the fetched HTML) and combine it with any author
            // CSS the caller supplied out-of-band. This is what lets fetched
            // pages style themselves, not just locally-supplied CSS.
            let embedded = extract_style_blocks(&req.html);
            // UA sheet first (lowest precedence), then embedded <style>, then
            // any caller-supplied author CSS (highest). The cascade resolves
            // precedence by source order within author origin.
            let mut combined_css = String::from(USER_AGENT_CSS);
            if !embedded.is_empty() {
                combined_css.push('\n');
                combined_css.push_str(&embedded);
            }
            if !req.css.trim().is_empty() {
                combined_css.push('\n');
                combined_css.push_str(&req.css);
            }

            let sheet = cap_css_parse::Stylesheet::parse_author(&combined_css);
            let styled = cap_style_cascade::StyledDom::new(dom, &[sheet]);

            bus.log(&format!(
                "{}: styled {} element(s) ({} bytes css)",
                req.label,
                styled.styled_elements().count(),
                combined_css.len()
            ));

            bus.publish_static(
                SOCK_STYLED_READY,
                StyledReady {
                    label: req.label,
                    styled: Arc::new(styled),
                },
            )?;
        }
        bus.sleep(std::time::Duration::from_millis(10));
        Ok(())
    }
}

/// Stage 2: styled DOM → primitive list. Wraps `cap_paint::paint`.
pub struct PaintStage;

impl Strand for PaintStage {
    fn name(&self) -> &str {
        "paint"
    }

    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<StyledReady>(SOCK_STYLED_READY);
        std::slice::from_ref(&S)
    }

    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<PaintReady>(SOCK_PAINT_READY);
        std::slice::from_ref(&S)
    }

    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for ready in bus.recv::<StyledReady>(SOCK_STYLED_READY)? {
            // Real layout (cap-layout/taffy) then layout-aware paint, instead of
            // the kit's origin-anchored cap-paint. Boxes land at true positions;
            // text runs are emitted for the render stage to shape + rasterize.
            let layout = pbe_layout::layout(&ready.styled, FRAME_W as f32, FRAME_H as f32);
            let painted = pbe_render::paint_with_layout(&ready.styled, &layout);
            bus.log(&format!(
                "{}: laid out {} box(es), painted {} primitive(s), {} text run(s)",
                ready.label,
                layout.len(),
                painted.primitives.len(),
                painted.texts.len()
            ));
            bus.publish_static(
                SOCK_PAINT_READY,
                PaintReady {
                    label: ready.label,
                    primitives: Arc::new(painted.primitives),
                    texts: Arc::new(painted.texts),
                },
            )?;
        }
        bus.sleep(std::time::Duration::from_millis(10));
        Ok(())
    }
}

/// Stage 3: primitive list → finished frame. Wraps `pbe_render` (display list +
/// software raster). This is the render off-ramp — the sealed-rasterizer-from-
/// outside step, swappable for a GPU backend later.
pub struct RenderStage;

impl Strand for RenderStage {
    fn name(&self) -> &str {
        "render"
    }

    fn inputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<PaintReady>(SOCK_PAINT_READY);
        std::slice::from_ref(&S)
    }

    fn outputs(&self) -> &[Socket] {
        const S: Socket = Socket::new::<FrameReady>(SOCK_FRAME_READY);
        std::slice::from_ref(&S)
    }

    fn run(&mut self, bus: &mut BusHandle) -> Result<(), StrandError> {
        for done in bus.recv::<PaintReady>(SOCK_PAINT_READY)? {
            let display_list = pbe_render::display_list(&done.primitives);
            // Full page: box layer + shaped/rasterized text on top.
            let raster = pbe_render::rasterize_page(
                &done.primitives,
                &done.texts,
                FRAME_W,
                FRAME_H,
                PAGE_BG,
            );
            let ppm = raster.to_ppm();
            bus.log(&format!(
                "{}: rendered {}x{} frame, {} text run(s) ({} bytes PPM)",
                done.label,
                FRAME_W,
                FRAME_H,
                done.texts.len(),
                ppm.len()
            ));
            bus.publish_static(
                SOCK_FRAME_READY,
                FrameReady {
                    label: done.label,
                    primitive_count: done.primitives.len(),
                    display_list,
                    width: FRAME_W,
                    height: FRAME_H,
                    ppm: Arc::new(ppm),
                },
            )?;
        }
        bus.sleep(std::time::Duration::from_millis(10));
        Ok(())
    }
}

/// Extract and concatenate the contents of every `<style>...</style>` block in
/// an HTML document. Pure string composition — case-insensitive on the tag,
/// tolerant of attributes on the opening tag (`<style type="text/css">`). Drives
/// nothing in the kit; it just recovers the CSS a page ships inline so the
/// cascade can see it.
pub fn extract_style_blocks(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut search_from = 0usize;

    while let Some(rel_open) = lower[search_from..].find("<style") {
        let open = search_from + rel_open;
        // Find the end of the opening tag '>' (skips any attributes).
        let Some(rel_gt) = lower[open..].find('>') else {
            break;
        };
        let content_start = open + rel_gt + 1;
        // Find the matching closing tag.
        let Some(rel_close) = lower[content_start..].find("</style>") else {
            break;
        };
        let content_end = content_start + rel_close;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(html[content_start..content_end].trim());
        search_from = content_end + "</style>".len();
    }
    out
}

/// Combine the UA stylesheet, the page's embedded `<style>` CSS, and any
/// caller-supplied author CSS, in cascade-precedence order (UA lowest).
pub fn build_combined_css(html: &str, author_css: &str) -> String {
    let embedded = extract_style_blocks(html);
    let mut css = String::from(USER_AGENT_CSS);
    if !embedded.is_empty() {
        css.push('\n');
        css.push_str(&embedded);
    }
    if !author_css.trim().is_empty() {
        css.push('\n');
        css.push_str(author_css);
    }
    css
}

/// Synchronous full render: HTML + author CSS → an RGBA8 framebuffer of size
/// `w`×`h` with vertical scroll `scroll_y` (pixels). This is the same pipeline
/// the bus stages run (parse → cascade → layout → paint → rasterize), exposed
/// as one call for the windowed shell, which renders on demand rather than via
/// the one-shot bus. Returns `(rgba_bytes, content_height)`.
pub fn render_to_rgba(
    html: &str,
    author_css: &str,
    w: u32,
    h: u32,
    scroll_y: f32,
) -> (Vec<u8>, f32) {
    let dom = cap_html_parse::parse_html(html);
    let css = build_combined_css(html, author_css);
    let sheet = cap_css_parse::Stylesheet::parse_author(&css);
    let styled = cap_style_cascade::StyledDom::new(dom, &[sheet]);

    let layout = pbe_layout::layout(&styled, w as f32, h as f32);
    let content_height = layout.content_height();
    let painted = pbe_render::paint_with_layout(&styled, &layout);

    // Apply scroll by shifting paint/text up by scroll_y.
    let primitives = pbe_render::translate_primitives(&painted.primitives, 0.0, -scroll_y);
    let texts: Vec<pbe_protocol::TextDraw> = painted
        .texts
        .iter()
        .map(|t| {
            let mut t = t.clone();
            t.top_y -= scroll_y;
            t
        })
        .collect();

    let raster = pbe_render::rasterize_page(&primitives, &texts, w, h, PAGE_BG);
    (raster.into_rgba(), content_height)
}

/// Register the render payload types for bus fan-out. Must be called once at
/// boot before the bus runs — the kernel needs a clone fn per custom type.
pub fn register_render_types() {
    spiderweb::register_clone_type::<FetchRequest>();
    spiderweb::register_clone_type::<RenderRequest>();
    spiderweb::register_clone_type::<StyledReady>();
    spiderweb::register_clone_type::<PaintReady>();
    spiderweb::register_clone_type::<FrameReady>();
}

#[cfg(test)]
mod tests {
    use super::extract_style_blocks;

    #[test]
    fn extracts_single_style_block() {
        let html = "<html><head><style>div{color:red}</style></head><body></body></html>";
        assert_eq!(extract_style_blocks(html), "div{color:red}");
    }

    #[test]
    fn extracts_multiple_blocks_concatenated() {
        let html = "<style>a{}</style>x<style>b{}</style>";
        assert_eq!(extract_style_blocks(html), "a{}\nb{}");
    }

    #[test]
    fn tolerates_attributes_and_case() {
        let html = r#"<STYLE type="text/css">body{margin:0}</STYLE>"#;
        assert_eq!(extract_style_blocks(html), "body{margin:0}");
    }

    #[test]
    fn empty_when_no_style() {
        assert_eq!(extract_style_blocks("<p>hi</p>"), "");
    }

    #[test]
    fn ignores_unterminated_style() {
        // No closing tag: nothing extracted, no panic.
        assert_eq!(extract_style_blocks("<style>div{}"), "");
    }
}
