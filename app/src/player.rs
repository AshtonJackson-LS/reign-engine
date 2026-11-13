//! The game on its own: a window that plays a project's start scene with
//! its Scripts, the same simulation and drawing as the editor's Play mode,
//! with no editor around it.
//!
//! An exported game reads its assets from [`PACK_FILE`] beside the
//! executable (in `Contents/Resources` inside a macOS app), and its title
//! and start scene from [`GAME_FILE`] there. With no pack it falls back to
//! a project's loose `assets/` folder, so `cargo run --bin <crate>-game`
//! plays the project it's built from.

use std::path::{Path, PathBuf};
use std::time::Duration;

use reign::app::winit::event::{ElementState, WindowEvent};
use reign::app::winit::keyboard::{KeyCode, PhysicalKey};
use reign::app::{App, AppConfig, Ctx, TickCtx};
use reign::assets::{AssetDb, AssetKind, Prefabs, SceneAsset, prefab};
use reign::behaviour::{Behaviours, Schedule};
use reign::render::Renderer;
use reign::scene::{ComponentRegistry, Scene};
use serde::{Deserialize, Serialize};

use crate::DB;
use crate::components::register_builtins;
use crate::editor::{PROJECT_FILE, Project};
use crate::runtime::{Keyboard, Scripts, World, schedule};
use crate::view::Sprites;

/// The packed assets an exported game ships with.
pub const PACK_FILE: &str = "assets.rpak";

/// The exported game's title and start scene, beside [`PACK_FILE`].
pub const GAME_FILE: &str = "game.ron";

/// The scene a game starts on when nothing names another.
pub const MAIN_SCENE: &str = "scenes/main.scene.ron";

/// What [`GAME_FILE`] holds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GameInfo {
    /// The window's title.
    pub title: String,
    /// The start scene's asset path, such as `scenes/main.scene.ron`. Empty
    /// picks one with [`choose_scene`].
    pub scene: String,
}

impl GameInfo {
    pub fn read(dir: &Path) -> Option<GameInfo> {
        let text = std::fs::read_to_string(dir.join(GAME_FILE)).ok()?;
        ron::from_str(&text).ok()
    }

    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()).unwrap_or_default()
    }
}

/// Every component the game knows: the built-in ones and the project's.
pub(crate) fn registry(scripts: &Scripts) -> ComponentRegistry {
    let mut registry = ComponentRegistry::new();
    register_builtins(&mut registry);
    (scripts.register)(&mut registry);
    registry
}

/// The project's Behaviours.
pub(crate) fn behaviours(scripts: &Scripts) -> Behaviours<World> {
    let mut behaviours = Behaviours::new();
    (scripts.behaviours)(&mut behaviours);
    behaviours
}

/// Which scene to start on, from the scene assets' paths: `wanted` if it's
/// one of them, else [`MAIN_SCENE`], else the first under `scenes/`, else
/// the first anywhere. Paths are compared sorted, so the choice never
/// depends on the order they're listed in.
pub fn choose_scene(scenes: &[String], wanted: Option<&str>) -> Option<String> {
    let mut sorted: Vec<&String> = scenes.iter().collect();
    sorted.sort();
    if let Some(w) = wanted.filter(|w| !w.is_empty())
        && let Some(found) = sorted.iter().find(|s| s.as_str() == w)
    {
        return Some((*found).clone());
    }
    if let Some(main) = sorted.iter().find(|s| s.as_str() == MAIN_SCENE) {
        return Some((*main).clone());
    }
    sorted.iter().find(|s| s.starts_with("scenes/")).or(sorted.first()).map(|s| (*s).clone())
}

/// The asset paths of every scene in `db`.
pub fn scene_paths(db: &AssetDb) -> Vec<String> {
    db.assets().into_iter().filter(|a| a.kind == AssetKind::Scene).map(|a| a.path).collect()
}

/// Loads a scene asset and stamps its prefab instances, as the editor
/// shows it.
pub fn load_scene(db: &AssetDb, registry: &ComponentRegistry, path: &str) -> Result<Scene, String> {
    let asset = db.load_path::<SceneAsset>(path).map_err(|e| e.to_string())?;
    // The prefab registry also knows PrefabInstance, which saved scenes
    // with prefabs in them hold.
    let prefabs = Prefabs::new(registry.clone());
    let mut scene = Scene::from_ron(&asset.text, prefabs.registry()).map_err(|e| e.to_string())?;
    for e in prefab::expand_all(&mut scene, &prefabs.with(db)) {
        log::warn!("{path}: {e}");
    }
    Ok(scene)
}

/// The folder an exported game's data is in: beside the executable, or a
/// macOS app's `Contents/Resources`.
pub fn data_dir(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    [dir.to_path_buf(), dir.join("../Resources")].into_iter().find(|d| d.join(PACK_FILE).is_file())
}

/// The project folder an executable was built in (`<project>/target/...`),
/// for development runs without a pack.
fn project_around(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .skip(1)
        .find(|d| d.join(PROJECT_FILE).is_file() && d.join("assets").is_dir())
        .map(Path::to_path_buf)
}

/// The running game.
pub struct Player {
    world: World,
    behaviours: Behaviours<World>,
    schedule: Schedule<World>,
    keyboard: Keyboard,
    sprites: Sprites,
}

impl Player {
    pub fn new(scripts: &Scripts, scene: Scene) -> Self {
        Player {
            world: World::new(scene),
            behaviours: behaviours(scripts),
            schedule: schedule(),
            keyboard: Keyboard::default(),
            sprites: Sprites::default(),
        }
    }
}

impl App for Player {
    fn tick(&mut self, ctx: &mut TickCtx) {
        let input = self.keyboard.frame(Some(ctx.pads()));
        self.world.step(input, &self.behaviours, &self.schedule);
    }

    fn draw(&mut self, r: &mut Renderer, _alpha: f32) {
        self.sprites.draw_game(r, &self.world.scene, DB.get());
    }

    fn event(&mut self, ctx: &mut Ctx, event: &WindowEvent) {
        // F11 switches between fullscreen and a window.
        if let WindowEvent::KeyboardInput { event: key, .. } = event
            && key.state == ElementState::Pressed
            && !key.repeat
            && key.physical_key == PhysicalKey::Code(KeyCode::F11)
        {
            ctx.set_fullscreen(!ctx.is_fullscreen());
        }
        self.keyboard.event(event);
    }
}

const USAGE: &str =
    "usage: <game> [PROJECT_DIR] [--scene PATH] [--fullscreen] [--exit-after SECONDS]";

/// Where the assets come from.
enum Source {
    /// An exported game's pack, and the folder it's in.
    Pack(PathBuf),
    /// A project folder's loose assets.
    Project(PathBuf),
}

/// Runs the game: see [`crate::play`].
pub fn run(scripts: Scripts) {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut project_dir = None;
    let mut wanted_scene = None;
    let mut fullscreen = false;
    let mut exit_after = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fullscreen" => fullscreen = true,
            "--scene" => {
                let Some(path) = args.next() else {
                    eprintln!(
                        "--scene needs an asset path, such as scenes/main.scene.ron\n{USAGE}"
                    );
                    std::process::exit(2);
                };
                wanted_scene = Some(path);
            }
            "--exit-after" => {
                let secs = args.next().and_then(|s| s.parse::<f64>().ok());
                let Some(secs) = secs else {
                    eprintln!("--exit-after needs a number of seconds\n{USAGE}");
                    std::process::exit(2);
                };
                exit_after = Some(Duration::from_secs_f64(secs));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            // macOS passes this when an app is launched from Finder.
            s if s.starts_with("-psn_") => {}
            s if s.starts_with('-') => {
                eprintln!("unknown flag {s}\n{USAGE}");
                std::process::exit(2);
            }
            _ => project_dir = Some(PathBuf::from(arg)),
        }
    }

    let exe = std::env::current_exe().unwrap_or_default();
    let source = match project_dir {
        Some(dir) => Some(Source::Project(dir)),
        None => data_dir(&exe)
            .map(Source::Pack)
            .or_else(|| project_around(&exe).map(Source::Project))
            .or_else(|| {
                let cwd = std::env::current_dir().ok()?;
                cwd.join(PROJECT_FILE).is_file().then_some(Source::Project(cwd))
            }),
    };
    let Some(source) = source else {
        log::error!(
            "no {PACK_FILE} beside the game and no project around it: export the game from the editor"
        );
        std::process::exit(1);
    };

    let (db, info) = match &source {
        Source::Pack(dir) => {
            let path = dir.join(PACK_FILE);
            let blob = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    log::error!("can't read {}: {e}", path.display());
                    std::process::exit(1);
                }
            };
            // The database borrows the pack for the whole run.
            let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
            log::info!("assets from {} ({} bytes)", path.display(), blob.len());
            (AssetDb::packed(blob), GameInfo::read(dir).unwrap_or_default())
        }
        Source::Project(root) => {
            log::info!("assets from the project at {}", root.display());
            let project = Project { root: root.clone() };
            let info = GameInfo { title: project.title(), scene: String::new() };
            (AssetDb::loose(project.assets()), info)
        }
    };
    let db = match db {
        Ok(db) => DB.get_or_init(|| db),
        Err(e) => {
            log::error!("can't read the game's assets: {e}");
            std::process::exit(1);
        }
    };

    let registry = registry(&scripts);
    let wanted = wanted_scene.as_deref().or(Some(info.scene.as_str()));
    let Some(path) = choose_scene(&scene_paths(db), wanted) else {
        log::error!("the game has no scenes");
        std::process::exit(1);
    };
    if let Some(w) = wanted.filter(|w| !w.is_empty() && *w != path) {
        log::warn!("no scene {w}: starting on {path}");
    }
    let scene = match load_scene(db, &registry, &path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("{path}: {e}");
            std::process::exit(1);
        }
    };
    log::info!("playing {path}: {} objects", scene.len());

    let title = match (info.title.is_empty(), scripts.project) {
        (false, _) => info.title.clone(),
        (true, Some(name)) => name.to_owned(),
        (true, None) => "Reign".to_owned(),
    };
    let config = AppConfig { title, fullscreen, exit_after, ..AppConfig::default() };
    reign::app::run(config, move |_ctx| Player::new(&scripts, scene));
}
