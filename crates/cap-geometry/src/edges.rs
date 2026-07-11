//! Edges — box model edges (padding, margin).

use crate::{AbsoluteLength, Pixels, ScaledPixels};
use std::fmt;
use std::ops;

/// The four edges of a box (top, right, bottom, left).
#[derive(Clone, Default, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Edges<T: Clone + fmt::Debug + Default + PartialEq> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Clone + fmt::Debug + Default + PartialEq> Edges<T> {
    pub fn all(value: T) -> Self {
        Self {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }

    pub fn map<U: Clone + fmt::Debug + Default + PartialEq>(
        &self,
        f: impl Fn(&T) -> U,
    ) -> Edges<U> {
        Edges {
            top: f(&self.top),
            right: f(&self.right),
            bottom: f(&self.bottom),
            left: f(&self.left),
        }
    }

    pub fn any<F: Fn(&T) -> bool>(&self, predicate: F) -> bool {
        predicate(&self.top)
            || predicate(&self.right)
            || predicate(&self.bottom)
            || predicate(&self.left)
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + Copy> Copy for Edges<T> {}

impl<T> ops::Mul for Edges<T>
where
    T: ops::Mul<Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    type Output = Self;
    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            top: self.top.clone() * rhs.top,
            right: self.right.clone() * rhs.right,
            bottom: self.bottom.clone() * rhs.bottom,
            left: self.left * rhs.left,
        }
    }
}

impl<T, S> ops::MulAssign<S> for Edges<T>
where
    T: ops::Mul<S, Output = T> + Clone + fmt::Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.top = self.top.clone() * rhs.clone();
        self.right = self.right.clone() * rhs.clone();
        self.bottom = self.bottom.clone() * rhs.clone();
        self.left = self.left.clone() * rhs;
    }
}

impl From<Pixels> for Edges<Pixels> {
    fn from(val: Pixels) -> Self {
        Edges::all(val)
    }
}

impl Edges<Pixels> {
    pub fn scale(&self, factor: f32) -> Edges<ScaledPixels> {
        Edges {
            top: self.top.scale(factor),
            right: self.right.scale(factor),
            bottom: self.bottom.scale(factor),
            left: self.left.scale(factor),
        }
    }

    pub fn max(&self) -> Pixels {
        self.top.max(self.right).max(self.bottom).max(self.left)
    }
}

impl Edges<AbsoluteLength> {
    pub fn to_pixels(self, rem_size: Pixels) -> Edges<Pixels> {
        Edges {
            top: self.top.to_pixels(rem_size),
            right: self.right.to_pixels(rem_size),
            bottom: self.bottom.to_pixels(rem_size),
            left: self.left.to_pixels(rem_size),
        }
    }
}
