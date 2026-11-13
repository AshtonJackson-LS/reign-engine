//! Scene files: RON round trips, exact numbers, and readable errors.

mod common;

use common::{Everything, Marker, registry};
use reign_core::{Fx, FxVec2, checksum};
use reign_scene::{Scene, Transform};

/// A scene with a hierarchy, components, inactive objects and despawn
/// churn, so the free list and generations are exercised too.
fn sample_scene() -> Scene {
    let mut scene = Scene::new();
    let root = scene.spawn("Root");
    scene.insert(root, Everything::sample());
    let child = scene.spawn_child(root, "Child");
    scene.object_mut(child).unwrap().transform =
        Transform { position: FxVec2::from_px(-3, 2), z: 5, flip_x: true, flip_y: false };
    scene.insert(child, Marker { tag: 1 });
    let doomed = scene.spawn("Doomed");
    let other = scene.spawn("Other");
    scene.object_mut(other).unwrap().active = false;
    scene.insert(other, Marker { tag: 2 });
    scene.despawn(doomed);
    let reused = scene.spawn_child(child, "Reused");
    scene.insert(reused, Everything::default());
    let gone = scene.spawn("Gone");
    scene.despawn(gone);
    scene
}

#[test]
fn save_then_load_gives_an_identical_scene() {
    let scene = sample_scene();
    let text = scene.to_ron().unwrap();
    let loaded = Scene::from_ron(&text, &registry()).unwrap_or_else(|e| panic!("{e}\n{text}"));
    assert_eq!(checksum(&loaded), checksum(&scene), "{text}");
    assert_eq!(loaded.to_ron().unwrap(), text, "saving again changes nothing");
    // Including what happens next: the same slots get reused.
    let (mut a, mut b) = (scene, loaded);
    assert_eq!(a.spawn("next"), b.spawn("next"));
    assert_eq!(a.spawn("next"), b.spawn("next"));
    assert_eq!(checksum(&a), checksum(&b));
}

#[test]
fn the_file_is_readable() {
    let text = sample_scene().to_ron().unwrap();
    assert!(text.starts_with("// A Reign scene."), "{text}");
    assert!(text.contains("name: \"Child\""), "{text}");
    assert!(text.contains("position: (x: \"-3\", y: \"2\")"), "{text}");
    assert!(text.contains("active: false"), "{text}");
    assert!(text.contains("\"Marker\": (tag: 1)"), "{text}");
    assert!(!text.contains("active: true"), "defaults are left out:\n{text}");
    assert!(text.contains("free: ["), "{text}");
}

#[test]
fn hand_written_numbers_load_exactly() {
    let text = r#"(
        version: 1,
        objects: [
            (id: (0, 0), name: "A", transform: (position: (x: "0.1", y: "-3.5"))),
            (id: (1, 0), name: "B", parent: Some((0, 0)), transform: (position: (x: 12, y: "-0.00390625"))),
        ],
    )"#;
    let scene = Scene::from_ron(text, &registry()).unwrap();
    let a = scene.roots()[0];
    let b = scene.children(a)[0];
    assert_eq!(
        scene.transform(a).unwrap().position,
        FxVec2 { x: Fx::from_raw(26), y: Fx::from_raw(-896) }
    );
    assert_eq!(
        scene.transform(b).unwrap().position,
        FxVec2 { x: Fx::from_px(12), y: Fx::from_raw(-1) }
    );
}

#[test]
fn objects_marked_dont_save_are_left_out() {
    let mut scene = Scene::new();
    let kept = scene.spawn("kept");
    let runtime = scene.spawn("runtime");
    scene.spawn_child(runtime, "under runtime");
    scene.object_mut(runtime).unwrap().dont_save = true;
    let text = scene.to_ron().unwrap();
    assert!(!text.contains("runtime"), "{text}");
    let loaded = Scene::from_ron(&text, &registry()).unwrap();
    assert_eq!(loaded.walk(), [kept]);
    // Their slots are free again, deterministically.
    let mut again = Scene::from_ron(&text, &registry()).unwrap();
    let mut loaded = loaded;
    assert_eq!(loaded.spawn("x"), again.spawn("x"));
}

#[test]
fn bad_files_say_what_is_wrong() {
    let reg = registry();
    let err = |text: &str| Scene::from_ron(text, &reg).unwrap_err().to_string();
    assert!(
        err(r#"(version: 1, objects: [(id: (0, 0), name: "A", components: {"Nope": ()})])"#)
            .contains("unknown component \"Nope\"")
    );
    assert!(
        err(r#"(version: 1, objects: [(id: (0, 0), name: "A", parent: Some((5, 0)))])"#)
            .contains("isn't listed before it")
    );
    assert!(
        err(r#"(version: 1, objects: [(id: (0, 0), name: "A"), (id: (0, 0), name: "B")])"#)
            .contains("used twice")
    );
    assert!(err(r#"(version: 1, objects: [(id: (0, 0), name: "A", transform: (position: (x: 1.5, y: "0")))])"#)
        .contains("float"));
    assert!(err(r#"(version: 1, objects: [(id: (0, 0), name: "A", transform: (position: (x: "1e3", y: "0")))])"#)
        .contains("not a decimal number"));
    assert!(err(r#"(version: 2, objects: [])"#).contains("version 2"));
    assert!(err(r#"(version: 1, objects: [(id: (0, 0), name: "A", components: {"Marker": (tag: 300)})])"#)
        .contains("Marker"));
}
