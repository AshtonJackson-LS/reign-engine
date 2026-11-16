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
use crate::settings::{Settings, Window};
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
    let (blob, start) = export::pack(&project.assets(), "").expect("packs");
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
    assert_eq!(export::pack(&dir, "").unwrap_err(), "the project has no scenes");
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
        icon: crate::icon::own_icon(&root),
        settings: Settings::default(),
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
        GameInfo {
            title: "Space: Rocks!".into(),
            scene: "scenes/main.scene.ron".into(),
            window: Window::default(),
            background: crate::settings::DEFAULT_BACKGROUND,
        }
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

/// The game's window icon travels with it: the default for a project with
/// none, the project's own `icon.png` (at the size an export keeps) for one
/// with its own.
#[test]
fn an_export_carries_the_games_icon() {
    use crate::icon;
    // No icon of the project's own: the Reign default.
    let (root, _, made) = assemble(Layout::Folder, "icon-default");
    let kept = icon::check(&std::fs::read(made.join(icon::FILE)).unwrap()).expect("a valid icon");
    assert_eq!((kept.width, kept.height), (export::EXPORTED_ICON_SIZE, export::EXPORTED_ICON_SIZE));
    assert_eq!(kept, icon::resize(icon::default_icon(), export::EXPORTED_ICON_SIZE));
    // The game finds it where it looks.
    assert_eq!(icon::project_icon(&made), kept.clone());
    let _ = std::fs::remove_dir_all(root);

    // A project's own icon, 512 pixels of one flat colour.
    let (project, root) = project_with_a_level("icon-own");
    let mut rgba = Vec::new();
    for _ in 0..512 * 512 {
        rgba.extend_from_slice(&[12, 34, 56, 255]);
    }
    let own = reign::assets::RgbaImage { rgba, width: 512, height: 512 };
    std::fs::write(root.join(icon::FILE), icon::encode_png(&own)).unwrap();
    let exe = root.join("fake-game");
    std::fs::write(&exe, b"not really a game").unwrap();
    let export = Export {
        title: "Own".into(),
        crate_name: "own".into(),
        exe,
        assets: project.assets(),
        out_dir: root.join("Builds"),
        layout: Layout::Folder,
        icon: icon::own_icon(&root),
        settings: Settings::default(),
    };
    let made = export::assemble(&export).unwrap();
    let kept = icon::check(&std::fs::read(made.join(icon::FILE)).unwrap()).unwrap();
    assert_eq!((kept.width, &kept.rgba[..4]), (256, &[12, 34, 56, 255][..]));
    let _ = std::fs::remove_dir_all(root);
}

/// What goes in the game's file when the project has its own icon, has none
/// but the program already has one, or has none and neither does the
/// program. (Windows only: elsewhere a program holds no icon.)
#[cfg(windows)]
#[test]
fn the_default_icon_only_fills_a_gap() {
    use crate::exe_icon::{self, probe};
    use crate::icon;
    let (project, root) = project_with_a_level("icon-gap");
    // Real programs made from this test's own (whether or not it was built
    // with an icon): one with no resources at all, and one with an icon
    // under an id of another tool's choosing.
    let bare = root.join("bare.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &bare).unwrap();
    probe::strip(&bare);
    let real = root.join("real.exe");
    std::fs::copy(&bare, &real).unwrap();
    let theirs = exe_icon::windows_entries(&reign::assets::RgbaImage {
        rgba: [9, 99, 199, 255].repeat(64 * 64),
        width: 64,
        height: 64,
    });
    probe::put_icon_as(&real, &theirs, 77, 500);
    let original = std::fs::read(&real).unwrap();
    let export = |exe: &Path, own: Option<reign::assets::RgbaImage>, name: &str| {
        let made = export::assemble(&Export {
            title: name.into(),
            crate_name: name.into(),
            exe: exe.to_owned(),
            assets: project.assets(),
            out_dir: root.join("Builds"),
            layout: Layout::Folder,
            icon: own,
            settings: Settings::default(),
        })
        .unwrap();
        made.join(format!("{name}.exe"))
    };

    // No icon of the project's own, and the program has one: its icon is
    // left as it was (only the version details go in).
    let kept = export(&real, None, "keeps");
    assert!(exe_icon::has_exe_icon(&kept));
    assert_eq!(
        probe::resource_size(&kept, probe::GROUP, 77),
        probe::resource_size(&real, probe::GROUP, 77),
        "the program's own icon was not touched"
    );
    assert_eq!(probe::resource_size(&kept, probe::GROUP, 1), None, "no default icon added");
    assert!(probe::resource_size(&kept, probe::VERSION, 1).is_some(), "the version went in");

    // The project's own icon replaces it.
    let mut rgba = Vec::new();
    for _ in 0..64 * 64 {
        rgba.extend_from_slice(&[1, 2, 3, 255]);
    }
    let own = reign::assets::RgbaImage { rgba, width: 64, height: 64 };
    let replaced = export(&real, Some(own), "replaced");
    assert_ne!(std::fs::read(&replaced).unwrap(), original, "the project's icon went in");
    assert!(probe::resource_size(&replaced, probe::GROUP, 1).is_some());

    // No icon anywhere: the default goes in.
    assert!(!exe_icon::has_exe_icon(&bare));
    let filled = export(&bare, None, "filled");
    assert!(exe_icon::has_exe_icon(&filled));
    let default = exe_icon::windows_entries(icon::default_icon());
    assert_eq!(
        probe::resource_size(&filled, probe::GROUP, 1),
        Some(exe_icon::group_icon(&default).len() as u32)
    );
    assert_eq!(
        probe::resource_size(&filled, probe::ICON, 7),
        Some(default[6].png.len() as u32),
        "the 256 px default"
    );

    // A file that isn't a program can't take an icon: that is a warning, not
    // a failure, and the game's data and window icon are still written.
    let plain = root.join("plain.exe");
    std::fs::write(&plain, b"not really a game").unwrap();
    assert!(!exe_icon::has_exe_icon(&plain));
    let made = export(&plain, None, "plain");
    assert_eq!(std::fs::read(&made).unwrap(), b"not really a game");
    assert!(made.with_file_name(icon::FILE).is_file());
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
    // The app's icon: the file, and the plist naming it (without .icns).
    assert!(plist.contains("<key>CFBundleIconFile</key><string>AppIcon</string>"), "{plist}");
    let icns = std::fs::read(resources.join("AppIcon.icns")).unwrap();
    assert_eq!(&icns[..4], b"icns");
    assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
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

/// Project Settings go with the game: its start scene, its window, and (on
/// macOS) its version and author in the app.
#[test]
fn an_export_carries_the_project_settings() {
    let (project, root) = project_with_a_level("settings");
    let exe = root.join("fake-game");
    std::fs::write(&exe, b"not really a game").unwrap();
    let settings = Settings {
        start_scene: "scenes/another.scene.ron".into(),
        window: Window { width: 1920, height: 1080, fullscreen: true },
        version: "2.4.1".into(),
        author: "Small & Co".into(),
        background: [10, 120, 200],
        ..Settings::default()
    };
    let mut export = Export {
        title: "Set".into(),
        crate_name: "set".into(),
        exe,
        assets: project.assets(),
        out_dir: root.join("Builds"),
        layout: Layout::MacApp,
        icon: None,
        settings: settings.clone(),
    };
    let made = export::assemble(&export).expect("assembles");
    let resources = made.join("Contents/Resources");
    let info = GameInfo::read(&resources).unwrap();
    assert_eq!(info.scene, "scenes/another.scene.ron");
    assert_eq!(info.window, settings.window);
    assert_eq!(info.background, settings.background);
    let plist = std::fs::read_to_string(made.join("Contents/Info.plist")).unwrap();
    assert!(plist.contains("<key>CFBundleShortVersionString</key><string>2.4.1</string>"));
    assert!(plist.contains("<key>CFBundleVersion</key><string>2.4.1</string>"));
    assert!(plist.contains("<string>© Small &amp; Co</string>"), "{plist}");

    // No author: no copyright line.
    export.settings.author.clear();
    export::assemble(&export).unwrap();
    let plist = std::fs::read_to_string(made.join("Contents/Info.plist")).unwrap();
    assert!(!plist.contains("NSHumanReadableCopyright"), "{plist}");

    // A start scene that isn't there stops the export rather than starting
    // the game somewhere else.
    export.settings.start_scene = "scenes/gone.scene.ron".into();
    let error = export::assemble(&export).unwrap_err();
    assert!(
        error.contains("scenes/gone.scene.ron") && error.contains("Project Settings"),
        "{error}"
    );
    // Settings that can't be used, too.
    export.settings = Settings { version: "soon".into(), ..settings };
    assert!(export::assemble(&export).unwrap_err().contains("version"));
    let _ = std::fs::remove_dir_all(root);
}

/// The settings file is read where the game and the tools read it.
#[test]
fn the_start_scene_setting_is_used_by_the_editor_and_simulate() {
    let (project, root) = project_with_a_level("start-scene");
    assert_eq!(project.start_scene(), project.scenes().join("main.scene.ron"));
    Settings { start_scene: "scenes/another.scene.ron".into(), ..Settings::default() }
        .save(&root)
        .unwrap();
    assert_eq!(project.start_scene(), project.scenes().join("another.scene.ron"));
    // One that isn't there: the editor still opens on main.
    Settings { start_scene: "scenes/gone.scene.ron".into(), ..Settings::default() }
        .save(&root)
        .unwrap();
    assert_eq!(project.start_scene(), project.scenes().join("main.scene.ron"));
    // And --check says so.
    let db = AssetDb::loose(project.assets()).unwrap();
    let mut report = crate::headless::Report::default();
    crate::headless::check_settings(Settings::load(&root), &db, &mut report);
    assert_eq!(report.errors.len(), 1, "{report:?}");
    assert!(report.errors[0].message.contains("gone.scene.ron"));
    let _ = std::fs::remove_dir_all(root);
}

/// Renaming keeps the engine version the hub wrote, and the folder.
#[test]
fn renaming_the_game_keeps_the_rest_of_the_project_file() {
    let root = temp_dir("rename");
    let project = Project::open(root.clone()).unwrap();
    let file = root.join(crate::editor::PROJECT_FILE);
    let before = std::fs::read_to_string(&file).unwrap();
    assert!(before.contains(env!("CARGO_PKG_VERSION")));
    project.set_title("  Space Rocks 2  ").unwrap();
    assert_eq!(project.title(), "Space Rocks 2");
    assert!(std::fs::read_to_string(&file).unwrap().contains(env!("CARGO_PKG_VERSION")));
    assert!(project.set_title("   ").unwrap_err().contains("empty"));
    assert!(project.set_title("a\nb").is_err());
    assert_eq!(project.title(), "Space Rocks 2", "a refused name changes nothing");
    let _ = std::fs::remove_dir_all(root);
}
