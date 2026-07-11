//! Font mechanism, two tiers, zero dependencies:
//!
//! 1. **Vector tier** — a TrueType (`.ttf`/`.ttc`) parser + anti-aliased glyph rasterizer.
//!    At startup the kit looks for a system font file (Segoe UI on Windows, DejaVu /
//!    Liberation on Linux, Arial/Helvetica on macOS — override with `PMRE_FONT` /
//!    `PMRE_FONT_BOLD`), reads it with `std::fs`, and rasterizes glyph outlines with an
//!    accumulation-buffer coverage rasterizer (the `fold` atom over signed edge areas).
//!    No crates: the parser and rasterizer are ~pure math over a byte slice.
//! 2. **Bitmap tier** — the original dependency-free 5×7 pixel font, kept verbatim as the
//!    fallback when no font file can be found. `crate::text` picks the tier.
//!
//! Mechanism only: this module turns `(char, size)` into coverage bitmaps and metrics.
//! It never decides colors, positions, or layout.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

// ─────────────────────────────────────────────────────────────────────────────
// Vector tier: TrueType parsing
// ─────────────────────────────────────────────────────────────────────────────

/// A rasterized glyph: an alpha-coverage bitmap positioned relative to the pen.
/// `left`/`top` offset the bitmap from `(pen_x, baseline_y)`; `top` is negative for
/// glyphs that rise above the baseline (almost all of them).
pub struct RasterGlyph {
    pub w: i32,
    pub h: i32,
    pub left: i32,
    pub top: i32,
    pub cov: Vec<u8>,
}

/// A parsed TrueType font plus its glyph caches. Cheap metric queries, cached rasters.
pub struct Font {
    data: Vec<u8>,
    upem: f32,
    loca_long: bool,
    num_glyphs: u16,
    glyf: usize,
    glyf_len: usize,
    loca: usize,
    cmap_sub: usize,
    cmap_format: u16,
    hmtx: usize,
    num_h_metrics: u16,
    ascent_units: f32,
    descent_units: f32, // positive magnitude below the baseline
    line_gap_units: f32,
    gids: Mutex<HashMap<char, u16>>,
    rasters: Mutex<HashMap<(u16, u16), Arc<RasterGlyph>>>,
}

// -- bounds-checked big-endian reads --------------------------------------------------

fn rd_u8(d: &[u8], o: usize) -> Option<u8> {
    d.get(o).copied()
}
fn rd_u16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(o)?, *d.get(o + 1)?]))
}
fn rd_i16(d: &[u8], o: usize) -> Option<i16> {
    rd_u16(d, o).map(|v| v as i16)
}
fn rd_u32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *d.get(o)?,
        *d.get(o + 1)?,
        *d.get(o + 2)?,
        *d.get(o + 3)?,
    ]))
}

impl Font {
    /// Parse a font from raw file bytes. Returns `None` on any structural problem —
    /// the caller falls back to the bitmap tier, never panics.
    pub fn parse(data: Vec<u8>) -> Option<Font> {
        let d = &data[..];
        // TrueType collection: use the first face.
        let mut base = 0usize;
        if rd_u32(d, 0)? == u32::from_be_bytes(*b"ttcf") {
            base = rd_u32(d, 12)? as usize;
        }
        let version = rd_u32(d, base)?;
        if version != 0x0001_0000 && version != u32::from_be_bytes(*b"true") {
            return None; // CFF (`OTTO`) outlines are out of scope for the reduced parser
        }
        let num_tables = rd_u16(d, base + 4)? as usize;
        let find = |tag: &[u8; 4]| -> Option<(usize, usize)> {
            for i in 0..num_tables {
                let rec = base + 12 + i * 16;
                if d.get(rec..rec + 4)? == tag {
                    return Some((rd_u32(d, rec + 8)? as usize, rd_u32(d, rec + 12)? as usize));
                }
            }
            None
        };
        let (head, _) = find(b"head")?;
        let (maxp, _) = find(b"maxp")?;
        let (cmap, _) = find(b"cmap")?;
        let (loca, _) = find(b"loca")?;
        let (glyf, glyf_len) = find(b"glyf")?;
        let (hhea, _) = find(b"hhea")?;
        let (hmtx, _) = find(b"hmtx")?;

        let upem = rd_u16(d, head + 18)? as f32;
        let loca_long = rd_i16(d, head + 50)? != 0;
        let num_glyphs = rd_u16(d, maxp + 4)?;
        let ascent_units = rd_i16(d, hhea + 4)? as f32;
        let descent_units = -(rd_i16(d, hhea + 6)? as f32); // stored negative in hhea
        let line_gap_units = rd_i16(d, hhea + 8)? as f32;
        let num_h_metrics = rd_u16(d, hhea + 34)?;

        let (cmap_sub, cmap_format) = pick_cmap(d, cmap)?;
        if upem < 16.0 {
            return None; // spec minimum is 16; smaller values explode the px scale
        }
        Some(Font {
            data,
            upem,
            loca_long,
            num_glyphs,
            glyf,
            glyf_len,
            loca,
            cmap_sub,
            cmap_format,
            hmtx,
            num_h_metrics,
            ascent_units,
            descent_units,
            line_gap_units,
            gids: Mutex::new(HashMap::new()),
            rasters: Mutex::new(HashMap::new()),
        })
    }

    /// Load and parse a font file.
    pub fn load(path: &std::path::Path) -> Option<Font> {
        Font::parse(std::fs::read(path).ok()?)
    }

    // -- metrics ----------------------------------------------------------------------

    /// Distance from baseline up to the em-box top, in px at `size`.
    pub fn ascent(&self, size: f32) -> f32 {
        self.ascent_units / self.upem * size
    }
    /// Distance from baseline down to the em-box bottom (positive), in px at `size`.
    pub fn descent(&self, size: f32) -> f32 {
        self.descent_units / self.upem * size
    }
    /// The font's natural line height in px at `size`.
    pub fn line_height(&self, size: f32) -> f32 {
        (self.ascent_units + self.descent_units + self.line_gap_units) / self.upem * size
    }

    /// Horizontal advance of `ch` in px at `size`.
    pub fn advance(&self, ch: char, size: f32) -> f32 {
        let gid = self.glyph_id(ch);
        self.advance_units(gid) / self.upem * size
    }

    fn advance_units(&self, gid: u16) -> f32 {
        let d = &self.data[..];
        let idx = gid.min(self.num_h_metrics.saturating_sub(1)) as usize;
        rd_u16(d, self.hmtx + idx * 4).unwrap_or(0) as f32
    }

    /// The glyph index for `ch` (0 = `.notdef`), cached per char.
    pub fn glyph_id(&self, ch: char) -> u16 {
        if let Some(&g) = self.gids.lock().unwrap().get(&ch) {
            return g;
        }
        let g = self.lookup_gid(ch as u32).unwrap_or(0);
        self.gids.lock().unwrap().insert(ch, g);
        g
    }

    fn lookup_gid(&self, c: u32) -> Option<u16> {
        let d = &self.data[..];
        let sub = self.cmap_sub;
        match self.cmap_format {
            4 => {
                if c > 0xFFFF {
                    return Some(0);
                }
                let seg_x2 = rd_u16(d, sub + 6)? as usize;
                let ends = sub + 14;
                let starts = ends + seg_x2 + 2;
                let deltas = starts + seg_x2;
                let ranges = deltas + seg_x2;
                let segs = seg_x2 / 2;
                // binary search for the first segment whose endCode >= c
                let (mut lo, mut hi) = (0usize, segs);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if (rd_u16(d, ends + mid * 2)? as u32) < c {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                if lo >= segs {
                    return Some(0);
                }
                let start = rd_u16(d, starts + lo * 2)? as u32;
                if c < start {
                    return Some(0);
                }
                let delta = rd_u16(d, deltas + lo * 2)?;
                let range_off = rd_u16(d, ranges + lo * 2)? as usize;
                if range_off == 0 {
                    return Some((c as u16).wrapping_add(delta));
                }
                let addr = ranges + lo * 2 + range_off + 2 * (c - start) as usize;
                let g = rd_u16(d, addr)?;
                if g == 0 {
                    Some(0)
                } else {
                    Some(g.wrapping_add(delta))
                }
            }
            12 => {
                let n = rd_u32(d, sub + 12)? as usize;
                let (mut lo, mut hi) = (0usize, n);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let rec = sub + 16 + mid * 12;
                    if rd_u32(d, rec + 4)? < c {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                if lo >= n {
                    return Some(0);
                }
                let rec = sub + 16 + lo * 12;
                let start = rd_u32(d, rec)?;
                if c < start {
                    return Some(0);
                }
                // checked add + range check: a malformed startGlyphID must map to
                // .notdef, not overflow or alias into a random glyph
                let g = rd_u32(d, rec + 8)?.checked_add(c - start)?;
                if g > u16::MAX as u32 {
                    return Some(0);
                }
                Some(g as u16)
            }
            _ => Some(0),
        }
    }

    // -- outlines ---------------------------------------------------------------------

    fn glyph_range(&self, gid: u16) -> Option<(usize, usize)> {
        if gid >= self.num_glyphs {
            return None;
        }
        let d = &self.data[..];
        let (a, b) = if self.loca_long {
            (
                rd_u32(d, self.loca + gid as usize * 4)? as usize,
                rd_u32(d, self.loca + gid as usize * 4 + 4)? as usize,
            )
        } else {
            (
                rd_u16(d, self.loca + gid as usize * 2)? as usize * 2,
                rd_u16(d, self.loca + gid as usize * 2 + 2)? as usize * 2,
            )
        };
        if b <= a || b > self.glyf_len {
            return None; // empty glyph (e.g. space)
        }
        Some((self.glyf + a, self.glyf + b))
    }

    /// Collect the glyph's contours as flattened polylines in **pixel space, y-down**,
    /// relative to `(pen, baseline)`. `xf` maps font units → font units (composites).
    fn contours(
        &self,
        gid: u16,
        scale: f32,
        xf: [f32; 6],
        depth: u8,
        out: &mut Vec<Vec<(f32, f32)>>,
    ) {
        if depth > 4 {
            return;
        }
        let Some((go, _end)) = self.glyph_range(gid) else {
            return;
        };
        let d = &self.data[..];
        let Some(n_contours) = rd_i16(d, go) else {
            return;
        };
        if n_contours >= 0 {
            self.simple_contours(go, n_contours as usize, scale, xf, out);
        } else {
            self.composite_contours(go, scale, xf, depth, out);
        }
    }

    fn simple_contours(
        &self,
        go: usize,
        nc: usize,
        scale: f32,
        xf: [f32; 6],
        out: &mut Vec<Vec<(f32, f32)>>,
    ) {
        let d = &self.data[..];
        let mut ends = Vec::with_capacity(nc);
        for i in 0..nc {
            let Some(e) = rd_u16(d, go + 10 + i * 2) else {
                return;
            };
            ends.push(e as usize);
        }
        let n_pts = match ends.last() {
            Some(&e) => e + 1,
            None => return,
        };
        let Some(ins_len) = rd_u16(d, go + 10 + nc * 2) else {
            return;
        };
        let mut p = go + 12 + nc * 2 + ins_len as usize;

        // flags, with repeat runs
        let mut flags = Vec::with_capacity(n_pts);
        while flags.len() < n_pts {
            let Some(f) = rd_u8(d, p) else { return };
            p += 1;
            flags.push(f);
            if f & 0x08 != 0 {
                let Some(rep) = rd_u8(d, p) else { return };
                p += 1;
                for _ in 0..rep {
                    if flags.len() < n_pts {
                        flags.push(f);
                    }
                }
            }
        }
        // x coordinates (deltas)
        let mut xs = Vec::with_capacity(n_pts);
        let mut x = 0i32;
        for &f in &flags {
            if f & 0x02 != 0 {
                let Some(v) = rd_u8(d, p) else { return };
                p += 1;
                x += if f & 0x10 != 0 { v as i32 } else { -(v as i32) };
            } else if f & 0x10 == 0 {
                let Some(v) = rd_i16(d, p) else { return };
                p += 2;
                x += v as i32;
            }
            xs.push(x as f32);
        }
        // y coordinates (deltas)
        let mut ys = Vec::with_capacity(n_pts);
        let mut y = 0i32;
        for &f in &flags {
            if f & 0x04 != 0 {
                let Some(v) = rd_u8(d, p) else { return };
                p += 1;
                y += if f & 0x20 != 0 { v as i32 } else { -(v as i32) };
            } else if f & 0x20 == 0 {
                let Some(v) = rd_i16(d, p) else { return };
                p += 2;
                y += v as i32;
            }
            ys.push(y as f32);
        }

        // transform to pixel space (y-down)
        let to_px = |i: usize| -> (f32, f32) {
            let (ux, uy) = (xs[i], ys[i]);
            let tx = xf[0] * ux + xf[2] * uy + xf[4];
            let ty = xf[1] * ux + xf[3] * uy + xf[5];
            (tx * scale, -ty * scale)
        };
        let on = |i: usize| flags[i] & 0x01 != 0;

        let mut start = 0usize;
        for &end in &ends {
            if end >= n_pts || end < start {
                return;
            }
            let count = end - start + 1;
            if count < 2 {
                start = end + 1;
                continue;
            }
            let idx = |k: usize| start + (k % count);
            // find an on-curve starting point, or synthesize one between two off points
            let mut poly: Vec<(f32, f32)> = Vec::with_capacity(count * 4);
            let first_on = (0..count).find(|&k| on(idx(k)));
            let (start_pt, k0) = match first_on {
                Some(k) => (to_px(idx(k)), k),
                None => {
                    let a = to_px(idx(0));
                    let b = to_px(idx(1));
                    (mid(a, b), 0)
                }
            };
            poly.push(start_pt);
            let mut prev = start_pt;
            let mut pending_ctrl: Option<(f32, f32)> = None;
            for step in 1..=count {
                let i = idx(k0 + step);
                let pt = to_px(i);
                if on(i) {
                    match pending_ctrl.take() {
                        Some(c) => flatten_quad(prev, c, pt, &mut poly),
                        None => poly.push(pt),
                    }
                    prev = pt;
                } else {
                    if let Some(c) = pending_ctrl.take() {
                        let m = mid(c, pt);
                        flatten_quad(prev, c, m, &mut poly);
                        prev = m;
                    }
                    pending_ctrl = Some(pt);
                }
            }
            if let Some(c) = pending_ctrl.take() {
                flatten_quad(prev, c, start_pt, &mut poly);
            }
            out.push(poly);
            start = end + 1;
        }
    }

    fn composite_contours(
        &self,
        go: usize,
        scale: f32,
        xf: [f32; 6],
        depth: u8,
        out: &mut Vec<Vec<(f32, f32)>>,
    ) {
        let d = &self.data[..];
        let mut p = go + 10;
        loop {
            let Some(flags) = rd_u16(d, p) else { return };
            let Some(child) = rd_u16(d, p + 2) else {
                return;
            };
            p += 4;
            let (dx, dy) = if flags & 0x0001 != 0 {
                let a = rd_i16(d, p).unwrap_or(0) as f32;
                let b = rd_i16(d, p + 2).unwrap_or(0) as f32;
                p += 4;
                (a, b)
            } else {
                let a = rd_u8(d, p).unwrap_or(0) as i8 as f32;
                let b = rd_u8(d, p + 1).unwrap_or(0) as i8 as f32;
                p += 2;
                (a, b)
            };
            // ARGS_ARE_XY_VALUES; point-matching args are treated as no offset
            let (dx, dy) = if flags & 0x0002 != 0 {
                (dx, dy)
            } else {
                (0.0, 0.0)
            };
            let f2 = |o: usize| rd_i16(d, o).unwrap_or(0) as f32 / 16384.0;
            let (a, b, c, dd) = if flags & 0x0008 != 0 {
                let s = f2(p);
                p += 2;
                (s, 0.0, 0.0, s)
            } else if flags & 0x0040 != 0 {
                let sx = f2(p);
                let sy = f2(p + 2);
                p += 4;
                (sx, 0.0, 0.0, sy)
            } else if flags & 0x0080 != 0 {
                let m = (f2(p), f2(p + 2), f2(p + 4), f2(p + 6));
                p += 8;
                (m.0, m.1, m.2, m.3)
            } else {
                (1.0, 0.0, 0.0, 1.0)
            };
            // child transform composed under the parent transform
            let child_xf = [
                xf[0] * a + xf[2] * b,
                xf[1] * a + xf[3] * b,
                xf[0] * c + xf[2] * dd,
                xf[1] * c + xf[3] * dd,
                xf[0] * dx + xf[2] * dy + xf[4],
                xf[1] * dx + xf[3] * dy + xf[5],
            ];
            self.contours(child, scale, child_xf, depth + 1, out);
            if flags & 0x0020 == 0 {
                return;
            }
        }
    }

    // -- rasterization ----------------------------------------------------------------

    /// Rasterize (and cache) the glyph for `ch` at `size` px. Size is quantized to
    /// quarter pixels for the cache key, so nearby sizes share bitmaps.
    pub fn raster(&self, ch: char, size: f32) -> Arc<RasterGlyph> {
        let gid = self.glyph_id(ch);
        let q = (size * 4.0).round().clamp(1.0, u16::MAX as f32) as u16;
        if let Some(g) = self.rasters.lock().unwrap().get(&(gid, q)) {
            return g.clone();
        }
        let g = Arc::new(self.rasterize(gid, q as f32 / 4.0));
        self.rasters.lock().unwrap().insert((gid, q), g.clone());
        g
    }

    fn rasterize(&self, gid: u16, size: f32) -> RasterGlyph {
        let scale = size / self.upem;
        let mut contours: Vec<Vec<(f32, f32)>> = Vec::new();
        self.contours(gid, scale, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], 0, &mut contours);

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for c in &contours {
            for &(x, y) in c {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
        // Reject unrasterizable outlines: empty glyphs, and malformed fonts whose
        // coordinates would demand an absurd bitmap (the module contract is "malformed
        // input degrades, never panics or aborts"). 4096px covers any sane UI glyph.
        const MAX_GLYPH_PX: f32 = 4096.0;
        let empty = RasterGlyph {
            w: 0,
            h: 0,
            left: 0,
            top: 0,
            cov: Vec::new(),
        };
        if !min_x.is_finite()
            || !min_y.is_finite()
            || !max_x.is_finite()
            || !max_y.is_finite()
            || max_x - min_x > MAX_GLYPH_PX
            || max_y - min_y > MAX_GLYPH_PX
            || min_x.abs().max(max_x.abs()) > 1e7
            || min_y.abs().max(max_y.abs()) > 1e7
        {
            return empty;
        }
        let left = min_x.floor() as i32 - 1;
        let top = min_y.floor() as i32 - 1;
        let w = (max_x.ceil() as i32 - left + 2).max(1);
        let h = (max_y.ceil() as i32 - top + 2).max(1);
        let (wu, hu) = (w as usize, h as usize);

        // Signed-area accumulation buffer (one extra column absorbs edge spill).
        let bw = wu + 1;
        let mut acc = vec![0.0f32; bw * hu];
        for c in &contours {
            let n = c.len();
            for i in 0..n {
                let p0 = c[i];
                let p1 = c[(i + 1) % n];
                accumulate_line(
                    &mut acc,
                    bw,
                    hu,
                    (p0.0 - left as f32, p0.1 - top as f32),
                    (p1.0 - left as f32, p1.1 - top as f32),
                );
            }
        }
        let mut cov = vec![0u8; wu * hu];
        for row in 0..hu {
            let mut sum = 0.0f32;
            for col in 0..wu {
                sum += acc[row * bw + col];
                cov[row * wu + col] = (sum.abs().min(1.0) * 255.0 + 0.5) as u8;
            }
        }
        RasterGlyph {
            w,
            h,
            left,
            top,
            cov,
        }
    }
}

fn mid(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    ((a.0 + b.0) * 0.5, (a.1 + b.1) * 0.5)
}

/// Flatten one quadratic Bézier into `poly` (endpoint included, start point not).
fn flatten_quad(p0: (f32, f32), c: (f32, f32), p1: (f32, f32), poly: &mut Vec<(f32, f32)>) {
    // Subdivision count from the control point's deviation — enough for sub-pixel error.
    let dev = ((c.0 - (p0.0 + p1.0) * 0.5).abs() + (c.1 - (p0.1 + p1.1) * 0.5).abs()).sqrt();
    let n = (dev * 2.0).ceil().clamp(2.0, 16.0) as usize;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        poly.push((
            u * u * p0.0 + 2.0 * u * t * c.0 + t * t * p1.0,
            u * u * p0.1 + 2.0 * u * t * c.1 + t * t * p1.1,
        ));
    }
}

/// Accumulate the signed-area contribution of one line segment into `acc` (row-major,
/// `bw` floats per row). Rows then prefix-sum to exact analytic coverage — the classic
/// font-rs accumulation algorithm, with bounds-safe writes.
pub(crate) fn accumulate_line(
    acc: &mut [f32],
    bw: usize,
    h: usize,
    p0: (f32, f32),
    p1: (f32, f32),
) {
    if p0.1 == p1.1 {
        return; // horizontal edges contribute no winding
    }
    let (dir, top, bot) = if p0.1 < p1.1 {
        (1.0f32, p0, p1)
    } else {
        (-1.0f32, p1, p0)
    };
    let dxdy = (bot.0 - top.0) / (bot.1 - top.1);
    let y0 = (top.1.max(0.0)) as usize;
    let y1 = (bot.1.ceil().min(h as f32)) as usize;
    let mut x = top.0 + (y0 as f32 - top.1).max(0.0) * dxdy;
    let max_x = (bw - 1) as f32;

    for y in y0..y1 {
        let row = y * bw;
        let dy = ((y + 1) as f32).min(bot.1) - (y as f32).max(top.1);
        let xnext = x + dxdy * dy;
        if dy <= 0.0 {
            x = xnext;
            continue;
        }
        let d = dy * dir;
        let (mut sx0, mut sx1) = if x < xnext { (x, xnext) } else { (xnext, x) };
        sx0 = sx0.clamp(0.0, max_x);
        sx1 = sx1.clamp(0.0, max_x);
        let x0f = sx0.floor();
        let x0i = x0f as usize;
        let span = sx1 - sx0;
        if span <= 1e-6 || sx1 <= x0f + 1.0 {
            // the crossing stays inside one pixel column
            let xm = 0.5 * (sx0 + sx1) - x0f;
            bump(acc, row + x0i, d * (1.0 - xm));
            bump(acc, row + x0i + 1, d * xm);
        } else {
            // spread across columns proportionally to horizontal overlap
            let inv = 1.0 / span;
            let x1c = sx1.ceil();
            let x1i = (x1c as usize).min(bw - 1);
            let first = x0f + 1.0 - sx0; // horizontal extent inside the first column
            let a_first = 0.5 * first * first * inv;
            let last = sx1 - (x1c - 1.0); // extent inside the last column
            let a_last = 0.5 * last * last * inv;
            bump(acc, row + x0i, d * a_first);
            if x1i == x0i + 2 {
                bump(acc, row + x0i + 1, d * (1.0 - a_first - a_last));
            } else if x1i > x0i + 2 {
                let a1 = (1.5 - (sx0 - x0f)) * inv;
                bump(acc, row + x0i + 1, d * (a1 - a_first));
                for xi in x0i + 2..x1i - 1 {
                    bump(acc, row + xi, d * inv);
                }
                let a2 = a1 + (x1i - x0i - 3) as f32 * inv;
                bump(acc, row + x1i - 1, d * (1.0 - a2 - a_last));
            }
            bump(acc, row + x1i, d * a_last);
        }
        x = xnext;
    }
}

#[inline]
fn bump(acc: &mut [f32], i: usize, v: f32) {
    if let Some(slot) = acc.get_mut(i) {
        *slot += v;
    }
}

// -- cmap subtable selection -----------------------------------------------------------

fn pick_cmap(d: &[u8], cmap: usize) -> Option<(usize, u16)> {
    let n = rd_u16(d, cmap + 2)? as usize;
    let mut best: Option<(usize, u16, u32)> = None; // (offset, format, score)
    for i in 0..n {
        let rec = cmap + 4 + i * 8;
        let plat = rd_u16(d, rec)?;
        let enc = rd_u16(d, rec + 2)?;
        let off = cmap + rd_u32(d, rec + 4)? as usize;
        let format = rd_u16(d, off)?;
        let score = match (plat, enc, format) {
            (3, 10, 12) | (0, 4..=6, 12) => 3,
            (3, 1, 4) | (0, 0..=3, 4) => 2,
            (_, _, 4) => 1,
            (_, _, 12) => 1,
            _ => 0,
        };
        if score > 0 && best.map(|(_, _, s)| score > s).unwrap_or(true) {
            best = Some((off, format, score));
        }
    }
    best.map(|(off, format, _)| (off, format))
}

// ─────────────────────────────────────────────────────────────────────────────
// System font discovery (std::fs only — still zero crates)
// ─────────────────────────────────────────────────────────────────────────────

fn font_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(windir) = std::env::var("WINDIR") {
        dirs.push(std::path::PathBuf::from(windir).join("Fonts"));
    }
    dirs.push("C:/Windows/Fonts".into());
    dirs.push("/usr/share/fonts/truetype/dejavu".into());
    dirs.push("/usr/share/fonts/truetype/liberation".into());
    dirs.push("/usr/share/fonts/TTF".into());
    dirs.push("/System/Library/Fonts/Supplemental".into());
    dirs.push("/Library/Fonts".into());
    dirs
}

const REGULAR_CANDIDATES: &[&str] = &[
    "segoeui.ttf",
    "arial.ttf",
    "tahoma.ttf",
    "calibri.ttf",
    "verdana.ttf",
    "DejaVuSans.ttf",
    "LiberationSans-Regular.ttf",
    "Arial.ttf",
    "Arial Unicode.ttf",
];

const BOLD_CANDIDATES: &[&str] = &[
    "segoeuib.ttf",
    "arialbd.ttf",
    "tahomabd.ttf",
    "calibrib.ttf",
    "verdanab.ttf",
    "DejaVuSans-Bold.ttf",
    "LiberationSans-Bold.ttf",
    "Arial Bold.ttf",
];

fn load_first(env_override: &str, names: &[&str]) -> Option<Arc<Font>> {
    if let Ok(p) = std::env::var(env_override) {
        if let Some(f) = Font::load(std::path::Path::new(&p)) {
            return Some(Arc::new(f));
        }
    }
    for dir in font_dirs() {
        for name in names {
            let p = dir.join(name);
            if p.is_file() {
                if let Some(f) = Font::load(&p) {
                    return Some(Arc::new(f));
                }
            }
        }
    }
    None
}

static REGULAR: OnceLock<Option<Arc<Font>>> = OnceLock::new();
static BOLD: OnceLock<Option<Arc<Font>>> = OnceLock::new();

/// The system regular font, loaded once. `None` when no font file exists — callers
/// fall back to the 5×7 bitmap tier.
pub fn regular() -> Option<&'static Arc<Font>> {
    REGULAR
        .get_or_init(|| load_first("PMRE_FONT", REGULAR_CANDIDATES))
        .as_ref()
}

/// The system bold font; falls back to the regular face when no bold file exists.
pub fn bold() -> Option<&'static Arc<Font>> {
    BOLD.get_or_init(|| load_first("PMRE_FONT_BOLD", BOLD_CANDIDATES))
        .as_ref()
        .or_else(regular)
}

/// True when the vector tier is active (a system font file was found and parsed).
pub fn has_vector_font() -> bool {
    regular().is_some()
}

// ─────────────────────────────────────────────────────────────────────────────
// Bitmap tier: the original dependency-free 5×7 pixel font (fallback)
// ─────────────────────────────────────────────────────────────────────────────

/// The 7-row bitmap for a character (5 significant low bits per row).
pub fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        ' ' => [0; 7],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'B' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ],
        'C' => [
            0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
        ],
        'D' => [
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'F' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'G' => [
            0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ],
        'H' => [
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'J' => [
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ],
        'K' => [
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001,
        ],
        'O' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'Q' => [
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'U' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ],
        'V' => [
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ],
        'W' => [
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001,
        ],
        'X' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ],
        'Y' => [
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        'Z' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '1' => [
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        '2' => [
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ],
        '3' => [
            0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110,
        ],
        '4' => [
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '6' => [
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ],
        '7' => [
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        '9' => [
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ],
        '.' => [0, 0, 0, 0, 0, 0b00110, 0b00110],
        ',' => [0, 0, 0, 0, 0b00110, 0b00100, 0b01000],
        ':' => [0, 0b00110, 0b00110, 0, 0b00110, 0b00110, 0],
        '-' => [0, 0, 0, 0b01110, 0, 0, 0],
        '/' => [
            0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000,
        ],
        '!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100],
        '?' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100],
        '(' => [
            0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010,
        ],
        ')' => [
            0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000,
        ],
        '%' => [
            0b11001, 0b11010, 0b00100, 0b01000, 0b10011, 0b00011, 0b00000,
        ],
        '+' => [0, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill a 10×10 axis-aligned square from (2,2) to (8,8) and check the accumulation
    /// rasterizer produces full coverage inside and none outside.
    #[test]
    fn accumulation_fills_a_square() {
        let (w, h) = (12usize, 12usize);
        let bw = w + 1;
        let mut acc = vec![0.0f32; bw * h];
        let quad = [(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0)];
        for i in 0..4 {
            accumulate_line(&mut acc, bw, h, quad[i], quad[(i + 1) % 4]);
        }
        let cov_at = |x: usize, y: usize| -> f32 {
            let mut s = 0.0;
            for c in 0..=x {
                s += acc[y * bw + c];
            }
            s.abs().min(1.0)
        };
        assert!(cov_at(5, 5) > 0.99, "interior must be fully covered");
        assert!(cov_at(10, 5) < 0.01, "right of the square must be empty");
        assert!(cov_at(5, 10) < 0.01, "below the square must be empty");
        assert!(
            (cov_at(2, 5) - 1.0).abs() < 0.01 || cov_at(2, 5) > 0.9,
            "on-edge column mostly covered"
        );
    }

    /// Sub-pixel edges must produce fractional coverage (anti-aliasing).
    #[test]
    fn accumulation_antialiases_fractional_edges() {
        let (w, h) = (8usize, 8usize);
        let bw = w + 1;
        let mut acc = vec![0.0f32; bw * h];
        // square from x=2.5 to x=5.5 — half-covered boundary columns
        let quad = [(2.5, 2.0), (5.5, 2.0), (5.5, 6.0), (2.5, 6.0)];
        for i in 0..4 {
            accumulate_line(&mut acc, bw, h, quad[i], quad[(i + 1) % 4]);
        }
        let mut s = 0.0;
        let row = 4 * bw;
        let mut cov = vec![0.0f32; w];
        for c in 0..w {
            s += acc[row + c];
            cov[c] = s.abs().min(1.0);
        }
        assert!(
            (cov[2] - 0.5).abs() < 0.05,
            "left edge ~half covered: {}",
            cov[2]
        );
        assert!(cov[3] > 0.99 && cov[4] > 0.99, "interior full");
        assert!(
            (cov[5] - 0.5).abs() < 0.05,
            "right edge ~half covered: {}",
            cov[5]
        );
        assert!(cov[6] < 0.01, "outside empty");
    }

    /// If a system font exists on this machine, it must parse and produce sane
    /// metrics and non-empty glyph rasters. Skips silently when no font is present.
    #[test]
    fn system_font_parses_and_rasterizes() {
        let Some(font) = regular() else {
            return;
        };
        assert!(font.ascent(16.0) > 8.0 && font.ascent(16.0) < 24.0);
        assert!(font.descent(16.0) > 0.0 && font.descent(16.0) < 10.0);
        for ch in ['A', 'g', 'w', '0', '.', 'é'] {
            let adv = font.advance(ch, 16.0);
            assert!(adv > 0.0 && adv < 32.0, "advance of {ch:?} = {adv}");
        }
        let g = font.raster('A', 16.0);
        assert!(g.w > 2 && g.h > 6, "raster of A is {}x{}", g.w, g.h);
        assert!(
            g.cov.iter().any(|&c| c > 200),
            "raster of A must have solid coverage somewhere"
        );
        assert!(g.top < 0, "A rises above the baseline (top = {})", g.top);
        // lowercase actually differs from uppercase now
        let lower = font.raster('a', 16.0);
        assert!(lower.h < g.h, "lowercase a shorter than uppercase A");
    }
}
