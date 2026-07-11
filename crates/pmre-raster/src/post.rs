//! Post-processing passes. Mechanism only — no draw-order or scene decisions.
//!
//! Functions take raw pixel slabs (`&[Rgba]`, `width`, `height`) and are therefore
//! independent of `Framebuffer` internals; the public `bloom` entry point accepts
//! `&mut Framebuffer` as a convenience wrapper.
//!
//! Algorithm ported from MM3E (`mm3e-orchestrator/src/post.rs`):
//! bright-pass → horizontal Gaussian → vertical Gaussian → additive composite.

use pmre_core::fair_queue::FairQueue;
use pmre_core::framebuffer::Framebuffer;
use pmre_core::paint::Rgba;

// ── Helpers ──────────────────────────────────────────────────────────────────

fn luminance(c: Rgba) -> f32 {
    0.299 * c.r + 0.587 * c.g + 0.114 * c.b
}

fn bright_pass(pixels: &[Rgba], threshold: f32) -> Vec<Rgba> {
    pixels
        .iter()
        .map(|&c| {
            let l = luminance(c);
            if l > threshold {
                let s = (l - threshold) / l;
                Rgba::new(c.r * s, c.g * s, c.b * s, 1.0)
            } else {
                Rgba::new(0.0, 0.0, 0.0, 0.0)
            }
        })
        .collect()
}

/// Normalized 1-D Gaussian kernel: `kernel[0]` = center, `kernel[k]` = weight for
/// each neighbor at distance `k` (symmetric — applied to both sides in the blur pass).
fn gaussian_kernel(sigma: f32, radius: usize) -> Vec<f32> {
    let mut k: Vec<f32> = (0..=radius)
        .map(|i| (-(i as f32 * i as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let total = k[0] + 2.0 * k[1..].iter().sum::<f32>();
    for v in &mut k {
        *v /= total;
    }
    k
}

fn blur_h(src: &[Rgba], width: u32, height: u32, kernel: &[f32]) -> Vec<Rgba> {
    let w = width as i32;
    let h = height as i32;
    let mut out = vec![Rgba::new(0.0, 0.0, 0.0, 0.0); (width * height) as usize];
    for y in 0..h {
        for x in 0..w {
            let center = src[(y * w + x) as usize];
            let (mut rv, mut gv, mut bv) = (
                center.r * kernel[0],
                center.g * kernel[0],
                center.b * kernel[0],
            );
            for (ki, &wt) in kernel[1..].iter().enumerate() {
                let k = (ki + 1) as i32;
                let cl = src[(y * w + (x - k).max(0)) as usize];
                let cr = src[(y * w + (x + k).min(w - 1)) as usize];
                rv += (cl.r + cr.r) * wt;
                gv += (cl.g + cr.g) * wt;
                bv += (cl.b + cr.b) * wt;
            }
            out[(y * w + x) as usize] = Rgba::new(rv, gv, bv, 1.0);
        }
    }
    out
}

fn blur_v(src: &[Rgba], width: u32, height: u32, kernel: &[f32]) -> Vec<Rgba> {
    let w = width as i32;
    let h = height as i32;
    let mut out = vec![Rgba::new(0.0, 0.0, 0.0, 0.0); (width * height) as usize];
    for y in 0..h {
        for x in 0..w {
            let center = src[(y * w + x) as usize];
            let (mut rv, mut gv, mut bv) = (
                center.r * kernel[0],
                center.g * kernel[0],
                center.b * kernel[0],
            );
            for (ki, &wt) in kernel[1..].iter().enumerate() {
                let k = (ki + 1) as i32;
                let ct = src[((y - k).max(0) * w + x) as usize];
                let cb = src[((y + k).min(h - 1) * w + x) as usize];
                rv += (ct.r + cb.r) * wt;
                gv += (ct.g + cb.g) * wt;
                bv += (ct.b + cb.b) * wt;
            }
            out[(y * w + x) as usize] = Rgba::new(rv, gv, bv, 1.0);
        }
    }
    out
}

// ── Public API ────────────────────────────────────────────────────────────────

// ── Parallel helpers ──────────────────────────────────────────────────────────

/// Distribute `h` row indices across `n` threads via a `FairQueue`, calling
/// `f(row, &mut row_pixels)` for each row. Each row is processed by exactly
/// one thread, so writes to `buf` are non-overlapping.
///
/// # Safety
/// `buf` has length `w * h`. Each row index is enqueued exactly once, so
/// `ptr.add(row * w)` references a unique, non-overlapping `[Rgba; w]` slice.
fn par_rows<F>(buf: &mut [Rgba], w: usize, h: usize, n: usize, f: F)
where
    F: Fn(usize, &mut [Rgba]) + Send + Sync,
{
    let q = FairQueue::new();
    for row in 0..h {
        q.push(row);
    }
    q.seal();

    let ptr = buf.as_mut_ptr() as usize; // erase lifetime for Send; safety upheld above
    std::thread::scope(|scope| {
        for _ in 0..n {
            let q = q.clone();
            let f = &f;
            scope.spawn(move || {
                while let Some(row) = q.pop() {
                    // SAFETY: each row consumed by exactly one thread; ranges never overlap.
                    let row_slice = unsafe {
                        std::slice::from_raw_parts_mut((ptr as *mut Rgba).add(row * w), w)
                    };
                    f(row, row_slice);
                }
            });
        }
    });
}

fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Additive Gaussian bloom.
///
/// Pixels whose luminance exceeds `threshold` are extracted, blurred with a separable
/// Gaussian of the given `sigma` and `radius`, then additively composited back onto `fb`.
/// Channels are clamped to `[0, 1]` after addition.
pub fn bloom(fb: &mut Framebuffer, threshold: f32, sigma: f32, radius: usize) {
    let kernel = gaussian_kernel(sigma, radius);
    let bright = bright_pass(fb.pixels(), threshold);
    let h_blur = blur_h(&bright, fb.width, fb.height, &kernel);
    let v_blur = blur_v(&h_blur, fb.width, fb.height, &kernel);
    for y in 0..fb.height {
        for x in 0..fb.width {
            let b = v_blur[(y * fb.width + x) as usize];
            if b.r == 0.0 && b.g == 0.0 && b.b == 0.0 {
                continue;
            }
            let base = fb.pixel(x, y);
            fb.set_pixel(
                x,
                y,
                Rgba::new(
                    (base.r + b.r).min(1.0),
                    (base.g + b.g).min(1.0),
                    (base.b + b.b).min(1.0),
                    base.a,
                ),
            );
        }
    }
}

/// Parallel variant of `bloom` using `FairQueue` to distribute row work across
/// all available CPU threads. The three separable passes (bright, H-blur,
/// V-blur + composite) are each parallelised by row.
pub fn bloom_parallel(fb: &mut Framebuffer, threshold: f32, sigma: f32, radius: usize) {
    let n = thread_count();
    let w = fb.width as usize;
    let h = fb.height as usize;
    let kernel = gaussian_kernel(sigma, radius);
    let orig: Vec<Rgba> = fb.pixels().to_vec();

    // Pass 1: bright-pass (each output pixel depends only on the same input pixel)
    let mut bright = vec![Rgba::new(0.0, 0.0, 0.0, 0.0); w * h];
    par_rows(&mut bright, w, h, n, |row, out| {
        for x in 0..w {
            let src = orig[row * w + x];
            let l = luminance(src);
            if l > threshold {
                let s = (l - threshold) / l;
                out[x] = Rgba::new(src.r * s, src.g * s, src.b * s, 1.0);
            }
        }
    });

    // Pass 2: horizontal Gaussian (each output row depends only on the same row of bright)
    let mut hblur = vec![Rgba::new(0.0, 0.0, 0.0, 0.0); w * h];
    par_rows(&mut hblur, w, h, n, |row, out| {
        for x in 0..w {
            let center = bright[row * w + x];
            let (mut rv, mut gv, mut bv) = (
                center.r * kernel[0],
                center.g * kernel[0],
                center.b * kernel[0],
            );
            for (ki, &wt) in kernel[1..].iter().enumerate() {
                let k = ki + 1;
                let cl = bright[row * w + x.saturating_sub(k)];
                let cr = bright[row * w + (x + k).min(w - 1)];
                rv += (cl.r + cr.r) * wt;
                gv += (cl.g + cr.g) * wt;
                bv += (cl.b + cr.b) * wt;
            }
            out[x] = Rgba::new(rv, gv, bv, 1.0);
        }
    });

    // Pass 3: vertical Gaussian + additive composite
    // Each output row y reads rows [y-radius..y+radius] from hblur (read-only) and
    // the same pixel from orig (read-only), then writes to result (one row per thread).
    let mut result: Vec<Rgba> = orig.clone();
    par_rows(&mut result, w, h, n, |row, out| {
        for x in 0..w {
            let center = hblur[row * w + x];
            let (mut rv, mut gv, mut bv) = (
                center.r * kernel[0],
                center.g * kernel[0],
                center.b * kernel[0],
            );
            for (ki, &wt) in kernel[1..].iter().enumerate() {
                let k = ki + 1;
                let ct = hblur[row.saturating_sub(k) * w + x];
                let cb = hblur[(row + k).min(h - 1) * w + x];
                rv += (ct.r + cb.r) * wt;
                gv += (ct.g + cb.g) * wt;
                bv += (ct.b + cb.b) * wt;
            }
            let base = orig[row * w + x];
            out[x] = if rv > 0.0 || gv > 0.0 || bv > 0.0 {
                Rgba::new(
                    (base.r + rv).min(1.0),
                    (base.g + gv).min(1.0),
                    (base.b + bv).min(1.0),
                    base.a,
                )
            } else {
                base
            };
        }
    });

    // Write result back
    for (dst, src) in fb.pixels_mut().iter_mut().zip(result.iter()) {
        *dst = *src;
    }
}
