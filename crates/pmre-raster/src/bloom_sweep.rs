//! Bloom strategy sweep — the same additive Gaussian bloom as `post::bloom`, but
//! factored into three independent, composable axes so every combination of the
//! kernel-borrowed primitives can be benchmarked against each other:
//!
//! * `Dispatch` — how row/tile work is handed to threads:
//!   - `Serial`: single thread (baseline).
//!   - `FairQueue`: the mutex work-queue primitive (`fair_queue::FairQueue`).
//!   - `Atomic`: a lockless `AtomicUsize` cursor; workers claim chunks via `fetch_add`
//!     (kernel: lockless run queue + NAPI-style coalescing).
//!   - `AtomicPool`: one persistent `thread::scope` for all passes, `Barrier` between
//!     them (kernel: kworker pool + `struct completion`).
//! * `Structure` — `Separable` (three full-frame passes) vs `TiledFused`
//!   (cache-blocked tiles with all passes fused while the tile is hot in L2).
//! * `Arith` — `Scalar` vs `Simd` (SSE2 `__m128`, four channels per instruction).
//!
//! Output is bit-comparable (within float reassociation) to `post::bloom`, so a
//! benchmark harness can reject any combination that drifts from the reference.

use pmre_core::fair_queue::FairQueue;
use pmre_core::framebuffer::Framebuffer;
use pmre_core::paint::Rgba;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Barrier;

const TRANSPARENT: Rgba = Rgba::new(0.0, 0.0, 0.0, 0.0);

// ── Strategy description ───────────────────────────────────────────────────────

/// How parallel work is distributed to threads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dispatch {
    /// Single-threaded baseline.
    Serial,
    /// Mutex FIFO work queue (`FairQueue`), threads respawned per pass.
    FairQueue,
    /// Lockless atomic cursor; workers claim row/tile chunks via `fetch_add`.
    Atomic,
    /// Static row-band per thread — the MM3E "lane / bus" model. Each thread owns one
    /// contiguous band and writes its own disjoint `chunks_mut` slice, stitched by
    /// position. No locks, no atomics, no `unsafe` aliasing; deterministic in thread count.
    Band,
    /// Persistent worker pool: one `thread::scope` across all passes, `Barrier` between.
    AtomicPool,
}

/// The pass structure of the bloom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Structure {
    /// Three full-frame separable passes (bright → H-blur → V-blur+composite).
    Separable,
    /// Cache-blocked tiles with all three passes fused per tile.
    TiledFused,
}

/// Inner-loop arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arith {
    Scalar,
    /// SSE2 four-wide; falls back to scalar on non-x86_64 targets.
    Simd,
}

/// A fully-specified bloom strategy: one point in the `Dispatch × Structure × Arith` space.
#[derive(Clone, Copy, Debug)]
pub struct Strategy {
    pub dispatch: Dispatch,
    pub structure: Structure,
    pub arith: Arith,
    /// Rows/tiles claimed per atomic `fetch_add` (work coalescing granularity).
    pub chunk: usize,
    /// Tile edge length for `TiledFused`.
    pub tile: usize,
}

impl Strategy {
    pub fn new(dispatch: Dispatch, structure: Structure, arith: Arith) -> Self {
        Self {
            dispatch,
            structure,
            arith,
            chunk: 16,
            tile: 64,
        }
    }
}

// ── Shared helpers ─────────────────────────────────────────────────────────────

fn thread_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Normalized symmetric 1-D Gaussian: `k[0]` center, `k[i]` weight at distance `i`.
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

#[inline]
fn luminance(c: Rgba) -> f32 {
    0.299 * c.r + 0.587 * c.g + 0.114 * c.b
}

#[inline]
fn clamp_idx(v: i32, n: usize) -> usize {
    v.clamp(0, n as i32 - 1) as usize
}

/// SAFETY: `base` points to a `[Rgba; w*h]`; `row < h`; the caller guarantees this
/// row is claimed by exactly one worker, so the returned slice never overlaps another.
#[inline]
unsafe fn row_mut<'a>(base: usize, w: usize, row: usize) -> &'a mut [Rgba] {
    std::slice::from_raw_parts_mut((base as *mut Rgba).add(row * w), w)
}

// ── Per-row kernels (shared by every Dispatch) ─────────────────────────────────

fn bright_row(orig: &[Rgba], w: usize, row: usize, threshold: f32, out: &mut [Rgba]) {
    let base = row * w;
    for x in 0..w {
        let c = orig[base + x];
        let l = luminance(c);
        out[x] = if l > threshold {
            let s = (l - threshold) / l;
            Rgba::new(c.r * s, c.g * s, c.b * s, 1.0)
        } else {
            TRANSPARENT
        };
    }
}

fn hblur_row(src: &[Rgba], w: usize, row: usize, kernel: &[f32], out: &mut [Rgba], arith: Arith) {
    match arith {
        Arith::Scalar => hblur_row_scalar(src, w, row, kernel, out),
        Arith::Simd => unsafe { hblur_row_simd(src, w, row, kernel, out) },
    }
}

fn hblur_row_scalar(src: &[Rgba], w: usize, row: usize, kernel: &[f32], out: &mut [Rgba]) {
    let base = row * w;
    let k0 = kernel[0];
    for x in 0..w {
        let c = src[base + x];
        let (mut rv, mut gv, mut bv) = (c.r * k0, c.g * k0, c.b * k0);
        for (ki, &wt) in kernel[1..].iter().enumerate() {
            let k = ki + 1;
            let cl = src[base + x.saturating_sub(k)];
            let cr = src[base + (x + k).min(w - 1)];
            rv += (cl.r + cr.r) * wt;
            gv += (cl.g + cr.g) * wt;
            bv += (cl.b + cr.b) * wt;
        }
        out[x] = Rgba::new(rv, gv, bv, 1.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn vblur_row(
    hblur: &[Rgba],
    orig: &[Rgba],
    w: usize,
    h: usize,
    row: usize,
    kernel: &[f32],
    out: &mut [Rgba],
    arith: Arith,
) {
    match arith {
        Arith::Scalar => vblur_row_scalar(hblur, orig, w, h, row, kernel, out),
        Arith::Simd => unsafe { vblur_row_simd(hblur, orig, w, h, row, kernel, out) },
    }
}

fn vblur_row_scalar(
    hblur: &[Rgba],
    orig: &[Rgba],
    w: usize,
    h: usize,
    row: usize,
    kernel: &[f32],
    out: &mut [Rgba],
) {
    let k0 = kernel[0];
    for x in 0..w {
        let c = hblur[row * w + x];
        let (mut rv, mut gv, mut bv) = (c.r * k0, c.g * k0, c.b * k0);
        for (ki, &wt) in kernel[1..].iter().enumerate() {
            let k = ki + 1;
            let ct = hblur[row.saturating_sub(k) * w + x];
            let cb = hblur[(row + k).min(h - 1) * w + x];
            rv += (ct.r + cb.r) * wt;
            gv += (ct.g + cb.g) * wt;
            bv += (ct.b + cb.b) * wt;
        }
        let base = orig[row * w + x];
        out[x] = composite(base, rv, gv, bv);
    }
}

#[inline]
fn composite(base: Rgba, rv: f32, gv: f32, bv: f32) -> Rgba {
    if rv > 0.0 || gv > 0.0 || bv > 0.0 {
        Rgba::new(
            (base.r + rv).min(1.0),
            (base.g + gv).min(1.0),
            (base.b + bv).min(1.0),
            base.a,
        )
    } else {
        base
    }
}

// ── SSE2 inner loops (four channels per instruction) ───────────────────────────

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn load(c: Rgba) -> std::arch::x86_64::__m128 {
    let a = [c.r, c.g, c.b, c.a];
    std::arch::x86_64::_mm_loadu_ps(a.as_ptr())
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn store4(v: std::arch::x86_64::__m128) -> [f32; 4] {
    let mut a = [0.0f32; 4];
    std::arch::x86_64::_mm_storeu_ps(a.as_mut_ptr(), v);
    a
}

#[cfg(target_arch = "x86_64")]
unsafe fn hblur_row_simd(src: &[Rgba], w: usize, row: usize, kernel: &[f32], out: &mut [Rgba]) {
    use std::arch::x86_64::*;
    let base = row * w;
    let radius = kernel.len() - 1;
    for x in 0..w {
        let mut acc = _mm_mul_ps(load(src[base + x]), _mm_set1_ps(kernel[0]));
        for k in 1..=radius {
            let cl = load(src[base + x.saturating_sub(k)]);
            let cr = load(src[base + (x + k).min(w - 1)]);
            acc = _mm_add_ps(acc, _mm_mul_ps(_mm_add_ps(cl, cr), _mm_set1_ps(kernel[k])));
        }
        let a = store4(acc);
        out[x] = Rgba::new(a[0], a[1], a[2], 1.0);
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn vblur_row_simd(
    hblur: &[Rgba],
    orig: &[Rgba],
    w: usize,
    h: usize,
    row: usize,
    kernel: &[f32],
    out: &mut [Rgba],
) {
    use std::arch::x86_64::*;
    let radius = kernel.len() - 1;
    for x in 0..w {
        let mut acc = _mm_mul_ps(load(hblur[row * w + x]), _mm_set1_ps(kernel[0]));
        for k in 1..=radius {
            let ct = load(hblur[row.saturating_sub(k) * w + x]);
            let cb = load(hblur[(row + k).min(h - 1) * w + x]);
            acc = _mm_add_ps(acc, _mm_mul_ps(_mm_add_ps(ct, cb), _mm_set1_ps(kernel[k])));
        }
        let a = store4(acc);
        let base = orig[row * w + x];
        out[x] = composite(base, a[0], a[1], a[2]);
    }
}

// Non-x86 fallbacks so the `Simd` variant stays callable everywhere.
#[cfg(not(target_arch = "x86_64"))]
unsafe fn hblur_row_simd(src: &[Rgba], w: usize, row: usize, kernel: &[f32], out: &mut [Rgba]) {
    hblur_row_scalar(src, w, row, kernel, out);
}

#[cfg(not(target_arch = "x86_64"))]
#[allow(clippy::too_many_arguments)]
unsafe fn vblur_row_simd(
    hblur: &[Rgba],
    orig: &[Rgba],
    w: usize,
    h: usize,
    row: usize,
    kernel: &[f32],
    out: &mut [Rgba],
) {
    vblur_row_scalar(hblur, orig, w, h, row, kernel, out);
}

// ── Row dispatch runners ───────────────────────────────────────────────────────

#[inline]
fn claim_rows(cursor: &AtomicUsize, h: usize, chunk: usize, mut f: impl FnMut(usize)) {
    loop {
        let start = cursor.fetch_add(chunk, Ordering::Relaxed);
        if start >= h {
            break;
        }
        for row in start..(start + chunk).min(h) {
            f(row);
        }
    }
}

/// Run `f(row, out_row)` over all `h` rows of `buf`, distributed per `disp`.
/// `AtomicPool` is handled separately by `separable_pool`, never here.
fn run_rows<F>(disp: Dispatch, buf: &mut [Rgba], w: usize, h: usize, chunk: usize, n: usize, f: &F)
where
    F: Fn(usize, &mut [Rgba]) + Sync,
{
    match disp {
        Dispatch::Serial => {
            for row in 0..h {
                f(row, &mut buf[row * w..row * w + w]);
            }
        }
        Dispatch::FairQueue => {
            let q = FairQueue::new();
            for row in 0..h {
                q.push(row);
            }
            q.seal();
            let base = buf.as_mut_ptr() as usize;
            std::thread::scope(|s| {
                for _ in 0..n {
                    let q = q.clone();
                    let f = &f;
                    s.spawn(move || {
                        while let Some(row) = q.pop() {
                            // SAFETY: each row index is enqueued once, so popped by one worker.
                            let out = unsafe { row_mut(base, w, row) };
                            f(row, out);
                        }
                    });
                }
            });
        }
        Dispatch::Atomic => {
            let cursor = AtomicUsize::new(0);
            let base = buf.as_mut_ptr() as usize;
            std::thread::scope(|s| {
                for _ in 0..n {
                    let cursor = &cursor;
                    let f = &f;
                    s.spawn(move || {
                        claim_rows(cursor, h, chunk, |row| {
                            // SAFETY: the atomic cursor hands each row to exactly one worker.
                            let out = unsafe { row_mut(base, w, row) };
                            f(row, out);
                        });
                    });
                }
            });
        }
        Dispatch::Band => {
            // MM3E lane/bus model: one contiguous band per thread, each writing its own
            // disjoint `chunks_mut` slice. Fully safe — no atomics, no raw pointers.
            let band = h.div_ceil(n).max(1);
            std::thread::scope(|s| {
                for (bi, chunk) in buf.chunks_mut(band * w).enumerate() {
                    let f = &f;
                    s.spawn(move || {
                        let y0 = bi * band;
                        for (r, out_row) in chunk.chunks_mut(w).enumerate() {
                            f(y0 + r, out_row);
                        }
                    });
                }
            });
        }
        Dispatch::AtomicPool => unreachable!("AtomicPool routed through separable_pool"),
    }
}

// ── Separable bloom (Serial / FairQueue / Atomic) ──────────────────────────────

fn separable(
    orig: &[Rgba],
    w: usize,
    h: usize,
    threshold: f32,
    kernel: &[f32],
    strat: Strategy,
    result: &mut [Rgba],
) {
    let n = thread_count();
    let chunk = strat.chunk.max(1);
    let arith = strat.arith;

    if strat.dispatch == Dispatch::AtomicPool {
        separable_pool(orig, w, h, threshold, kernel, arith, chunk, n, result);
        return;
    }

    let disp = strat.dispatch;
    let mut bright = vec![TRANSPARENT; w * h];
    let mut hblur = vec![TRANSPARENT; w * h];

    run_rows(disp, &mut bright, w, h, chunk, n, &|row, out| {
        bright_row(orig, w, row, threshold, out)
    });
    run_rows(disp, &mut hblur, w, h, chunk, n, &|row, out| {
        hblur_row(&bright, w, row, kernel, out, arith)
    });
    run_rows(disp, result, w, h, chunk, n, &|row, out| {
        vblur_row(&hblur, orig, w, h, row, kernel, out, arith)
    });
}

/// Persistent worker pool: one `thread::scope` spans all three passes, with a
/// `Barrier` enforcing pass ordering. Threads are spawned once, not per pass.
#[allow(clippy::too_many_arguments)]
fn separable_pool(
    orig: &[Rgba],
    w: usize,
    h: usize,
    threshold: f32,
    kernel: &[f32],
    arith: Arith,
    chunk: usize,
    n: usize,
    result: &mut [Rgba],
) {
    let mut bright = vec![TRANSPARENT; w * h];
    let mut hblur = vec![TRANSPARENT; w * h];

    let bright_base = bright.as_mut_ptr() as usize;
    let hblur_base = hblur.as_mut_ptr() as usize;
    let result_base = result.as_mut_ptr() as usize;

    let barrier = Barrier::new(n);
    let cur0 = AtomicUsize::new(0);
    let cur1 = AtomicUsize::new(0);
    let cur2 = AtomicUsize::new(0);

    std::thread::scope(|s| {
        for _ in 0..n {
            let (barrier, cur0, cur1, cur2) = (&barrier, &cur0, &cur1, &cur2);
            s.spawn(move || {
                // Phase 0 — bright-pass.
                claim_rows(cur0, h, chunk, |row| {
                    // SAFETY: atomic cursor gives each row to one worker; rows disjoint.
                    let out = unsafe { row_mut(bright_base, w, row) };
                    bright_row(orig, w, row, threshold, out);
                });
                barrier.wait();

                // Phase 1 — horizontal blur (reads the now-complete bright buffer).
                let bright_ro =
                    unsafe { std::slice::from_raw_parts(bright_base as *const Rgba, w * h) };
                claim_rows(cur1, h, chunk, |row| {
                    let out = unsafe { row_mut(hblur_base, w, row) };
                    hblur_row(bright_ro, w, row, kernel, out, arith);
                });
                barrier.wait();

                // Phase 2 — vertical blur + composite (reads the complete hblur buffer).
                let hblur_ro =
                    unsafe { std::slice::from_raw_parts(hblur_base as *const Rgba, w * h) };
                claim_rows(cur2, h, chunk, |row| {
                    let out = unsafe { row_mut(result_base, w, row) };
                    vblur_row(hblur_ro, orig, w, h, row, kernel, out, arith);
                });
            });
        }
    });
}

// ── Tiled + fused bloom ────────────────────────────────────────────────────────

/// One output tile: bright → H-blur → V-blur+composite computed entirely in
/// thread-local scratch (with a `radius` halo) so intermediates stay in cache.
#[allow(clippy::too_many_arguments)]
fn tile_compute(
    orig: &[Rgba],
    w: usize,
    h: usize,
    threshold: f32,
    kernel: &[f32],
    radius: usize,
    arith: Arith,
    tx: usize,
    ty: usize,
    tw: usize,
    th: usize,
    bright: &mut [Rgba], // scratch, (th+2r) * (tw+2r)
    hblur: &mut [Rgba],  // scratch, (th+2r) * tw
    result_base: usize,
) {
    let r = radius;
    let rw = tw + 2 * r;
    let rh = th + 2 * r;

    // 1 — bright-pass over the tile plus a full halo, clamped at image edges.
    for j in 0..rh {
        let gy = clamp_idx(ty as i32 - r as i32 + j as i32, h);
        for i in 0..rw {
            let gx = clamp_idx(tx as i32 - r as i32 + i as i32, w);
            let c = orig[gy * w + gx];
            let l = luminance(c);
            bright[j * rw + i] = if l > threshold {
                let s = (l - threshold) / l;
                Rgba::new(c.r * s, c.g * s, c.b * s, 1.0)
            } else {
                TRANSPARENT
            };
        }
    }

    // 2 — horizontal blur, trimming the horizontal halo (output cols 0..tw).
    let k0 = kernel[0];
    for j in 0..rh {
        for i in 0..tw {
            let c = bright[j * rw + (i + r)];
            let (mut rv, mut gv, mut bv) = (c.r * k0, c.g * k0, c.b * k0);
            for (ki, &wt) in kernel[1..].iter().enumerate() {
                let k = ki + 1;
                let cl = bright[j * rw + (i + r - k)];
                let cr = bright[j * rw + (i + r + k)];
                rv += (cl.r + cr.r) * wt;
                gv += (cl.g + cr.g) * wt;
                bv += (cl.b + cr.b) * wt;
            }
            hblur[j * tw + i] = Rgba::new(rv, gv, bv, 1.0);
        }
    }

    // 3 — vertical blur + composite, writing the tile's disjoint output block.
    for jo in 0..th {
        let gy = ty + jo;
        for i in 0..tw {
            let c = hblur[(jo + r) * tw + i];
            let (mut rv, mut gv, mut bv) = (c.r * k0, c.g * k0, c.b * k0);
            for (ki, &wt) in kernel[1..].iter().enumerate() {
                let k = ki + 1;
                let ct = hblur[(jo + r - k) * tw + i];
                let cb = hblur[(jo + r + k) * tw + i];
                rv += (ct.r + cb.r) * wt;
                gv += (ct.g + cb.g) * wt;
                bv += (ct.b + cb.b) * wt;
            }
            let base = orig[gy * w + tx + i];
            let out = composite(base, rv, gv, bv);
            // SAFETY: tiles are disjoint output blocks; each result pixel written once.
            unsafe {
                *((result_base as *mut Rgba).add(gy * w + tx + i)) = out;
            }
        }
    }
    let _ = arith; // tiled inner loops are scalar; `arith` reserved for parity in the matrix
}

#[allow(clippy::too_many_arguments)]
fn tiled(
    orig: &[Rgba],
    w: usize,
    h: usize,
    threshold: f32,
    kernel: &[f32],
    radius: usize,
    strat: Strategy,
    result: &mut [Rgba],
) {
    let n = thread_count();
    let chunk = strat.chunk.max(1);
    let tile = strat.tile.max(1);
    let arith = strat.arith;
    let ntx = w.div_ceil(tile);
    let nty = h.div_ceil(tile);
    let ntiles = ntx * nty;
    let rw = tile + 2 * radius;
    let rh = tile + 2 * radius;

    let result_base = result.as_mut_ptr() as usize;

    let run_tile = |t: usize, bright: &mut [Rgba], hblur: &mut [Rgba]| {
        let tx = (t % ntx) * tile;
        let ty = (t / ntx) * tile;
        let tw = tile.min(w - tx);
        let th = tile.min(h - ty);
        tile_compute(
            orig,
            w,
            h,
            threshold,
            kernel,
            radius,
            arith,
            tx,
            ty,
            tw,
            th,
            bright,
            hblur,
            result_base,
        );
    };

    match strat.dispatch {
        Dispatch::Serial => {
            let mut bright = vec![TRANSPARENT; rw * rh];
            let mut hblur = vec![TRANSPARENT; rh * tile];
            for t in 0..ntiles {
                run_tile(t, &mut bright, &mut hblur);
            }
        }
        Dispatch::FairQueue => {
            let q = FairQueue::new();
            for t in 0..ntiles {
                q.push(t);
            }
            q.seal();
            std::thread::scope(|s| {
                for _ in 0..n {
                    let q = q.clone();
                    let run_tile = &run_tile;
                    s.spawn(move || {
                        let mut bright = vec![TRANSPARENT; rw * rh];
                        let mut hblur = vec![TRANSPARENT; rh * tile];
                        while let Some(t) = q.pop() {
                            run_tile(t, &mut bright, &mut hblur);
                        }
                    });
                }
            });
        }
        // MM3E lane/bus model over the tile grid: each thread owns one contiguous
        // range of tile indices, processed with its own scratch. Static, deterministic.
        Dispatch::Band => {
            let per = ntiles.div_ceil(n).max(1);
            std::thread::scope(|s| {
                for ti in 0..n {
                    let run_tile = &run_tile;
                    s.spawn(move || {
                        let mut bright = vec![TRANSPARENT; rw * rh];
                        let mut hblur = vec![TRANSPARENT; rh * tile];
                        for t in (ti * per)..((ti + 1) * per).min(ntiles) {
                            run_tile(t, &mut bright, &mut hblur);
                        }
                    });
                }
            });
        }
        // Single fused phase: a persistent pool and a per-pass pool are identical here,
        // so `Atomic` and `AtomicPool` share the same lockless-cursor path.
        Dispatch::Atomic | Dispatch::AtomicPool => {
            let cursor = AtomicUsize::new(0);
            std::thread::scope(|s| {
                for _ in 0..n {
                    let cursor = &cursor;
                    let run_tile = &run_tile;
                    s.spawn(move || {
                        let mut bright = vec![TRANSPARENT; rw * rh];
                        let mut hblur = vec![TRANSPARENT; rh * tile];
                        loop {
                            let start = cursor.fetch_add(chunk, Ordering::Relaxed);
                            if start >= ntiles {
                                break;
                            }
                            for t in start..(start + chunk).min(ntiles) {
                                run_tile(t, &mut bright, &mut hblur);
                            }
                        }
                    });
                }
            });
        }
    }
}

// ── Public entry point ─────────────────────────────────────────────────────────

/// Apply additive Gaussian bloom to `fb` using the given `strat`. Output matches
/// `post::bloom(fb, threshold, sigma, radius)` within float-reassociation error.
pub fn bloom_with(
    fb: &mut Framebuffer,
    threshold: f32,
    sigma: f32,
    radius: usize,
    strat: Strategy,
) {
    let w = fb.width as usize;
    let h = fb.height as usize;
    if w == 0 || h == 0 {
        return;
    }
    let kernel = gaussian_kernel(sigma, radius);
    let orig = fb.pixels().to_vec();
    let mut result = orig.clone();

    match strat.structure {
        Structure::Separable => separable(&orig, w, h, threshold, &kernel, strat, &mut result),
        Structure::TiledFused => tiled(&orig, w, h, threshold, &kernel, radius, strat, &mut result),
    }

    fb.pixels_mut().copy_from_slice(&result);
}
