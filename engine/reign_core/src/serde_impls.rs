//! Serde support, with the `serde` feature. [`Fx`] is written as an exact
//! decimal string (`"12.5"`), so scene files stay readable and load to the
//! same bits everywhere. Whole numbers are accepted as plain integers too;
//! floating-point numbers are refused rather than rounded through `f64`.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Fx, FxVec2};

impl Serialize for Fx {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Fx {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct FxVisitor;
        impl Visitor<'_> for FxVisitor {
            type Value = Fx;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a decimal string like \"-12.375\", or a whole number")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Fx, E> {
                Fx::parse_decimal(v).map_err(E::custom)
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Fx, E> {
                Fx::parse_decimal(&v.to_string()).map_err(E::custom)
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Fx, E> {
                Fx::parse_decimal(&v.to_string()).map_err(E::custom)
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Fx, E> {
                Err(E::custom(format!(
                    "{v} is a float; write fixed-point numbers as decimal strings, like \"{v}\""
                )))
            }
        }
        d.deserialize_any(FxVisitor)
    }
}

impl Serialize for FxVec2 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut v = s.serialize_struct("FxVec2", 2)?;
        v.serialize_field("x", &self.x)?;
        v.serialize_field("y", &self.y)?;
        v.end()
    }
}

impl<'de> Deserialize<'de> for FxVec2 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename = "FxVec2", deny_unknown_fields)]
        struct Raw {
            x: Fx,
            y: Fx,
        }
        let Raw { x, y } = Raw::deserialize(d)?;
        Ok(FxVec2 { x, y })
    }
}
