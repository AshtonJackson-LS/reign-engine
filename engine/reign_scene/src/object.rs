//! Object ids, game objects and their built-in [`Transform`].

use std::fmt;

use reign_core::FxVec2;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Reflect;

/// A handle to an object in a [`crate::Scene`]. The generation changes each
/// time a slot is reused, so a handle to a despawned object never finds the
/// object that replaced it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId {
    pub index: u32,
    pub generation: u32,
}

impl ObjectId {
    pub const fn new(index: u32, generation: u32) -> Self {
        ObjectId { index, generation }
    }
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.index)?;
        if self.generation > 0 {
            write!(f, "v{}", self.generation)?;
        }
        Ok(())
    }
}

/// Written as `(index, generation)`.
impl Serialize for ObjectId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.index, self.generation).serialize(s)
    }
}

impl<'de> Deserialize<'de> for ObjectId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let (index, generation) = <(u32, u32)>::deserialize(d)?;
        Ok(ObjectId { index, generation })
    }
}

/// Where an object is, relative to its parent: every object has one.
///
/// Positions are fixed-point world pixels, so they can feed the simulation.
/// There is no rotation or scale: this is a pixel-art engine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Transform {
    /// Relative to the parent, or to the world for a root object.
    pub position: FxVec2,
    /// Draw order: higher draws over lower.
    pub z: i32,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Transform {
    pub fn at(position: FxVec2) -> Self {
        Transform { position, ..Transform::default() }
    }

    pub fn at_px(x: i32, y: i32) -> Self {
        Transform::at(FxVec2::from_px(x, y))
    }
}

/// An object in a scene: a name, a transform, a place in the hierarchy.
/// Its components are stored by type in the scene.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GameObject {
    pub name: String,
    /// An inactive object, and everything under it, is skipped by queries.
    pub active: bool,
    pub transform: Transform,
    /// Made at runtime and never written to scene files (Unity's
    /// `HideFlags.DontSave`).
    pub dont_save: bool,
    pub(crate) parent: Option<ObjectId>,
    pub(crate) children: Vec<ObjectId>,
}

impl GameObject {
    pub(crate) fn new(name: String, parent: Option<ObjectId>) -> Self {
        GameObject {
            name,
            active: true,
            transform: Transform::default(),
            dont_save: false,
            parent,
            children: Vec::new(),
        }
    }

    pub fn parent(&self) -> Option<ObjectId> {
        self.parent
    }

    /// In order.
    pub fn children(&self) -> &[ObjectId] {
        &self.children
    }
}
