//! Tests for exporting: packing a project and playing it from the pack,
//! the start scene, and the exported layouts.

use std::path::{Path, PathBuf};

use reign::assets::{AssetDb, SpriteSheet};
use reign::behaviour::Behaviours;
use reign::core::{FxVec2, InputFrame};
use reign::editor::EditorApp;

use crate::editor::{Project, ReignEngine};
use crate::export::{self, Export, Layout, file_title};
use crate::player::{
    self, GAME_FILE, GameInfo, PACK_FILE, choose_scene, data_dir, load_scene, scene_paths,
};
use crate::runtime::{Scripts, World, schedule};
use crate::{SpriteRenderer, starter};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("reign-export-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// A project whose main scene has a box above a platform, and a second
/// scene; every asset has its `.meta`.
fn project_with_a_level(name: &str) -> (Project, PathBuf) {
    let root = temp_dir(name);
    let project = Project::open(root.clone()).expect("project opens");
    let mut app = ReignEngine::new(Project { root: root.clone() }, Scripts::none(), None);
    app.create("2D Object/Physics/Static Platform", None, (0, 100)).expect("platform");
    app.create("2D Object/Physics/Dynamic Box", None, (10, 0)).expect("box");
    let text = app.world.scene.to_ron().expect("saves");
    std::fs::write(project.scenes().join("main.scene.ron"), text).unwrap();
    let empty = reign::scene::Scene::new().to_ron().expect("saves");
    std::fs::write(project.scenes().join("another.scene.ron"), empty).unwrap();
    AssetDb::loose(project.assets()).expect("metas are written");
    (project, root)
}

fn leak(blob: Vec<u8>) -> &'static [u8] {
    Box::leak(blob.into_boxed_slice())
}

/// Steps a scene the way the game does, with no input.
fn play(scene: reign::scene::Scene, ticks: u32) -> World {
    let mut world = World::new(scene);
    let behaviours = Behaviours::new();
    let schedule = schedule();
    for _ in 0..ticks {
        world.step(InputFrame::default(), &behaviours, &schedule);
    }
    world
}

#[test]
fn the_start_scene_is_main_then_the_first_in_scenes() {
    let paths = |p: &[&str]| p.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let all = paths(&[
        "levels/b.scene.ron",
        "scenes/z.scene.ron",
        "scenes/main.scene.ron",
        "a.scene.ron",
    ]);
    assert_eq!(choose_scene(&all, None).as_deref(), Some("scenes/main.scene.ron"));
    assert_eq!(choose_scene(&all, Some("")).as_deref(), Some("scenes/main.scene.ron"));
    assert_eq!(
        choose_scene(&all, Some("levels/b.scene.ron")).as_deref(),
        Some("levels/b.scene.ron")
    );
    assert_eq!(
        choose_scene(&all, Some("nope.scene.ron")).as_deref(),
        Some("scenes/main.scene.ron")
    );
    let no_main = paths(&["levels/b.scene.ron", "scenes/z.scene.ron", "scenes/c.scene.ron"]);
    assert_eq!(choose_scene(&no_main, None).as_deref(), Some("scenes/c.scene.ron"));
    let elsewhere = paths(&["levels/b.scene.ron", "a.scene.ron"]);
    assert_eq!(choose_scene(&elsewhere, None).as_deref(), Some("a.scene.ron"));
    assert_eq!(choose_scene(&[], None), None);
}

#[test]
fn a_packed_project_plays_like_the_loose_one() {
    let (project, root) = project_with_a_level("roundtrip");
    let (blob, start) = export::pack(&project.assets()).expect("packs");
    assert_eq!(start, "scenes/main.scene.ron");
    let packed = AssetDb::packed(leak(blob)).expect("pack reads");
    let loose = AssetDb::loose(project.assets()).expect("loose reads");
    let mut scenes = scene_paths(&packed);
    scenes.sort();
    assert_eq!(scenes, ["scenes/another.scene.ron", "scenes/main.scene.ron"]);
    assert_eq!(packed.assets(), loose.assets(), "same assets, GUIDs and paths");

    let registry = player::registry(&Scripts::none());
    let from_pack = load_scene(&packed, &registry, &start).expect("loads from the pack");
    let from_disk = load_scene(&loose, &registry, &start).expect("loads from disk");
    assert_eq!(from_pack, from_disk);
    assert_eq!(from_pack.len(), 3, "camera, platform and box");

    // The sprites it draws are in the pack.
    for (_, sr) in from_pack.iter::<SpriteRenderer>() {
        let sheet = packed.load::<SpriteSheet>(sr.sheet).expect("sheet");
        assert_eq!(sr.sheet.guid, starter::SHAPES_SHEET);
        packed.load(sheet.texture).expect("texture");
    }

    // Same simulation: the box lands on the platform either way.
    let a = play(from_pack, 360);
    let b = play(from_disk, 360);
    assert_eq!(a, b);
    let (boxed, _) = a.scene.iter::<crate::physics::Rigidbody>().next().expect("the box");
    assert_eq!(a.scene.world_position(boxed), FxVec2::from_px(10, 80));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn packing_needs_a_scene() {
    let dir = temp_dir("noscene");
    std::fs::create_dir_all(&dir).unwrap();
    starter::install(&dir).unwrap();
    AssetDb::loose(&dir).unwrap();
    assert_eq!(export::pack(&dir).unwrap_err(), "the project has no scenes");
    let _ = std::fs::remove_dir_all(dir);
}

fn assemble(layout: Layout, name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let (project, root) = project_with_a_level(name);
    let exe = root.join("fake-game");
    std::fs::write(&exe, b"not really a game").unwrap();
    let out_dir = root.join("Builds");
    let export = Export {
        title: "Space: Rocks!".into(),
        crate_name: "space_rocks".into(),
        exe,
        assets: project.assets(),
        out_dir: out_dir.clone(),
        layout,
    };
    let made = export::assemble(&export).expect("assembles");
    // Again, over the first: replaces it.
    assert_eq!(export::assemble(&export).expect("assembles again"), made);
    (root, out_dir, made)
}

fn check_data(dir: &Path) {
    let info = GameInfo::read(dir).expect("game.ron");
    assert_eq!(
        info,
        GameInfo { title: "Space: Rocks!".into(), scene: "scenes/main.scene.ron".into() }
    );
    let blob = std::fs::read(dir.join(PACK_FILE)).unwrap();
    let db = AssetDb::packed(leak(blob)).expect("the exported pack reads");
    let registry = player::registry(&Scripts::none());
    load_scene(&db, &registry, &info.scene).expect("the start scene loads");
}

#[test]
fn a_folder_export_has_the_game_and_its_data() {
    let (root, out_dir, made) = assemble(Layout::Folder, "folder");
    assert_eq!(made, out_dir.join("Space Rocks!"));
    let exe = made.join(format!("space_rocks{}", std::env::consts::EXE_SUFFIX));
    assert_eq!(std::fs::read(&exe).unwrap(), b"not really a game");
    assert_eq!(data_dir(&exe).as_deref(), Some(made.as_path()));
    assert!(made.join(GAME_FILE).is_file());
    check_data(&made);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_mac_export_is_an_app_bundle() {
    let (root, out_dir, made) = assemble(Layout::MacApp, "macapp");
    assert_eq!(made, out_dir.join("Space Rocks!.app"));
    let exe =
        made.join("Contents/MacOS").join(format!("space_rocks{}", std::env::consts::EXE_SUFFIX));
    assert!(exe.is_file());
    let resources = made.join("Contents/Resources");
    assert_eq!(
        data_dir(&exe).map(|d| d.canonicalize().unwrap()),
        Some(resources.canonicalize().unwrap())
    );
    let plist = std::fs::read_to_string(made.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("<key>CFBundleExecutable</key><string>space_rocks"), "{plist}");
    assert!(plist.contains("<string>Space: Rocks!</string>"), "{plist}");
    check_data(&resources);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn titles_make_safe_file_names() {
    assert_eq!(file_title("My Game"), "My Game");
    assert_eq!(file_title("  A/B: C?  "), "A B C");
    assert_eq!(file_title("..."), "Game");
    assert_eq!(file_title(""), "Game");
}
