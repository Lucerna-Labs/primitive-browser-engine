//! Framebuffer + alpha-over compositing (`blend`/`over` = the `combine` atom, weight = alpha)
//! + a dependency-free 24-bit BMP encoder so output is viewable without any external crate.

use crate::paint::Rgba;

pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pixels: Vec<Rgba>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32, clear: Rgba) -> Self {
        Self {
            width,
            height,
            pixels: vec![clear; (width * height) as usize],
        }
    }

    /// Porter-Duff "over": straight-alpha `src` composited onto the stored pixel.
    pub fn blend_over(&mut self, x: u32, y: u32, src: Rgba) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = (y * self.width + x) as usize;
        self.pixels[i] = crate::paint::over(self.pixels[i], src);
    }

    /// Encode as a 24-bit BMP, flattening straight alpha over `background`.
    pub fn to_bmp(&self, background: Rgba) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;
        let pad = (4 - (w * 3) % 4) % 4;
        let pixel_bytes = (w * 3 + pad) * h;
        let file_size = 54 + pixel_bytes;

        let mut out = Vec::with_capacity(file_size);
        // BITMAPFILEHEADER (14 bytes)
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(file_size as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&54u32.to_le_bytes());
        // BITMAPINFOHEADER (40 bytes)
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(self.width as i32).to_le_bytes());
        out.extend_from_slice(&(self.height as i32).to_le_bytes()); // positive => bottom-up
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&24u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(pixel_bytes as u32).to_le_bytes());
        out.extend_from_slice(&2835i32.to_le_bytes()); // ~72 DPI
        out.extend_from_slice(&2835i32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());

        let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        for y in (0..h).rev() {
            for x in 0..w {
                let px = self.pixels[y * w + x];
                let a = px.a.clamp(0.0, 1.0);
                let r = px.r * a + background.r * (1.0 - a);
                let g = px.g * a + background.g * (1.0 - a);
                let b = px.b * a + background.b * (1.0 - a);
                out.push(to_u8(b));
                out.push(to_u8(g));
                out.push(to_u8(r));
            }
            out.resize(out.len() + pad, 0);
        }
        out
    }

    /// Flatten to opaque `0x00RRGGBB` pixels for software presentation (e.g. softbuffer).
    pub fn to_u32(&self, background: Rgba) -> Vec<u32> {
        let to_u8 = |c: f32| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let mut out = Vec::with_capacity((self.width * self.height) as usize);
        for px in &self.pixels {
            let a = px.a.clamp(0.0, 1.0);
            let r = to_u8(px.r * a + background.r * (1.0 - a));
            let g = to_u8(px.g * a + background.g * (1.0 - a));
            let b = to_u8(px.b * a + background.b * (1.0 - a));
            out.push((r << 16) | (g << 8) | b);
        }
        out
    }

    /// Read the stored straight-alpha pixel at `(x, y)`; transparent if out of bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width || y >= self.height {
            return Rgba::new(0.0, 0.0, 0.0, 0.0);
        }
        self.pixels[(y * self.width + x) as usize]
    }

    /// Read-only view of the raw pixel buffer in row-major order.
    pub fn pixels(&self) -> &[Rgba] {
        &self.pixels
    }

    /// Mutable view of the raw pixel buffer for bulk writes (e.g. GPU readback).
    pub fn pixels_mut(&mut self) -> &mut [Rgba] {
        &mut self.pixels
    }

    /// Direct pixel write, bypassing Porter-Duff compositing.
    pub fn set_pixel(&mut self, x: u32, y: u32, c: Rgba) {
        if x < self.width && y < self.height {
            self.pixels[(y * self.width + x) as usize] = c;
        }
    }
}

/// A pixel sink the rasterizer can target: the whole framebuffer, or one row-band of it.
/// Keeping `scan_convert` / `text` generic over this lets the lane renderer write straight
/// into a slice of the final buffer (no per-band temp buffer, no stitch) at absolute coords.
pub trait Surface {
    fn width(&self) -> u32;
    fn height(&self) -> u32;
    /// Device rows this surface accepts, `[lo, hi)`. Defaults to the whole height; a band
    /// view narrows it so the rasterizer skips rows outside the band.
    fn row_range(&self) -> (u32, u32) {
        (0, self.height())
    }
    fn blend_over(&mut self, x: u32, y: u32, src: Rgba);
}

impl Surface for Framebuffer {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.height
    }
    fn blend_over(&mut self, x: u32, y: u32, src: Rgba) {
        Framebuffer::blend_over(self, x, y, src);
    }
}

/// A view over one contiguous row-band of a frame — pixels for device rows
/// `[y0, y0 + band_h)`, addressed in **absolute** device coordinates. Writing through it
/// composites into the band's own slice; distinct bands of a frame are disjoint, so lanes
/// never alias. No coordinate translation, so output matches a full-frame render exactly.
pub struct BandView<'a> {
    pixels: &'a mut [Rgba],
    width: u32,
    y0: u32,
    band_h: u32,
    full_h: u32,
}

impl<'a> BandView<'a> {
    /// `pixels` is the band's rows (length `band_h * width`); `y0` is its first device row;
    /// `full_h` is the height of the whole frame.
    pub fn new(pixels: &'a mut [Rgba], width: u32, y0: u32, full_h: u32) -> Self {
        let band_h = pixels.len() as u32 / width.max(1);
        Self {
            pixels,
            width,
            y0,
            band_h,
            full_h,
        }
    }
}

impl Surface for BandView<'_> {
    fn width(&self) -> u32 {
        self.width
    }
    fn height(&self) -> u32 {
        self.full_h
    }
    fn row_range(&self) -> (u32, u32) {
        (self.y0, self.y0 + self.band_h)
    }
    fn blend_over(&mut self, x: u32, y: u32, src: Rgba) {
        if x >= self.width || y < self.y0 || y >= self.y0 + self.band_h {
            return;
        }
        let i = ((y - self.y0) * self.width + x) as usize;
        self.pixels[i] = crate::paint::over(self.pixels[i], src);
    }
}
