//! Typed handles: an asset's GUID, tagged with the asset type.

use std::any::Any;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use reign_scene::reflect::{Mut, Reflect, ReflectAsset};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Asset, Guid};

/// A reference to an asset of type `T`, by GUID.
///
/// A handle is plain data (`Copy + Eq + Hash`, written as the GUID's hex
/// text), so it can sit in simulation state and scene files. A nil handle
/// (`Handle::NONE`) points at nothing.
pub struct Handle<T> {
    pub guid: Guid,
    _type: PhantomData<fn() -> T>,
}

impl<T> Handle<T> {
    pub const NONE: Handle<T> = Handle::new(Guid::NIL);

    pub const fn new(guid: Guid) -> Self {
        Handle { guid, _type: PhantomData }
    }

    pub fn is_none(&self) -> bool {
        self.guid.is_nil()
    }

    /// The same GUID as a handle to another type.
    pub fn cast<U>(self) -> Handle<U> {
        Handle::new(self.guid)
    }
}

impl<T> Clone for Handle<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Handle<T> {}
impl<T> PartialEq for Handle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.guid == other.guid
    }
}
impl<T> Eq for Handle<T> {}
impl<T> PartialOrd for Handle<T> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Handle<T> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.guid.cmp(&other.guid)
    }
}
impl<T> Hash for Handle<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.guid.hash(state);
    }
}
impl<T> Default for Handle<T> {
    fn default() -> Self {
        Handle::NONE
    }
}
impl<T> fmt::Debug for Handle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Handle({})", self.guid)
    }
}

impl<T> Serialize for Handle<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.guid.serialize(s)
    }
}

impl<'de, T> Deserialize<'de> for Handle<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Guid::deserialize(d).map(Handle::new)
    }
}

impl<T: Asset> ReflectAsset for Handle<T> {
    fn guid(&self) -> u128 {
        self.guid.0
    }
    fn set_guid(&mut self, guid: u128) {
        self.guid = Guid(guid);
    }
    fn asset_kind(&self) -> &'static str {
        T::KIND.name()
    }
}

impl<T: Asset> Reflect for Handle<T> {
    fn type_name(&self) -> &'static str {
        "Handle"
    }
    fn reflect_mut(&mut self) -> Mut<'_> {
        Mut::Asset(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Texture;

    #[test]
    fn handles_serialize_as_guids_and_hash_by_guid() {
        let h: Handle<Texture> = Handle::new(Guid(0xabc));
        let text = ron::to_string(&h).unwrap();
        assert_eq!(text, "\"00000000000000000000000000000abc\"");
        let back: Handle<Texture> = ron::from_str(&text).unwrap();
        assert_eq!(back, h);
        let other: Handle<Texture> = Handle::new(Guid(0xabd));
        assert_eq!(reign_core::checksum(&h), reign_core::checksum(&back));
        assert_ne!(reign_core::checksum(&h), reign_core::checksum(&other));
        assert!(Handle::<Texture>::default().is_none());
        let mut h2 = h;
        match h2.reflect_mut() {
            Mut::Asset(a) => {
                assert_eq!(a.asset_kind(), "Texture");
                a.set_guid(7);
            }
            _ => panic!("handles reflect as assets"),
        }
        assert_eq!(h2.guid, Guid(7));
    }
}
