//! Stable state fingerprints for replay checks and rollback desync detection.
//!
//! `std`'s `DefaultHasher` is allowed to change between Rust releases, so two
//! peers on different toolchains could disagree about identical state. FNV-1a
//! is fixed forever and cheap enough to run on every saved tick.

use std::hash::{Hash, Hasher};

/// 64-bit FNV-1a.
#[derive(Clone, Debug)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Fnv64(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv64 {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    // Sizes are hashed as u64 so 32- and 64-bit builds agree.
    fn write_usize(&mut self, n: usize) {
        self.write(&(n as u64).to_le_bytes());
    }

    fn write_isize(&mut self, n: isize) {
        self.write(&(n as i64).to_le_bytes());
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

/// The fingerprint of anything hashable.
pub fn checksum<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut h = Fnv64::default();
    value.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        // FNV-1a 64 of "a", from the reference test suite.
        let mut h = Fnv64::default();
        h.write(b"a");
        assert_eq!(h.finish(), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn different_values_differ() {
        assert_ne!(checksum(&(1u32, 2u32)), checksum(&(2u32, 1u32)));
    }
}
