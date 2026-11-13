//! Stable asset identities.

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An asset's identity, kept in its `.meta` file so it survives moves and
/// renames. Written as 32 lowercase hex digits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub u128);

impl Guid {
    /// A fresh, effectively unique id. Only for creating assets: never call
    /// it from simulation code.
    pub fn new() -> Guid {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let mut halves = [0u64; 2];
        for (i, half) in halves.iter_mut().enumerate() {
            let mut h = RandomState::new().build_hasher();
            h.write_u128(now);
            h.write_u64(n);
            h.write_usize(i);
            *half = h.finish();
        }
        Guid((halves[0] as u128) << 64 | halves[1] as u128)
    }

    pub const NIL: Guid = Guid(0);

    pub fn is_nil(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl FromStr for Guid {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 32 {
            return Err(format!("{s:?} is not a 32-digit hex GUID"));
        }
        u128::from_str_radix(s, 16).map(Guid).map_err(|_| format!("{s:?} is not a hex GUID"))
    }
}

impl Serialize for Guid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Guid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_are_unique_and_round_trip_as_text() {
        let a = Guid::new();
        let b = Guid::new();
        assert_ne!(a, b);
        assert_eq!(a.to_string().len(), 32);
        assert_eq!(a.to_string().parse::<Guid>(), Ok(a));
        assert_eq!(Guid(1).to_string(), "00000000000000000000000000000001");
        assert!("xyz".parse::<Guid>().is_err());
        let text = ron::to_string(&a).unwrap();
        assert_eq!(ron::from_str::<Guid>(&text).unwrap(), a);
    }
}
