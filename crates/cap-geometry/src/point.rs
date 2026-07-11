//! Point — 2D location.

use crate::{Along, Axis, Pixels, ScaledPixels};
use std::fmt;
use std::ops;

/// A location in 2D space.
#[derive(Copy, Clone, Default, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct Point<T: Clone + fmt::Debug + Default + PartialEq> {
    pub x: T,
    pub y: T,
}

pub const fn point<T: Clone + fmt::Debug + Default + PartialEq>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T: Clone + fmt::Debug + Default + PartialEq> Point<T> {
    pub const fn new(x: T, y: T) -> Self {
        Point { x, y }
    }

    pub fn map<U: Clone + fmt::Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Point<U> {
        Point {
            x: f(self.x.clone()),
            y: f(self.y.clone()),
        }
    }
}

impl<T> Along for Point<T>
where
    T: Clone + fmt::Debug + Default + PartialEq,
{
    type Unit = T;

    fn along(&self, axis: Axis) -> T {
        match axis {
            Axis::Horizontal => self.x.clone(),
            Axis::Vertical => self.y.clone(),
        }
    }

    fn apply_along(&self, axis: Axis, f: impl FnOnce(T) -> T) -> Point<T> {
        match axis {
            Axis::Horizontal => Point {
                x: f(self.x.clone()),
                y: self.y.clone(),
            },
            Axis::Vertical => Point {
                x: self.x.clone(),
                y: f(self.y.clone()),
            },
        }
    }
}

impl Point<Pixels> {
    pub fn scale(&self, factor: f32) -> Point<ScaledPixels> {
        Point {
            x: self.x.scale(factor),
            y: self.y.scale(factor),
        }
    }

    pub fn magnitude(&self) -> f64 {
        ((self.x.0.powi(2) + self.y.0.powi(2)) as f64).sqrt()
    }
}

impl<T> Point<T>
where
    T: ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn relative_to(&self, origin: &Point<T>) -> Point<T> {
        point(
            self.x.clone() - origin.x.clone(),
            self.y.clone() - origin.y.clone(),
        )
    }
}

impl<T, Rhs> ops::Mul<Rhs> for Point<T>
where
    T: ops::Mul<Rhs, Output = T> + Clone + fmt::Debug + Default + PartialEq,
    Rhs: Clone + fmt::Debug,
{
    type Output = Point<T>;
    fn mul(self, rhs: Rhs) -> Self::Output {
        Point {
            x: self.x * rhs.clone(),
            y: self.y * rhs,
        }
    }
}

impl<T, S> ops::MulAssign<S> for Point<T>
where
    T: ops::Mul<S, Output = T> + Clone + fmt::Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.x = self.x.clone() * rhs.clone();
        self.y = self.y.clone() * rhs;
    }
}

impl<T, S> ops::Div<S> for Point<T>
where
    T: ops::Div<S, Output = T> + Clone + fmt::Debug + Default + PartialEq,
    S: Clone,
{
    type Output = Self;
    fn div(self, rhs: S) -> Self::Output {
        Self {
            x: self.x / rhs.clone(),
            y: self.y / rhs,
        }
    }
}

impl<T: ops::Add<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::Add for Point<T> {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl<T: ops::Add<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::AddAssign
    for Point<T>
{
    fn add_assign(&mut self, rhs: Self) {
        self.x = self.x.clone() + rhs.x;
        self.y = self.y.clone() + rhs.y;
    }
}

impl<T: ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::Sub for Point<T> {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

impl<T: ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::SubAssign
    for Point<T>
{
    fn sub_assign(&mut self, rhs: Self) {
        self.x = self.x.clone() - rhs.x;
        self.y = self.y.clone() - rhs.y;
    }
}

impl<T: ops::Neg<Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::Neg for Point<T> {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self {
            x: -self.x,
            y: -self.y,
        }
    }
}

impl<T: PartialOrd + Clone + fmt::Debug + Default + PartialEq> Point<T> {
    pub fn max(&self, other: &Self) -> Self {
        Point {
            x: if self.x >= other.x {
                self.x.clone()
            } else {
                other.x.clone()
            },
            y: if self.y >= other.y {
                self.y.clone()
            } else {
                other.y.clone()
            },
        }
    }

    pub fn min(&self, other: &Self) -> Self {
        Point {
            x: if self.x <= other.x {
                self.x.clone()
            } else {
                other.x.clone()
            },
            y: if self.y <= other.y {
                self.y.clone()
            } else {
                other.y.clone()
            },
        }
    }

    pub fn clamp(&self, min: &Self, max: &Self) -> Self {
        self.max(min).min(max)
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + fmt::Display> fmt::Display for Point<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq> fmt::Debug for Point<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Point {{ x: {:?}, y: {:?} }}", self.x, self.y)
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq> From<crate::Size<T>> for Point<T> {
    fn from(size: crate::Size<T>) -> Self {
        Self {
            x: size.width,
            y: size.height,
        }
    }
}
