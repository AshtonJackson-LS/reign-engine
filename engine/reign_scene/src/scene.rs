//! The scene: game objects in a generational arena, and their components.

use std::hash::{Hash, Hasher};
use std::iter::Peekable;
use std::slice::IterMut;
use std::sync::Arc;

use reign_core::FxVec2;

use crate::component::{Bounds, Component, ErasedStorage, Storage};
use crate::object::{GameObject, ObjectId, Transform};
use crate::reflect::Reflect;
use crate::registry::ComponentInfo;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Slot {
    generation: u32,
    object: Option<GameObject>,
}

/// Game objects and their components.
///
/// A scene is part of the simulation state: it's `Clone` and `Hash`, and
/// everything in it iterates in a fixed order (objects by id, children in
/// order), so rollback and checksums work on it like on any other state.
/// Cloning is cheap: the object table and each component type's storage are
/// shared until one side changes them (copy-on-write), so a snapshot only
/// copies what was edited after it.
///
/// Queries ([`Scene::iter`], [`Scene::iter_mut`], [`Scene::iter2_mut`]) skip
/// inactive objects and everything under them.
#[derive(Clone, Default)]
pub struct Scene {
    /// Shared twice over: the table, and each slot, so a snapshot copies
    /// only the slots changed after it.
    slots: Arc<Vec<Arc<Slot>>>,
    /// Free slot indices, highest first: the last (lowest) is reused
    /// first, so objects spawned one after another get ascending ids and
    /// iterate in the order they were made.
    free: Vec<u32>,
    roots: Vec<ObjectId>,
    /// One storage per component type, sorted by name.
    components: Vec<(&'static str, Arc<dyn ErasedStorage>)>,
    /// Counts structural changes (spawns, despawns, components added or
    /// removed, objects touched through `object_mut`). Not content: left out of `Hash` and `Eq`. Lets callers
    /// skip work when nothing was added since they last looked.
    revision: u64,
}

impl Hash for Scene {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.slots.hash(state);
        self.free.hash(state);
        self.roots.hash(state);
        // Empty storages are skipped: a type that was added and removed
        // again leaves the scene equal to one that never had it.
        for (_, storage) in self.components.iter().filter(|(_, s)| s.len() > 0) {
            storage.hash_into(state);
        }
    }
}

impl PartialEq for Scene {
    fn eq(&self, other: &Self) -> bool {
        fn filled(s: &Scene) -> Vec<(&'static str, &dyn ErasedStorage)> {
            s.components.iter().filter(|(_, c)| c.len() > 0).map(|(n, c)| (*n, &**c)).collect()
        }
        let (a, b) = (filled(self), filled(other));
        self.slots == other.slots
            && self.free == other.free
            && self.roots == other.roots
            && a.len() == b.len()
            && a.iter().zip(&b).all(|((na, sa), (nb, sb))| na == nb && sa.eq_dyn(*sb))
    }
}

/// Components hold no floats, so equality is reflexive.
impl Eq for Scene {}

impl std::fmt::Debug for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scene")
            .field("objects", &self.len())
            .field("components", &self.components.iter().map(|(n, _)| n).collect::<Vec<_>>())
            .finish()
    }
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Live objects.
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.object.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Bumped by every spawn, despawn and added or removed component.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether `id` names a live object (not a despawned one, nor whatever
    /// later took its slot).
    pub fn contains(&self, id: ObjectId) -> bool {
        self.object(id).is_some()
    }

    pub fn object(&self, id: ObjectId) -> Option<&GameObject> {
        let slot = self.slots.get(id.index as usize)?;
        slot.object.as_ref().filter(|_| slot.generation == id.generation)
    }

    pub fn object_mut(&mut self, id: ObjectId) -> Option<&mut GameObject> {
        self.contains(id).then_some(())?;
        // It may become active: count it as a structural change.
        self.revision += 1;
        Arc::make_mut(&mut Arc::make_mut(&mut self.slots)[id.index as usize]).object.as_mut()
    }

    pub fn name(&self, id: ObjectId) -> Option<&str> {
        self.object(id).map(|o| o.name.as_str())
    }

    pub fn transform(&self, id: ObjectId) -> Option<&Transform> {
        self.object(id).map(|o| &o.transform)
    }

    pub fn transform_mut(&mut self, id: ObjectId) -> Option<&mut Transform> {
        self.object_mut(id).map(|o| &mut o.transform)
    }

    /// Top-level objects, in order.
    pub fn roots(&self) -> &[ObjectId] {
        &self.roots
    }

    pub fn parent(&self, id: ObjectId) -> Option<ObjectId> {
        self.object(id)?.parent
    }

    pub fn children(&self, id: ObjectId) -> &[ObjectId] {
        self.object(id).map_or(&[], |o| &o.children)
    }

    /// Every live object, parents before children, siblings in order.
    pub fn walk(&self) -> Vec<ObjectId> {
        let mut out = Vec::with_capacity(self.len());
        let mut stack: Vec<ObjectId> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            out.push(id);
            stack.extend(self.children(id).iter().rev());
        }
        out
    }

    /// Every live object, by id.
    pub fn ids(&self) -> impl Iterator<Item = ObjectId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.object.as_ref().map(|_| ObjectId::new(i as u32, s.generation)))
    }

    /// Active, and so is every ancestor.
    pub fn is_active(&self, id: ObjectId) -> bool {
        active_in(&self.slots, id)
    }

    /// The object's position in the world: its own plus every ancestor's.
    pub fn world_position(&self, id: ObjectId) -> FxVec2 {
        let mut position = FxVec2::ZERO;
        let mut at = Some(id);
        while let Some(object) = at.and_then(|id| self.object(id)) {
            position += object.transform.position;
            at = object.parent;
        }
        position
    }

    /// Moves the object so its world position is `position`.
    pub fn set_world_position(&mut self, id: ObjectId, position: FxVec2) {
        let parent = self.parent(id).map_or(FxVec2::ZERO, |p| self.world_position(p));
        if let Some(t) = self.transform_mut(id) {
            t.position = position - parent;
        }
    }

    // Spawning and the hierarchy.

    /// A new empty object at the top level.
    pub fn spawn(&mut self, name: impl Into<String>) -> ObjectId {
        let id = self.allocate(GameObject::new(name.into(), None));
        self.roots.push(id);
        id
    }

    /// A new empty object under `parent`, after its other children. A
    /// missing parent makes it a top-level object.
    pub fn spawn_child(&mut self, parent: ObjectId, name: impl Into<String>) -> ObjectId {
        if !self.contains(parent) {
            return self.spawn(name);
        }
        let id = self.allocate(GameObject::new(name.into(), Some(parent)));
        self.object_mut(parent).expect("checked").children.push(id);
        id
    }

    fn allocate(&mut self, object: GameObject) -> ObjectId {
        self.revision += 1;
        let slots = Arc::make_mut(&mut self.slots);
        match self.free.pop() {
            Some(index) => {
                let slot = Arc::make_mut(&mut slots[index as usize]);
                slot.object = Some(object);
                ObjectId::new(index, slot.generation)
            }
            None => {
                slots.push(Arc::new(Slot { generation: 0, object: Some(object) }));
                ObjectId::new(slots.len() as u32 - 1, 0)
            }
        }
    }

    /// Removes the object, everything under it and all their components.
    /// False if it was already gone.
    pub fn despawn(&mut self, id: ObjectId) -> bool {
        if !self.contains(id) {
            return false;
        }
        self.detach(id);
        self.revision += 1;
        let mut doomed = vec![id];
        let mut i = 0;
        while i < doomed.len() {
            doomed.extend_from_slice(self.children(doomed[i]));
            i += 1;
        }
        for (_, storage) in &mut self.components {
            if doomed.iter().any(|d| storage.contains(*d)) {
                let storage = unique(storage);
                for d in &doomed {
                    storage.remove(*d);
                }
            }
        }
        let slots = Arc::make_mut(&mut self.slots);
        for d in doomed.iter().rev() {
            let slot = Arc::make_mut(&mut slots[d.index as usize]);
            slot.object = None;
            slot.generation = slot.generation.wrapping_add(1);
            let at = self.free.partition_point(|&f| f > d.index);
            self.free.insert(at, d.index);
        }
        true
    }

    /// Takes `id` out of its parent's children, or the roots.
    fn detach(&mut self, id: ObjectId) {
        match self.parent(id) {
            Some(parent) => {
                let parent = self.object_mut(parent).expect("parents are live");
                parent.children.retain(|c| *c != id);
            }
            None => self.roots.retain(|r| *r != id),
        }
    }

    /// Whether `ancestor` is `id` or above it.
    pub fn is_ancestor(&self, ancestor: ObjectId, id: ObjectId) -> bool {
        let mut at = Some(id);
        while let Some(a) = at {
            if a == ancestor {
                return true;
            }
            at = self.parent(a);
        }
        false
    }

    /// Moves `id` under `parent` (or to the top level), at `index` among its
    /// new siblings or last. Its local position is kept. Refused (false) if
    /// either is gone or `parent` is `id` or below it.
    pub fn set_parent(
        &mut self,
        id: ObjectId,
        parent: Option<ObjectId>,
        index: Option<usize>,
    ) -> bool {
        if !self.contains(id) || parent.is_some_and(|p| !self.contains(p)) {
            return false;
        }
        if parent.is_some_and(|p| self.is_ancestor(id, p)) {
            return false;
        }
        self.detach(id);
        self.object_mut(id).expect("checked").parent = parent;
        let siblings = match parent {
            Some(p) => &mut self.object_mut(p).expect("checked").children,
            None => &mut self.roots,
        };
        let at = index.unwrap_or(siblings.len()).min(siblings.len());
        siblings.insert(at, id);
        true
    }

    /// [`Scene::set_parent`], keeping the object where it is in the world.
    pub fn set_parent_keep_world(
        &mut self,
        id: ObjectId,
        parent: Option<ObjectId>,
        index: Option<usize>,
    ) -> bool {
        let world = self.world_position(id);
        let done = self.set_parent(id, parent, index);
        if done {
            self.set_world_position(id, world);
        }
        done
    }

    /// Copies the object, its components and everything under it, placing
    /// the copy right after the original. Returns the copy.
    pub fn duplicate(&mut self, id: ObjectId) -> Option<ObjectId> {
        let copy = self.copy_tree(id, self.parent(id))?;
        // copy_tree appended it; move it next to the original.
        let siblings = match self.parent(id) {
            Some(p) => self.children(p),
            None => self.roots(),
        };
        let after = siblings.iter().position(|s| *s == id).map(|i| i + 1);
        self.set_parent(copy, self.parent(id), after);
        Some(copy)
    }

    fn copy_tree(&mut self, id: ObjectId, parent: Option<ObjectId>) -> Option<ObjectId> {
        let original = self.object(id)?.clone();
        let copy = match parent {
            Some(p) => self.spawn_child(p, original.name.clone()),
            None => self.spawn(original.name.clone()),
        };
        let object = self.object_mut(copy).expect("just made");
        object.active = original.active;
        object.transform = original.transform;
        object.dont_save = original.dont_save;
        for (_, storage) in &mut self.components {
            if storage.contains(id) {
                unique(storage).duplicate(id, copy);
            }
        }
        for child in original.children {
            self.copy_tree(child, Some(copy));
        }
        Some(copy)
    }

    /// Copies `from`'s components from another scene onto `to`, replacing
    /// any of the same types.
    pub fn copy_components_from(&mut self, to: ObjectId, src: &Scene, from: ObjectId) {
        for (_, storage) in &src.components {
            if storage.contains(from) {
                storage.copy_to_scene(from, self, to);
            }
        }
    }

    /// Copies object `from` of another scene, and everything under it,
    /// into new objects here under `parent` (or at the top level). Names,
    /// active flags, transforms and components come along. Returns
    /// `(source id, new id)` pairs, parents first. Ids inside components
    /// are left as they were; remap them with the pairs.
    pub fn copy_tree_from(
        &mut self,
        src: &Scene,
        from: ObjectId,
        parent: Option<ObjectId>,
    ) -> Vec<(ObjectId, ObjectId)> {
        let mut pairs = Vec::new();
        let mut stack = vec![(from, parent)];
        while let Some((at, parent)) = stack.pop() {
            let Some(original) = src.object(at) else { continue };
            let copy = match parent {
                Some(p) => self.spawn_child(p, original.name.clone()),
                None => self.spawn(original.name.clone()),
            };
            let object = self.object_mut(copy).expect("just made");
            object.active = original.active;
            object.transform = original.transform;
            object.dont_save = original.dont_save;
            self.copy_components_from(copy, src, at);
            pairs.push((at, copy));
            stack.extend(original.children.iter().rev().map(|c| (*c, Some(copy))));
        }
        pairs
    }

    // Components.

    fn storage_index(&self, name: &str) -> Result<usize, usize> {
        self.components.binary_search_by(|(n, _)| (*n).cmp(name))
    }

    /// A typed lookup: names are usually the same `&'static str`, so a
    /// pointer scan finds them without comparing text.
    fn typed_index<T: Component>(&self) -> Option<usize> {
        let name = T::NAME;
        self.components
            .iter()
            .position(|(n, _)| std::ptr::eq(n.as_ptr(), name.as_ptr()) && n.len() == name.len())
            .or_else(|| self.storage_index(name).ok())
    }

    fn storage<T: Component>(&self) -> Option<&Storage<T>> {
        let i = self.typed_index::<T>()?;
        Some(downcast(&*self.components[i].1))
    }

    fn storage_mut<T: Component>(&mut self) -> Option<&mut Storage<T>> {
        let i = self.typed_index::<T>()?;
        Some(downcast_mut(unique(&mut self.components[i].1)))
    }

    fn typed_storage_or_insert<T: Component>(&mut self) -> &mut Storage<T> {
        let i = match self.storage_index(T::NAME) {
            Ok(i) => i,
            Err(i) => {
                self.components.insert(i, (T::NAME, Arc::new(Storage::<T>::new())));
                i
            }
        };
        downcast_mut(unique(&mut self.components[i].1))
    }

    pub(crate) fn storage_or_insert(&mut self, info: &ComponentInfo) -> &mut dyn ErasedStorage {
        self.revision += 1;
        let i = match self.storage_index(info.name) {
            Ok(i) => i,
            Err(i) => {
                self.components.insert(i, (info.name, (info.new_storage)()));
                i
            }
        };
        unique(&mut self.components[i].1)
    }

    /// Adds `value` to the object, replacing and returning any `T` it had.
    /// Does nothing to a despawned object.
    pub fn insert<T: Component>(&mut self, id: ObjectId, value: T) -> Option<T> {
        if !self.contains(id) {
            return None;
        }
        let old = self.typed_storage_or_insert::<T>().insert(id, value);
        if old.is_none() {
            self.revision += 1;
        }
        old
    }

    pub fn remove<T: Component>(&mut self, id: ObjectId) -> Option<T> {
        self.storage::<T>()?.get(id)?;
        self.revision += 1;
        self.storage_mut::<T>()?.remove(id)
    }

    pub fn get<T: Component>(&self, id: ObjectId) -> Option<&T> {
        self.storage::<T>()?.get(id)
    }

    pub fn get_mut<T: Component>(&mut self, id: ObjectId) -> Option<&mut T> {
        let i = self.typed_index::<T>()?;
        let at = downcast::<T>(&*self.components[i].1).find(id).ok()?;
        Some(&mut downcast_mut::<T>(unique(&mut self.components[i].1)).items[at].1)
    }

    /// How many objects have a `T` (active or not).
    pub fn count<T: Component>(&self) -> usize {
        self.storage::<T>().map_or(0, |s| s.items.len())
    }

    pub fn has<T: Component>(&self, id: ObjectId) -> bool {
        self.get::<T>(id).is_some()
    }

    /// Every active object with a `T`, by id.
    pub fn iter<T: Component>(&self) -> impl Iterator<Item = (ObjectId, &T)> + '_ {
        let items = self.storage::<T>().map_or(&[][..], |s| &s.items[..]);
        items.iter().filter(|(id, _)| self.is_active(*id)).map(|(id, c)| (*id, c))
    }

    /// Every active object with a `T`, by id, mutably.
    pub fn iter_mut<T: Component>(&mut self) -> impl Iterator<Item = (ObjectId, &mut T)> + '_ {
        let index = self.typed_index::<T>();
        let Scene { slots, components, .. } = self;
        let items = match index {
            Some(i) => &mut downcast_mut::<T>(unique(&mut components[i].1)).items[..],
            None => &mut [][..],
        };
        let slots = &**slots;
        items.iter_mut().filter(move |(id, _)| active_in(slots, *id)).map(|(id, c)| (*id, c))
    }

    /// Borrows several distinct component stores at once, without moving values.
    /// Each view visits active objects in ascending `ObjectId` order. Views
    /// support independent iteration and lookup, including cross-object systems.
    /// Duplicate types (or registered names) panic before any mutable borrow.
    pub fn query_mut<Q: QueryMut>(&mut self) -> Q::Views<'_> {
        Q::borrow(self)
    }

    /// Every active object with a `T`, with its transform.
    pub fn iter_with_transform<T: Component>(
        &self,
    ) -> impl Iterator<Item = (ObjectId, &Transform, &T)> + '_ {
        self.iter::<T>().map(|(id, c)| (id, &self.object(id).expect("live").transform, c))
    }

    /// Every active object with both an `A` and a `B`, by id. `A` and `B`
    /// must be different types.
    pub fn iter2_mut<A: Component, B: Component>(
        &mut self,
    ) -> impl Iterator<Item = (ObjectId, &mut A, &mut B)> + '_ {
        assert_ne!(A::NAME, B::NAME, "iter2_mut needs two different component types");
        self.typed_storage_or_insert::<A>();
        self.typed_storage_or_insert::<B>();
        let ia = self.storage_index(A::NAME).expect("inserted");
        let ib = self.storage_index(B::NAME).expect("inserted");
        let Scene { slots, components, .. } = self;
        let (sa, sb) = if ia < ib {
            let (lo, hi) = components.split_at_mut(ib);
            (&mut lo[ia].1, &mut hi[0].1)
        } else {
            let (lo, hi) = components.split_at_mut(ia);
            (&mut hi[0].1, &mut lo[ib].1)
        };
        Join2 {
            a: downcast_mut::<A>(unique(sa)).items.iter_mut().peekable(),
            b: downcast_mut::<B>(unique(sb)).items.iter_mut().peekable(),
            slots,
        }
    }

    /// The registered names of the object's components, sorted.
    pub fn component_names(&self, id: ObjectId) -> Vec<&'static str> {
        self.components.iter().filter(|(_, s)| s.contains(id)).map(|(n, _)| *n).collect()
    }

    pub fn has_component(&self, id: ObjectId, name: &str) -> bool {
        self.storage_index(name).is_ok_and(|i| self.components[i].1.contains(id))
    }

    /// The object's `name` component, for reflection.
    pub fn component_mut(&mut self, id: ObjectId, name: &str) -> Option<&mut dyn Reflect> {
        let i = self.storage_index(name).ok()?;
        if !self.components[i].1.contains(id) {
            return None;
        }
        unique(&mut self.components[i].1).reflect_mut(id)
    }

    pub fn remove_component(&mut self, id: ObjectId, name: &str) -> bool {
        let Ok(i) = self.storage_index(name) else { return false };
        self.revision += 1;
        self.components[i].1.contains(id) && unique(&mut self.components[i].1).remove(id)
    }

    /// Where the editor lets you click the object, relative to its position:
    /// the first component (by name) that says.
    pub fn bounds(&self, id: ObjectId) -> Option<Bounds> {
        self.components.iter().find_map(|(_, s)| s.bounds(id))
    }

    pub(crate) fn serialize_component(
        &self,
        id: ObjectId,
        name: &str,
    ) -> Option<&dyn erased_serde::Serialize> {
        let i = self.storage_index(name).ok()?;
        self.components[i].1.serialize(id)
    }

    /// The free slots, as the ids their next objects would have had, most
    /// recently freed last.
    pub(crate) fn free_slots(&self) -> Vec<ObjectId> {
        self.free.iter().map(|&i| ObjectId::new(i, self.slots[i as usize].generation)).collect()
    }

    /// Loading: puts an object in exactly the slot `id` names, after its
    /// parent's other children (or the other roots).
    pub(crate) fn fill_slot(
        &mut self,
        id: ObjectId,
        name: String,
        parent: Option<ObjectId>,
        active: bool,
        transform: Transform,
    ) {
        let slots = Arc::make_mut(&mut self.slots);
        let index = id.index as usize;
        if slots.len() <= index {
            slots.resize(index + 1, Arc::new(Slot { generation: 0, object: None }));
        }
        let mut object = GameObject::new(name, parent);
        object.active = active;
        object.transform = transform;
        slots[index] = Arc::new(Slot { generation: id.generation, object: Some(object) });
        match parent {
            Some(p) => self.object_mut(p).expect("checked by the loader").children.push(id),
            None => self.roots.push(id),
        }
    }

    /// Loading: restores the free slots' generations. Every empty slot is
    /// free, reused lowest index first.
    pub(crate) fn finish_load(&mut self, free: Vec<ObjectId>) {
        let slots = Arc::make_mut(&mut self.slots);
        for id in &free {
            let index = id.index as usize;
            if slots.len() <= index {
                slots.resize(index + 1, Arc::new(Slot { generation: 0, object: None }));
            }
            if slots[index].object.is_none() {
                Arc::make_mut(&mut slots[index]).generation = id.generation;
            }
        }
        self.free =
            (0..slots.len() as u32).rev().filter(|&i| slots[i as usize].object.is_none()).collect();
    }

    /// Runs the queued commands in order.
    pub fn apply(&mut self, commands: &mut Commands) {
        let mut spawned = Vec::new();
        for op in commands.ops.drain(..) {
            match op {
                Op::Spawn { name, parent } => {
                    let id = match parent.map(|p| p.resolve(&spawned)) {
                        Some(Some(parent)) => self.spawn_child(parent, name),
                        _ => self.spawn(name),
                    };
                    spawned.push(id);
                }
                Op::Despawn(target) => {
                    if let Some(id) = target.resolve(&spawned) {
                        self.despawn(id);
                    }
                }
                Op::Run(f) => f(self, &spawned),
            }
        }
    }
}

/// A split borrow of one component store. Structural changes must wait until
/// all views have been dropped; component values are edited in place.
pub struct ComponentViewMut<'a, T> {
    items: &'a mut [(ObjectId, T)],
    slots: &'a [Arc<Slot>],
}

impl<T> ComponentViewMut<'_, T> {
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &T)> {
        self.items.iter().filter(|(id, _)| active_in(self.slots, *id)).map(|(id, c)| (*id, c))
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (ObjectId, &mut T)> {
        let slots = self.slots;
        self.items.iter_mut().filter(move |(id, _)| active_in(slots, *id)).map(|(id, c)| (*id, c))
    }

    /// Like `Scene::get`, direct lookup includes inactive objects.
    pub fn get(&self, id: ObjectId) -> Option<&T> {
        self.items.binary_search_by_key(&id, |(id, _)| *id).ok().map(|i| &self.items[i].1)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        self.items.binary_search_by_key(&id, |(id, _)| *id).ok().map(|i| &mut self.items[i].1)
    }
}

/// Tuple of component types accepted by `Scene::query_mut` (one to eight).
/// The tuple's views borrow disjoint stores, enforced by safe slice APIs.
pub trait QueryMut {
    type Views<'a>;
    fn borrow(scene: &mut Scene) -> Self::Views<'_>;
}

macro_rules! query_tuple {
    ($($t:ident:$i:tt),+) => {
        impl<$($t: Component),+> QueryMut for ($($t,)+) {
            type Views<'a> = ($(ComponentViewMut<'a, $t>,)+);

            fn borrow(scene: &mut Scene) -> Self::Views<'_> {
                let names = [$($t::NAME),+];
                for (i, name) in names.iter().enumerate() {
                    assert!(!names[..i].contains(name), "query_mut needs distinct component types");
                }
                $(scene.typed_storage_or_insert::<$t>();)+
                let indices = [$(scene.typed_index::<$t>().expect("inserted")),+];
                let stores = scene.components.get_disjoint_mut(indices).expect("distinct stores");
                // Array destructuring moves each unique borrow exactly once.
                #[allow(non_snake_case)]
                let [$($t),+] = stores;
                ($(ComponentViewMut {
                    items: &mut downcast_mut::<$t>(unique(&mut $t.1)).items,
                    slots: &scene.slots,
                },)+)
            }
        }
    };
}

query_tuple!(A:0);
query_tuple!(A:0, B:1);
query_tuple!(A:0, B:1, C:2);
query_tuple!(A:0, B:1, C:2, D:3);
query_tuple!(A:0, B:1, C:2, D:3, E:4);
query_tuple!(A:0, B:1, C:2, D:3, E:4, F:5);
query_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6);
query_tuple!(A:0, B:1, C:2, D:3, E:4, F:5, G:6, H:7);

/// Checks `id` and its ancestors are active.
fn active_in(slots: &[Arc<Slot>], id: ObjectId) -> bool {
    let mut at = Some(id);
    while let Some(id) = at {
        let Some(object) = slots
            .get(id.index as usize)
            .filter(|s| s.generation == id.generation)
            .and_then(|s| s.object.as_ref())
        else {
            return false;
        };
        if !object.active {
            return false;
        }
        at = object.parent;
    }
    true
}

/// Makes a storage unshared, copying it if a clone of the scene still
/// holds it, and returns it.
fn unique(storage: &mut Arc<dyn ErasedStorage>) -> &mut dyn ErasedStorage {
    if Arc::get_mut(storage).is_none() {
        *storage = storage.clone_arc();
    }
    Arc::get_mut(storage).expect("just made unique")
}

fn downcast<T: Component>(storage: &dyn ErasedStorage) -> &Storage<T> {
    storage.as_any().downcast_ref().unwrap_or_else(|| {
        panic!("two component types are both named {:?}", T::NAME);
    })
}

fn downcast_mut<T: Component>(storage: &mut dyn ErasedStorage) -> &mut Storage<T> {
    storage.as_any_mut().downcast_mut().unwrap_or_else(|| {
        panic!("two component types are both named {:?}", T::NAME);
    })
}

struct Join2<'a, A, B> {
    a: Peekable<IterMut<'a, (ObjectId, A)>>,
    b: Peekable<IterMut<'a, (ObjectId, B)>>,
    slots: &'a [Arc<Slot>],
}

impl<'a, A, B> Iterator for Join2<'a, A, B> {
    type Item = (ObjectId, &'a mut A, &'a mut B);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let ia = self.a.peek()?.0;
            let ib = self.b.peek()?.0;
            if ia < ib {
                self.a.next();
            } else if ib < ia {
                self.b.next();
            } else {
                let (id, a) = self.a.next()?;
                let (_, b) = self.b.next()?;
                if active_in(self.slots, *id) {
                    return Some((*id, a, b));
                }
            }
        }
    }
}

/// An object a queued command acts on: one that exists, or one an earlier
/// command in the same buffer spawns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Id(ObjectId),
    /// The `n`th object this buffer spawns.
    Spawned(usize),
}

impl Target {
    fn resolve(self, spawned: &[ObjectId]) -> Option<ObjectId> {
        match self {
            Target::Id(id) => Some(id),
            Target::Spawned(n) => spawned.get(n).copied(),
        }
    }
}

impl From<ObjectId> for Target {
    fn from(id: ObjectId) -> Self {
        Target::Id(id)
    }
}

type Deferred = Box<dyn FnOnce(&mut Scene, &[ObjectId]) + Send + Sync>;

enum Op {
    Spawn { name: String, parent: Option<Target> },
    Despawn(Target),
    Run(Deferred),
}

/// Spawns, despawns and component changes queued during a tick and applied
/// together, in order, with [`Scene::apply`], so nothing changes under a
/// query that is still running.
#[derive(Default)]
pub struct Commands {
    ops: Vec<Op>,
    spawns: usize,
}

impl Commands {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn spawn(&mut self, name: impl Into<String>) -> Target {
        self.ops.push(Op::Spawn { name: name.into(), parent: None });
        self.spawns += 1;
        Target::Spawned(self.spawns - 1)
    }

    pub fn spawn_child(&mut self, parent: impl Into<Target>, name: impl Into<String>) -> Target {
        self.ops.push(Op::Spawn { name: name.into(), parent: Some(parent.into()) });
        self.spawns += 1;
        Target::Spawned(self.spawns - 1)
    }

    pub fn despawn(&mut self, target: impl Into<Target>) {
        self.ops.push(Op::Despawn(target.into()));
    }

    pub fn insert<T: Component>(&mut self, target: impl Into<Target>, value: T) {
        let target = target.into();
        self.run(move |scene, spawned| {
            if let Some(id) = target.resolve(spawned) {
                scene.insert(id, value);
            }
        });
    }

    pub fn remove<T: Component>(&mut self, target: impl Into<Target>) {
        let target = target.into();
        self.run(move |scene, spawned| {
            if let Some(id) = target.resolve(spawned) {
                scene.remove::<T>(id);
            }
        });
    }

    pub fn set_position(&mut self, target: impl Into<Target>, position: FxVec2) {
        let target = target.into();
        self.run(move |scene, spawned| {
            if let Some(t) = target.resolve(spawned).and_then(|id| scene.transform_mut(id)) {
                t.position = position;
            }
        });
    }

    /// Queues any change to the scene. `f` gets the scene and the ids of
    /// the objects this buffer has spawned so far, in order.
    pub fn run(&mut self, f: impl FnOnce(&mut Scene, &[ObjectId]) + Send + Sync + 'static) {
        self.ops.push(Op::Run(Box::new(f)));
    }

    /// Existing objects this buffer despawns, in order (not ones it spawns
    /// itself). The Behaviour runtime calls their `on_destroy` first.
    pub fn despawned(&self) -> Vec<ObjectId> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                Op::Despawn(Target::Id(id)) => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Moves `other`'s commands to the end of this buffer. Its `Spawned`
    /// targets keep counting from this buffer's spawns.
    pub fn append(&mut self, other: &mut Commands) {
        let offset = self.spawns;
        for op in other.ops.drain(..) {
            let shift = |t: Target| match t {
                Target::Spawned(n) => Target::Spawned(n + offset),
                id => id,
            };
            self.ops.push(match op {
                Op::Spawn { name, parent } => Op::Spawn { name, parent: parent.map(shift) },
                Op::Despawn(t) => Op::Despawn(shift(t)),
                Op::Run(f) => Op::Run(Box::new(move |scene, spawned| {
                    f(scene, spawned.get(offset..).unwrap_or(&[]))
                })),
            });
        }
        self.spawns += std::mem::take(&mut other.spawns);
    }
}
