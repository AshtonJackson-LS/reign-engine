//! Prefabs: a tree of game objects saved as an asset and placed many times.
//!
//! A prefab file (`*.prefab.ron`) is a scene file whose first top-level
//! object is the prefab's root. Ids in it are local to the file.
//!
//! - **Instances.** A scene places a prefab with an object that has a
//!   [`PrefabInstance`] component. Loading or instantiating *stamps* the
//!   prefab onto that object: the root's components go on it, and the
//!   prefab's children are made under it (marked `dont_save`, since they
//!   come from the prefab). The instance's own name and transform stay its
//!   own.
//! - **Overrides.** What an instance changes is kept as data on its
//!   [`PrefabInstance`]: property overrides by `(object path, component,
//!   field path)` (see [`reign_scene::path`]), added components (as RON) and
//!   removed ones. [`overrides`] works them out by comparing the instance
//!   with its prefab; [`saved_form`] writes instances back as just their
//!   `PrefabInstance`, so a scene file follows its prefabs as they change.
//! - **Nesting.** An object inside a prefab may itself be a prefab
//!   instance. An object path names objects through the nesting: `[]` is
//!   the root, `[3]` the object with local index 3, `[3, 1]` the object
//!   with local index 1 inside the prefab placed at 3.
//! - **Runtime.** [`instantiate`] and [`InstantiateExt::instantiate`] stamp
//!   a prefab during a tick. Objects are allocated in the prefab's order
//!   through the scene's allocator, so every peer gets the same ids.
//!
//! Prefab variants (a prefab whose root is an instance of another) are not
//! supported yet: such a file fails to load with a clear error.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use reign_core::FxVec2;
use reign_scene::path::{self, Value};
use reign_scene::{Commands, Component, ComponentRegistry, ObjectId, Scene, Target};
use serde::{Deserialize, Serialize};

use crate::{Asset, AssetDb, AssetError, AssetKind, Guid, Handle, Meta};

/// A prefab file's text. Parse it with [`Template::parse`] (or let
/// [`Prefabs`] do it).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Prefab {
    pub text: String,
}

impl Asset for Prefab {
    const KIND: AssetKind = AssetKind::Prefab;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        let text = std::str::from_utf8(bytes).map_err(|e| AssetError(e.to_string()))?;
        Ok(Prefab { text: text.to_owned() })
    }
}

/// An object inside an instance: local indices through the nesting.
pub type ObjectPath = Vec<u32>;

/// One changed field. `path` starts with the component's name, or
/// `GameObject` for the object's `name`, `active` and `transform`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Override {
    pub object: ObjectPath,
    pub path: String,
    pub value: Value,
}

/// A component the instance has and the prefab doesn't, as RON.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AddedComponent {
    pub object: ObjectPath,
    pub component: String,
    pub ron: String,
}

/// A component the prefab has and the instance doesn't.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RemovedComponent {
    pub object: ObjectPath,
    pub component: String,
}

/// Marks an object as a placed prefab, with what this placement changes.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, reign_scene::Component,
)]
#[component(name = "PrefabInstance")]
pub struct PrefabInstance {
    pub prefab: Handle<Prefab>,
    #[reflect(skip)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    #[reflect(skip)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<AddedComponent>,
    #[reflect(skip)]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<RemovedComponent>,
    /// Which object each prefab object became, once stamped.
    #[reflect(skip)]
    #[serde(skip)]
    pub objects: Vec<(ObjectPath, ObjectId)>,
}

impl PrefabInstance {
    pub fn new(prefab: Handle<Prefab>) -> Self {
        PrefabInstance { prefab, ..Default::default() }
    }

    /// The object a prefab path became.
    pub fn object(&self, path: &[u32]) -> Option<ObjectId> {
        self.objects.iter().find(|(p, _)| p == path).map(|(_, id)| *id)
    }

    /// The prefab path of an object of this instance.
    pub fn path_of(&self, id: ObjectId) -> Option<&ObjectPath> {
        self.objects.iter().find(|(_, o)| *o == id).map(|(p, _)| p)
    }

    pub fn has_overrides(&self) -> bool {
        !(self.overrides.is_empty() && self.added.is_empty() && self.removed.is_empty())
    }
}

/// A prefab ready to stamp: its objects built once, nested prefabs
/// included, in a scene of their own. Immutable.
#[derive(Clone)]
pub struct Template {
    pub handle: Handle<Prefab>,
    pub scene: Scene,
    pub root: ObjectId,
    /// Every object by path, in stamping order.
    pub paths: Vec<(ObjectPath, ObjectId)>,
}

/// Where prefabs come from while stamping, and the component types.
pub trait PrefabLookup {
    fn template(&self, prefab: Handle<Prefab>) -> Result<Arc<Template>, String>;
    fn registry(&self) -> &ComponentRegistry;
}

/// Nesting deeper than this is taken to be a prefab containing itself.
const MAX_DEPTH: usize = 16;

impl Template {
    /// Parses a prefab file and builds it, stamping nested prefabs from
    /// `lookup`.
    pub fn parse(
        handle: Handle<Prefab>,
        text: &str,
        lookup: &dyn PrefabLookup,
    ) -> Result<Template, String> {
        let file = Scene::from_ron(text, lookup.registry()).map_err(|e| e.to_string())?;
        let &src_root = file.roots().first().ok_or("a prefab needs a root object")?;
        if file.has::<PrefabInstance>(src_root) {
            return Err("prefab variants (a root that is itself an instance) are not supported \
                        yet"
            .into());
        }
        let mut scene = Scene::new();
        let pairs = scene.copy_tree_from(&file, src_root, None);
        let root = pairs[0].1;
        remap(&mut scene, &pairs);
        let mut paths = Vec::new();
        for &(src, id) in &pairs {
            let path = if id == root { vec![] } else { vec![src.index] };
            paths.push((path.clone(), id));
            if id != root {
                scene.object_mut(id).expect("made").dont_save = true;
            }
            if id != root && scene.has::<PrefabInstance>(id) {
                let inner = expand_in(&mut scene, id, lookup)?;
                for (p, inner_id) in inner.into_iter().filter(|(p, _)| !p.is_empty()) {
                    paths.push(([path.clone(), p].concat(), inner_id));
                }
            }
        }
        scene
            .insert(root, PrefabInstance { objects: paths.clone(), ..PrefabInstance::new(handle) });
        Ok(Template { handle, scene, root, paths })
    }

    /// The root object's name.
    pub fn name(&self) -> &str {
        self.scene.name(self.root).unwrap_or("Prefab")
    }

    /// The template object at `path`.
    pub fn object(&self, path: &[u32]) -> Option<ObjectId> {
        self.paths.iter().find(|(p, _)| p == path).map(|(_, id)| *id)
    }
}

/// Rewrites ids inside the copied components from source ids to copies.
fn remap(scene: &mut Scene, pairs: &[(ObjectId, ObjectId)]) {
    let lookup = |id: ObjectId| pairs.iter().find(|(s, _)| *s == id).map_or(id, |(_, n)| *n);
    for &(_, id) in pairs {
        for name in scene.component_names(id) {
            if let Some(c) = scene.component_mut(id, name) {
                path::remap_objects(c, &mut |o| lookup(o));
            }
        }
        if let Some(inst) = scene.get_mut::<PrefabInstance>(id) {
            for (_, o) in &mut inst.objects {
                *o = lookup(*o);
            }
        }
    }
}

/// Stamps `template` onto `target`: the root's components (all but
/// `PrefabInstance`) onto it, the children under it. Returns the
/// instance's objects by path.
fn stamp(scene: &mut Scene, target: ObjectId, template: &Template) -> Vec<(ObjectPath, ObjectId)> {
    let t = &template.scene;
    let own = scene.get::<PrefabInstance>(target).cloned();
    scene.copy_components_from(target, t, template.root);
    match own {
        Some(own) => {
            scene.insert(target, own);
        }
        None => {
            scene.remove::<PrefabInstance>(target);
        }
    }
    let mut pairs = vec![(template.root, target)];
    for &child in t.children(template.root) {
        pairs.extend(scene.copy_tree_from(t, child, Some(target)));
    }
    remap(scene, &pairs);
    let new = |id: ObjectId| pairs.iter().find(|(s, _)| *s == id).map(|(_, n)| *n);
    template.paths.iter().filter_map(|(p, id)| Some((p.clone(), new(*id)?))).collect()
}

/// Stamps the instance on `root` from its prefab and applies its
/// overrides. Returns its objects by path (also stored on it).
fn expand_in(
    scene: &mut Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<Vec<(ObjectPath, ObjectId)>, String> {
    thread_local!(static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });
    let depth = DEPTH.with(|d| d.get());
    if depth > MAX_DEPTH {
        return Err("prefabs nested too deep (does a prefab contain itself?)".into());
    }
    let inst = scene.get::<PrefabInstance>(root).cloned().ok_or("not a prefab instance")?;
    DEPTH.with(|d| d.set(depth + 1));
    let template = lookup.template(inst.prefab);
    DEPTH.with(|d| d.set(depth));
    let template = template?;
    let objects = stamp(scene, root, &template);
    if let Some(i) = scene.get_mut::<PrefabInstance>(root) {
        i.objects = objects.clone();
    }
    apply(scene, root, lookup.registry())?;
    Ok(objects)
}

/// Stamps the prefab instance on `root` (an object with a
/// [`PrefabInstance`] and nothing else from its prefab yet).
pub fn expand(
    scene: &mut Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<Vec<(ObjectPath, ObjectId)>, String> {
    expand_in(scene, root, lookup)
}

/// Stamps every unexpanded instance in a freshly loaded scene, parents
/// first. Errors are collected, not fatal: a missing prefab leaves its
/// instance as a bare object.
pub fn expand_all(scene: &mut Scene, lookup: &dyn PrefabLookup) -> Vec<String> {
    let mut errors = Vec::new();
    for id in scene.walk() {
        let pending = scene.get::<PrefabInstance>(id).is_some_and(|i| i.objects.is_empty());
        if pending && let Err(e) = expand_in(scene, id, lookup) {
            errors.push(format!("{}: {e}", scene.name(id).unwrap_or("?")));
        }
    }
    errors
}

/// Applies the instance's overrides, added and removed components.
fn apply(scene: &mut Scene, root: ObjectId, registry: &ComponentRegistry) -> Result<(), String> {
    let inst = scene.get::<PrefabInstance>(root).cloned().ok_or("not a prefab instance")?;
    for r in &inst.removed {
        if let Some(id) = inst.object(&r.object) {
            scene.remove_component(id, &r.component);
        }
    }
    for a in &inst.added {
        if let Some(id) = inst.object(&a.object) {
            registry.insert_ron(scene, id, &a.component, &a.ron).map_err(|e| e.to_string())?;
        }
    }
    for o in &inst.overrides {
        let Some(id) = inst.object(&o.object) else { continue };
        set_property(scene, id, &o.path, &o.value);
    }
    Ok(())
}

/// Sets `Component.field.sub` (or `GameObject.name`, `.active`,
/// `.transform...`) on an object.
pub fn set_property(scene: &mut Scene, id: ObjectId, property: &str, value: &Value) -> bool {
    let (head, rest) = property.split_once('.').unwrap_or((property, ""));
    if head == "GameObject" {
        let Some(object) = scene.object_mut(id) else { return false };
        return match (rest, value) {
            ("name", Value::Str(s)) => {
                object.name.clone_from(s);
                true
            }
            ("active", Value::Bool(b)) => {
                object.active = *b;
                true
            }
            (t, v) => match t.strip_prefix("transform") {
                Some(sub) => path::set(&mut object.transform, sub.trim_start_matches('.'), v),
                None => false,
            },
        };
    }
    scene.component_mut(id, head).is_some_and(|c| path::set(c, rest, value))
}

/// Reads `Component.field.sub` (or a `GameObject` property) of an object.
pub fn get_property(scene: &mut Scene, id: ObjectId, property: &str) -> Option<Value> {
    let (head, rest) = property.split_once('.').unwrap_or((property, ""));
    if head == "GameObject" {
        let object = scene.object_mut(id)?;
        return match rest {
            "name" => Some(Value::Str(object.name.clone())),
            "active" => Some(Value::Bool(object.active)),
            t => path::get(&mut object.transform, t.strip_prefix("transform")?.trim_matches('.')),
        };
    }
    path::get(scene.component_mut(id, head)?, rest)
}

/// Every overridable leaf of an object: its `GameObject` properties
/// (unless it is the instance root, whose name and place are its own) and
/// each component's fields, with ids inside mapped through `ids`.
fn object_leaves(
    scene: &mut Scene,
    id: ObjectId,
    is_root: bool,
    ids: &dyn Fn(ObjectId) -> ObjectId,
) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    if !is_root && let Some(o) = scene.object_mut(id) {
        out.push(("GameObject.name".to_owned(), Value::Str(o.name.clone())));
        out.push(("GameObject.active".to_owned(), Value::Bool(o.active)));
        path::leaves(&mut o.transform, "GameObject.transform", &mut out);
    }
    for name in scene.component_names(id) {
        if name == PrefabInstance::NAME {
            continue;
        }
        if let Some(c) = scene.component_mut(id, name) {
            path::leaves(c, name, &mut out);
        }
    }
    for (_, v) in &mut out {
        if let Value::Object(o) = v {
            *o = ids(*o);
        }
    }
    out
}

/// Works out what the instance on `root` changes from its prefab: the
/// instance's [`PrefabInstance`] with fresh overrides, added and removed
/// components.
pub fn overrides(
    scene: &mut Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<PrefabInstance, String> {
    let inst = scene.get::<PrefabInstance>(root).cloned().ok_or("not a prefab instance")?;
    let template = lookup.template(inst.prefab)?;
    let mut fresh = template.scene.clone();
    let mut out =
        PrefabInstance { objects: inst.objects.clone(), ..PrefabInstance::new(inst.prefab) };
    // Template ids, seen as the instance's.
    let to_instance = |id: ObjectId| {
        template
            .paths
            .iter()
            .find(|(_, t)| *t == id)
            .and_then(|(p, _)| inst.object(p))
            .unwrap_or(id)
    };
    for (p, tid) in &template.paths {
        let Some(iid) = inst.object(p) else { continue };
        if !scene.contains(iid) {
            continue;
        }
        let is_root = p.is_empty();
        let theirs: Vec<&str> = fresh.component_names(*tid);
        let mine: Vec<&str> = scene.component_names(iid);
        for name in theirs.iter().filter(|n| !mine.contains(n)) {
            out.removed.push(RemovedComponent { object: p.clone(), component: (*name).to_owned() });
        }
        for name in mine.iter().filter(|n| !theirs.contains(n) && **n != PrefabInstance::NAME) {
            let ron = scene.component_ron(iid, name).ok_or("component won't serialize")?;
            out.added.push(AddedComponent {
                object: p.clone(),
                component: (*name).to_owned(),
                ron,
            });
        }
        let before = object_leaves(&mut fresh, *tid, is_root, &to_instance);
        let after = object_leaves(scene, iid, is_root, &|id| id);
        for (property, value) in after {
            let component = property.split('.').next().unwrap_or("");
            let shared = component == "GameObject" || theirs.contains(&component);
            if shared && !before.iter().any(|(q, w)| *q == property && *w == value) {
                out.overrides.push(Override { object: p.clone(), path: property, value });
            }
        }
    }
    Ok(out)
}

/// The prefab's value of an instance object's property, to revert to.
pub fn prefab_value(
    scene: &Scene,
    root: ObjectId,
    object: ObjectId,
    property: &str,
    lookup: &dyn PrefabLookup,
) -> Option<Value> {
    let inst = scene.get::<PrefabInstance>(root)?;
    let template = lookup.template(inst.prefab).ok()?;
    let p = inst.path_of(object)?;
    let tid = template.object(p)?;
    let mut fresh = template.scene.clone();
    let mut value = get_property(&mut fresh, tid, property)?;
    if let Value::Object(o) = &mut value {
        let path = template.paths.iter().find(|(_, t)| t == o).map(|(p, _)| p.clone());
        if let Some(mapped) = path.and_then(|p| inst.object(&p)) {
            *o = mapped;
        }
    }
    Some(value)
}

/// The scene as it should be saved: each top-level instance reduced to its
/// `PrefabInstance` with up-to-date overrides. Objects that came from a
/// prefab are already `dont_save`.
pub fn saved_form(scene: &Scene, lookup: &dyn PrefabLookup) -> Result<Scene, String> {
    let mut out = scene.clone();
    let roots: Vec<ObjectId> = out
        .walk()
        .into_iter()
        .filter(|id| out.has::<PrefabInstance>(*id) && !out.object(*id).expect("live").dont_save)
        .collect();
    for root in roots {
        let mut inst = overrides(&mut out, root, lookup)?;
        inst.objects.clear();
        for name in out.component_names(root) {
            out.remove_component(root, name);
        }
        out.insert(root, inst);
    }
    Ok(out)
}

/// The re-stamped instance: prefab objects made again from the current
/// prefab, keeping the instance's overrides and its own added children.
/// Use after the prefab changed.
pub fn refresh(scene: &mut Scene, root: ObjectId, lookup: &dyn PrefabLookup) -> Result<(), String> {
    let inst = scene.get::<PrefabInstance>(root).cloned().ok_or("not a prefab instance")?;
    let made: Vec<ObjectId> = scene
        .children(root)
        .iter()
        .copied()
        .filter(|c| scene.object(*c).is_some_and(|o| o.dont_save))
        .collect();
    for id in made {
        scene.despawn(id);
    }
    for name in scene.component_names(root) {
        if name != PrefabInstance::NAME {
            scene.remove_component(root, name);
        }
    }
    let mut bare = PrefabInstance { objects: Vec::new(), ..inst };
    // Overrides on objects the prefab no longer has are dropped.
    let template = lookup.template(bare.prefab)?;
    bare.overrides.retain(|o| template.object(&o.object).is_some());
    scene.insert(root, bare);
    expand_in(scene, root, lookup).map(|_| ())
}

/// The prefab file for the tree at `root`, as it is in `scene` (nested
/// instances kept as instances). The root's position becomes zero.
pub fn prefab_from(
    scene: &Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<String, String> {
    // An instance's own prefab objects become the new prefab's content.
    let mut live = scene.clone();
    unpack(&mut live, root);
    let saved = saved_form(&live, lookup)?;
    let mut out = Scene::new();
    let pairs = out.copy_tree_from(&saved, root, None);
    remap(&mut out, &pairs);
    let new_root = pairs[0].1;
    out.object_mut(new_root).expect("made").transform.position = FxVec2::ZERO;
    out.object_mut(new_root).expect("made").dont_save = false;
    out.remove::<PrefabInstance>(new_root);
    out.to_ron().map_err(|e| e.to_string())
}

/// Stores the instance's current overrides on its [`PrefabInstance`], so a
/// later [`refresh`] keeps them. Call after editing an instance.
pub fn sync_overrides(
    scene: &mut Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<(), String> {
    let fresh = overrides(scene, root, lookup)?;
    scene.insert(root, fresh);
    Ok(())
}

/// Sets one property back to the prefab's value and stops overriding it.
pub fn revert_property(
    scene: &mut Scene,
    root: ObjectId,
    object: ObjectId,
    property: &str,
    lookup: &dyn PrefabLookup,
) -> Result<(), String> {
    let value = prefab_value(scene, root, object, property, lookup).ok_or("no prefab value")?;
    set_property(scene, object, property, &value);
    sync_overrides(scene, root, lookup)
}

/// Drops every override: the instance becomes its prefab again.
pub fn revert_all(
    scene: &mut Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<(), String> {
    let inst = scene.get::<PrefabInstance>(root).cloned().ok_or("not a prefab instance")?;
    scene
        .insert(root, PrefabInstance { objects: inst.objects, ..PrefabInstance::new(inst.prefab) });
    refresh(scene, root, lookup)
}

/// The prefab text with this instance's overrides applied to it. Save it
/// over the prefab, then [`refresh`] the instances: this one comes out
/// with no overrides.
pub fn apply_all(
    scene: &mut Scene,
    root: ObjectId,
    lookup: &dyn PrefabLookup,
) -> Result<String, String> {
    let text = prefab_from(scene, root, lookup)?;
    let inst = scene.get::<PrefabInstance>(root).cloned().ok_or("not a prefab instance")?;
    scene
        .insert(root, PrefabInstance { objects: inst.objects, ..PrefabInstance::new(inst.prefab) });
    Ok(text)
}

/// Turns the tree at `root` into an instance of `prefab` (just saved from
/// it with [`prefab_from`]): it is re-stamped, so its objects are the
/// prefab's.
pub fn make_instance(
    scene: &mut Scene,
    root: ObjectId,
    prefab: Handle<Prefab>,
    lookup: &dyn PrefabLookup,
) -> Result<(), String> {
    let children: Vec<ObjectId> = scene.children(root).to_vec();
    for c in children {
        scene.despawn(c);
    }
    for name in scene.component_names(root) {
        scene.remove_component(root, name);
    }
    scene.insert(root, PrefabInstance::new(prefab));
    expand_in(scene, root, lookup).map(|_| ())
}

/// Makes the instance plain objects: its `PrefabInstance` goes and its
/// prefab objects become ordinary, saved objects. Nested instances stay
/// instances.
pub fn unpack(scene: &mut Scene, root: ObjectId) -> bool {
    let Some(inst) = scene.remove::<PrefabInstance>(root) else { return false };
    for (p, id) in &inst.objects {
        let top_level = p.len() == 1;
        if top_level && let Some(o) = scene.object_mut(*id) {
            o.dont_save = false;
        }
    }
    true
}

/// Stamps `template` as a new object at `position` (under `parent`, or at
/// the top level). Deterministic: ids come from the scene's allocator in
/// the prefab's order.
pub fn instantiate(
    scene: &mut Scene,
    template: &Template,
    position: FxVec2,
    parent: Option<ObjectId>,
) -> ObjectId {
    let name = template.name().to_owned();
    let id = match parent {
        Some(p) => scene.spawn_child(p, name),
        None => scene.spawn(name),
    };
    stamp_new(scene, id, template, position);
    id
}

fn stamp_new(scene: &mut Scene, id: ObjectId, template: &Template, position: FxVec2) {
    if let Some(o) = scene.object_mut(id) {
        o.transform = *template.scene.transform(template.root).expect("root");
        o.transform.position = position;
        o.active = template.scene.object(template.root).is_some_and(|r| r.active);
    }
    scene.insert(id, PrefabInstance::new(template.handle));
    let objects = stamp(scene, id, template);
    if let Some(i) = scene.get_mut::<PrefabInstance>(id) {
        i.objects = objects;
    }
}

/// `commands.instantiate(template, position)`, for Behaviours.
pub trait InstantiateExt {
    /// Queues a new instance of `template` at `position`. The returned
    /// target can take more commands (insert, despawn) in the same buffer.
    fn instantiate(&mut self, template: Arc<Template>, position: FxVec2) -> Target;
}

impl InstantiateExt for Commands {
    fn instantiate(&mut self, template: Arc<Template>, position: FxVec2) -> Target {
        let target = self.spawn(template.name().to_owned());
        self.run(move |scene, spawned| {
            let id = match target {
                Target::Spawned(n) => spawned[n],
                Target::Id(id) => id,
            };
            stamp_new(scene, id, &template, position);
        });
        target
    }
}

/// Parsed prefabs from an asset database, cached until they change.
pub struct Prefabs {
    registry: ComponentRegistry,
    cache: Mutex<BTreeMap<Guid, (u32, Arc<Template>)>>,
}

impl Prefabs {
    /// `registry` must know every component the prefabs use.
    pub fn new(mut registry: ComponentRegistry) -> Self {
        registry.register::<PrefabInstance>();
        Prefabs { registry, cache: Mutex::new(BTreeMap::new()) }
    }

    pub fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }

    /// Looking prefabs up in `db`.
    pub fn with<'a>(&'a self, db: &'a AssetDb) -> impl PrefabLookup + 'a {
        FromDb { prefabs: self, db }
    }

    /// Forgets every parsed prefab (after hot reload, say).
    pub fn clear(&self) {
        self.cache.lock().expect("prefab cache").clear();
    }
}

struct FromDb<'a> {
    prefabs: &'a Prefabs,
    db: &'a AssetDb,
}

impl PrefabLookup for FromDb<'_> {
    fn template(&self, prefab: Handle<Prefab>) -> Result<Arc<Template>, String> {
        let version = self.db.version(prefab.guid);
        if let Some((v, t)) = self.prefabs.cache.lock().expect("prefab cache").get(&prefab.guid)
            && *v == version
        {
            return Ok(t.clone());
        }
        let file = self.db.load(prefab).map_err(|e| e.to_string())?;
        let template = Arc::new(Template::parse(prefab, &file.text, self)?);
        let mut cache = self.prefabs.cache.lock().expect("prefab cache");
        cache.insert(prefab.guid, (version, template.clone()));
        Ok(template)
    }

    fn registry(&self) -> &ComponentRegistry {
        &self.prefabs.registry
    }
}

/// Prefab texts held in memory, for tools and tests.
pub struct MemoryPrefabs {
    pub registry: ComponentRegistry,
    pub texts: BTreeMap<Guid, String>,
}

impl MemoryPrefabs {
    pub fn new(mut registry: ComponentRegistry) -> Self {
        registry.register::<PrefabInstance>();
        MemoryPrefabs { registry, texts: BTreeMap::new() }
    }
}

impl PrefabLookup for MemoryPrefabs {
    fn template(&self, prefab: Handle<Prefab>) -> Result<Arc<Template>, String> {
        let text = self.texts.get(&prefab.guid).ok_or("no such prefab")?;
        Template::parse(prefab, text, self).map(Arc::new)
    }

    fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }
}

#[cfg(test)]
#[path = "prefab_tests.rs"]
mod tests;
