//! # cap-geometry
//!
//! Renderer-neutral 2D geometry primitives for UI and layout calculations.
//!
//! This crate extracts the geometry types from Zed's GPUI, removing all
//! rendering, serialization, and UI framework dependencies. It provides:
//!
//! - **Pixels / DevicePixels / ScaledPixels** — typed pixel units with DPI awareness
//! - **Rems** — font-relative sizing unit
//! - **Length / DefiniteLength / AbsoluteLength** — layout length system (px, rem, %)
//! - **Point / Size / Bounds** — core 2D spatial types
//! - **Edges / Corners** — box model primitives (padding, margin, border-radius)
//! - **Axis / Along** — axis-aware dimension access
//! - **Anchor** — corner/edge positioning
//! - **Half / IsZero** — numeric convenience traits
//!
//! ## Architecture
//!
//! This crate is the **foundation** for the `cap-*` crate ecosystem. Every other
//! capability crate (layout, scene, text, keymap) references these types. No
//! renderer or windowing dependency is introduced here.
//!
//! ```text
//! cap-geometry  ← cap-layout, cap-scene, cap-text-layout, cap-keymap
//! ```

mod axis;
mod bounds;
mod corners;
mod edges;
mod length;
mod pixels;
mod point;
mod size;
mod traits;

pub use axis::*;
pub use bounds::*;
pub use corners::*;
pub use edges::*;
pub use length::*;
pub use pixels::*;
pub use point::*;
pub use size::*;
pub use traits::*;
