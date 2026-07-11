//! Lowest-level rendering primitives shared by every PMRE elevation.

pub mod fair_queue;
pub mod framebuffer;
pub mod geom;
pub mod paint;

pub use framebuffer::{BandView, Framebuffer, Surface};
pub use geom::{Affine, Vec2};
pub use paint::{Bounds, DrawCmd, Paint, Rgba, Shape};
