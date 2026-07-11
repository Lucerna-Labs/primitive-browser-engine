//! # pbe — the primitive browser engine CLI
//!
//! `pmre-kit` / `pmre-orchestrator` is already a complete, self-contained
//! HTML/CSS renderer — `render_html` does parse + layout + paint + raster in
//! one call. This binary is a thin, direct caller: load a page (local file,
//! built-in demo, or a fetched URL over `pbe-net`'s sealed-curl on-ramp),
//! render it, write the result to `out/`. No bus, no message types, no
//! intermediate stages — there is nothing here to orchestrate.

use pmre_orchestrator::Quality;
use std::path::PathBuf;

/// Default frame size (CSS px == device px for now).
const FRAME_W: u32 = 800;
const FRAME_H: u32 = 600;

/// Opaque white page background.
const PAGE_BG: pmre_kit::Rgba = pmre_kit::Rgba::new(1.0, 1.0, 1.0, 1.0);

/// The built-in demo page, used when no input files are given. Inline-styled:
/// `pmre-kit`'s HTML reducer only reads `style="..."` attributes, not
/// `<style>` blocks or selectors.
const DEMO_HTML: &str = r#"<div style="background:#1e2430; width:640px; height:200px"><p style="color:#e6e9f0">Hello from the primitive browser engine</p></div>"#;

/// Where the page came from, so the output artifact can be labeled.
enum Source {
    Local { label: String, html: String },
    Url(String),
}

fn parse_quality(s: &str) -> Option<Quality> {
    match s {
        "fast" => Some(Quality::Fast),
        "balanced" => Some(Quality::Balanced),
        "full" => Some(Quality::Full),
        "tiled-balanced" => Some(Quality::TiledBalanced),
        "tiled-full" => Some(Quality::TiledFull),
        "parallel-balanced" => Some(Quality::ParallelBalanced),
        "parallel-full" => Some(Quality::ParallelFull),
        "gpu-balanced" => Some(Quality::GpuBalanced),
        "gpu-full" => Some(Quality::GpuFull),
        _ => None,
    }
}

/// Build the on-ramp + quality tier from CLI args.
///
/// - `pbe`                                      → the built-in demo page, Fast
/// - `pbe --url <URL>`                          → fetch + render a live page
/// - `pbe <html-file>`                          → render a local file
/// - `pbe <html-file> --quality tiled-full`     → same, with CPU bloom post
///
/// Returns an error string on unreadable input or bad flags.
fn args_from_cli(raw: &[String]) -> Result<(Source, Quality), String> {
    let mut positional: Vec<String> = Vec::new();
    let mut url: Option<String> = None;
    let mut quality = Quality::Fast;
    let mut it = raw.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--url" => {
                url = Some(
                    it.next()
                        .cloned()
                        .ok_or_else(|| "--url requires a URL argument".to_string())?,
                );
            }
            "--quality" => {
                let s = it
                    .next()
                    .ok_or_else(|| "--quality requires a tier name".to_string())?;
                quality = parse_quality(s).ok_or_else(|| {
                    format!(
                        "unknown --quality tier '{s}' (fast | balanced | full | tiled-balanced | \
                         tiled-full | parallel-balanced | parallel-full | gpu-balanced | gpu-full)"
                    )
                })?;
            }
            other => positional.push(other.to_string()),
        }
    }
    let source = match (url, positional.as_slice()) {
        (Some(u), _) => Source::Url(u),
        (None, []) => Source::Local {
            label: "demo".into(),
            html: DEMO_HTML.into(),
        },
        (None, [html_path, ..]) => {
            let html = std::fs::read_to_string(html_path)
                .map_err(|e| format!("cannot read HTML '{html_path}': {e}"))?;
            let label = std::path::Path::new(html_path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("page")
                .to_string();
            Source::Local { label, html }
        }
    };
    Ok((source, quality))
}

fn main() {
    let cli: Vec<String> = std::env::args().skip(1).collect();
    let (source, quality) = match args_from_cli(&cli) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERROR: {e}");
            eprintln!("usage: pbe [<html-file>] | pbe --url <URL> [--quality tiled-full|...]");
            std::process::exit(2);
        }
    };

    let (label, html) = match source {
        Source::Local { label, html } => (label, html),
        Source::Url(url) => {
            println!("fetching {url}...");
            match pbe_net::fetch(&url) {
                Ok(page) => {
                    println!(
                        "fetched {} → HTTP {} ({} bytes html)",
                        page.final_url,
                        page.status,
                        page.body.len()
                    );
                    (page.final_url, page.body)
                }
                Err(e) => {
                    eprintln!("ERROR: fetch failed for {url}: {e:?}");
                    std::process::exit(1);
                }
            }
        }
    };

    let fb = match quality {
        Quality::Fast => pmre_orchestrator::render_html(&html, FRAME_W, FRAME_H, PAGE_BG),
        q => {
            let root = pmre_kit::html::parse(&html);
            pmre_orchestrator::render_uxi_quality(&root, FRAME_W, FRAME_H, PAGE_BG, q)
        }
    };
    let bmp = fb.to_bmp(PAGE_BG);

    let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("out");
    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        eprintln!(
            "\nERROR: could not create out dir {}: {e}",
            out_dir.display()
        );
        std::process::exit(1);
    }
    // The label may be a URL (slashes, colons) — sanitize for a filename.
    let safe_label: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe_label = safe_label.trim_matches('_').to_string();
    let safe_label = if safe_label.is_empty() {
        "page".to_string()
    } else {
        safe_label
    };
    let bmp_path = out_dir.join(format!("{safe_label}.bmp"));

    if let Err(e) = std::fs::write(&bmp_path, &bmp) {
        eprintln!("\nERROR: could not write {}: {e}", bmp_path.display());
        std::process::exit(1);
    }

    println!(
        "RESULT: '{label}' rendered {FRAME_W}x{FRAME_H} → {} ({} bytes)",
        bmp_path.display(),
        bmp.len()
    );
}
