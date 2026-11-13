//! Seeded RNG for gameplay randomness. Part of simulation state, so it is
//! saved, restored and rolled back with everything else.

/// PCG32 (XSH RR variant).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Rng {
    state: u64,
}

impl Rng {
    const MUL: u64 = 6364136223846793005;
    const INC: u64 = 1442695040888963407;

    pub fn new(seed: u64) -> Self {
        let mut rng = Rng { state: 0 };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(Self::MUL).wrapping_add(Self::INC);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        xorshifted.rotate_right((old >> 59) as u32)
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        let span = (hi - lo) as u32 + 1;
        lo + (self.next_u32() % span) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn range_is_inclusive() {
        let mut rng = Rng::new(7);
        for _ in 0..1000 {
            let v = rng.range(-2, 2);
            assert!((-2..=2).contains(&v));
        }
    }
}
