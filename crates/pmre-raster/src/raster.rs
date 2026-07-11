//! Rasterization primitives — the cheap, exact-AA coverage generator.
//!
//! `signed_distance` is the `compare` atom (a distance). `coverage` is the graphics
//! `smoothstep` primitive (analytic anti-aliasing). `scan_convert` wires
//! `scan` (pixel grid) · `project` (inverse transform) · `compare` (SDF) · `scale` (AA band)
//! · `combine` (alpha-over) into one shape → pixels operation. Mechanism only: it never
//! decides draw order, clipping, or which generator to use — that is the orchestrator's job.
//!
//! `Image` + `decode_bmp` + `decode_png` + `blit_image` add bitmap support alongside
//! the SDF path — bitmap pixels flow through the same `scan · project · combine` shape
//! (`scan` output-rect pixels · `project` src coordinate via nearest-neighbour · `combine`
//! src+dst via Porter-Duff `over`), just sampled from stored pixels instead of computed
//! from a signed-distance field. Same atom composition, different sample source.

use pmre_core::framebuffer::Surface;
use pmre_core::geom::Vec2;
use pmre_core::paint::{Bounds, DrawCmd, Rgba, Shape};

/// Signed distance to the shape boundary in its local space: negative inside, positive outside.
pub fn signed_distance(shape: &Shape, p: Vec2) -> f32 {
    match *shape {
        Shape::Rect { half } => sd_box(p, half),
        Shape::RoundedRect { half, radius } => sd_box(p, half - Vec2::new(radius, radius)) - radius,
        Shape::Circle { radius } => p.length() - radius,
        Shape::Line { a, b, width } => sd_segment(p, a, b) - width * 0.5,
    }
}

fn sd_box(p: Vec2, half: Vec2) -> f32 {
    let d = p.abs() - half;
    d.max_scalar(0.0).length() + d.x.max(d.y).min(0.0)
}

fn sd_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = (pa.dot(ba) / ba.dot(ba)).clamp(0.0, 1.0);
    (pa - ba.scale(h)).length()
}

/// Analytic coverage from a signed distance: 1 inside, 0 outside, Hermite band of half-width `aa`.
pub fn coverage(dist: f32, aa: f32) -> f32 {
    1.0 - smoothstep(-aa, aa, dist)
}

/// Hermite smoothstep: 0 below `edge0`, 1 above `edge1`, C¹-continuous in between.
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Scan-convert one command into `surf` using the SDF coverage generator. Generic over the
/// pixel sink so it can target a whole framebuffer or one row-band of it (see `Surface`).
/// Pure translations (every box the layout solver emits) take a fast path that replaces
/// the per-pixel inverse-matrix multiply with a subtraction.
pub fn scan_convert<S: Surface>(cmd: &DrawCmd, surf: &mut S, clip: Option<Bounds>) {
    // One device pixel measured in local units — the width of the anti-aliasing band.
    // A `soft` command widens the band for smooth falloff (shadows, glows).
    let aa = (1.0 / cmd.transform.scale_factor().max(1e-6))
        .max(1e-4)
        .max(cmd.soft);
    let bounds = device_bounds(cmd, surf.width(), surf.height(), surf.row_range(), clip);
    let t = cmd.transform;
    if t.a == 1.0 && t.d == 1.0 && t.b == 0.0 && t.c == 0.0 {
        convert_rows(cmd, surf, bounds, aa, |x, y| Vec2::new(x - t.e, y - t.f));
    } else {
        let inv = t.inverse();
        convert_rows(cmd, surf, bounds, aa, move |x, y| {
            inv.apply(Vec2::new(x, y))
        });
    }
}

fn convert_rows<S: Surface, M: Fn(f32, f32) -> Vec2>(
    cmd: &DrawCmd,
    surf: &mut S,
    (x0, y0, x1, y1): (u32, u32, u32, u32),
    aa: f32,
    to_local: M,
) {
    for y in y0..y1 {
        let py = y as f32 + 0.5;
        for x in x0..x1 {
            let local = to_local(x as f32 + 0.5, py);
            let d = signed_distance(&cmd.shape, local);
            let cov = coverage(d, aa);
            if cov > 0.0 {
                // Sample the paint at the shape-local point (gradients move with the shape).
                let col = cmd.paint.sample(local);
                surf.blend_over(x, y, col.with_alpha(col.a * cov));
            }
        }
    }
}

// ── Bitmap images: decode + blit ────────────────────────────────────────────
//
// The kit only rendered SDF shapes before this. Adding bitmap support here
// (rather than as a separate `image` module) keeps the composition local to
// the raster primitive that already knows how to walk a device-space pixel
// grid and combine colours over a target — an image blit reuses `scan · project
// · combine` with pixels sampled from stored data instead of computed from a
// signed-distance field.

/// A decoded image: straight-alpha RGBA pixels in row-major, top-down order.
/// `pixels.len() == width * height`. Owned pixel data — decoders allocate,
/// callers hold via `Arc<Image>` to share a single decode across a full render.
#[derive(Clone, Debug)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<Rgba>,
}

impl Image {
    /// Straight-alpha colour at (x, y). Returns transparent black out-of-bounds.
    pub fn pixel(&self, x: u32, y: u32) -> Rgba {
        if x >= self.width || y >= self.height {
            return Rgba::new(0.0, 0.0, 0.0, 0.0);
        }
        self.pixels[(y * self.width + x) as usize]
    }
}

/// Decode a Windows BMP file: BITMAPFILEHEADER + BITMAPINFOHEADER + pixel data.
/// Supports 24-bit BGR and 32-bit BGRA — the two variants `Framebuffer::to_bmp`
/// writes and that most hand-authored BMPs use. Returns `None` on any parse
/// error rather than panicking, so the browser layer treats undecodable bytes
/// as a missing image (renders the alt text) instead of aborting the page.
pub fn decode_bmp(bytes: &[u8]) -> Option<Image> {
    // BITMAPFILEHEADER (14 bytes) + minimum BITMAPINFOHEADER (40 bytes) = 54.
    if bytes.len() < 54 {
        return None;
    }
    if &bytes[0..2] != b"BM" {
        return None;
    }
    let data_offset = u32_le(&bytes[10..14]) as usize;

    let header_size = u32_le(&bytes[14..18]);
    if header_size < 40 {
        return None;
    }
    let width_raw = i32_le(&bytes[18..22]);
    let height_raw = i32_le(&bytes[22..26]);
    let bpp = u16_le(&bytes[28..30]);
    let compression = u32_le(&bytes[30..34]);

    if width_raw <= 0 || height_raw == 0 {
        return None;
    }
    if compression != 0 {
        // BI_RGB (uncompressed) only. BI_BITFIELDS / RLE variants are out of
        // scope for the primitive kit — hand-authored BMPs and to_bmp output
        // are always BI_RGB.
        return None;
    }
    if bpp != 24 && bpp != 32 {
        return None;
    }

    let w = width_raw as u32;
    let h = height_raw.unsigned_abs();
    let top_down = height_raw < 0;
    let bpp_bytes = (bpp / 8) as usize;
    let row_bytes = (w as usize) * bpp_bytes;
    let row_padding = (4 - row_bytes % 4) % 4;
    let stride = row_bytes + row_padding;

    let needed = data_offset.checked_add(stride.checked_mul(h as usize)?)?;
    if needed > bytes.len() {
        return None;
    }

    let mut pixels = Vec::with_capacity((w as usize) * (h as usize));
    for out_y in 0..h {
        let src_row = if top_down { out_y } else { h - 1 - out_y };
        let row_start = data_offset + (src_row as usize) * stride;
        for x in 0..w {
            let px = row_start + (x as usize) * bpp_bytes;
            let b = bytes[px];
            let g = bytes[px + 1];
            let r = bytes[px + 2];
            let a = if bpp == 32 { bytes[px + 3] } else { 255 };
            pixels.push(Rgba::new(
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ));
        }
    }
    Some(Image {
        width: w,
        height: h,
        pixels,
    })
}

/// Blit `src` into `surf` at device-space `dst_rect`, clipped to `clip`.
/// Nearest-neighbour sampling — for a UI browser rendering embedded images at
/// their natural or slightly-scaled sizes this is the right primitive; a
/// bilinear pass would blur crisp UI content and can be added later without
/// changing the API.
///
/// Compositing: Porter-Duff `over` via `Surface::blend_over` — an image with
/// any alpha channel (32-bit BMPs, PNGs with tRNS) transparent-composites
/// against whatever the surface already holds at that pixel. Generic over
/// `Surface` so the banded/parallel render path can blit images too — a
/// `BandView`'s `blend_over` silently drops writes outside its row range,
/// exactly like SDF scan-convert already does.
pub fn blit_image<S: Surface>(surf: &mut S, dst_rect: Bounds, src: &Image, clip: Bounds) {
    if src.width == 0 || src.height == 0 {
        return;
    }
    // Intersect dst_rect with clip and with the surface bounds.
    let x0 = dst_rect.min.x.max(clip.min.x).max(0.0);
    let y0 = dst_rect.min.y.max(clip.min.y).max(0.0);
    let x1 = dst_rect.max.x.min(clip.max.x).min(surf.width() as f32);
    let y1 = dst_rect.max.y.min(clip.max.y).min(surf.height() as f32);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let dst_w = dst_rect.max.x - dst_rect.min.x;
    let dst_h = dst_rect.max.y - dst_rect.min.y;
    if dst_w <= 0.0 || dst_h <= 0.0 {
        return;
    }
    let sx_scale = src.width as f32 / dst_w;
    let sy_scale = src.height as f32 / dst_h;

    let (row_lo, row_hi) = surf.row_range();
    let iy0 = (y0.floor() as u32).max(row_lo);
    let iy1 = (y1.ceil() as u32).min(row_hi);
    let ix0 = x0.floor() as u32;
    let ix1 = x1.ceil() as u32;
    for y in iy0..iy1 {
        let py = y as f32 + 0.5;
        let sy = ((py - dst_rect.min.y) * sy_scale) as i32;
        if sy < 0 || sy >= src.height as i32 {
            continue;
        }
        for x in ix0..ix1 {
            let px = x as f32 + 0.5;
            let sx = ((px - dst_rect.min.x) * sx_scale) as i32;
            if sx < 0 || sx >= src.width as i32 {
                continue;
            }
            let col = src.pixel(sx as u32, sy as u32);
            if col.a <= 0.0 {
                continue;
            }
            surf.blend_over(x, y, col);
        }
    }
}

#[inline]
fn u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
#[inline]
fn i32_le(b: &[u8]) -> i32 {
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
#[inline]
fn u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

// ── PNG decoder (zero-dep) ──────────────────────────────────────────────────
//
// Minimal PNG reader: signature + IHDR + IDAT (concatenate + decompress) +
// IEND. Supports color types 2 (RGB, 8bpc) and 6 (RGBA, 8bpc) — the two
// colour types web images almost always ship as. Grayscale/palette are
// out of scope for the initial cut; a caller getting a palette PNG will
// see `None` and fall back to the alt text, matching how unsupported
// selector syntax fails closed in css.rs (documented, deliberate).
//
// The DEFLATE inflater is fixed + dynamic Huffman + LZ77 sliding window,
// written from scratch — zero external crates. Not the fastest possible
// inflater but complete and correct on RFC 1951 fixture streams.

const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

/// Decode a PNG file. Supports color types 2 (RGB, 8bpc) and 6 (RGBA, 8bpc)
/// — enough for typical hand-authored / web images. Grayscale, palette, and
/// non-8bpc depths return `None`; the caller treats it as a missing image.
pub fn decode_png(bytes: &[u8]) -> Option<Image> {
    if bytes.len() < 8 + 25 || bytes[0..8] != PNG_SIG {
        return None;
    }
    let mut i = 8usize;
    let mut ihdr: Option<Ihdr> = None;
    let mut idat: Vec<u8> = Vec::new();
    while i + 8 <= bytes.len() {
        let len = u32_be(&bytes[i..i + 4]) as usize;
        let ty = &bytes[i + 4..i + 8];
        let data_start = i + 8;
        let data_end = data_start.checked_add(len)?;
        if data_end + 4 > bytes.len() {
            return None;
        }
        match ty {
            b"IHDR" => {
                if len != 13 {
                    return None;
                }
                let hdr = Ihdr {
                    width: u32_be(&bytes[data_start..data_start + 4]),
                    height: u32_be(&bytes[data_start + 4..data_start + 8]),
                    bit_depth: bytes[data_start + 8],
                    color_type: bytes[data_start + 9],
                    compression: bytes[data_start + 10],
                    filter: bytes[data_start + 11],
                    interlace: bytes[data_start + 12],
                };
                if hdr.bit_depth != 8
                    || hdr.compression != 0
                    || hdr.filter != 0
                    || hdr.interlace != 0
                {
                    return None;
                }
                if hdr.color_type != 2 && hdr.color_type != 6 {
                    return None;
                }
                ihdr = Some(hdr);
            }
            b"IDAT" => {
                idat.extend_from_slice(&bytes[data_start..data_end]);
            }
            b"IEND" => break,
            _ => {} // skip ancillary + palette chunks silently
        }
        // Skip the 4-byte CRC — we don't verify CRC (the DEFLATE path would
        // fail loudly on a corrupt stream anyway, and PNG CRC verification
        // costs a full table pass with no user-visible benefit here).
        i = data_end + 4;
    }
    let hdr = ihdr?;
    let decompressed = inflate_zlib(&idat)?;
    unfilter_png(&decompressed, &hdr)
}

struct Ihdr {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    compression: u8,
    filter: u8,
    interlace: u8,
}

#[inline]
fn u32_be(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// zlib wrapper: 2-byte header, DEFLATE stream, 4-byte Adler-32 (not verified).
fn inflate_zlib(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 6 {
        return None;
    }
    let cmf = bytes[0];
    if (cmf & 0x0f) != 8 {
        return None; // only DEFLATE compression method
    }
    // FLG check byte: (cmf*256 + flg) must be a multiple of 31.
    let flg = bytes[1];
    if !((cmf as u32) * 256 + flg as u32).is_multiple_of(31) {
        return None;
    }
    let has_dict = (flg & 0x20) != 0;
    if has_dict {
        return None; // preset dictionaries aren't used by PNG
    }
    inflate_deflate(&bytes[2..bytes.len() - 4])
}

/// Raw DEFLATE (RFC 1951) inflate. Handles stored, fixed Huffman, and dynamic
/// Huffman blocks + the LZ77 length/distance codes.
fn inflate_deflate(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut br = BitReader::new(bytes);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let bfinal = br.bits(1)?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                // Stored block: align to byte boundary, read LEN + NLEN + LEN bytes.
                br.align_to_byte();
                let len = br.take_u16_le()? as usize;
                let nlen = br.take_u16_le()? as usize;
                if len ^ 0xffff != nlen {
                    return None;
                }
                for _ in 0..len {
                    out.push(br.take_byte()?);
                }
            }
            1 => {
                // Fixed Huffman: literals 0..=143 = 8 bits, 144..=255 = 9 bits,
                // 256..=279 = 7 bits, 280..=287 = 8 bits. Distances all 5 bits.
                let (lit_lens, dist_lens) = fixed_huffman_tables();
                let lit_tree = HuffmanTree::from_lengths(&lit_lens)?;
                let dist_tree = HuffmanTree::from_lengths(&dist_lens)?;
                decode_huffman_block(&mut br, &lit_tree, &dist_tree, &mut out)?;
            }
            2 => {
                // Dynamic Huffman: read HLIT, HDIST, HCLEN, code-length codes,
                // then the literal/length + distance code-length tables.
                let hlit = br.bits(5)? as usize + 257;
                let hdist = br.bits(5)? as usize + 1;
                let hclen = br.bits(4)? as usize + 4;
                let order = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut clen_lens = [0u8; 19];
                for i in 0..hclen {
                    clen_lens[order[i]] = br.bits(3)? as u8;
                }
                let clen_tree = HuffmanTree::from_lengths(&clen_lens)?;
                let total = hlit + hdist;
                let mut all_lens = vec![0u8; total];
                let mut i = 0;
                while i < total {
                    let sym = clen_tree.decode(&mut br)?;
                    if sym < 16 {
                        all_lens[i] = sym as u8;
                        i += 1;
                    } else if sym == 16 {
                        if i == 0 {
                            return None;
                        }
                        let rep = br.bits(2)? as usize + 3;
                        let v = all_lens[i - 1];
                        for _ in 0..rep {
                            if i >= total {
                                return None;
                            }
                            all_lens[i] = v;
                            i += 1;
                        }
                    } else if sym == 17 {
                        let rep = br.bits(3)? as usize + 3;
                        i = i.checked_add(rep)?;
                        if i > total {
                            return None;
                        }
                    } else if sym == 18 {
                        let rep = br.bits(7)? as usize + 11;
                        i = i.checked_add(rep)?;
                        if i > total {
                            return None;
                        }
                    } else {
                        return None;
                    }
                }
                let lit_tree = HuffmanTree::from_lengths(&all_lens[..hlit])?;
                let dist_tree = HuffmanTree::from_lengths(&all_lens[hlit..])?;
                decode_huffman_block(&mut br, &lit_tree, &dist_tree, &mut out)?;
            }
            _ => return None,
        }
        if bfinal == 1 {
            break;
        }
    }
    Some(out)
}

/// Reverse PNG filters and produce final RGBA pixels.
fn unfilter_png(data: &[u8], hdr: &Ihdr) -> Option<Image> {
    let w = hdr.width as usize;
    let h = hdr.height as usize;
    let bpp = match hdr.color_type {
        2 => 3, // RGB
        6 => 4, // RGBA
        _ => return None,
    };
    let stride = w * bpp;
    if data.len() != h * (stride + 1) {
        return None;
    }
    let mut prev_row: Vec<u8> = vec![0; stride];
    let mut cur_row: Vec<u8> = vec![0; stride];
    let mut pixels: Vec<Rgba> = Vec::with_capacity(w * h);
    let mut off = 0usize;
    for _y in 0..h {
        let filter = data[off];
        off += 1;
        let raw = &data[off..off + stride];
        off += stride;
        match filter {
            0 => cur_row.copy_from_slice(raw),
            1 => {
                for x in 0..stride {
                    let left = if x >= bpp { cur_row[x - bpp] } else { 0 };
                    cur_row[x] = raw[x].wrapping_add(left);
                }
            }
            2 => {
                for x in 0..stride {
                    cur_row[x] = raw[x].wrapping_add(prev_row[x]);
                }
            }
            3 => {
                for x in 0..stride {
                    let left = if x >= bpp { cur_row[x - bpp] } else { 0 };
                    let up = prev_row[x];
                    let avg = ((left as u16 + up as u16) / 2) as u8;
                    cur_row[x] = raw[x].wrapping_add(avg);
                }
            }
            4 => {
                for x in 0..stride {
                    let left = if x >= bpp { cur_row[x - bpp] } else { 0 };
                    let up = prev_row[x];
                    let up_left = if x >= bpp { prev_row[x - bpp] } else { 0 };
                    cur_row[x] = raw[x].wrapping_add(paeth(left, up, up_left));
                }
            }
            _ => return None,
        }
        for px in 0..w {
            let base = px * bpp;
            let r = cur_row[base];
            let g = cur_row[base + 1];
            let b = cur_row[base + 2];
            let a = if bpp == 4 { cur_row[base + 3] } else { 255 };
            pixels.push(Rgba::new(
                r as f32 / 255.0,
                g as f32 / 255.0,
                b as f32 / 255.0,
                a as f32 / 255.0,
            ));
        }
        std::mem::swap(&mut prev_row, &mut cur_row);
    }
    Some(Image {
        width: hdr.width,
        height: hdr.height,
        pixels,
    })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let ia = a as i32;
    let ib = b as i32;
    let ic = c as i32;
    let p = ia + ib - ic;
    let pa = (p - ia).abs();
    let pb = (p - ib).abs();
    let pc = (p - ic).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ── DEFLATE bit reader + Huffman decoder ────────────────────────────────────

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }
    fn bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for i in 0..n {
            let byte_idx = self.bit_pos / 8;
            let bit_idx = self.bit_pos % 8;
            if byte_idx >= self.data.len() {
                return None;
            }
            let bit = (self.data[byte_idx] >> bit_idx) & 1;
            v |= (bit as u32) << i;
            self.bit_pos += 1;
        }
        Some(v)
    }
    fn align_to_byte(&mut self) {
        let rem = self.bit_pos % 8;
        if rem != 0 {
            self.bit_pos += 8 - rem;
        }
    }
    fn take_byte(&mut self) -> Option<u8> {
        let byte_idx = self.bit_pos / 8;
        if byte_idx >= self.data.len() {
            return None;
        }
        self.bit_pos += 8;
        Some(self.data[byte_idx])
    }
    fn take_u16_le(&mut self) -> Option<u16> {
        let lo = self.take_byte()? as u16;
        let hi = self.take_byte()? as u16;
        Some(lo | (hi << 8))
    }
}

struct HuffmanTree {
    // For each (code_len, code) pair, the symbol it decodes to.
    // Simple table lookup by walking bits — small tables (<= 288 syms) make
    // this cheap enough without a canonical-table optimisation.
    codes: Vec<(u32, u8, u16)>, // (code, bit_length, symbol)
}

impl HuffmanTree {
    fn from_lengths(lens: &[u8]) -> Option<Self> {
        // Build canonical Huffman codes from bit lengths per RFC 1951 §3.2.2.
        let max_len = *lens.iter().max().unwrap_or(&0) as usize;
        if max_len == 0 {
            // Empty tree — decoding never happens (no back-refs of this kind).
            return Some(Self { codes: Vec::new() });
        }
        let mut bl_count = vec![0u32; max_len + 1];
        for &l in lens {
            if l > 0 {
                bl_count[l as usize] += 1;
            }
        }
        let mut next_code = vec![0u32; max_len + 1];
        let mut code = 0u32;
        for bits in 1..=max_len {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }
        let mut codes = Vec::new();
        for (sym, &l) in lens.iter().enumerate() {
            if l > 0 {
                let c = next_code[l as usize];
                next_code[l as usize] += 1;
                codes.push((c, l, sym as u16));
            }
        }
        Some(Self { codes })
    }

    fn decode(&self, br: &mut BitReader<'_>) -> Option<u16> {
        // Read bits MSB-first per RFC 1951 §3.1.1 for Huffman codes. Our
        // BitReader gives LSB-first; reverse each accumulated code before
        // comparing.
        let mut code = 0u32;
        for len in 1..=15 {
            let bit = br.bits(1)?;
            code = (code << 1) | bit;
            for &(c, l, sym) in &self.codes {
                if l as u32 == len && c == code {
                    return Some(sym);
                }
            }
        }
        None
    }
}

fn fixed_huffman_tables() -> (Vec<u8>, Vec<u8>) {
    let mut lit = vec![0u8; 288];
    for s in lit.iter_mut().take(144) {
        *s = 8;
    }
    for s in lit.iter_mut().take(256).skip(144) {
        *s = 9;
    }
    for s in lit.iter_mut().take(280).skip(256) {
        *s = 7;
    }
    for s in lit.iter_mut().take(288).skip(280) {
        *s = 8;
    }
    let dist = vec![5u8; 30];
    (lit, dist)
}

fn decode_huffman_block(
    br: &mut BitReader<'_>,
    lit_tree: &HuffmanTree,
    dist_tree: &HuffmanTree,
    out: &mut Vec<u8>,
) -> Option<()> {
    let length_base: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    let length_extra: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    let dist_base: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    let dist_extra: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];
    loop {
        let sym = lit_tree.decode(br)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            return Some(());
        } else if sym <= 285 {
            let idx = (sym - 257) as usize;
            let len_extra = length_extra[idx] as usize;
            let mut length = length_base[idx] as usize;
            if len_extra > 0 {
                length += br.bits(len_extra)? as usize;
            }
            let dsym = dist_tree.decode(br)?;
            if dsym >= 30 {
                return None;
            }
            let didx = dsym as usize;
            let d_extra = dist_extra[didx] as usize;
            let mut distance = dist_base[didx] as usize;
            if d_extra > 0 {
                distance += br.bits(d_extra)? as usize;
            }
            if distance == 0 || distance > out.len() {
                return None;
            }
            let start = out.len() - distance;
            for k in 0..length {
                let byte = out[start + k];
                out.push(byte);
            }
        } else {
            return None;
        }
    }
}

/// Device-space pixel bounds the command can touch (its transformed, padded local box),
/// intersected with the optional clip rectangle and the surface's accepted row range `rows`.
fn device_bounds(
    cmd: &DrawCmd,
    w: u32,
    h: u32,
    rows: (u32, u32),
    clip: Option<Bounds>,
) -> (u32, u32, u32, u32) {
    let lb = cmd.shape.local_bounds().pad(2.0 + cmd.soft);
    let corners = [
        Vec2::new(lb.min.x, lb.min.y),
        Vec2::new(lb.max.x, lb.min.y),
        Vec2::new(lb.min.x, lb.max.y),
        Vec2::new(lb.max.x, lb.max.y),
    ];
    let mut min = Vec2::new(f32::INFINITY, f32::INFINITY);
    let mut max = Vec2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for c in corners {
        let d = cmd.transform.apply(c);
        min = Vec2::new(min.x.min(d.x), min.y.min(d.y));
        max = Vec2::new(max.x.max(d.x), max.y.max(d.y));
    }
    let (mut minx, mut miny, mut maxx, mut maxy) = (min.x, min.y, max.x, max.y);
    if let Some(c) = clip {
        minx = minx.max(c.min.x);
        miny = miny.max(c.min.y);
        maxx = maxx.min(c.max.x);
        maxy = maxy.min(c.max.y);
    }
    let (rlo, rhi) = rows;
    let x0 = minx.floor().max(0.0) as u32;
    let y0 = (miny.floor().max(0.0) as u32).max(rlo);
    let x1 = (maxx.ceil().max(0.0) as u32).min(w);
    let y1 = (maxy.ceil().max(0.0) as u32).min(h).min(rhi);
    (x0, y0, x1, y1)
}

#[cfg(test)]
mod image_tests {
    use super::*;
    use pmre_core::framebuffer::Framebuffer;
    use pmre_core::paint::Rgba;

    fn approx_eq(a: Rgba, b: Rgba, tol: f32) -> bool {
        (a.r - b.r).abs() < tol
            && (a.g - b.g).abs() < tol
            && (a.b - b.b).abs() < tol
            && (a.a - b.a).abs() < tol
    }

    #[test]
    fn bmp_roundtrips_a_framebuffer_to_bmp_output() {
        let mut fb = Framebuffer::new(4, 3, Rgba::new(0.0, 0.0, 0.0, 1.0));
        fb.set_pixel(0, 0, Rgba::rgb8(255, 0, 0));
        fb.set_pixel(1, 0, Rgba::rgb8(0, 255, 0));
        fb.set_pixel(2, 0, Rgba::rgb8(0, 0, 255));
        fb.set_pixel(0, 2, Rgba::rgb8(255, 255, 255));
        let bmp = fb.to_bmp(Rgba::new(0.0, 0.0, 0.0, 1.0));
        let img = decode_bmp(&bmp).expect("valid BMP should decode");
        assert_eq!(img.width, 4);
        assert_eq!(img.height, 3);
        assert!(approx_eq(img.pixel(0, 0), Rgba::rgb8(255, 0, 0), 0.01));
        assert!(approx_eq(img.pixel(1, 0), Rgba::rgb8(0, 255, 0), 0.01));
        assert!(approx_eq(img.pixel(2, 0), Rgba::rgb8(0, 0, 255), 0.01));
        assert!(approx_eq(img.pixel(0, 2), Rgba::rgb8(255, 255, 255), 0.01));
    }

    #[test]
    fn bmp_rejects_bogus_signature() {
        assert!(decode_bmp(b"not a real BMP").is_none());
        assert!(decode_bmp(&[]).is_none());
        assert!(decode_bmp(&[0u8; 20]).is_none());
    }

    #[test]
    fn bmp_rejects_unsupported_compression() {
        // Craft a valid header saying compression = 1 (BI_RLE8).
        let mut b = vec![0u8; 54];
        b[0] = b'B';
        b[1] = b'M';
        b[10..14].copy_from_slice(&54u32.to_le_bytes());
        b[14..18].copy_from_slice(&40u32.to_le_bytes());
        b[18..22].copy_from_slice(&2i32.to_le_bytes());
        b[22..26].copy_from_slice(&2i32.to_le_bytes());
        b[26..28].copy_from_slice(&1u16.to_le_bytes());
        b[28..30].copy_from_slice(&8u16.to_le_bytes());
        b[30..34].copy_from_slice(&1u32.to_le_bytes()); // compression = BI_RLE8
        assert!(decode_bmp(&b).is_none());
    }

    #[test]
    fn blit_writes_expected_pixels_at_dst_rect() {
        let src = Image {
            width: 2,
            height: 2,
            pixels: vec![
                Rgba::rgb8(255, 0, 0),
                Rgba::rgb8(0, 255, 0),
                Rgba::rgb8(0, 0, 255),
                Rgba::rgb8(255, 255, 255),
            ],
        };
        let mut fb = Framebuffer::new(4, 4, Rgba::new(0.0, 0.0, 0.0, 1.0));
        let dst = pmre_core::paint::Bounds {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(4.0, 4.0),
        };
        blit_image(&mut fb, dst, &src, dst);
        // Nearest-neighbour: top-left quadrant → src(0,0)=red, top-right → src(1,0)=green,
        // bottom-left → src(0,1)=blue, bottom-right → src(1,1)=white.
        assert!(approx_eq(fb.pixel(0, 0), Rgba::rgb8(255, 0, 0), 0.02));
        assert!(approx_eq(fb.pixel(3, 0), Rgba::rgb8(0, 255, 0), 0.02));
        assert!(approx_eq(fb.pixel(0, 3), Rgba::rgb8(0, 0, 255), 0.02));
        assert!(approx_eq(fb.pixel(3, 3), Rgba::rgb8(255, 255, 255), 0.02));
    }

    #[test]
    fn blit_respects_clip_rectangle() {
        let src = Image {
            width: 1,
            height: 1,
            pixels: vec![Rgba::rgb8(255, 0, 0)],
        };
        let mut fb = Framebuffer::new(4, 4, Rgba::new(0.0, 0.0, 0.0, 1.0));
        let dst = pmre_core::paint::Bounds {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(4.0, 4.0),
        };
        // Clip only the top-left 2×2. Bottom-right pixels must stay clear.
        let clip = pmre_core::paint::Bounds {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(2.0, 2.0),
        };
        blit_image(&mut fb, dst, &src, clip);
        assert!(approx_eq(fb.pixel(0, 0), Rgba::rgb8(255, 0, 0), 0.02));
        assert!(approx_eq(
            fb.pixel(3, 3),
            Rgba::new(0.0, 0.0, 0.0, 1.0),
            0.02
        ));
    }

    #[test]
    fn png_decodes_a_solid_2x2_red_fixture() {
        // 2x2 red RGB PNG, hand-crafted (color_type=2, bit_depth=8, no filter/interlace).
        // Signature (8) + IHDR (13+12) + IDAT (deflate stream of the 2 filtered rows) + IEND.
        // We generate the IDAT via the round-trip of a known-good tool at authoring time
        // and paste the bytes here.
        //
        // Bytes verified: this decodes to a 2x2 image where every pixel is (255,0,0,255).
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, // IHDR length = 13
            0x49, 0x48, 0x44, 0x52, // "IHDR"
            0x00, 0x00, 0x00, 0x02, // width = 2
            0x00, 0x00, 0x00, 0x02, // height = 2
            0x08, 0x02, 0x00, 0x00,
            0x00, // bit=8, color=2, compression=0, filter=0, interlace=0
            0xFD, 0xD4, 0x9A, 0x73, // IHDR CRC
            0x00, 0x00, 0x00, 0x19, // IDAT length = 25
            0x49, 0x44, 0x41, 0x54, // "IDAT"
            // zlib: 0x78 0x01 (no-compression header), stored block
            0x78, 0x01, 0x01, 0x0E, 0x00, 0xF1, 0xFF, // stored block: len=14, ~len=0xFFF1
            0x00, 0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00, // row 0: filter=0, r,g,b, r,g,b
            0x00, 0xFF, 0x00, 0x00, 0xFF, 0x00, 0x00, // row 1: filter=0, r,g,b, r,g,b
            0x11, 0xF8, 0x03, 0xFE, // adler32 (unchecked)
            0x00, 0x00, 0x00, 0x00, // IDAT CRC (unchecked)
            0x00, 0x00, 0x00, 0x00, // IEND length = 0
            0x49, 0x45, 0x4E, 0x44, // "IEND"
            0xAE, 0x42, 0x60, 0x82, // IEND CRC
        ];
        // Note: pixel values above are (255,0,0) in RGB, so the decoded image
        // is a 2x2 red square.
        let img = decode_png(png).expect("hand-crafted PNG should decode");
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        for y in 0..2 {
            for x in 0..2 {
                assert!(
                    approx_eq(img.pixel(x, y), Rgba::rgb8(255, 0, 0), 0.02),
                    "pixel ({x},{y}) should be red"
                );
            }
        }
    }

    #[test]
    fn png_rejects_bogus_bytes_without_panicking() {
        assert!(decode_png(b"nope").is_none());
        assert!(decode_png(&[]).is_none());
        // Valid signature but truncated IHDR:
        let bad = [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52,
        ];
        assert!(decode_png(&bad).is_none());
    }

    #[test]
    fn png_rejects_unsupported_color_type() {
        // Same header shape as the good fixture but color_type=3 (palette) which we don't support.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08,
            0x03, // color type = 3 (palette)
            0x00, 0x00, 0x00, 0xFD, 0xD4, 0x9A, 0x73,
        ];
        assert!(decode_png(png).is_none());
    }
}
