use reign_core::{Fx, FxVec2, checksum};
use reign_scene::path::Value;
use reign_scene::{Commands, Component, ComponentRegistry, ObjectId, Scene};
use serde::{Deserialize, Serialize};

use super::*;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Stats {
    hp: i32,
    speed: Fx,
    /// Points at another object of the same prefab.
    eye: ObjectId,
    tags: Vec<u8>,
    boss: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Glow {
    radius: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Tag {
    label: String,
}

fn registry() -> ComponentRegistry {
    let mut r = ComponentRegistry::new();
    r.register::<Stats>().register::<Glow>().register::<Tag>();
    r
}

const GOBLIN: Guid = Guid(0x11);
const CAMP: Guid = Guid(0x22);

/// A goblin: a root with stats, an eye with a glow, and a tagged hat.
fn goblin() -> String {
    let mut s = Scene::new();
    let root = s.spawn("Goblin");
    let eye = s.spawn_child(root, "Eye");
    let hat = s.spawn_child(root, "Hat");
    s.insert(root, Stats { hp: 5, speed: Fx::ONE, eye, tags: vec![1, 2], boss: None });
    s.insert(eye, Glow { radius: 10 });
    s.insert(hat, Tag { label: "pointy".into() });
    s.transform_mut(eye).unwrap().position = FxVec2::new(Fx::from_px(2), Fx::from_px(-8));
    s.to_ron().unwrap()
}

/// A camp: a fire, and a goblin placed in it with a bigger glow.
fn camp(lookup: &MemoryPrefabs) -> String {
    let mut s = Scene::new();
    let root = s.spawn("Camp");
    s.insert(root, Glow { radius: 40 });
    let g = s.spawn_child(root, "Guard");
    s.insert(g, PrefabInstance::new(Handle::new(GOBLIN)));
    expand(&mut s, g, lookup).unwrap();
    let inst = s.get::<PrefabInstance>(g).unwrap().clone();
    let eye = inst.object(&[1]).unwrap();
    s.get_mut::<Glow>(eye).unwrap().radius = 25;
    prefab_from(&s, root, lookup).unwrap()
}

fn prefabs() -> MemoryPrefabs {
    let mut p = MemoryPrefabs::new(registry());
    p.texts.insert(GOBLIN, goblin());
    let camp = camp(&p);
    p.texts.insert(CAMP, camp);
    p
}

fn placed(lookup: &MemoryPrefabs, prefab: Guid) -> (Scene, ObjectId) {
    let mut scene = Scene::new();
    scene.spawn("Other object");
    let root = scene.spawn("Instance");
    scene.transform_mut(root).unwrap().position = FxVec2::new(Fx::from_px(100), Fx::ZERO);
    scene.insert(root, PrefabInstance::new(Handle::new(prefab)));
    expand(&mut scene, root, lookup).unwrap();
    (scene, root)
}

#[test]
fn instances_get_the_prefabs_objects_with_references_remapped() {
    let lookup = prefabs();
    let (scene, root) = placed(&lookup, GOBLIN);
    let inst = scene.get::<PrefabInstance>(root).unwrap();
    let eye = inst.object(&[1]).unwrap();
    assert_eq!(scene.get::<Stats>(root).unwrap().eye, eye, "points at its own eye");
    assert_eq!(scene.get::<Glow>(eye).unwrap().radius, 10);
    assert_eq!(scene.children(root).len(), 2);
    assert!(scene.object(eye).unwrap().dont_save, "prefab objects aren't saved");
    assert_eq!(scene.world_position(eye).x, Fx::from_px(102));
}

#[test]
fn overrides_round_trip_through_save_and_load_then_revert_and_apply() {
    let lookup = prefabs();
    let (mut scene, root) = placed(&lookup, GOBLIN);
    let inst = scene.get::<PrefabInstance>(root).unwrap().clone();
    let (eye, hat) = (inst.object(&[1]).unwrap(), inst.object(&[2]).unwrap());
    // Edit: a field, a nested list, an option, a child's component and
    // name, add a component and remove one.
    let stats = scene.get_mut::<Stats>(root).unwrap();
    stats.hp = 9;
    stats.tags.push(7);
    stats.boss = Some(3);
    scene.get_mut::<Glow>(eye).unwrap().radius = 33;
    scene.object_mut(eye).unwrap().name = "Big eye".into();
    scene.insert(hat, Glow { radius: 4 });
    scene.remove::<Tag>(hat);
    let edited = overrides(&mut scene, root, &lookup).unwrap();
    let paths: Vec<_> =
        edited.overrides.iter().map(|o| (o.object.clone(), o.path.as_str())).collect();
    assert_eq!(
        paths,
        [
            (vec![], "Stats.hp"),
            (vec![], "Stats.tags.#len"),
            (vec![], "Stats.tags.2"),
            (vec![], "Stats.boss"),
            (vec![], "Stats.boss.?"),
            (vec![1], "GameObject.name"),
            (vec![1], "Glow.radius"),
        ]
    );
    assert_eq!(edited.added.len(), 1);
    assert_eq!(edited.removed, [RemovedComponent { object: vec![2], component: "Tag".into() }]);

    // Save, load, expand: the same instance comes back.
    let mut reg = registry();
    reg.register::<PrefabInstance>();
    let text = saved_form(&scene, &lookup).unwrap().to_ron().unwrap();
    assert!(!text.contains("pointy"), "prefab content isn't copied into the scene");
    let mut loaded = Scene::from_ron(&text, &reg).unwrap();
    assert!(expand_all(&mut loaded, &lookup).is_empty());
    let root2 = loaded.iter::<PrefabInstance>().next().unwrap().0;
    let inst2 = loaded.get::<PrefabInstance>(root2).unwrap().clone();
    let (eye2, hat2) = (inst2.object(&[1]).unwrap(), inst2.object(&[2]).unwrap());
    let stats = loaded.get::<Stats>(root2).unwrap();
    assert_eq!((stats.hp, stats.tags.clone(), stats.boss), (9, vec![1, 2, 7], Some(3)));
    assert_eq!(stats.eye, eye2);
    assert_eq!(loaded.get::<Glow>(eye2).unwrap().radius, 33);
    assert_eq!(loaded.name(eye2), Some("Big eye"));
    assert_eq!(loaded.get::<Glow>(hat2).unwrap().radius, 4);
    assert!(!loaded.has::<Tag>(hat2));
    let again = overrides(&mut loaded, root2, &lookup).unwrap();
    assert_eq!(
        (&again.overrides, &again.added, &again.removed),
        (&edited.overrides, &edited.added, &edited.removed)
    );

    // Revert one field: it takes the prefab's value and stops overriding.
    let value = prefab_value(&loaded, root2, eye2, "Glow.radius", &lookup).unwrap();
    assert_eq!(value, Value::Int(10));
    revert_property(&mut loaded, root2, eye2, "Glow.radius", &lookup).unwrap();
    assert_eq!(loaded.get::<Glow>(eye2).unwrap().radius, 10);
    let after = loaded.get::<PrefabInstance>(root2).unwrap().clone();
    assert!(!after.overrides.iter().any(|o| o.path == "Glow.radius"));
    assert_eq!(after.overrides.len(), edited.overrides.len() - 1);
    // A refresh (the prefab changed) keeps the remaining overrides.
    refresh(&mut loaded, root2, &lookup).unwrap();
    assert_eq!(loaded.get::<Stats>(root2).unwrap().hp, 9);

    // Revert all: back to the prefab.
    let mut reverted = loaded.clone();
    revert_all(&mut reverted, root2, &lookup).unwrap();
    assert_eq!(reverted.get::<Stats>(root2).unwrap().hp, 5);
    assert!(!overrides(&mut reverted, root2, &lookup).unwrap().has_overrides());

    // Apply all: the instance becomes the prefab, and has no overrides.
    let mut applied = prefabs();
    let text = apply_all(&mut loaded, root2, &applied).unwrap();
    applied.texts.insert(GOBLIN, text);
    refresh(&mut loaded, root2, &applied).unwrap();
    let clean = overrides(&mut loaded, root2, &applied).unwrap();
    assert!(!clean.has_overrides(), "{clean:?}");
    let stats = loaded.get::<Stats>(root2).unwrap();
    assert_eq!((stats.hp, stats.boss), (9, Some(3)));
    let eye3 = loaded.get::<PrefabInstance>(root2).unwrap().object(&[1]).unwrap();
    assert_eq!(stats.eye, eye3);
}

#[test]
fn nested_prefabs_resolve_with_their_own_and_outer_overrides() {
    let lookup = prefabs();
    let (mut scene, root) = placed(&lookup, CAMP);
    let inst = scene.get::<PrefabInstance>(root).unwrap().clone();
    assert_eq!(scene.get::<Glow>(root).unwrap().radius, 40);
    let guard_index = inst.objects.iter().find(|(p, _)| p.len() == 1).unwrap().0[0];
    let guard = inst.object(&[guard_index]).unwrap();
    let eye = inst.object(&[guard_index, 1]).unwrap();
    assert!(scene.is_ancestor(guard, eye));
    assert_eq!(scene.get::<Glow>(eye).unwrap().radius, 25, "the camp's override of the goblin");
    assert_eq!(scene.get::<Stats>(guard).unwrap().eye, eye, "inner references remapped");
    // An override of the nested goblin from the outer instance.
    scene.get_mut::<Stats>(guard).unwrap().hp = 1;
    let o = overrides(&mut scene, root, &lookup).unwrap();
    assert_eq!(o.overrides.len(), 1);
    assert_eq!(o.overrides[0].object, vec![guard_index]);
    // And it survives a save and load.
    let mut reg = registry();
    reg.register::<PrefabInstance>();
    let text = saved_form(&scene, &lookup).unwrap().to_ron().unwrap();
    let mut loaded = Scene::from_ron(&text, &reg).unwrap();
    assert!(expand_all(&mut loaded, &lookup).is_empty());
    let root2 = loaded.roots()[1];
    let inst2 = loaded.get::<PrefabInstance>(root2).unwrap().clone();
    let guard2 = inst2.object(&[guard_index]).unwrap();
    assert_eq!(loaded.get::<Stats>(guard2).unwrap().hp, 1);
    assert_eq!(loaded.get::<Glow>(inst2.object(&[guard_index, 1]).unwrap()).unwrap().radius, 25);
}

#[test]
fn a_prefab_inside_itself_is_an_error_not_a_hang() {
    let mut lookup = MemoryPrefabs::new(registry());
    let mut s = Scene::new();
    let root = s.spawn("Loop");
    let child = s.spawn_child(root, "Again");
    s.insert(child, PrefabInstance::new(Handle::new(GOBLIN)));
    lookup.texts.insert(GOBLIN, s.to_ron().unwrap());
    let mut scene = Scene::new();
    let r = scene.spawn("x");
    scene.insert(r, PrefabInstance::new(Handle::new(GOBLIN)));
    assert!(expand(&mut scene, r, &lookup).is_err());
}

#[test]
fn instantiate_gives_the_same_ids_on_two_peers() {
    let lookup = prefabs();
    let goblin = lookup.template(Handle::new(GOBLIN)).unwrap();
    let camp = lookup.template(Handle::new(CAMP)).unwrap();
    let peer = |seed: u32| {
        let _ = seed;
        let mut scene = Scene::new();
        for i in 0..5 {
            scene.spawn(format!("filler {i}"));
        }
        let mut log = Vec::new();
        for tick in 0..60u32 {
            let mut commands = Commands::new();
            let at = FxVec2::new(Fx::from_px(tick as i32), Fx::ZERO);
            let t = if tick % 3 == 0 { camp.clone() } else { goblin.clone() };
            commands.instantiate(t, at);
            if tick % 4 == 1 {
                let oldest = scene.iter::<PrefabInstance>().next().map(|(id, _)| id);
                if let Some(id) = oldest {
                    commands.despawn(id);
                }
            }
            scene.apply(&mut commands);
            log.push(scene.iter::<PrefabInstance>().map(|(id, _)| id).collect::<Vec<_>>());
        }
        (checksum(&scene), log)
    };
    let (a, b) = (peer(1), peer(2));
    assert_eq!(a, b);
    assert!(a.1.last().unwrap().len() > 10);
}
