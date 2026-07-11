//! Bounds — rectangular area in 2D space.

use crate::{Anchor, Edges, Half, Pixels, Point, ScaledPixels, Size, point, size};
use std::fmt;
use std::ops;

/// A rectangular area defined by an origin point and a size.
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct Bounds<T: Clone + fmt::Debug + Default + PartialEq> {
    pub origin: Point<T>,
    pub size: Size<T>,
}

pub fn bounds<T: Clone + fmt::Debug + Default + PartialEq>(
    origin: Point<T>,
    size: Size<T>,
) -> Bounds<T> {
    Bounds { origin, size }
}

impl<T: Clone + fmt::Debug + Default + PartialEq> Bounds<T> {
    pub fn new(origin: Point<T>, s: Size<T>) -> Self {
        Bounds { origin, size: s }
    }
}

impl<T> Bounds<T>
where
    T: ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn from_corners(top_left: Point<T>, bottom_right: Point<T>) -> Self {
        Bounds {
            origin: Point {
                x: top_left.x.clone(),
                y: top_left.y.clone(),
            },
            size: Size {
                width: bottom_right.x - top_left.x,
                height: bottom_right.y - top_left.y,
            },
        }
    }
}

impl<T> Bounds<T>
where
    T: ops::Sub<T, Output = T> + Half + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn centered_at(center: Point<T>, s: Size<T>) -> Self {
        let origin = Point {
            x: center.x - s.width.half(),
            y: center.y - s.height.half(),
        };
        Self::new(origin, s)
    }

    pub fn from_anchor_and_size(corner: Anchor, origin: Point<T>, s: Size<T>) -> Bounds<T> {
        let origin = match corner {
            Anchor::TopLeft => origin,
            Anchor::TopRight => Point {
                x: origin.x - s.width.clone(),
                y: origin.y,
            },
            Anchor::BottomLeft => Point {
                x: origin.x,
                y: origin.y - s.height.clone(),
            },
            Anchor::BottomRight => Point {
                x: origin.x - s.width.clone(),
                y: origin.y - s.height.clone(),
            },
            Anchor::TopCenter => Point {
                x: origin.x - s.width.half(),
                y: origin.y,
            },
            Anchor::BottomCenter => Point {
                x: origin.x - s.width.half(),
                y: origin.y - s.height.clone(),
            },
            Anchor::LeftCenter => Point {
                x: origin.x,
                y: origin.y - s.height.half(),
            },
            Anchor::RightCenter => Point {
                x: origin.x - s.width.clone(),
                y: origin.y - s.height.half(),
            },
        };
        Bounds { origin, size: s }
    }
}

impl<T> Bounds<T>
where
    T: ops::Add<T, Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn top(&self) -> T {
        self.origin.y.clone()
    }

    pub fn bottom(&self) -> T {
        self.origin.y.clone() + self.size.height.clone()
    }

    pub fn left(&self) -> T {
        self.origin.x.clone()
    }

    pub fn right(&self) -> T {
        self.origin.x.clone() + self.size.width.clone()
    }

    pub fn top_right(&self) -> Point<T> {
        Point {
            x: self.right(),
            y: self.top(),
        }
    }

    pub fn bottom_right(&self) -> Point<T> {
        Point {
            x: self.right(),
            y: self.bottom(),
        }
    }

    pub fn bottom_left(&self) -> Point<T> {
        Point {
            x: self.left(),
            y: self.bottom(),
        }
    }

    pub fn contains(&self, p: &Point<T>) -> bool
    where
        T: PartialOrd,
    {
        p.x >= self.origin.x
            && p.x < self.origin.x.clone() + self.size.width.clone()
            && p.y >= self.origin.y
            && p.y < self.origin.y.clone() + self.size.height.clone()
    }

    pub fn is_contained_within(&self, other: &Self) -> bool
    where
        T: PartialOrd,
    {
        other.contains(&self.origin) && other.contains(&self.bottom_right())
    }

    pub fn map<U: Clone + fmt::Debug + Default + PartialEq>(
        &self,
        f: impl Fn(T) -> U,
    ) -> Bounds<U> {
        Bounds {
            origin: self.origin.map(&f),
            size: self.size.map(f),
        }
    }

    pub fn map_origin(self, f: impl Fn(T) -> T) -> Self {
        Bounds {
            origin: self.origin.map(f),
            size: self.size,
        }
    }

    pub fn map_size(self, f: impl Fn(T) -> T) -> Self {
        Bounds {
            origin: self.origin,
            size: self.size.map(f),
        }
    }

    pub fn half_perimeter(&self) -> T {
        self.size.width.clone() + self.size.height.clone()
    }
}

impl<T> Bounds<T>
where
    T: ops::Add<T, Output = T> + Half + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone().half(),
            y: self.origin.y.clone() + self.size.height.clone().half(),
        }
    }

    pub fn top_center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.half(),
            y: self.origin.y.clone(),
        }
    }

    pub fn bottom_center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.half(),
            y: self.bottom(),
        }
    }

    pub fn left_center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone(),
            y: self.origin.y.clone() + self.size.height.half(),
        }
    }

    pub fn right_center(&self) -> Point<T> {
        Point {
            x: self.right(),
            y: self.origin.y.clone() + self.size.height.half(),
        }
    }

    pub fn corner(&self, c: Anchor) -> Point<T> {
        match c {
            Anchor::TopLeft => self.origin.clone(),
            Anchor::TopRight => self.top_right(),
            Anchor::BottomLeft => self.bottom_left(),
            Anchor::BottomRight => self.bottom_right(),
            Anchor::TopCenter => self.top_center(),
            Anchor::BottomCenter => self.bottom_center(),
            Anchor::LeftCenter => self.left_center(),
            Anchor::RightCenter => self.right_center(),
        }
    }
}

impl<T> Bounds<T>
where
    T: ops::Add<T, Output = T>
        + ops::Sub<T, Output = T>
        + PartialOrd
        + Clone
        + fmt::Debug
        + Default
        + PartialEq,
{
    pub fn intersects(&self, other: &Self) -> bool {
        let my_br = self.bottom_right();
        let their_br = other.bottom_right();
        self.origin.x < their_br.x
            && my_br.x > other.origin.x
            && self.origin.y < their_br.y
            && my_br.y > other.origin.y
    }

    pub fn intersect(&self, other: &Self) -> Self {
        let ul = self.origin.max(&other.origin);
        let br = self.bottom_right().min(&other.bottom_right()).max(&ul);
        Self::from_corners(ul, br)
    }

    pub fn union(&self, other: &Self) -> Self {
        let tl = self.origin.min(&other.origin);
        let br = self.bottom_right().max(&other.bottom_right());
        Bounds::from_corners(tl, br)
    }

    pub fn localize(&self, p: &Point<T>) -> Option<Point<T>> {
        self.contains(p).then(|| p.relative_to(&self.origin))
    }

    pub fn is_empty(&self) -> bool {
        self.size.width <= T::default() || self.size.height <= T::default()
    }
}

impl<T> Bounds<T>
where
    T: ops::Add<T, Output = T> + ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    pub fn dilate(&self, amount: T) -> Bounds<T>
    where
        T: Clone,
    {
        let double = amount.clone() + amount.clone();
        Bounds {
            origin: self.origin.clone() - point(amount.clone(), amount),
            size: self.size.clone() + size(double.clone(), double),
        }
    }

    pub fn extend(&self, amount: Edges<T>) -> Bounds<T> {
        Bounds {
            origin: self.origin.clone() - point(amount.left.clone(), amount.top.clone()),
            size: self.size.clone()
                + size(
                    amount.left.clone() + amount.right.clone(),
                    amount.top.clone() + amount.bottom,
                ),
        }
    }

    pub fn inset(&self, amount: T) -> Bounds<T>
    where
        T: ops::Neg<Output = T>,
    {
        self.dilate(-amount)
    }

    pub fn space_within(&self, outer: &Self) -> Edges<T> {
        Edges {
            top: self.top() - outer.top(),
            right: outer.right() - self.right(),
            bottom: outer.bottom() - self.bottom(),
            left: self.left() - outer.left(),
        }
    }
}

impl Bounds<Pixels> {
    pub fn scale(&self, factor: f32) -> Bounds<ScaledPixels> {
        Bounds {
            origin: self.origin.scale(factor),
            size: self.size.scale(factor),
        }
    }
}

impl<T, Rhs> ops::Mul<Rhs> for Bounds<T>
where
    T: ops::Mul<Rhs, Output = Rhs> + Clone + fmt::Debug + Default + PartialEq,
    Point<T>: ops::Mul<Rhs, Output = Point<Rhs>>,
    Rhs: Clone + fmt::Debug + Default + PartialEq,
{
    type Output = Bounds<Rhs>;
    fn mul(self, rhs: Rhs) -> Self::Output {
        Bounds {
            origin: self.origin * rhs.clone(),
            size: self.size * rhs,
        }
    }
}

impl<T> ops::Add<Point<T>> for Bounds<T>
where
    T: ops::Add<T, Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    type Output = Self;
    fn add(self, rhs: Point<T>) -> Self::Output {
        Self {
            origin: self.origin + rhs,
            size: self.size,
        }
    }
}

impl<T> ops::Sub<Point<T>> for Bounds<T>
where
    T: ops::Sub<T, Output = T> + Clone + fmt::Debug + Default + PartialEq,
{
    type Output = Self;
    fn sub(self, rhs: Point<T>) -> Self::Output {
        Self {
            origin: self.origin - rhs,
            size: self.size,
        }
    }
}

impl<T: Clone + fmt::Debug + Default + PartialEq + fmt::Display + ops::Add<T, Output = T>>
    fmt::Display for Bounds<T>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} - {} (size {})",
            self.origin,
            self.bottom_right(),
            self.size
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::px;

    #[test]
    fn test_bounds_intersects() {
        let b1 = Bounds::new(point(px(0.0), px(0.0)), size(px(5.0), px(5.0)));
        let b2 = Bounds::new(point(px(4.0), px(4.0)), size(px(5.0), px(5.0)));
        let b3 = Bounds::new(point(px(10.0), px(10.0)), size(px(5.0), px(5.0)));

        assert!(b1.intersects(&b2));
        assert!(!b1.intersects(&b3));
        assert!(b1.intersects(&b1));
    }

    #[test]
    fn test_bounds_contains() {
        let b = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0)));
        assert!(b.contains(&point(px(5.0), px(5.0))));
        assert!(!b.contains(&point(px(15.0), px(15.0))));
    }

    #[test]
    fn test_bounds_intersect_union() {
        let b1 = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(10.0)));
        let b2 = Bounds::new(point(px(5.0), px(5.0)), size(px(10.0), px(10.0)));

        let intersection = b1.intersect(&b2);
        assert_eq!(intersection.origin, point(px(5.0), px(5.0)));
        assert_eq!(intersection.size, size(px(5.0), px(5.0)));

        let union = b1.union(&b2);
        assert_eq!(union.origin, point(px(0.0), px(0.0)));
        assert_eq!(union.size, size(px(15.0), px(15.0)));
    }

    #[test]
    fn test_bounds_center() {
        let b = Bounds::new(point(px(0.0), px(0.0)), size(px(10.0), px(20.0)));
        assert_eq!(b.center(), point(px(5.0), px(10.0)));
    }

    #[test]
    fn test_bounds_dilate_inset() {
        let b = Bounds::new(point(px(10.0), px(10.0)), size(px(10.0), px(10.0)));
        let dilated = b.dilate(px(5.0));
        assert_eq!(dilated.origin, point(px(5.0), px(5.0)));
        assert_eq!(dilated.size, size(px(20.0), px(20.0)));

        let inset = b.inset(px(5.0));
        assert_eq!(inset.origin, point(px(15.0), px(15.0)));
        assert_eq!(inset.size, size(px(0.0), px(0.0)));
    }

    #[test]
    fn test_pixels_construction() {
        let p = px(42.0);
        assert_eq!(p.as_f32(), 42.0);
        assert_eq!(p.floor().as_f32(), 42.0);
        assert_eq!(px(42.7).floor().as_f32(), 42.0);
        assert_eq!(px(42.3).ceil().as_f32(), 43.0);
    }

    #[test]
    fn test_point_magnitude() {
        let p = point(px(3.0), px(4.0));
        assert!((p.magnitude() - 5.0).abs() < 0.001);
    }
}
