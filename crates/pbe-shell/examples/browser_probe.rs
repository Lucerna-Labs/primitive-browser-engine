//! Self-verifying headless demo of the browser chrome: opens a page, clicks
//! the address bar, types a second page's path, presses Enter to navigate,
//! clicks Back, clicks Forward, then clicks a rendered `<a href>` link on the
//! page to navigate to a third page — asserting the label changes correctly
//! at each step — before rendering the final frame to a BMP. Mirrors
//! `pmre-orchestrator`'s own `todo` example: typing/clicking driven entirely
//! through the engine's real event handling (`dispatch`/`UiEvent`), not by
//! touching browser state directly.
//!
//! Run: cargo run -p pbe-shell --example browser_probe

use pbe_shell::{Browser, Quality, UiEvent};
use std::path::PathBuf;

const PAGE_TWO: &str = r#"<div style="background:#27ae60"><p style="color:#ffffff; font-size:22px">Page Two</p></div>"#;
const PAGE_THREE: &str = r#"<div style="background:#8e44ad"><p style="color:#ffffff; font-size:22px">Page Three</p></div>"#;

/// A stylesheet fetched from disk and injected by the browser before parse.
/// Uses a type selector the kit's <style>-block reader already supports —
/// the composition path is what we're verifying, not any new kit capability.
const EXTERNAL_CSS: &str = "h1 { color: #d35400; } p { color: #16a085; }";

/// A page whose only style comes from `<link rel="stylesheet" href="…">` —
/// with an unstyled body, so if the fetch-and-inject composition works, the
/// headline and paragraph will pick up the stylesheet's colours; if it
/// doesn't, they'll fall back to the default black.
const LINKED_PAGE_TEMPLATE: &str = r#"<html><head><link rel="stylesheet" href="{CSS}"></head><body><h1>Linked page</h1><p>Styled from an external sheet.</p></body></html>"#;

fn main() {
    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("out");
    std::fs::create_dir_all(&out_dir).expect("create out dir");
    let page_two_path = out_dir.join("browser-probe-page-two.html");
    let page_three_path = out_dir.join("browser-probe-page-three.html");
    std::fs::write(&page_two_path, PAGE_TWO).expect("write page two");
    std::fs::write(&page_three_path, PAGE_THREE).expect("write page three");

    // Page one links to page three, so clicking a rendered <a href> can be
    // exercised too, not just typed navigation.
    let page_one = format!(
        r#"<div style="background:#c0392b; padding:20px"><p style="color:#ffffff; font-size:22px">Page One</p><p style="color:#ffffff; font-size:20px">Go to <a href="{}">page three</a> now</p></div>"#,
        page_three_path.to_str().unwrap()
    );
    let page_one_path = out_dir.join("browser-probe-page-one.html");
    std::fs::write(&page_one_path, &page_one).expect("write page one");

    let mut browser = Browser::open(page_one_path.to_str().unwrap(), 900, 650);
    assert_eq!(browser.label(), page_one_path.to_str().unwrap());
    assert!(!browser.can_go_back());
    assert!(!browser.can_go_forward());

    // Focus the address bar, type the second page's path, press Enter.
    browser.focus_address_bar();
    browser.type_address(page_two_path.to_str().unwrap());
    browser.dispatch(UiEvent::Enter);

    assert_eq!(
        browser.label(),
        page_two_path.to_str().unwrap(),
        "Enter in the address bar should navigate to the typed path"
    );
    assert!(
        browser.can_go_back(),
        "navigating should make Back available"
    );
    assert!(!browser.can_go_forward());

    // Click Back — should return to page one.
    browser.click_back();
    assert_eq!(
        browser.label(),
        page_one_path.to_str().unwrap(),
        "Back should return to the first page"
    );
    assert!(!browser.can_go_back());
    assert!(
        browser.can_go_forward(),
        "after Back, Forward should be available"
    );

    // Click Forward — should go to page two again.
    browser.click_forward();
    assert_eq!(browser.label(), page_two_path.to_str().unwrap());

    // Back to page one, then click the rendered <a href> link — should
    // navigate to page three, through the exact same event path a real
    // mouse click would take (hit-test the wrapped link text, resolve its
    // href, navigate).
    browser.click_back();
    assert_eq!(browser.label(), page_one_path.to_str().unwrap());
    let clicked = browser.click_first_link();
    assert!(clicked, "expected to find a rendered link on page one");
    assert_eq!(
        browser.label(),
        page_three_path.to_str().unwrap(),
        "clicking the <a href> link should navigate to its target"
    );

    let fb = browser.render();
    let bmp_path = out_dir.join("browser-probe.bmp");
    std::fs::write(&bmp_path, fb.to_bmp(pbe_shell::PAGE_BG)).expect("write bmp");

    // ────────────────────────────────────────────────────────────────────
    // Composition test #1: external stylesheet fetch + inject
    // ────────────────────────────────────────────────────────────────────
    // Write the CSS to disk under a distinctive name, then load a page that
    // <link rel=stylesheet>s it. The browser must scan the link, fetch the
    // file, inject it as a <style> block, and let the kit's existing
    // <style>-reading html::parse apply it. We verify by rendering the linked
    // page and checking that at least one non-default-coloured pixel appears
    // in the frame — the CSS says the heading is #d35400 (orange), a colour
    // that does not appear anywhere on the chrome or default text.
    let css_path = out_dir.join("browser-probe.css");
    std::fs::write(&css_path, EXTERNAL_CSS).expect("write external css");
    let linked_html = LINKED_PAGE_TEMPLATE.replace("{CSS}", css_path.to_str().unwrap());
    let linked_path = out_dir.join("browser-probe-linked.html");
    std::fs::write(&linked_path, &linked_html).expect("write linked html");

    let linked_browser = Browser::open(linked_path.to_str().unwrap(), 900, 650);
    let linked_fb = linked_browser.render();
    let linked_bmp = out_dir.join("browser-probe-linked.bmp");
    std::fs::write(&linked_bmp, linked_fb.to_bmp(pbe_shell::PAGE_BG)).expect("write linked bmp");

    // #d35400 in the framebuffer's f32 RGBA rounds to ~0.827, 0.329, 0.
    let mut orange_pixels = 0u32;
    for p in linked_fb.pixels() {
        if (p.r - 0.827).abs() < 0.04 && (p.g - 0.329).abs() < 0.04 && p.b < 0.04 {
            orange_pixels += 1;
        }
    }
    assert!(
        orange_pixels > 20,
        "external stylesheet should have coloured the heading orange \
         (found only {orange_pixels} matching pixels); \
         fetch-and-inject composition may have failed"
    );

    // ────────────────────────────────────────────────────────────────────
    // Composition test #2: bloom Quality tier reaches the browser
    // ────────────────────────────────────────────────────────────────────
    // render() and render_with_quality(Quality::Fast) must produce identical
    // frames; render_with_quality(Quality::TiledFull) must produce a
    // different frame (the tiled CPU bloom the earlier benchmark showed
    // beats the wgpu path 1.27x–1.73x on this hardware).
    let fast = linked_browser.render_with_quality(Quality::Fast);
    let plain = linked_browser.render();
    assert_eq!(
        fast.pixels().len(),
        plain.pixels().len(),
        "Fast and plain render must produce same-sized frames"
    );
    let identical = fast
        .pixels()
        .iter()
        .zip(plain.pixels().iter())
        .all(|(a, b)| {
            (a.r - b.r).abs() < 1e-6 && (a.g - b.g).abs() < 1e-6 && (a.b - b.b).abs() < 1e-6
        });
    assert!(
        identical,
        "Quality::Fast must be byte-identical to render() — the doctrine is \
         that no post-processing is byte-identical"
    );

    let bloomed = linked_browser.render_with_quality(Quality::TiledFull);
    let bloomed_bmp = out_dir.join("browser-probe-bloom.bmp");
    std::fs::write(&bloomed_bmp, bloomed.to_bmp(pbe_shell::PAGE_BG)).expect("write bloomed bmp");
    let bloom_changed_pixels = fast
        .pixels()
        .iter()
        .zip(bloomed.pixels().iter())
        .filter(|(a, b)| {
            (a.r - b.r).abs() > 1e-4 || (a.g - b.g).abs() > 1e-4 || (a.b - b.b).abs() > 1e-4
        })
        .count();
    assert!(
        bloom_changed_pixels > 100,
        "Quality::TiledFull must visibly change the frame (found only \
         {bloom_changed_pixels} changed pixels)"
    );

    println!(
        "browser_probe: typed navigation + Back/Forward + link-click all verified through real dispatch(); wrote {}",
        bmp_path.display()
    );
    println!(
        "browser_probe: external stylesheet composition verified ({orange_pixels} orange pixels from #d35400 rule); wrote {}",
        linked_bmp.display()
    );
    println!(
        "browser_probe: Quality tier composition verified (Fast == plain, TiledFull changed {bloom_changed_pixels} pixels); wrote {}",
        bloomed_bmp.display()
    );

    // ────────────────────────────────────────────────────────────────────
    // Composition test #3: <img> prefetch + BMP decode + kit blit
    // ────────────────────────────────────────────────────────────────────
    // Write a small deliberately-off-colour BMP to disk (cyan #00e6ff — a
    // colour that appears nowhere on the chrome, default text, or the other
    // fixture pages), reference it via <img src="…">, load through Browser,
    // and count cyan pixels in the rendered frame. Success = the browser
    // scanned the img tag, fetched the file, decoded via decode_bmp, handed
    // the Arc<Image> to html::parse_with_images, layout emitted a
    // Painted::Image at the 32×32 rect, and orchestrator's paint_one_box
    // called raster::blit_image.
    let img_path = out_dir.join("browser-probe.bmp-cyan");
    // Encode a 32×32 solid cyan BMP by round-tripping through Framebuffer.
    let mut cyan_fb = pmre_kit::Framebuffer::new(32, 32, pmre_kit::Rgba::rgb8(0, 230, 255));
    // The clear color above already fills — no more setup needed. Force
    // alpha = 1.0 explicitly on every pixel since to_bmp composites against
    // the background arg.
    for p in cyan_fb.pixels_mut() {
        *p = pmre_kit::Rgba::rgb8(0, 230, 255);
    }
    std::fs::write(
        &img_path,
        cyan_fb.to_bmp(pmre_kit::Rgba::new(1.0, 1.0, 1.0, 1.0)),
    )
    .expect("write cyan bmp");

    let img_page = format!(
        r#"<div style="background:#ffffff; padding:20px"><p>Image below</p><img src="{}" width="32" height="32"></div>"#,
        img_path.to_str().unwrap().replace('\\', "/")
    );
    let img_html_path = out_dir.join("browser-probe-img.html");
    std::fs::write(&img_html_path, &img_page).expect("write img html");

    let img_browser = Browser::open(img_html_path.to_str().unwrap(), 900, 650);
    let img_fb = img_browser.render();
    let img_bmp = out_dir.join("browser-probe-img.bmp");
    std::fs::write(&img_bmp, img_fb.to_bmp(pbe_shell::PAGE_BG)).expect("write img frame");

    // #00e6ff in normalised RGBA ≈ (0.0, 0.902, 1.0).
    let mut cyan_pixels = 0u32;
    for p in img_fb.pixels() {
        if p.r < 0.05 && (p.g - 0.902).abs() < 0.04 && p.b > 0.95 {
            cyan_pixels += 1;
        }
    }
    assert!(
        cyan_pixels > 500,
        "expected ~1024 cyan pixels from the 32×32 img blit (found only {cyan_pixels}); \
         img prefetch + decode + kit blit composition may have failed"
    );

    println!(
        "browser_probe: <img> composition verified ({cyan_pixels} cyan #00e6ff pixels from the 32x32 BMP blit); wrote {}",
        img_bmp.display()
    );
}
