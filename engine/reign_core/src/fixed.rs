//! Fixed-point numbers for positions and velocities.
//!
//! One unit is 1/256 of a world pixel, so movement can be tuned in subpixels
//! while staying bit-exact across machines.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

/// Signed fixed-point value with 8 fractional bits (24.8).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Fx(i32);

impl Fx {
    pub const FRAC_BITS: u32 = 8;
    pub const ONE: Fx = Fx(1 << Self::FRAC_BITS);
    pub const ZERO: Fx = Fx(0);

    pub const fn from_raw(raw: i32) -> Self {
        Fx(raw)
    }

    pub const fn raw(self) -> i32 {
        self.0
    }

    pub const fn from_px(px: i32) -> Self {
        Fx(px << Self::FRAC_BITS)
    }

    /// Whole pixels plus a subpixel fraction in 1/256ths, for tuning tables.
    pub const fn from_subpx(px: i32, sub: i32) -> Self {
        Fx((px << Self::FRAC_BITS) + sub)
    }

    /// Rounds toward negative infinity to the containing pixel.
    pub const fn floor_px(self) -> i32 {
        self.0 >> Self::FRAC_BITS
    }

    /// Rounds to the nearest whole pixel, halves rounding up.
    pub const fn round_px(self) -> i32 {
        (self.0 + (1 << (Self::FRAC_BITS - 1))) >> Self::FRAC_BITS
    }

    pub const fn signum(self) -> i32 {
        self.0.signum()
    }

    pub const fn abs(self) -> Self {
        Fx(self.0.abs())
    }

    pub fn clamp(self, lo: Fx, hi: Fx) -> Self {
        Fx(self.0.clamp(lo.0, hi.0))
    }

    /// Moves toward `target` by at most `step`, never overshooting.
    pub fn approach(self, target: Fx, step: Fx) -> Self {
        if self < target {
            Fx((self.0 + step.0).min(target.0))
        } else {
            Fx((self.0 - step.0).max(target.0))
        }
    }

    /// For presentation only. Never feed the result back into the simulation.
    pub fn to_f32(self) -> f32 {
        self.0 as f32 / Self::ONE.0 as f32
    }
}

impl Add for Fx {
    type Output = Fx;
    fn add(self, rhs: Fx) -> Fx {
        Fx(self.0 + rhs.0)
    }
}

impl AddAssign for Fx {
    fn add_assign(&mut self, rhs: Fx) {
        self.0 += rhs.0;
    }
}

impl Sub for Fx {
    type Output = Fx;
    fn sub(self, rhs: Fx) -> Fx {
        Fx(self.0 - rhs.0)
    }
}

impl SubAssign for Fx {
    fn sub_assign(&mut self, rhs: Fx) {
        self.0 -= rhs.0;
    }
}

impl Neg for Fx {
    type Output = Fx;
    fn neg(self) -> Fx {
        Fx(-self.0)
    }
}

impl Mul for Fx {
    type Output = Fx;
    fn mul(self, rhs: Fx) -> Fx {
        Fx(((self.0 as i64 * rhs.0 as i64) >> Self::FRAC_BITS) as i32)
    }
}

impl Mul<i32> for Fx {
    type Output = Fx;
    fn mul(self, rhs: i32) -> Fx {
        Fx(self.0 * rhs)
    }
}

impl Div<i32> for Fx {
    type Output = Fx;
    fn div(self, rhs: i32) -> Fx {
        Fx(self.0 / rhs)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FxVec2 {
    pub x: Fx,
    pub y: Fx,
}

impl FxVec2 {
    pub const ZERO: FxVec2 = FxVec2 { x: Fx::ZERO, y: Fx::ZERO };

    pub const fn new(x: Fx, y: Fx) -> Self {
        FxVec2 { x, y }
    }

    pub const fn from_px(x: i32, y: i32) -> Self {
        FxVec2 { x: Fx::from_px(x), y: Fx::from_px(y) }
    }
}

impl Add for FxVec2 {
    type Output = FxVec2;
    fn add(self, rhs: FxVec2) -> FxVec2 {
        FxVec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl AddAssign for FxVec2 {
    fn add_assign(&mut self, rhs: FxVec2) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl Sub for FxVec2 {
    type Output = FxVec2;
    fn sub(self, rhs: FxVec2) -> FxVec2 {
        FxVec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl SubAssign for FxVec2 {
    fn sub_assign(&mut self, rhs: FxVec2) {
        self.x -= rhs.x;
        self.y -= rhs.y;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_round_trip() {
        assert_eq!(Fx::from_px(-3).floor_px(), -3);
        assert_eq!(Fx::from_subpx(2, 255).floor_px(), 2);
        assert_eq!(Fx::from_subpx(-1, 128).floor_px(), -1);
        assert_eq!(Fx::from_subpx(2, 128).round_px(), 3);
        assert_eq!(Fx::from_subpx(2, 127).round_px(), 2);
        assert_eq!(Fx::from_subpx(-3, 128).round_px(), -2);
    }

    #[test]
    fn multiply_keeps_scale() {
        let half = Fx::from_raw(128);
        assert_eq!(Fx::from_px(6) * half, Fx::from_px(3));
        assert_eq!(Fx::from_px(-6) * half, Fx::from_px(-3));
    }

    #[test]
    fn approach_never_overshoots() {
        let v = Fx::from_px(1).approach(Fx::from_px(2), Fx::from_px(5));
        assert_eq!(v, Fx::from_px(2));
        let v = Fx::from_px(1).approach(Fx::from_px(-2), Fx::from_px(1));
        assert_eq!(v, Fx::ZERO);
    }
}
