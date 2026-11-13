//! Scene files: RON, one object per entry, components keyed by their
//! registered name.
//!
//! ```ron
//! // A Reign scene. ...
//! (
//!     version: 1,
//!     objects: [
//!         (
//!             id: (0, 0),
//!             name: "Lamp",
//!             transform: (position: (x: "136", y: "800")),
//!             components: {"Lamp": (lit: false)},
//!         ),
//!         (
//!             id: (1, 0),
//!             name: "Flame",
//!             parent: Some((0, 0)),
//!             active: false,
//!             transform: (position: (x: "0", y: "-32"), z: 2),
//!         ),
//!     ],
//! )
//! ```
//!
//! Objects are listed parents first, in hierarchy order; a child's place
//! among its siblings is its place in the list. `id` is the object's
//! `(index, generation)`, so ids stored inside components keep pointing at
//! the same objects. Defaults are left out: `parent: None`, `active: true`,
//! empty components, zero `z`, no flips. `Fx` values are exact decimal
//! strings. Objects marked `dont_save` (and everything under them) are not
//! written. A scene that has despawned objects also lists its free slots,
//! so loading it back gives exactly the same scene (and the same hash).

use std::collections::BTreeMap;
use std::fmt;

use reign_core::FxVec2;
use ron::value::RawValue;
use serde::{Deserialize, Serialize};

use crate::object::{ObjectId, Transform};
use crate::{ComponentRegistry, Scene};

const VERSION: u32 = 1;
const HEADER: &str =
    "// A Reign scene. Objects are listed parents first; ids are (index, generation).
// Fixed-point numbers are exact decimal strings. Edit by hand or in the editor.
";

/// Why a scene file couldn't be read or written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SceneError(pub String);

impl fmt::Display for SceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SceneError {}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn is_true(v: &bool) -> bool {
    *v
}

fn yes() -> bool {
    true
}

/// The transform, leaving out what's default.
#[derive(Serialize, Deserialize, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
struct TransformFile {
    #[serde(skip_serializing_if = "is_default")]
    position: FxVec2,
    #[serde(skip_serializing_if = "is_default")]
    z: i32,
    #[serde(skip_serializing_if = "is_default")]
    flip_x: bool,
    #[serde(skip_serializing_if = "is_default")]
    flip_y: bool,
}

#[derive(Serialize)]
struct ObjectOut<'a> {
    id: ObjectId,
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<ObjectId>,
    #[serde(skip_serializing_if = "is_true")]
    active: bool,
    #[serde(skip_serializing_if = "is_default")]
    transform: TransformFile,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    components: BTreeMap<&'static str, &'a dyn erased_serde::Serialize>,
}

#[derive(Serialize)]
struct SceneOut<'a> {
    version: u32,
    objects: Vec<ObjectOut<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    free: Vec<ObjectId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObjectIn {
    id: ObjectId,
    name: String,
    #[serde(default)]
    parent: Option<ObjectId>,
    #[serde(default = "yes")]
    active: bool,
    #[serde(default)]
    transform: TransformFile,
    #[serde(default)]
    components: BTreeMap<String, Box<RawValue>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneIn {
    version: u32,
    objects: Vec<ObjectIn>,
    #[serde(default)]
    free: Vec<ObjectId>,
}

impl Scene {
    /// The scene as RON, ready to save.
    pub fn to_ron(&self) -> Result<String, SceneError> {
        let saved: Vec<ObjectId> = self
            .walk()
            .into_iter()
            .filter(|id| {
                let mut at = Some(*id);
                while let Some(a) = at {
                    if self.object(a).is_none_or(|o| o.dont_save) {
                        return false;
                    }
                    at = self.parent(a);
                }
                true
            })
            .collect();
        let mut objects = Vec::with_capacity(saved.len());
        for &id in &saved {
            let object = self.object(id).expect("walked");
            let t = object.transform;
            let mut components = BTreeMap::new();
            for name in self.component_names(id) {
                let value = self.serialize_component(id, name).expect("listed");
                components.insert(name, value);
            }
            objects.push(ObjectOut {
                id,
                name: &object.name,
                parent: object.parent(),
                active: object.active,
                transform: TransformFile {
                    position: t.position,
                    z: t.z,
                    flip_x: t.flip_x,
                    flip_y: t.flip_y,
                },
                components,
            });
        }
        let out = SceneOut { version: VERSION, objects, free: self.free_slots() };
        let pretty = ron::ser::PrettyConfig::new()
            .indentor("    ")
            .depth_limit(3)
            .struct_names(false)
            .separate_tuple_members(false);
        let body = ron::ser::to_string_pretty(&out, pretty)
            .map_err(|e| SceneError(format!("couldn't write the scene: {e}")))?;
        Ok(format!("{HEADER}{body}\n"))
    }

    /// Reads a scene file. Every component in it must be registered.
    pub fn from_ron(text: &str, registry: &ComponentRegistry) -> Result<Scene, SceneError> {
        let file: SceneIn =
            ron::from_str(text).map_err(|e| SceneError(format!("not a scene file: {e}")))?;
        if file.version != VERSION {
            return Err(SceneError(format!(
                "scene file version {} (this build reads {VERSION})",
                file.version
            )));
        }
        let mut scene = Scene::new();
        let mut seen = std::collections::BTreeSet::new();
        for object in file.objects {
            let id = object.id;
            if !seen.insert(id.index) {
                return Err(SceneError(format!("object index {} is used twice", id.index)));
            }
            if let Some(parent) = object.parent
                && !scene.contains(parent)
            {
                return Err(SceneError(format!(
                    "{id} {:?} names parent {parent}, which isn't listed before it",
                    object.name
                )));
            }
            scene.fill_slot(id, object.name, object.parent, object.active, {
                let t = object.transform;
                Transform { position: t.position, z: t.z, flip_x: t.flip_x, flip_y: t.flip_y }
            });
            for (name, raw) in object.components {
                let info = registry.get(&name).ok_or_else(|| {
                    SceneError(format!("unknown component {name:?} (is it registered?)"))
                })?;
                let mut de = ron::Deserializer::from_str(raw.get_ron())
                    .map_err(|e| SceneError(format!("{name}: {e}")))?;
                let mut erased = <dyn erased_serde::Deserializer>::erase(&mut de);
                (info.insert_from)(&mut scene, id, &mut erased)
                    .map_err(|e| SceneError(format!("{id} {name}: {e}")))?;
            }
        }
        scene.finish_load(file.free);
        Ok(scene)
    }
}

impl Scene {
    /// One component of an object as RON text, the way a scene file
    /// writes it.
    pub fn component_ron(&self, id: ObjectId, name: &str) -> Option<String> {
        let value = self.serialize_component(id, name)?;
        ron::ser::to_string(value).ok()
    }
}

impl ComponentRegistry {
    /// Reads a component from RON text (as [`Scene::component_ron`] writes
    /// it) onto `id`, replacing any of its type.
    pub fn insert_ron(
        &self,
        scene: &mut Scene,
        id: ObjectId,
        name: &str,
        text: &str,
    ) -> Result<(), SceneError> {
        let info = self
            .get(name)
            .ok_or_else(|| SceneError(format!("unknown component {name:?} (is it registered?)")))?;
        let mut de =
            ron::Deserializer::from_str(text).map_err(|e| SceneError(format!("{name}: {e}")))?;
        let mut erased = <dyn erased_serde::Deserializer>::erase(&mut de);
        (info.insert_from)(scene, id, &mut erased)
            .map_err(|e| SceneError(format!("{id} {name}: {e}")))
    }
}
