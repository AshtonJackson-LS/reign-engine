//! The scene's object table, hierarchy, queries and commands.

use std::any::Any;

use reign_core::{Fx, FxVec2, checksum};
use reign_scene::reflect::ReflectStruct;
use reign_scene::{Commands, Component, FieldInfo, Mut, ObjectId, Reflect, Scene};
use serde::{Deserialize, Serialize};

/// A component written by hand, as the derive would.
macro_rules! component {
    ($name:ident { $($field:ident: $ty:ty),* }) => {
        #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
        struct $name { $($field: $ty),* }
        impl ReflectStruct for $name {
            fn fields(&self) -> &'static [FieldInfo] {
                const F: &[FieldInfo] = &[$(FieldInfo::new(stringify!($field), stringify!($ty))),*];
                F
            }
            fn field_mut(&mut self, index: usize) -> Option<&mut dyn Reflect> {
                let fields: Vec<&mut dyn Reflect> = vec![$(&mut self.$field),*];
                fields.into_iter().nth(index)
            }
        }
        impl Reflect for $name {
            fn type_name(&self) -> &'static str { stringify!($name) }
            fn reflect_mut(&mut self) -> Mut<'_> { Mut::Struct(self) }
            fn as_any(&self) -> &dyn Any { self }
            fn as_any_mut(&mut self) -> &mut dyn Any { self }
        }
        impl Component for $name {
            const NAME: &'static str = stringify!($name);
        }
    };
}

component!(Health { hp: i32 });
component!(Speed { per_tick: Fx });

#[test]
fn stale_handles_are_rejected() {
    let mut scene = Scene::new();
    let a = scene.spawn("a");
    scene.insert(a, Health { hp: 3 });
    assert!(scene.despawn(a));
    assert!(!scene.despawn(a), "already gone");
    let b = scene.spawn("b");
    assert_eq!(b.index, a.index, "the slot is reused");
    assert_ne!(b, a);
    assert!(!scene.contains(a));
    assert!(scene.object(a).is_none());
    assert!(scene.get::<Health>(a).is_none());
    assert!(scene.insert(a, Health { hp: 9 }).is_none());
    assert!(scene.get::<Health>(b).is_none(), "a stale insert must not land on b");
    assert!(scene.transform_mut(a).is_none());
    assert!(!scene.set_parent(b, Some(a), None));
}

#[test]
fn hierarchy_and_world_positions() {
    let mut scene = Scene::new();
    let root = scene.spawn("root");
    let child = scene.spawn_child(root, "child");
    let grandchild = scene.spawn_child(child, "grandchild");
    scene.transform_mut(root).unwrap().position = FxVec2::from_px(100, 10);
    scene.transform_mut(child).unwrap().position = FxVec2::from_px(5, 5);
    scene.transform_mut(grandchild).unwrap().position = FxVec2::from_px(1, 2);
    assert_eq!(scene.world_position(grandchild), FxVec2::from_px(106, 17));
    assert_eq!(scene.walk(), [root, child, grandchild]);

    // No cycles.
    assert!(!scene.set_parent(root, Some(grandchild), None));
    // Reparent keeping the world position.
    assert!(scene.set_parent_keep_world(grandchild, None, Some(0)));
    assert_eq!(scene.roots(), [grandchild, root]);
    assert_eq!(scene.world_position(grandchild), FxVec2::from_px(106, 17));

    // Despawning a parent takes its children and their components.
    scene.insert(child, Health { hp: 1 });
    let deep = scene.spawn_child(child, "deep");
    scene.insert(deep, Health { hp: 2 });
    assert!(scene.despawn(root));
    assert!(!scene.contains(child) && !scene.contains(deep));
    assert_eq!(scene.iter::<Health>().count(), 0);
    assert_eq!(scene.roots(), [grandchild]);
}

#[test]
fn inactive_objects_and_their_children_are_skipped() {
    let mut scene = Scene::new();
    let parent = scene.spawn("parent");
    let child = scene.spawn_child(parent, "child");
    let other = scene.spawn("other");
    for id in [parent, child, other] {
        scene.insert(id, Health { hp: 1 });
    }
    scene.object_mut(parent).unwrap().active = false;
    let seen: Vec<_> = scene.iter::<Health>().map(|(id, _)| id).collect();
    assert_eq!(seen, [other]);
    assert_eq!(scene.iter_mut::<Health>().count(), 1);
    assert!(scene.get::<Health>(child).is_some(), "direct access still works");
}

/// Runs the same spawns and despawns in a different order of operations
/// and checks queries agree, and iterate by id.
#[test]
fn iteration_order_is_deterministic_after_churn() {
    fn churn(order: &[usize]) -> (Scene, Vec<(ObjectId, i32)>) {
        let mut scene = Scene::new();
        let ids: Vec<_> = (0..20).map(|i| scene.spawn(format!("o{i}"))).collect();
        for &i in order {
            scene.insert(ids[i], Health { hp: i as i32 });
        }
        for &i in order.iter().filter(|i| *i % 3 == 0) {
            scene.despawn(ids[i]);
        }
        for i in 0..5 {
            let id = scene.spawn(format!("new{i}"));
            scene.insert(id, Health { hp: 100 + i });
        }
        let items = scene.iter::<Health>().map(|(id, h)| (id, h.hp)).collect();
        (scene, items)
    }
    let forward: Vec<usize> = (0..20).collect();
    let (a, items_a) = churn(&forward);
    let (b, items_b) = churn(&forward);
    assert_eq!(items_a, items_b);
    assert_eq!(checksum(&a), checksum(&b));
    assert!(items_a.windows(2).all(|w| w[0].0 < w[1].0), "sorted by id: {items_a:?}");
    let backward: Vec<usize> = (0..20).rev().collect();
    let (_, items_c) = churn(&backward);
    let mut values_a: Vec<_> = items_a.iter().map(|i| i.1).collect();
    let mut values_c: Vec<_> = items_c.iter().map(|i| i.1).collect();
    values_a.sort();
    values_c.sort();
    assert_eq!(values_a, values_c);
}

#[test]
fn join_visits_objects_with_both_components() {
    let mut scene = Scene::new();
    let ids: Vec<_> = (0..8).map(|i| scene.spawn(format!("o{i}"))).collect();
    for (i, id) in ids.iter().enumerate() {
        if i % 2 == 0 {
            scene.insert(*id, Health { hp: i as i32 });
        }
        if i % 3 == 0 {
            scene.insert(*id, Speed { per_tick: Fx::from_px(i as i32) });
        }
    }
    let mut joined = Vec::new();
    for (id, health, speed) in scene.iter2_mut::<Health, Speed>() {
        health.hp += 10;
        speed.per_tick = Fx::ZERO;
        joined.push(id);
    }
    assert_eq!(joined, [ids[0], ids[6]]);
    assert_eq!(scene.get::<Health>(ids[0]).unwrap().hp, 10);
    assert_eq!(scene.get::<Health>(ids[2]).unwrap().hp, 2, "not in the join");
    // The other order joins the same objects.
    assert_eq!(scene.iter2_mut::<Speed, Health>().count(), 2);
}

#[test]
fn clones_are_independent_and_hash_the_same() {
    let mut scene = Scene::new();
    let a = scene.spawn("a");
    scene.insert(a, Health { hp: 5 });
    let snapshot = scene.clone();
    assert_eq!(checksum(&snapshot), checksum(&scene));
    scene.get_mut::<Health>(a).unwrap().hp = 6;
    scene.transform_mut(a).unwrap().position.x = Fx::ONE;
    assert_eq!(snapshot.get::<Health>(a).unwrap().hp, 5, "copy-on-write");
    assert_eq!(snapshot.transform(a).unwrap().position, FxVec2::ZERO);
    assert_ne!(checksum(&snapshot), checksum(&scene));
    scene = snapshot.clone();
    assert_eq!(checksum(&snapshot), checksum(&scene));

    // A type added and removed again leaves an equal scene.
    let mut other = snapshot.clone();
    other.insert(a, Speed::default());
    other.remove::<Speed>(a);
    assert_eq!(checksum(&other), checksum(&snapshot));
}

#[test]
fn commands_apply_in_order() {
    let mut scene = Scene::new();
    let keep = scene.spawn("keep");
    let doomed = scene.spawn("doomed");
    let mut commands = Commands::new();
    let parent = commands.spawn("spawned");
    commands.insert(parent, Health { hp: 7 });
    let child = commands.spawn_child(parent, "child");
    commands.set_position(child, FxVec2::from_px(3, 4));
    commands.despawn(doomed);
    commands.insert(keep, Speed { per_tick: Fx::ONE });
    // Nothing happens until applied.
    assert!(scene.contains(doomed));
    scene.apply(&mut commands);
    assert!(commands.is_empty());
    assert!(!scene.contains(doomed));
    let spawned = scene.roots()[1];
    assert_eq!(scene.name(spawned), Some("spawned"));
    assert_eq!(scene.get::<Health>(spawned).unwrap().hp, 7);
    let child = scene.children(spawned)[0];
    assert_eq!(scene.world_position(child), FxVec2::from_px(3, 4));
    assert_eq!(scene.get::<Speed>(keep).unwrap().per_tick, Fx::ONE);
}

#[test]
fn duplicate_copies_the_subtree_next_to_the_original() {
    let mut scene = Scene::new();
    let first = scene.spawn("first");
    let parent = scene.spawn("parent");
    let child = scene.spawn_child(parent, "child");
    let last = scene.spawn("last");
    scene.insert(parent, Health { hp: 1 });
    scene.insert(child, Speed { per_tick: Fx::ONE });
    let copy = scene.duplicate(parent).unwrap();
    assert_eq!(scene.roots(), [first, parent, copy, last]);
    assert_eq!(scene.get::<Health>(copy), Some(&Health { hp: 1 }));
    let copied_child = scene.children(copy)[0];
    assert_ne!(copied_child, child);
    assert_eq!(scene.get::<Speed>(copied_child).unwrap().per_tick, Fx::ONE);
}

#[test]
fn components_by_name_for_the_editor() {
    let mut scene = Scene::new();
    let a = scene.spawn("a");
    scene.insert(a, Health { hp: 4 });
    scene.insert(a, Speed::default());
    assert_eq!(scene.component_names(a), ["Health", "Speed"]);
    let health = scene.component_mut(a, "Health").unwrap();
    let Mut::Struct(fields) = health.reflect_mut() else { panic!() };
    let Mut::Int(mut hp) = fields.field_mut(0).unwrap().reflect_mut() else { panic!() };
    hp.set(11);
    assert_eq!(scene.get::<Health>(a).unwrap().hp, 11);
    assert!(scene.remove_component(a, "Health"));
    assert!(!scene.remove_component(a, "Health"));
    assert_eq!(scene.component_names(a), ["Speed"]);
}

component!(Mana { points: i32 });

#[test]
fn split_query_keeps_all_borrows_live_and_snapshots_isolated() {
    let mut scene = Scene::new();
    let gone = scene.spawn("gone");
    let inactive = scene.spawn("inactive");
    let child = scene.spawn_child(inactive, "child");
    scene.object_mut(inactive).unwrap().active = false;
    scene.despawn(gone);
    let a = scene.spawn("reused slot");
    let b = scene.spawn("other");
    for id in [b, child, a] {
        scene.insert(id, Health { hp: 10 });
        scene.insert(id, Speed { per_tick: Fx::ONE });
    }
    scene.insert(b, Mana { points: 5 });
    let snapshot = scene.clone();
    let before = checksum(&snapshot);
    // Deliberately request stores out of their alphabetical storage order.
    let (mut speed, mut health, mut mana) = scene.query_mut::<(Speed, Health, Mana)>();
    let mut refs: Vec<_> = health.iter_mut().collect();
    assert_eq!(refs.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [a, b]);
    assert!(speed.get(gone).is_none());
    let velocity = speed.get_mut(a).unwrap();
    let magic = mana.get_mut(b).unwrap();
    refs[0].1.hp -= 2;
    refs[1].1.hp += 1;
    velocity.per_tick += Fx::ONE;
    magic.points = 9;
    assert_eq!(snapshot.get::<Health>(a).unwrap().hp, 10);
    assert_eq!(checksum(&snapshot), before);
    assert_eq!(scene.get::<Health>(a).unwrap().hp, 8);
    assert_eq!(scene.get::<Mana>(b).unwrap().points, 9);
    assert_eq!(scene.get::<Health>(child).unwrap().hp, 10);
}

#[test]
fn querying_absent_stores_preserves_hash_and_equality() {
    let mut scene = Scene::new();
    let before = scene.clone();
    let (a, b, c) = scene.query_mut::<(Health, Speed, Mana)>();
    assert_eq!(a.iter().count() + b.iter().count() + c.iter().count(), 0);
    assert_eq!(checksum(&scene), checksum(&before));
    assert_eq!(scene, before);
}

#[test]
#[should_panic(expected = "distinct component types")]
fn split_query_refuses_aliasing() {
    Scene::new().query_mut::<(Health, Speed, Health)>();
}
