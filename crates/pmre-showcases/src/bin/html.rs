//! HTML/CSS demo: an HTML document with inline CSS, reduced to the same math primitives
//! and rendered with no browser and no Vello — real inline text flow (bold / links wrap
//! with plain words), box shadows, lists, margins, and anti-aliased system-font glyphs.
//!
//! Run: cargo run -p pmre-showcases --bin html

use pmre_kit::Rgba;
use pmre_orchestrator::render_html;

fn main() {
    let doc = r#"
<div style="display:flex; flex-direction:column; width:900px; height:640px; background:#15171d">
  <div style="display:flex; flex-direction:row; height:54px; background:#1e2027; align-items:center; padding:0 18px; gap:24px">
    <span style="color:#eef2f8; font-size:18px; font-weight:bold">Browser</span>
    <span style="color:#9aa2b4; font-size:14px">Home</span>
    <span style="color:#9aa2b4; font-size:14px">Docs</span>
    <span style="color:#9aa2b4; font-size:14px">About</span>
  </div>
  <div style="display:flex; flex-direction:row; gap:16px; padding:18px">
    <div style="display:flex; flex-direction:column; flex:1; background:#23262f; border-radius:12px; border:1px solid #363a46; padding:16px; gap:10px; box-shadow: 0 6px 18px rgba(0,0,0,0.45)">
      <div style="width:44px; height:44px; border-radius:10px; background:#5c9ef6"></div>
      <h3 style="color:#eef2f8">Primitives</h3>
      <span style="color:#9aa2b4; font-size:13px">HTML and CSS reduced to math &mdash; eight root atoms</span>
    </div>
    <div style="display:flex; flex-direction:column; flex:1; background:#23262f; border-radius:12px; border:1px solid #363a46; padding:16px; gap:10px; box-shadow: 0 6px 18px rgba(0,0,0,0.45)">
      <div style="width:44px; height:44px; border-radius:10px; background:#34d399"></div>
      <h3 style="color:#eef2f8">Layout</h3>
      <span style="color:#9aa2b4; font-size:13px">flex box-model solver with margins &amp; percentages</span>
    </div>
    <div style="display:flex; flex-direction:column; flex:1; background:#23262f; border-radius:12px; border:1px solid #363a46; padding:16px; gap:10px; box-shadow: 0 6px 18px rgba(0,0,0,0.45)">
      <div style="width:44px; height:44px; border-radius:10px; background:#fbbf60"></div>
      <h3 style="color:#eef2f8">Raster</h3>
      <span style="color:#9aa2b4; font-size:13px">SDF coverage + TrueType glyph accumulation</span>
    </div>
  </div>
  <div style="display:flex; flex-direction:column; flex:1; padding:0 18px 18px 18px">
    <div style="display:flex; flex-direction:column; flex:1; background:#20232b; border-radius:12px; border:1px solid #343845; padding:6px 20px 16px 20px; box-shadow: 0 6px 18px rgba(0,0,0,0.45)">
      <h2 style="color:#eef2f8">Inline text flow</h2>
      <p style="color:#b6bdcc; font-size:14px">
        This paragraph mixes <b>bold words</b>, a <a>real underlined link</a>, and
        <span style="color:#34d399">colored spans</span> in one wrapping flow &mdash; every word
        placed by the same greedy line breaker, every glyph rasterized from the system
        font's quadratic B&eacute;zier outlines by an accumulation-buffer coverage pass.
        No browser engine, no font crate, no GPU. Just math over a byte slice.
      </p>
      <hr>
      <ul style="color:#9aa2b4; font-size:13px">
        <li>anti-aliased vector glyphs with real baselines and kerning-free metrics</li>
        <li><b>margins</b>, <b>percent widths</b>, <b>box-shadows</b>, <b>opacity</b>, entities, comments</li>
        <li>rgb / rgba / hsl / hex / named colors &middot; text-align &middot; font-weight</li>
      </ul>
    </div>
  </div>
</div>
"#;

    let clear = Rgba::rgb8(21, 23, 29);
    let (w, h) = (900u32, 640u32);
    let fb = render_html(doc, w, h, clear);
    let bmp = fb.to_bmp(clear);
    let path = r"html.bmp";
    std::fs::write(path, bmp).expect("write html.bmp");
    println!("wrote {path} ({w}x{h})");
}
