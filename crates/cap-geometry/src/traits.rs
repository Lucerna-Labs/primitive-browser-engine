//! Common traits — Half, IsZero.

/// Types that can compute half their value.
pub trait Half {
    fn half(&self) -> Self;
}

impl Half for i32 {
    fn half(&self) -> Self {
        self / 2
    }
}

impl Half for f32 {
    fn half(&self) -> Self {
        self / 2.0
    }
}

impl Half for crate::Pixels {
    fn half(&self) -> Self {
        Self(self.0 / 2.0)
    }
}

impl Half for crate::DevicePixels {
    fn half(&self) -> Self {
        Self(self.0 / 2)
    }
}

impl Half for crate::ScaledPixels {
    fn half(&self) -> Self {
        Self(self.0 / 2.0)
    }
}

impl Half for crate::Rems {
    fn half(&self) -> Self {
        Self(self.0 / 2.0)
    }
}

/// Types that can report whether they are zero.
pub trait IsZero {
    fn is_zero(&self) -> bool;
}

impl IsZero for crate::Pixels {
    fn is_zero(&self) -> bool {
        self.0 == 0.0
    }
}

impl IsZero for crate::DevicePixels {
    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl IsZero for crate::ScaledPixels {
    fn is_zero(&self) -> bool {
        self.0 == 0.0
    }
}

impl IsZero for crate::Rems {
    fn is_zero(&self) -> bool {
        self.0 == 0.0
    }
}

impl IsZero for crate::AbsoluteLength {
    fn is_zero(&self) -> bool {
        match self {
            crate::AbsoluteLength::Pixels(p) => p.is_zero(),
            crate::AbsoluteLength::Rems(r) => r.is_zero(),
        }
    }
}

impl IsZero for crate::DefiniteLength {
    fn is_zero(&self) -> bool {
        match self {
            crate::DefiniteLength::Absolute(l) => l.is_zero(),
            crate::DefiniteLength::Fraction(f) => *f == 0.0,
        }
    }
}

impl IsZero for crate::Length {
    fn is_zero(&self) -> bool {
        match self {
            crate::Length::Definite(l) => l.is_zero(),
            crate::Length::Auto => false,
        }
    }
}

impl<T: IsZero + Clone + std::fmt::Debug + Default + PartialEq> IsZero for crate::Point<T> {
    fn is_zero(&self) -> bool {
        self.x.is_zero() && self.y.is_zero()
    }
}

impl<T: IsZero + Clone + std::fmt::Debug + Default + PartialEq> IsZero for crate::Size<T> {
    fn is_zero(&self) -> bool {
        self.width.is_zero() || self.height.is_zero()
    }
}

impl<T: IsZero + Clone + std::fmt::Debug + Default + PartialEq> IsZero for crate::Bounds<T> {
    fn is_zero(&self) -> bool {
        self.size.is_zero()
    }
}

impl<T: IsZero + Clone + std::fmt::Debug + Default + PartialEq> IsZero for crate::Corners<T> {
    fn is_zero(&self) -> bool {
        self.top_left.is_zero()
            && self.top_right.is_zero()
            && self.bottom_right.is_zero()
            && self.bottom_left.is_zero()
    }
}
