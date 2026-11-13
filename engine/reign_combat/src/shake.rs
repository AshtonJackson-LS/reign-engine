/// A screen shake: strength in world pixels and how long it lasts.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    reign_scene::Reflect,
)]
pub struct Shake {
    pub amplitude: u8,
    pub ticks: u16,
}

impl Shake {
    pub const NONE: Shake = Shake { amplitude: 0, ticks: 0 };

    pub const fn new(amplitude: u8, ticks: u16) -> Self {
        Shake { amplitude, ticks }
    }
}

/// The running screen shake. It lives in the simulation so replays shake
/// the same way, but only the renderer reads the offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ScreenShake {
    amplitude: u8,
    duration: u16,
    left: u16,
}

impl ScreenShake {
    /// Starts `shake` unless a stronger one is already running.
    pub fn add(&mut self, shake: Shake) {
        if shake.ticks == 0 || shake.amplitude < self.current() {
            return;
        }
        self.amplitude = shake.amplitude;
        self.duration = shake.ticks;
        self.left = shake.ticks;
    }

    pub fn step(&mut self) {
        self.left = self.left.saturating_sub(1);
    }

    /// Strength right now, fading linearly to zero.
    fn current(&self) -> u8 {
        if self.duration == 0 {
            return 0;
        }
        (self.amplitude as u32 * self.left as u32).div_ceil(self.duration as u32) as u8
    }

    /// Camera offset in whole world pixels. Jumps to a new spot every few
    /// ticks so the shake reads at any refresh rate.
    pub fn offset(&self, tick: u64) -> (i32, i32) {
        let a = self.current() as i32;
        if a == 0 {
            return (0, 0);
        }
        let n = (tick / 3).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let pick = |bits: u64| (bits % (2 * a as u64 + 1)) as i32 - a;
        (pick(n >> 16), pick(n >> 40))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fades_out_and_stays_in_bounds() {
        let mut s = ScreenShake::default();
        s.add(Shake::new(3, 12));
        for tick in 0..12 {
            let (x, y) = s.offset(tick);
            assert!(x.abs() <= 3 && y.abs() <= 3);
            s.step();
        }
        assert_eq!(s.offset(99), (0, 0));
    }

    #[test]
    fn weaker_shakes_do_not_cut_stronger_ones() {
        let mut s = ScreenShake::default();
        s.add(Shake::new(3, 12));
        s.add(Shake::new(1, 30));
        assert_eq!(s.current(), 3);
    }
}
