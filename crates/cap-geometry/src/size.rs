//! Size — 2D dimensions.

use crate::{Along, Axis, Half, Pixels, Point, ScaledPixels};
use std::fmt;
use std::ops;

/// A two-dimensional size with width and height.
#[derive(Clone, Default, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct Size<T: Clone + fmt::Debug + Default + PartialEq> {
    pub width: T,
    pub height: T,
}

pub const fn size<T: Clone + fmt::Debug + Default + PartialEq>(width: T, height: T) -> Size<T> {
    Size { width, height }
}

impl<T: Clone + fmt::Debug + Default + PartialEq> Size<T> {
    pub fn new(width: T, height: T) -> Self {
        size(width, height)
    }

    pub fn map<U: Clone + fmt::Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Size<U> {
        Size {
            width: f(self.width.clone()),
            height: f(self.height.clone()),
        }
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + Half> Size<T> {
    pub fn center(&self) -> Point<T> {
        Point {
            x: self.width.half(),
            y: self.height.half(),
        }
    }
}

impl Size<Pixels> {
    pub fn scale(&self, factor: f32) -> Size<ScaledPixels> {
        Size {
            width: self.width.scale(factor),
            height: self.height.scale(factor),
        }
    }
}

impl<T> Along for Size<T>
where
    T: Clone + fmt::Debug + Default + PartialEq,
{
    type Unit = T;

    fn along(&self, axis: Axis) -> T {
        match axis {
            Axis::Horizontal => self.width.clone(),
            Axis::Vertical => self.height.clone(),
        }
    }

    fn apply_along(&self, axis: Axis, f: impl FnOnce(T) -> T) -> Self {
        match axis {
            Axis::Horizontal => Size {
                width: f(self.width.clone()),
                height: self.height.clone(),
            },
            Axis::Vertical => Size {
                width: self.width.clone(),
                height: f(self.height.clone()),
            },
        }
    }
}

impl<T: PartialOrd + Clone + fmt::Debug + Default + PartialEq> Size<T> {
    pub fn max(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width {
                self.width.clone()
            } else {
                other.width.clone()
            },
            height: if self.height >= other.height {
                self.height.clone()
            } else {
                other.height.clone()
            },
        }
    }

    pub fn min(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width {
                other.width.clone()
            } else {
                self.width.clone()
            },
            height: if self.height >= other.height {
                other.height.clone()
            } else {
                self.height.clone()
            },
        }
    }
}

impl<T, Rhs> ops::Mul<Rhs> for Size<T>
where
    T: ops::Mul<Rhs, Output = Rhs> + Clone + fmt::Debug + Default + PartialEq,
    Rhs: Clone + fmt::Debug + Default + PartialEq,
{
    type Output = Size<Rhs>;
    fn mul(self, rhs: Rhs) -> Self::Output {
        Size {
            width: self.width * rhs.clone(),
            height: self.height * rhs,
        }
    }
}

impl<T, S> ops::MulAssign<S> for Size<T>
where
    T: ops::Mul<S, Output = T> + Clone + fmt::Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.width = self.width.clone() * rhs.clone();
        self.height = self.height.clone() * rhs;
    }
}

impl<T: ops::Add<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::Add for Size<T> {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self {
            width: self.width + rhs.width,
            height: self.height + rhs.height,
        }
    }
}

impl<T: ops::Add<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::AddAssign
    for Size<T>
{
    fn add_assign(&mut self, rhs: Self) {
        self.width = self.width.clone() + rhs.width;
        self.height = self.height.clone() + rhs.height;
    }
}

impl<T: ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::Sub for Size<T> {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            width: self.width - rhs.width,
            height: self.height - rhs.height,
        }
    }
}

impl<T: ops::Neg<Output = T> + Clone + fmt::Debug + Default + PartialEq> ops::Neg for Size<T> {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self {
            width: -self.width,
            height: -self.height,
        }
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + Copy> Copy for Size<T> {}

impl<T: Clone + fmt::Debug + Default + PartialEq> fmt::Debug for Size<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Size {{ {:?} × {:?} }}", self.width, self.height)
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + fmt::Display> fmt::Display for Size<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} × {}", self.width, self.height)
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq> From<Point<T>> for Size<T> {
    fn from(p: Point<T>) -> Self {
        Self {
            width: p.x,
            height: p.y,
        }
    }
}
