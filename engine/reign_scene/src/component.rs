//! Components and their per-type storage.

use std::any::Any;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::ObjectId;
use crate::reflect::Reflect;

/// A rectangle in world pixels, relative to an object's position: where
/// the editor lets you click it and draws its outline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Bounds {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Bounds { x, y, w, h }
    }
}

/// Data attached to a game object. Usually `#[derive(Component)]`.
///
/// Components are part of the simulation state, so they're `Clone`, `Eq`
/// and `Hash` (rollback and checksums), and hold no floats. `Default` is what
/// Add Component and Reset use.
pub trait Component:
    Reflect
    + Clone
    + PartialEq
    + Eq
    + Hash
    + Default
    + Serialize
    + DeserializeOwned
    + Send
    + Sync
    + 'static
{
    /// The name in scene files and the editor. Stable: renaming it breaks
    /// saved scenes.
    const NAME: &'static str;

    /// Where the object can be clicked in the editor, relative to its
    /// position. `None` uses a default box.
    fn bounds(&self) -> Option<Bounds> {
        None
    }
}

/// One component type's values, sorted by object id so iteration order
/// never depends on the order things were added or removed.
#[derive(Clone)]
pub(crate) struct Storage<T> {
    pub(crate) items: Vec<(ObjectId, T)>,
}

impl<T> Storage<T> {
    pub(crate) fn new() -> Self {
        Storage { items: Vec::new() }
    }

    pub(crate) fn find(&self, id: ObjectId) -> Result<usize, usize> {
        self.items.binary_search_by_key(&id, |(i, _)| *i)
    }

    pub(crate) fn get(&self, id: ObjectId) -> Option<&T> {
        self.find(id).ok().map(|i| &self.items[i].1)
    }

    pub(crate) fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        self.find(id).ok().map(|i| &mut self.items[i].1)
    }

    /// Adds or replaces; returns the old value.
    pub(crate) fn insert(&mut self, id: ObjectId, value: T) -> Option<T> {
        match self.find(id) {
            Ok(i) => Some(std::mem::replace(&mut self.items[i].1, value)),
            Err(i) => {
                self.items.insert(i, (id, value));
                None
            }
        }
    }

    pub(crate) fn remove(&mut self, id: ObjectId) -> Option<T> {
        self.find(id).ok().map(|i| self.items.remove(i).1)
    }
}

/// A storage with its type erased, so the scene can clone, hash and edit
/// every component type the same way.
pub(crate) trait ErasedStorage: Send + Sync {
    fn clone_arc(&self) -> Arc<dyn ErasedStorage>;
    fn hash_into(&self, state: &mut dyn Hasher);
    /// Same type and equal values.
    fn eq_dyn(&self, other: &dyn ErasedStorage) -> bool;
    fn len(&self) -> usize;
    fn contains(&self, id: ObjectId) -> bool;
    fn remove(&mut self, id: ObjectId) -> bool;
    fn insert_default(&mut self, id: ObjectId);
    /// Copies `from`'s component onto `to`, if `from` has one.
    fn duplicate(&mut self, from: ObjectId, to: ObjectId);
    fn reflect_mut(&mut self, id: ObjectId) -> Option<&mut dyn Reflect>;
    fn serialize(&self, id: ObjectId) -> Option<&dyn erased_serde::Serialize>;
    fn bounds(&self, id: ObjectId) -> Option<Bounds>;
    /// Clones `from`'s component onto `to` in another scene.
    fn copy_to_scene(&self, from: ObjectId, scene: &mut crate::Scene, to: ObjectId);
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component> ErasedStorage for Storage<T> {
    fn clone_arc(&self) -> Arc<dyn ErasedStorage> {
        Arc::new(self.clone())
    }

    fn hash_into(&self, mut state: &mut dyn Hasher) {
        T::NAME.hash(&mut state);
        self.items.hash(&mut state);
    }

    fn eq_dyn(&self, other: &dyn ErasedStorage) -> bool {
        other.as_any().downcast_ref::<Self>().is_some_and(|o| o.items == self.items)
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn contains(&self, id: ObjectId) -> bool {
        self.find(id).is_ok()
    }

    fn remove(&mut self, id: ObjectId) -> bool {
        Storage::remove(self, id).is_some()
    }

    fn insert_default(&mut self, id: ObjectId) {
        self.insert(id, T::default());
    }

    fn duplicate(&mut self, from: ObjectId, to: ObjectId) {
        if let Some(value) = self.get(from).cloned() {
            self.insert(to, value);
        }
    }

    fn reflect_mut(&mut self, id: ObjectId) -> Option<&mut dyn Reflect> {
        self.get_mut(id).map(|c| c as &mut dyn Reflect)
    }

    fn serialize(&self, id: ObjectId) -> Option<&dyn erased_serde::Serialize> {
        self.get(id).map(|c| c as &dyn erased_serde::Serialize)
    }

    fn bounds(&self, id: ObjectId) -> Option<Bounds> {
        self.get(id).and_then(Component::bounds)
    }

    fn copy_to_scene(&self, from: ObjectId, scene: &mut crate::Scene, to: ObjectId) {
        if let Some(value) = self.get(from) {
            scene.insert(to, value.clone());
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
