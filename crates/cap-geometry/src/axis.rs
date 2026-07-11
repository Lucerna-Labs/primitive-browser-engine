//! Axis, Anchor, and axis-aware trait.

/// Axis in a 2D cartesian space.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Axis {
    Vertical,
    Horizontal,
}

impl Axis {
    /// Swap to the opposite axis.
    pub fn invert(self) -> Self {
        match self {
            Axis::Vertical => Axis::Horizontal,
            Axis::Horizontal => Axis::Vertical,
        }
    }
}

/// Identifies a reference point on a 2D box (corners + edges).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    TopCenter,
    BottomCenter,
    LeftCenter,
    RightCenter,
}

impl Anchor {
    pub fn opposite(self) -> Self {
        match self {
            Anchor::TopLeft => Anchor::BottomRight,
            Anchor::TopRight => Anchor::BottomLeft,
            Anchor::BottomLeft => Anchor::TopRight,
            Anchor::BottomRight => Anchor::TopLeft,
            Anchor::TopCenter => Anchor::BottomCenter,
            Anchor::BottomCenter => Anchor::TopCenter,
            Anchor::LeftCenter => Anchor::RightCenter,
            Anchor::RightCenter => Anchor::LeftCenter,
        }
    }

    pub fn other_side_along(self, axis: Axis) -> Self {
        match axis {
            Axis::Vertical => match self {
                Anchor::TopLeft => Anchor::BottomLeft,
                Anchor::TopRight => Anchor::BottomRight,
                Anchor::BottomLeft => Anchor::TopLeft,
                Anchor::BottomRight => Anchor::TopRight,
                Anchor::TopCenter => Anchor::BottomCenter,
                Anchor::BottomCenter => Anchor::TopCenter,
                a => a,
            },
            Axis::Horizontal => match self {
                Anchor::TopLeft => Anchor::TopRight,
                Anchor::TopRight => Anchor::TopLeft,
                Anchor::BottomLeft => Anchor::BottomRight,
                Anchor::BottomRight => Anchor::BottomLeft,
                Anchor::LeftCenter => Anchor::RightCenter,
                Anchor::RightCenter => Anchor::LeftCenter,
                a => a,
            },
        }
    }

    pub fn is_center(&self) -> bool {
        matches!(
            self,
            Self::TopCenter | Self::BottomCenter | Self::LeftCenter | Self::RightCenter
        )
    }
}

/// Trait for accessing a value along a given axis.
pub trait Along {
    type Unit;

    /// Get the value along the given axis.
    fn along(&self, axis: Axis) -> Self::Unit;

    /// Apply a function to the value along the given axis.
    fn apply_along(&self, axis: Axis, f: impl FnOnce(Self::Unit) -> Self::Unit) -> Self;
}
