//! Reign's scene model: game objects with components, Unity style.
//!
//! - A [`Scene`] holds [`GameObject`]s in a generational arena. Each has a
//!   name, an active flag, a [`Transform`] and a place in the hierarchy
//!   (a parent and ordered children). [`ObjectId`]s are generational, so a
//!   handle to a despawned object never resolves to its replacement.
//! - [`Component`]s are plain structs stored per type, sorted by object id.
//!   Game logic is ordinary Rust that queries them ([`Scene::get`],
//!   [`Scene::iter_mut`], [`Scene::iter2_mut`]) from its fixed tick, and
//!   queues spawns and despawns in [`Commands`] to apply at a fixed point.
//! - [`Reflect`] describes components field by field for the editor;
//!   [`ComponentRegistry`] knows every component type by name, for scene
//!   files and the Add Component menu.
//!
//! - Scenes save to and load from RON ([`Scene::to_ron`],
//!   [`Scene::from_ron`]); see the `file` module docs for the format.
//!
//! A scene is simulation state: `Clone + Hash`, deterministic in every
//! iteration, free of floats and of randomly seeded hash maps.

// Lets the derives name this crate `::reign_scene` from inside it too.
extern crate self as reign_scene;

mod component;
mod file;
mod object;
pub mod path;
pub mod reflect;
mod registry;
mod scene;

pub use component::{Bounds, Component};
pub use file::SceneError;
pub use object::{GameObject, ObjectId, Transform};
pub use reflect::{FieldInfo, Mut, Reflect, Rgba8};
pub use registry::{ComponentInfo, ComponentRegistry};
/// `#[derive(Component)]` and `#[derive(Reflect)]`.
pub use reign_derive::{Component, Reflect};
pub use scene::{Commands, ComponentViewMut, QueryMut, Scene, Target};
