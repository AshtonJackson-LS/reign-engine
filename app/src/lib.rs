//! Reign Engine's editor around a project folder.
//!
//! ```text
//! reign-engine [PROJECT_DIR] [--scene FILE] [--play] [--exit-after SECONDS]
//! reign-engine [PROJECT_DIR] --export OUT_DIR
//! ```
//!
//! A project is a folder with an `assets/` directory. Scenes live in
//! `assets/scenes/*.scene.ron`; the editor opens `assets/scenes/main.scene.ron`
//! (making an empty one if needed) unless `--scene` names another. Without
//! `PROJECT_DIR` it uses `~/ReignProjects/Default`. `--export` builds the
//! game into `OUT_DIR` as Scripts → Export Game does, with no window.
//!
//! A project with code is a Rust crate whose binary is its own editor: it
//! calls [`run`] with the [`Scripts`] that `reign_build` generates from
//! `src/components/`. Code in a component file starts with
//! `use reign_engine::prelude::*;`. Its second binary calls [`play`]: the
//! game without the editor, which Scripts → Export Game builds and ships.

// Lets the engine's own tests compile the component templates, which say
// `use reign_engine::prelude::*;` as a project's code does.
extern crate self as reign_engine;

mod code;
mod components;
mod editor;
mod export;
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

pub use components::{Camera, Image, SpriteRenderer, TilemapRenderer};
pub use player::{GAME_FILE, GameInfo, PACK_FILE};
pub use runtime::{Ctx, CtxExt, Message, Scripts, TICKS_PER_SECOND, World};

use editor::{Project, ReignEngine};

/// The project's assets, loaded once at startup.
pub(crate) static DB: OnceLock<AssetDb> = OnceLock::new();

/// Everything a component file needs: `use reign_engine::prelude::*;`.
pub mod prelude {
    pub use reign::assets::{Handle, SpriteSheet, Texture, Tilemap};
    pub use reign::behaviour::{Behaviour, Envelope, To};
    pub use reign::core::{Button, Fx, FxVec2};
    pub use reign::scene::{Component, ObjectId, Reflect, Rgba8, Scene, Transform};
    pub use serde::{Deserialize, Serialize};

    pub use crate::physics::{BodyType, BoxCollider, Rigidbody, TilemapCollider};
    pub use crate::{
        Camera, Ctx, CtxExt, Image, Message, SpriteRenderer, TICKS_PER_SECOND, TilemapRenderer,
        World,
    };
}

const USAGE: &str = "usage: reign-engine [PROJECT_DIR] [--scene FILE] [--play] [--exit-after SECONDS] | [PROJECT_DIR] --export OUT_DIR";

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
mod tests;
