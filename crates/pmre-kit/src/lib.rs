//! pmre-kit — the single primitive kit for the primitive math rendering engine.
//!
//! All dumb mechanism, no policy. The eight root atoms
//! (`scan · hash · fold · project · scale · compare · combine · order`, per
//! `primitves math/_taxonomy-root/ROOT_ATOMS.md`) plus the rendering primitives wired
//! from them. Every decision — draw order, clipping, which coverage generator fires —
//! lives in `pmre-orchestrator`, never here. If a primitive in this crate grows an `if`
//! that makes a value judgement, that `if` belongs in the orchestrator.

pub use pmre_core::{fair_queue, framebuffer, geom, paint};
pub use pmre_html::{css, html};
pub use pmre_layout::{layout, ux};
pub use pmre_raster::{bloom_sweep, path, post, raster};
pub use pmre_text::{font, text};

pub use pmre_core::framebuffer::{BandView, Framebuffer, Surface};
pub use pmre_core::geom::{Affine, Vec2};
pub use pmre_core::paint::{Bounds, DrawCmd, Paint, Rgba, Shape};
pub use pmre_layout::ux::{Align, Dim, Dir, Edges, Justify, Shadow, Span, Style, UxNode};
pub use pmre_raster::path::PathCmd;

/// The eight root atoms — the canonical vocabulary the whole kit specializes from.
/// Each does exactly one thing and makes no decisions.
pub mod atoms {
    /// `scan` — stream a region into discrete units (here: a pixel grid into coordinates).
    pub fn scan(width: u32, height: u32) -> impl Iterator<Item = (u32, u32)> {
        (0..height).flat_map(move |y| (0..width).map(move |x| (x, y)))
    }

    /// `hash` — a unit → a stable integer (FNV-1a).
    pub fn hash(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// `fold` — reduce a stream to an accumulator.
    pub fn fold<T, A, F: FnMut(A, &T) -> A>(items: &[T], init: A, mut f: F) -> A {
        let mut acc = init;
        for it in items {
            acc = f(acc, it);
        }
        acc
    }

    /// `project` — a vector through a basis: the dot product.
    pub fn project(v: &[f32], basis: &[f32]) -> f32 {
        v.iter().zip(basis).map(|(a, b)| a * b).sum()
    }

    /// `scale` — divide by a reference.
    pub fn scale(v: f32, reference: f32) -> f32 {
        v / reference
    }

    /// `compare` — a distance over a pair (Euclidean).
    pub fn compare(a: &[f32], b: &[f32]) -> f32 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y) * (x - y))
            .sum::<f32>()
            .sqrt()
    }

    /// `combine` — a weighted sum of `(weight, signal)` terms.
    pub fn combine(terms: &[(f32, f32)]) -> f32 {
        terms.iter().map(|(w, s)| w * s).sum()
    }

    /// `order` — indices of `items` sorted by `key`, descending.
    pub fn order<T, K: Fn(&T) -> f32>(items: &[T], key: K) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..items.len()).collect();
        idx.sort_by(|&i, &j| {
            key(&items[j])
                .partial_cmp(&key(&items[i]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        idx
    }
}
