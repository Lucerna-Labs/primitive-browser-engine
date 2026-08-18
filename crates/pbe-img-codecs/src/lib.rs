//! # pbe-img-codecs
//!
//! Image decoders for the formats most real web images use \u2014 **JPEG, WebP,
//! GIF** \u2014 that decode to the kit's [`Image`](pmre_raster::Image) type. One
//! swappable modular crate; the in-kit BMP/PNG decoders stay zero-dependency
//! and untouched, exactly as the doctrine prescribes ("don't reimplement what
//! already works; keep each concern in its own crate").
//!
//! ## Why a separate crate (and why `image` behind it)
//!
//! The kit's own `decode_bmp`/`decode_png` are zero-dep hand-written
//! decoders, deliberately small (BMP is trivial; PNG's DEFLATE+Huffman is
//! already substantial). A correct JPEG decoder (Huffman + IDCT + YCbCr) and
//! a correct WebP decoder (VP8 lossy + lossless) are each larger than the
//! entire kit's raster core \u2014 reimplementing them would be the same category
//! of mistake as reimplementing crypto. Instead this crate binds the
//! well-tested [`image`] crate's decoders behind a thin, swappable boundary,
//! the same way the protocol layer binds the sealed system HTTP client.
//!
//! ## Dispatch
//!
//! [`decode`] inspects the magic bytes and dispatches to the right codec:
//!
//! | Magic bytes            | Format | Source              |
//! |------------------------|--------|---------------------|
//! | `\xFF\xD8\xFF`         | JPEG   | `image` crate       |
//! | `RIFF....WEBP`         | WebP   | `image` crate       |
//! | `GIF8`                 | GIF    | `image` crate       |
//!
//! Returns [`Option<Image>`] so the browser layer treats undecodable bytes as
//! a missing image (renders the alt text) rather than aborting the page,
//! matching the kit's `decode_bmp`/`decode_png` contract.

use image::ImageReader;
use pmre_core::Rgba;
use pmre_raster::Image;
use std::io::Cursor;

/// Decode a JPEG, WebP, or GIF byte stream into the kit's [`Image`]. Returns
/// `None` on any parse error (corrupt stream, truncated, unsupported
/// sub-format) so callers treat it as a missing image, not a fatal page
/// error. Magic-byte routing: JPEG `\xFF\xD8\xFF`, WebP `RIFF....WEBP`,
/// GIF `GIF8`.
pub fn decode(bytes: &[u8]) -> Option<Image> {
    if !is_jpeg(bytes) && !is_webp(bytes) && !is_gif(bytes) {
        return None;
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let dyn_img = reader.decode().ok()?;
    let rgba = dyn_img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    let pixels = rgba
        .pixels()
        .map(|p| {
            Rgba::new(
                p[0] as f32 / 255.0,
                p[1] as f32 / 255.0,
                p[2] as f32 / 255.0,
                p[3] as f32 / 255.0,
            )
        })
        .collect();
    Some(Image {
        width,
        height,
        pixels,
    })
}

/// JPEG SOI marker: `\xFF\xD8\xFF`.
fn is_jpeg(b: &[u8]) -> bool {
    b.len() >= 3 && b[0] == 0xFF && b[1] == 0xD8 && b[2] == 0xFF
}

/// WebP: `RIFF` + 4 bytes size + `WEBP`.
fn is_webp(b: &[u8]) -> bool {
    b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
}

/// GIF: `GIF8`.
fn is_gif(b: &[u8]) -> bool {
    b.len() >= 4 && &b[0..4] == b"GIF8"
}

/// Whether this crate can decode the given magic bytes (for the browser
/// layer's format dispatch alongside the kit's BMP/PNG detection).
pub fn handles(bytes: &[u8]) -> bool {
    is_jpeg(bytes) || is_webp(bytes) || is_gif(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_detects_jpeg_magic() {
        assert!(handles(&[0xFF, 0xD8, 0xFF, 0xE0]));
    }

    #[test]
    fn handles_detects_webp_magic() {
        let mut b = b"RIFF".to_vec();
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(b"WEBP");
        assert!(handles(&b));
    }

    #[test]
    fn handles_detects_gif_magic() {
        assert!(handles(b"GIF89a"));
    }

    #[test]
    fn handles_rejects_png_and_bmp() {
        assert!(!handles(&[0x89, 0x50, 0x4E, 0x47])); // PNG
        assert!(!handles(b"BM")); // BMP
    }

    #[test]
    fn decode_returns_none_for_garbage() {
        assert!(decode(b"not an image at all").is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn decode_returns_none_for_truncated_jpeg_magic() {
        // Only the magic, no valid JPEG body.
        assert!(decode(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00]).is_none());
    }

    #[test]
    fn decode_a_synthetic_jpeg_round_trips_dimensions() {
        // Encode a 4x3 solid JPEG with the image crate, then decode it back
        // through this crate and check the dimensions survive.
        let img = image::RgbImage::from_pixel(4, 3, image::Rgb([200, 50, 120]));
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        let bytes = buf.into_inner();
        assert!(handles(&bytes));
        let decoded = decode(&bytes).expect("jpeg should decode");
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 3);
        // The pixel should be close to the input (JPEG is lossy but a solid
        // block decodes to ~the same colour within a few levels).
        let p = decoded.pixel(0, 0);
        assert!((p.r - 200.0 / 255.0).abs() < 0.05);
        assert!((p.g - 50.0 / 255.0).abs() < 0.05);
        assert!((p.b - 120.0 / 255.0).abs() < 0.05);
    }

    #[test]
    fn decode_a_synthetic_webp_round_trips_dimensions() {
        let img = image::RgbImage::from_pixel(5, 2, image::Rgb([10, 220, 90]));
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::WebP)
            .unwrap();
        let bytes = buf.into_inner();
        assert!(handles(&bytes));
        let decoded = decode(&bytes).expect("webp should decode");
        assert_eq!(decoded.width, 5);
        assert_eq!(decoded.height, 2);
    }
}
