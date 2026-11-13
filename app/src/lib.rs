//! Reign Engine's editor around a project folder.
//!
//! ```text
//! reign-engine [PROJECT_DIR] [--scene FILE] [--play] [--exit-after SECONDS]
//! reign-engine [PROJECT_DIR] --export OUT_DIR
//! reign-engine [PROJECT_DIR] --check | --describe | --simulate TICKS
//! ```
//!
//! A project is a folder with an `assets/` directory. Scenes live in
//! `assets/scenes/*.scene.ron`; the editor opens `assets/scenes/main.scene.ron`
//! (making an empty one if needed) unless `--scene` names another. Without
//! `PROJECT_DIR` it uses `~/ReignProjects/Default`. `--export` builds the
//! game into `OUT_DIR` as Scripts → Export Game does, with no window.
//! `--check`, `--describe` and `--simulate` also open no window and print
//! JSON: see [`headless`].
//!
//! A project with code is a Rust crate whose binary is its own editor: it
//! calls [`run`] with the [`Scripts`] that `reign_build` generates from
//! `src/components/`. Code in a component file starts with
//! `use reign_engine::prelude::*;`. Its second binary calls [`play`]: the
//! game without the editor, which Scripts → Export Game builds and ships.

// Lets the engine's own tests compile the component templates, which say
// `use reign_engine::prelude::*;` as a project's code does.
extern crate self as reign_engine;

mod audio;
mod code;
mod components;
mod editor;
mod export;
mod font;
pub mod headless;
mod input;
pub mod physics;
mod player;
mod runtime;
mod starter;
mod tilemap_assets;
mod view;

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use reign::app::AppConfig;
use reign::assets::AssetDb;
use reign::editor::{EditorConfig, init_logger, run_editor};

pub use components::{
    Animator, AudioSource, Camera, Image, NamedClip, SpriteRenderer, Text, TextAlign,
    TilemapRenderer,
};
pub use player::{GAME_FILE, GameInfo, PACK_FILE};
pub use runtime::{Ctx, CtxExt, Event, Message, Scripts, TICKS_PER_SECOND, World};

use editor::{Project, ReignEngine};

/// The project's assets, loaded once at startup.
pub(crate) static DB: OnceLock<AssetDb> = OnceLock::new();

/// Everything a component file needs: `use reign_engine::prelude::*;`.
pub mod prelude {
    pub use reign::assets::{
        AnimationClip, AudioClip, Font, Handle, SpriteSheet, Texture, Tilemap,
    };
    pub use reign::behaviour::{Behaviour, Envelope, To};
    pub use reign::core::{Button, Fx, FxVec2};
    pub use reign::scene::{Component, ObjectId, Reflect, Rgba8, Scene, Transform};
    pub use serde::{Deserialize, Serialize};

    pub use crate::physics::{BodyType, BoxCollider, Rigidbody, TilemapCollider};
    pub use crate::{
        Animator, AudioSource, Camera, Ctx, CtxExt, Image, Message, NamedClip, SpriteRenderer,
        TICKS_PER_SECOND, Text, TextAlign, TilemapRenderer, World,
    };
}

const USAGE: &str = "usage: reign-engine [PROJECT_DIR] [--scene FILE] [--play] [--exit-after SECONDS]
       reign-engine [PROJECT_DIR] --export OUT_DIR
       reign-engine [PROJECT_DIR] --check
       reign-engine [PROJECT_DIR] --describe
       reign-engine [PROJECT_DIR] --simulate TICKS [--scene ASSET_PATH] [--input SPEC] [--every TICKS] [--watch NAMES]
  --check     load every scene and prefab; print problems as JSON (exit 1 on errors)
  --describe  print components, fields, input actions, scenes and assets as JSON
  --simulate  play a scene for TICKS ticks (120 a second) with no window; print the objects as JSON
  --input     held actions: Name, Name@TICK or Name@FROM-TO, comma separated (Right,Jump@30-45)
  --every     also print the objects every that many ticks
  --watch     only these objects, by name, comma separated";

fn default_project() -> PathBuf {
    reign_build::ide::home().join("ReignProjects").join("Default")
}

/// Runs the editor with a project's code (or [`Scripts::none`]), reading
/// the project and options from the command line.
pub fn run(scripts: Scripts) {
    init_logger();

    let mut project_dir = None;
    let mut scene = None;
    let mut start_playing = false;
    let mut exit_after = None;
    let mut export_to = None;
    let mut headless = None;
    let (mut sim_input, mut sim_every, mut sim_watch) = (None, None, Vec::new());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--play" => start_playing = true,
            "--scene" => {
                let Some(path) = args.next() else {
                    eprintln!("--scene needs a file\n{USAGE}");
                    std::process::exit(2);
                };
                scene = Some(PathBuf::from(path));
            }
            "--export" => {
                let Some(path) = args.next() else {
                    eprintln!("--export needs an output folder\n{USAGE}");
                    std::process::exit(2);
                };
                export_to = Some(PathBuf::from(path));
            }
            "--exit-after" => {
                let secs = args.next().and_then(|s| s.parse::<f64>().ok());
                let Some(secs) = secs else {
                    eprintln!("--exit-after needs a number of seconds\n{USAGE}");
                    std::process::exit(2);
                };
                exit_after = Some(Duration::from_secs_f64(secs));
            }
            "--check" => headless = Some(Headless::Check),
            "--describe" => headless = Some(Headless::Describe),
            "--simulate" => {
                let Some(ticks) = args.next().and_then(|s| s.parse::<u64>().ok()) else {
                    eprintln!("--simulate needs a number of ticks (120 a second)\n{USAGE}");
                    std::process::exit(2);
                };
                headless = Some(Headless::Simulate(ticks));
            }
            "--input" => {
                let Some(spec) = args.next() else {
                    eprintln!("--input needs actions, such as Right,Jump@30-45\n{USAGE}");
                    std::process::exit(2);
                };
                sim_input = Some(spec);
            }
            "--every" => {
                let Some(ticks) = args.next().and_then(|s| s.parse::<u64>().ok()) else {
                    eprintln!("--every needs a number of ticks\n{USAGE}");
                    std::process::exit(2);
                };
                sim_every = Some(ticks);
            }
            "--watch" => {
                let Some(names) = args.next() else {
                    eprintln!("--watch needs object names, comma separated\n{USAGE}");
                    std::process::exit(2);
                };
                sim_watch = names.split(',').map(|n| n.trim().to_owned()).collect();
            }
            "-h" | "--help" => {
                println!("Reign Engine {}\n{USAGE}", env!("CARGO_PKG_VERSION"));
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

    let project = match Project::open(project_dir.unwrap_or_else(default_project)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("can't open the project: {e}");
            std::process::exit(1);
        }
    };
    let code = match scripts.project {
        Some(name) => format!(" with {name}'s code"),
        None => String::new(),
    };
    log::info!(
        "Reign Engine {}: project {}{code}",
        env!("CARGO_PKG_VERSION"),
        project.root.display()
    );

    if let Some(out_dir) = export_to {
        // Opening the assets writes any missing .meta, as the editor would.
        if let Err(e) = AssetDb::loose(project.assets()) {
            log::error!("can't read the project's assets: {e}");
            std::process::exit(1);
        }
        match export::export_project(&project.root, &project.title(), &out_dir) {
            Ok(path) => log::info!("exported the game: {}", path.display()),
            Err(e) => {
                log::error!("couldn't export the game: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    if let Some(command) = headless {
        let db = match AssetDb::loose(project.assets()) {
            Ok(db) => DB.get_or_init(|| db),
            Err(e) => {
                log::error!("can't read the project's assets: {e}");
                std::process::exit(1);
            }
        };
        let command = match command {
            Headless::Check => headless::Command::Check,
            Headless::Describe => headless::Command::Describe,
            Headless::Simulate(ticks) => headless::Command::Simulate {
                ticks,
                // An asset path (scenes/x.scene.ron), or a file under assets/.
                scene: scene.map(|s| asset_path(&project, &s)),
                input: sim_input,
                every: sim_every,
                watch: sim_watch,
            },
        };
        std::process::exit(headless::run(command, db, &scripts, &project.title()));
    }

    match AssetDb::loose(project.assets()) {
        Ok(db) => {
            let db = DB.get_or_init(|| db);
            if let Err(e) = db.watch() {
                log::warn!("hot reload is off: {e}");
            }
        }
        Err(e) => log::error!("can't read the project's assets: {e}"),
    }

    let mut config = EditorConfig::default();
    config.app = AppConfig {
        title: format!("{} - Reign Engine", project.title()),
        exit_after,
        ..config.app
    };
    config.start_playing = start_playing;
    run_editor(config, move |_ctx| ReignEngine::new(project, scripts, scene));
}

/// A headless command named on the command line.
enum Headless {
    Check,
    Describe,
    Simulate(u64),
}

/// `--scene` as an asset path: a file under the project's `assets/` loses
/// that prefix; anything else is taken as an asset path already.
fn asset_path(project: &Project, scene: &std::path::Path) -> String {
    let full = std::path::absolute(scene).unwrap_or_else(|_| scene.to_owned());
    let assets = std::path::absolute(project.assets()).unwrap_or_else(|_| project.assets());
    let relative = full.strip_prefix(&assets).unwrap_or(scene);
    relative.to_string_lossy().replace('\\', "/")
}

/// Plays the game with a project's code, no editor: the start scene, the
/// same simulation and drawing as the editor's Play mode.
///
/// ```text
/// <game> [PROJECT_DIR] [--scene PATH] [--fullscreen] [--exit-after SECONDS]
/// ```
///
/// Assets come from [`PACK_FILE`] beside the executable (or in a macOS
/// app's `Contents/Resources`), as Export Game lays them out. Without one
/// they come from a project's loose `assets/`: `PROJECT_DIR`, else the
/// project the executable was built in, else the current folder. The start
/// scene is `--scene`, else the one [`GAME_FILE`] names, else
/// `scenes/main.scene.ron`, else the first scene. F11 toggles fullscreen.
pub fn play(scripts: Scripts) {
    player::run(scripts);
}

#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod feature_tests;
#[cfg(test)]
mod tests;
