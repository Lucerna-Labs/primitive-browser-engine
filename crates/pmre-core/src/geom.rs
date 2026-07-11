//! Geometry mechanism: 2-D points/vectors and affine transforms.
//! `Affine::apply` is the `project` root atom specialized to a 2-D point × matrix.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn scale(self, s: f32) -> Vec2 {
        Vec2::new(self.x * s, self.y * s)
    }
    pub fn dot(self, o: Vec2) -> f32 {
        self.x * o.x + self.y * o.y
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    pub fn abs(self) -> Vec2 {
        Vec2::new(self.x.abs(), self.y.abs())
    }
    /// component-wise max with a scalar.
    pub fn max_scalar(self, m: f32) -> Vec2 {
        Vec2::new(self.x.max(m), self.y.max(m))
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

/// 2-D affine transform mapping local space to device space:
/// `x' = a*x + c*y + e`, `y' = b*x + d*y + f`.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f32, y: f32) -> Affine {
        Affine {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: x,
            f: y,
        }
    }

    pub fn scale(s: f32) -> Affine {
        Affine {
            a: s,
            b: 0.0,
            c: 0.0,
            d: s,
            e: 0.0,
            f: 0.0,
        }
    }

    /// Compose: apply `self` first, then `next`.
    pub fn then(self, next: Affine) -> Affine {
        Affine {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    /// The `project` atom: transform a point through the matrix.
    pub fn apply(self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.a * p.x + self.c * p.y + self.e,
            self.b * p.x + self.d * p.y + self.f,
        )
    }

    pub fn determinant(self) -> f32 {
        self.a * self.d - self.b * self.c
    }

    pub fn inverse(self) -> Affine {
        let inv = 1.0 / self.determinant();
        let a = self.d * inv;
        let b = -self.b * inv;
        let c = -self.c * inv;
        let d = self.a * inv;
        Affine {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        }
    }

    /// Average linear scale (exact for similarity transforms); used to size the AA band.
    pub fn scale_factor(self) -> f32 {
        self.determinant().abs().sqrt()
    }
}
