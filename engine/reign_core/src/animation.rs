//! Sprite timelines sampled on the simulation clock, including during hitstop.

use crate::TICK_HZ;

#[derive(Clone, Copy, Debug)]
pub struct Clip {
    pub row: u32,
    pub ms: &'static [u16],
    pub looping: bool,
}

impl Clip {
    /// First tick at or after a frame boundary. The end boundary is allowed.
    pub const fn tick_at_frame(&self, frame: usize) -> u16 {
        assert!(frame <= self.ms.len());
        let mut ms = 0u32;
        let mut i = 0;
        while i < frame {
            ms += self.ms[i] as u32;
            i += 1;
        }
        (ms * TICK_HZ).div_ceil(1000) as u16
    }

    pub const fn ticks(&self) -> u16 {
        self.tick_at_frame(self.ms.len())
    }

    /// Loop at the authored duration, or hold the final frame once finished.
    pub fn frame(&self, tick: u32) -> u32 {
        assert!(!self.ms.is_empty());
        let total: u64 = self.ms.iter().map(|&ms| ms as u64 * TICK_HZ as u64).sum();
        assert!(total > 0);
        let mut time = tick as u64 * 1000;
        if self.looping {
            time %= total;
        }
        let mut end = 0;
        for (i, &ms) in self.ms.iter().enumerate() {
            end += ms as u64 * TICK_HZ as u64;
            if time < end {
                return i as u32;
            }
        }
        self.ms.len() as u32 - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_boundaries_agree_with_simulation_ticks() {
        let clip = Clip { row: 0, ms: &[170, 130, 60, 80, 160, 120, 120], looping: false };
        assert_eq!(clip.tick_at_frame(2), 36);
        assert_eq!(clip.tick_at_frame(3), 44);
        assert_eq!(clip.frame(35), 1);
        assert_eq!(clip.frame(36), 2);
        assert_eq!(clip.frame(43), 2);
        assert_eq!(clip.frame(44), 3);
        assert_eq!(clip.frame(1000), 6);
        let looping = Clip { row: 0, ms: &[100, 100], looping: true };
        assert_eq!(looping.frame(24), 0);
    }
}
