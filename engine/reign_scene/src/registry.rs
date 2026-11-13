//! The component types a game uses, registered by name at startup.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::component::{Component, ErasedStorage, Storage};
use crate::{ObjectId, Scene};

/// What the scene and editor can do with a registered component type
/// without knowing it.
#[derive(Clone, Copy)]
pub struct ComponentInfo {
    pub name: &'static str,
    pub(crate) new_storage: fn() -> Arc<dyn ErasedStorage>,
    pub(crate) insert_from: InsertFrom,
}

pub(crate) type InsertFrom = fn(
    &mut Scene,
    ObjectId,
    &mut dyn erased_serde::Deserializer<'_>,
) -> Result<(), erased_serde::Error>;

/// The component types a scene may hold, by [`Component::NAME`].
///
/// Registration is explicit: call [`ComponentRegistry::register`] for each
/// type at startup. Scene files and the editor's Add Component menu only
/// know registered types.
#[derive(Clone, Default)]
pub struct ComponentRegistry {
    types: BTreeMap<&'static str, ComponentInfo>,
}

impl ComponentRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T`. Registering the same type twice is harmless; two
    /// types with the same name panic.
    pub fn register<T: Component>(&mut self) -> &mut Self {
        let info = ComponentInfo {
            name: T::NAME,
            new_storage: || Arc::new(Storage::<T>::new()),
            insert_from: |scene, id, de| {
                let value: T = erased_serde::deserialize(de)?;
                scene.insert(id, value);
                Ok(())
            },
        };
        if let Some(existing) = self.types.get(T::NAME) {
            let same = (existing.new_storage)().as_any().is::<Storage<T>>();
            assert!(same, "two component types are both named {:?}", T::NAME);
        }
        self.types.insert(T::NAME, info);
        self
    }

    pub fn get(&self, name: &str) -> Option<&ComponentInfo> {
        self.types.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.types.contains_key(name)
    }

    /// Registered names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.types.keys().copied()
    }

    /// Adds `name`'s default value to `id`, replacing any it has. False if
    /// the type isn't registered or the object doesn't exist.
    pub fn insert_default(&self, scene: &mut Scene, id: ObjectId, name: &str) -> bool {
        let Some(info) = self.types.get(name) else { return false };
        if !scene.contains(id) {
            return false;
        }
        scene.storage_or_insert(info).insert_default(id);
        true
    }
}
