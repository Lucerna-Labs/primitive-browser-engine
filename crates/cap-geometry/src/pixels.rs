//! Pixel unit types — logical, device, and scaled.

use crate::{Size, size};
use std::cmp;
use std::fmt;
use std::hash;
use std::iter;
use std::ops;

// ─── Pixels (logical) ─────────────────────────────────────────────

/// Logical pixels — the base unit of measurement in UI layout.
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub struct Pixels(pub f32);

impl Pixels {
    pub const ZERO: Pixels = Pixels(0.0);
    pub const MAX: Pixels = Pixels(f32::MAX);
    pub fn as_f32(self) -> f32 {
        self.0
    }
    pub fn floor(self) -> Self {
        Self(self.0.floor())
    }
    pub fn round(self) -> Self {
        Self(self.0.round())
    }
    pub fn ceil(self) -> Self {
        Self(self.0.ceil())
    }
    pub fn scale(self, factor: f32) -> ScaledPixels {
        ScaledPixels(self.0 * factor)
    }
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }
    pub fn signum(self) -> f32 {
        self.0.signum()
    }
    pub fn to_f64(self) -> f64 {
        self.0 as f64
    }
}

impl Eq for Pixels {}
impl PartialOrd for Pixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Pixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}
impl hash::Hash for Pixels {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl ops::Add for Pixels {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl ops::AddAssign for Pixels {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}
impl ops::Sub for Pixels {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl ops::SubAssign for Pixels {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}
impl ops::Div for Pixels {
    type Output = f32;
    fn div(self, rhs: Self) -> f32 {
        self.0 / rhs.0
    }
}
impl ops::DivAssign for Pixels {
    fn div_assign(&mut self, rhs: Self) {
        self.0 /= rhs.0;
    }
}
impl ops::Rem for Pixels {
    type Output = Self;
    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}
impl ops::RemAssign for Pixels {
    fn rem_assign(&mut self, rhs: Self) {
        self.0 %= rhs.0;
    }
}
impl ops::Mul<f32> for Pixels {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}
impl ops::Mul<Pixels> for f32 {
    type Output = Pixels;
    fn mul(self, rhs: Pixels) -> Self::Output {
        Pixels(self * rhs.0)
    }
}
impl ops::Mul<usize> for Pixels {
    type Output = Self;
    fn mul(self, rhs: usize) -> Self {
        Self(self.0 * rhs as f32)
    }
}
impl ops::MulAssign<f32> for Pixels {
    fn mul_assign(&mut self, rhs: f32) {
        self.0 *= rhs;
    }
}
impl ops::Neg for Pixels {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}
impl iter::Sum for Pixels {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::ZERO, |a, b| a + b)
    }
}

impl fmt::Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0)
    }
}
impl fmt::Display for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0)
    }
}
impl From<f32> for Pixels {
    fn from(v: f32) -> Self {
        Pixels(v)
    }
}
impl From<f64> for Pixels {
    fn from(v: f64) -> Self {
        Pixels(v as f32)
    }
}
impl From<Pixels> for f32 {
    fn from(p: Pixels) -> Self {
        p.0
    }
}
impl From<Pixels> for f64 {
    fn from(p: Pixels) -> Self {
        p.0 as f64
    }
}
impl From<u32> for Pixels {
    fn from(v: u32) -> Self {
        Pixels(v as f32)
    }
}
impl From<Pixels> for u32 {
    fn from(p: Pixels) -> Self {
        p.0 as u32
    }
}

// ─── DevicePixels (physical) ──────────────────────────────────────

/// Physical pixels on the display.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct DevicePixels(pub i32);

impl DevicePixels {
    pub fn to_bytes(self, bytes_per_pixel: u8) -> u32 {
        self.0 as u32 * bytes_per_pixel as u32
    }
}

impl ops::Add for DevicePixels {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl ops::AddAssign for DevicePixels {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}
impl ops::Sub for DevicePixels {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl ops::SubAssign for DevicePixels {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}
impl ops::Div for DevicePixels {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        Self(self.0 / rhs.0)
    }
}

impl fmt::Debug for DevicePixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px(device)", self.0)
    }
}
impl From<i32> for DevicePixels {
    fn from(v: i32) -> Self {
        DevicePixels(v)
    }
}
impl From<DevicePixels> for i32 {
    fn from(v: DevicePixels) -> Self {
        v.0
    }
}

// ─── ScaledPixels ─────────────────────────────────────────────────

/// Scaled pixels — logical pixels multiplied by a display scale factor.
#[derive(Clone, Copy, Default, PartialEq)]
#[repr(transparent)]
pub struct ScaledPixels(pub f32);

impl ScaledPixels {
    pub fn as_f32(self) -> f32 {
        self.0
    }
    pub fn floor(self) -> Self {
        Self(self.0.floor())
    }
    pub fn round(self) -> Self {
        Self(self.0.round())
    }
    pub fn ceil(self) -> Self {
        Self(self.0.ceil())
    }
}

impl Eq for ScaledPixels {}
impl PartialOrd for ScaledPixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for ScaledPixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}
impl ops::Add for ScaledPixels {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl ops::AddAssign for ScaledPixels {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}
impl ops::Sub for ScaledPixels {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl ops::SubAssign for ScaledPixels {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}
impl ops::Div for ScaledPixels {
    type Output = f32;
    fn div(self, rhs: Self) -> f32 {
        self.0 / rhs.0
    }
}
impl ops::Mul<f32> for ScaledPixels {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}

impl fmt::Debug for ScaledPixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px(scaled)", self.0)
    }
}
impl From<ScaledPixels> for DevicePixels {
    fn from(s: ScaledPixels) -> Self {
        DevicePixels(s.0.ceil() as i32)
    }
}
impl From<DevicePixels> for ScaledPixels {
    fn from(d: DevicePixels) -> Self {
        ScaledPixels(d.0 as f32)
    }
}
impl From<f32> for ScaledPixels {
    fn from(v: f32) -> Self {
        ScaledPixels(v)
    }
}

// ─── Conversions ──────────────────────────────────────────────────

impl Size<DevicePixels> {
    pub fn to_pixels(self, scale_factor: f32) -> Size<Pixels> {
        size(
            px(self.width.0 as f32 / scale_factor),
            py(self.height.0 as f32 / scale_factor),
        )
    }
}

impl Size<Pixels> {
    pub fn to_device_pixels(self, scale_factor: f32) -> Size<DevicePixels> {
        size(
            DevicePixels((self.width.0 * scale_factor).round() as i32),
            DevicePixels((self.height.0 * scale_factor).round() as i32),
        )
    }
}

/// Construct a `Pixels` value.
pub const fn px(pixels: f32) -> Pixels {
    Pixels(pixels)
}
/// Construct a `Pixels` value (y-variant for readability).
pub const fn py(pixels: f32) -> Pixels {
    Pixels(pixels)
}
