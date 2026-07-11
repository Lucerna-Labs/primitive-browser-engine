//! Length types — Pixels, Rems, Fractions, Auto.

use crate::{Pixels, px};
use std::fmt;
use std::ops;

/// A length in rems — relative to the root font size.
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub struct Rems(pub f32);

impl Rems {
    pub fn to_pixels(self, rem_size: Pixels) -> Pixels {
        self * rem_size
    }
}

impl ops::Add for Rems {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl ops::Sub for Rems {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl ops::Mul<Pixels> for Rems {
    type Output = Pixels;
    fn mul(self, other: Pixels) -> Pixels {
        Pixels(self.0 * other.0)
    }
}
impl ops::Neg for Rems {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}

impl fmt::Debug for Rems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}rem", self.0)
    }
}
impl fmt::Display for Rems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}rem", self.0)
    }
}

/// Construct a `Rems` value.
pub const fn rems(v: f32) -> Rems {
    Rems(v)
}

// ──────────────────────────────────────────────────────────────────

/// An absolute length in pixels or rems.
#[derive(Clone, Copy, PartialEq)]
pub enum AbsoluteLength {
    Pixels(Pixels),
    Rems(Rems),
}

impl AbsoluteLength {
    pub fn is_zero(&self) -> bool {
        match self {
            Self::Pixels(p) => p.0 == 0.0,
            Self::Rems(r) => r.0 == 0.0,
        }
    }
    pub fn to_pixels(self, rem_size: Pixels) -> Pixels {
        match self {
            Self::Pixels(p) => p,
            Self::Rems(r) => r.to_pixels(rem_size),
        }
    }
    pub fn to_rems(self, rem_size: Pixels) -> Rems {
        match self {
            Self::Pixels(p) => Rems(p.0 / rem_size.0),
            Self::Rems(r) => r,
        }
    }
}

impl Default for AbsoluteLength {
    fn default() -> Self {
        px(0.0).into()
    }
}
impl From<Pixels> for AbsoluteLength {
    fn from(p: Pixels) -> Self {
        Self::Pixels(p)
    }
}
impl From<Rems> for AbsoluteLength {
    fn from(r: Rems) -> Self {
        Self::Rems(r)
    }
}
impl ops::Neg for AbsoluteLength {
    type Output = Self;
    fn neg(self) -> Self {
        match self {
            Self::Pixels(p) => Self::Pixels(-p),
            Self::Rems(r) => Self::Rems(-r),
        }
    }
}
impl fmt::Debug for AbsoluteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for AbsoluteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pixels(p) => write!(f, "{p}"),
            Self::Rems(r) => write!(f, "{r}"),
        }
    }
}

/// A non-auto length that can be defined in pixels, rems, or fraction of parent.
#[derive(Clone, Copy, PartialEq)]
pub enum DefiniteLength {
    Absolute(AbsoluteLength),
    Fraction(f32),
}

impl DefiniteLength {
    pub fn to_pixels(self, base_size: AbsoluteLength, rem_size: Pixels) -> Pixels {
        match self {
            Self::Absolute(l) => l.to_pixels(rem_size),
            Self::Fraction(frac) => match base_size {
                AbsoluteLength::Pixels(p) => p * frac,
                AbsoluteLength::Rems(r) => r * rem_size * frac,
            },
        }
    }
}

impl Default for DefiniteLength {
    fn default() -> Self {
        Self::Absolute(AbsoluteLength::default())
    }
}
impl From<Pixels> for DefiniteLength {
    fn from(p: Pixels) -> Self {
        Self::Absolute(p.into())
    }
}
impl From<Rems> for DefiniteLength {
    fn from(r: Rems) -> Self {
        Self::Absolute(r.into())
    }
}
impl From<AbsoluteLength> for DefiniteLength {
    fn from(l: AbsoluteLength) -> Self {
        Self::Absolute(l)
    }
}
impl ops::Neg for DefiniteLength {
    type Output = Self;
    fn neg(self) -> Self {
        match self {
            Self::Absolute(l) => Self::Absolute(-l),
            Self::Fraction(f) => Self::Fraction(-f),
        }
    }
}
impl fmt::Debug for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absolute(l) => write!(f, "{l}"),
            Self::Fraction(frac) => write!(f, "{}%", (*frac * 100.0) as i32),
        }
    }
}

/// A length that can be pixels, rems, fraction, or auto.
#[derive(Clone, Copy, PartialEq)]
pub enum Length {
    Definite(DefiniteLength),
    Auto,
}

impl Default for Length {
    fn default() -> Self {
        Self::Definite(DefiniteLength::default())
    }
}
impl From<Pixels> for Length {
    fn from(p: Pixels) -> Self {
        Self::Definite(p.into())
    }
}
impl From<Rems> for Length {
    fn from(r: Rems) -> Self {
        Self::Definite(r.into())
    }
}
impl From<DefiniteLength> for Length {
    fn from(l: DefiniteLength) -> Self {
        Self::Definite(l)
    }
}
impl From<AbsoluteLength> for Length {
    fn from(l: AbsoluteLength) -> Self {
        Self::Definite(l.into())
    }
}
impl fmt::Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Definite(l) => write!(f, "{l}"),
            Self::Auto => write!(f, "auto"),
        }
    }
}

/// Construct a relative (fractional) definite length.
pub const fn relative(fraction: f32) -> DefiniteLength {
    DefiniteLength::Fraction(fraction)
}
/// Construct an auto length.
pub const fn auto() -> Length {
    Length::Auto
}
