//! Corners — box corner values (border-radius).

use crate::{AbsoluteLength, Anchor, Half, Pixels, ScaledPixels, Size};
use std::cmp;
use std::fmt;
use std::ops;

/// The four corners of a box.
#[derive(Clone, Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Corners<T: Clone + fmt::Debug + Default + PartialEq> {
    pub top_left: T,
    pub top_right: T,
    pub bottom_right: T,
    pub bottom_left: T,
}

impl<T: Clone + fmt::Debug + Default + PartialEq> Corners<T> {
    pub fn all(value: T) -> Self {
        Self {
            top_left: value.clone(),
            top_right: value.clone(),
            bottom_right: value.clone(),
            bottom_left: value,
        }
    }

    pub fn map<U: Clone + fmt::Debug + Default + PartialEq>(
        &self,
        f: impl Fn(&T) -> U,
    ) -> Corners<U> {
        Corners {
            top_left: f(&self.top_left),
            top_right: f(&self.top_right),
            bottom_right: f(&self.bottom_right),
            bottom_left: f(&self.bottom_left),
        }
    }
}

impl<T> Corners<T>
where
    T: ops::Add<T, Output = T> + Half + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn corner(&self, c: Anchor) -> T {
        match c {
            Anchor::TopLeft => self.top_left.clone(),
            Anchor::TopRight => self.top_right.clone(),
            Anchor::BottomLeft => self.bottom_left.clone(),
            Anchor::BottomRight => self.bottom_right.clone(),
            Anchor::TopCenter => (self.top_left.clone() + self.top_right.clone()).half(),
            Anchor::BottomCenter => (self.bottom_left.clone() + self.bottom_right.clone()).half(),
            Anchor::LeftCenter => (self.top_left.clone() + self.bottom_left.clone()).half(),
            Anchor::RightCenter => (self.top_right.clone() + self.bottom_right.clone()).half(),
        }
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + Copy> Copy for Corners<T> {}

impl<T> ops::Mul for Corners<T>
where
    T: ops::Mul<Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            top_left: self.top_left.clone() * rhs.top_left,
            top_right: self.top_right.clone() * rhs.top_right,
            bottom_right: self.bottom_right.clone() * rhs.bottom_right,
            bottom_left: self.bottom_left * rhs.bottom_left,
        }
    }
}

impl<T, S> ops::MulAssign<S> for Corners<T>
where
    T: ops::Mul<S, Output = T> + Clone + fmt::Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.top_left = self.top_left.clone() * rhs.clone();
        self.top_right = self.top_right.clone() * rhs.clone();
        self.bottom_right = self.bottom_right.clone() * rhs.clone();
        self.bottom_left = self.bottom_left.clone() * rhs;
    }
}

impl Corners<Pixels> {
    pub fn scale(&self, factor: f32) -> Corners<ScaledPixels> {
        Corners {
            top_left: self.top_left.scale(factor),
            top_right: self.top_right.scale(factor),
            bottom_right: self.bottom_right.scale(factor),
            bottom_left: self.bottom_left.scale(factor),
        }
    }

    pub fn max(&self) -> Pixels {
        self.top_left
            .max(self.top_right)
            .max(self.bottom_right)
            .max(self.bottom_left)
    }

    pub fn clamp_radii_for_quad_size(self, s: Size<Pixels>) -> Corners<Pixels> {
        let max_val = Pixels(cmp::min(s.width, s.height).0 / 2.0);
        Corners {
            top_left: cmp::min(self.top_left, max_val),
            top_right: cmp::min(self.top_right, max_val),
            bottom_right: cmp::min(self.bottom_right, max_val),
            bottom_left: cmp::min(self.bottom_left, max_val),
        }
    }
}

impl Corners<AbsoluteLength> {
    pub fn to_pixels(self, rem_size: Pixels) -> Corners<Pixels> {
        Corners {
            top_left: self.top_left.to_pixels(rem_size),
            top_right: self.top_right.to_pixels(rem_size),
            bottom_right: self.bottom_right.to_pixels(rem_size),
            bottom_left: self.bottom_left.to_pixels(rem_size),
        }
    }
}

impl From<Pixels> for Corners<Pixels> {
    fn from(val: Pixels) -> Self {
        Corners::all(val)
    }
}
