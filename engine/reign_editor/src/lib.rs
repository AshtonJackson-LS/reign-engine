//! The Reign editor shell.
//!
//! [`run_editor`] opens a window with docked panels around a running
//! [`reign_app::App`]:
//!
//! - **Scene / Game**: the app's frame, rendered offscreen and shown as a texture at
//!   a whole-number scale where it fits. While playing, keyboard, mouse and
//!   pads reach the app while the view is hovered or focused (clicked).
//!   While not playing, clicking selects the topmost object under the
//!   cursor and dragging moves it (snapped to pixels; Ctrl snaps to the
//!   16-pixel grid). Apps implementing `draw_scene` get an independent edit
//!   camera: middle-drag or Space-drag pans, scroll zooms at the cursor,
//!   and F frames the selection. Game input is only sent while playing.
//! - **Hierarchy**: the app's [`Scene`] as a tree: select, create,
//!   duplicate, delete, drag to reparent, toggle active. Right-click (or
//!   the toolbar's GameObject menu) creates an empty object or anything
//!   the app offers ([`EditorApp::create_items`]); Ctrl+Shift+N (or
//!   Ctrl+N) makes an empty object and Alt+Shift+N an empty child;
//!   Ctrl+I (or F2, or a double-click) renames the selection in place.
//! - **Inspector**: the selected object's name, transform and components,
//!   edited through reflection, with Add Component, Reset and Remove.
//! - **Console**: everything logged through `log`, with level filters.
//! - **Debug**: the app's own toggles, from [`EditorApp::debug_ui`].
//! - **Project**: the app's asset folder ([`EditorApp::assets`]): a folder
//!   tree and a thumbnail grid with search and a type filter; create,
//!   rename, move and delete assets and folders with their `.meta` files.
//!   Drag an asset onto a handle field in the Inspector, or a sprite into
//!   the Game view to place it ([`EditorApp::create_from_asset`]). The
//!   Inspector shows a selected asset's import settings (Apply, Revert).
//!   Reloaded assets flash.
//! - **Tilemap**: the Tilemap painter (brush, eraser, rectangle, flood
//!   fill, eyedropper; collision, marker and visual layers).
//! - **Sprites**: the Sprite slicer (grid or by hand; names, pivots,
//!   density), saving a sprite sheet.
//! - **Animation**: the Animation editor (frame timeline, durations, loop
//!   mode, events, and a 120 Hz preview with play, pause, step and scrub).
//! - A toolbar with Play, Pause, Step, Stop, the time scale, undo and redo,
//!   and the Scene menu (Save, Open).
//!
//! Every scene edit can be undone (Ctrl+Z) and redone (Ctrl+Y or
//! Ctrl+Shift+Z); in an asset tool, the same keys undo the tool's own
//! edits while the pointer is over it; Ctrl+S saves the scene to its file, and an unsaved scene
//! is marked with `*` in the title.
//!
//! The editor starts in edit mode with the app loaded but not ticking.
//! Play snapshots the app ([`EditorApp::snapshot`]) and runs it; Stop puts
//! the snapshot back ([`EditorApp::restore`]), so edits made while playing
//! are discarded, as in Unity. Ticks keep their fixed length: time scale
//! changes how many run per frame.
//!
//! The editor knows nothing about any particular game; a game opts in by
//! implementing [`EditorApp`] for its [`reign_app::App`].

mod anim_editor;
mod assets_ui;
mod console;
mod create;
mod gizmo;
mod hierarchy;
mod history;
mod hitboxes;
mod inspector;
mod play;
mod prefab_ui;
mod scene_view;
mod shell;
mod slicer;
mod tilemap_painter;
mod undo;

use std::path::{Path, PathBuf};

pub use egui;
pub use reign_app::AppConfig;

pub use anim_editor::frame_rect;
pub use assets_ui::{AssetDrag, referrers};
pub use console::{CONSOLE_LINES, ConsoleBuffer, LogLine, init_logger};
pub use create::CreateRequest;
pub use history::{HISTORY_LIMIT, History};
pub use play::{PlayControl, PlayMode, TIME_SCALES};
pub use scene_view::SceneView;
pub use slicer::{fit as grid_fit, slice as slice_grid};
pub use undo::{Edits, TOOL_HISTORY};

use reign_app::{App, Ctx};
use reign_assets::{AssetChange, AssetDb, Guid};
use reign_scene::{ComponentRegistry, ObjectId, Scene};

/// An app the editor can host.
///
/// The Hierarchy, Inspector and Game-view gizmos work on the app's
/// [`Scene`] through reflection, so an app only says where its scene and
/// component registry are. Edits go straight into the scene; the app hears
/// about them through [`EditorApp::scene_edited`].
pub trait EditorApp: App {
    /// Everything Stop must put back: at least the simulation state,
    /// including the scene.
    type Snapshot: Clone;

    /// Taken when Play starts a run.
    fn snapshot(&self) -> Self::Snapshot;

    /// Puts a snapshot back when Stop ends the run.
    fn restore(&mut self, snapshot: Self::Snapshot);

    /// Applied at startup and around each editor frame, including while
    /// stopped. Keep background services such as audio in sync with Play.
    fn set_play_mode(&mut self, _mode: PlayMode) {}

    /// The scene the Hierarchy and Inspector show, if the app has one.
    fn scene(&self) -> Option<&Scene> {
        None
    }

    fn scene_mut(&mut self) -> Option<&mut Scene> {
        None
    }

    /// The component types Add Component offers.
    fn registry(&self) -> Option<&ComponentRegistry> {
        None
    }

    /// Called every frame before the editor reads the scene: copy state
    /// that still lives outside the scene into it, so it shows up.
    fn sync_scene(&mut self) {}

    /// Called after the editor changed the scene (an edit, a move, undo):
    /// rebuild whatever the app derives from it.
    fn scene_edited(&mut self) {}

    /// Where Save writes the current scene, if it has a file.
    fn scene_path(&self) -> Option<PathBuf> {
        None
    }

    /// Scene files the Open menu offers.
    fn scene_files(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Replaces the current scene with the one in `path`.
    fn open_scene(&mut self, _path: &Path) -> Result<(), String> {
        Err("this app can't open scenes".into())
    }

    /// The world pixel at the top-left of the last frame drawn (the
    /// camera), for clicking and gizmos in the Game view.
    fn view_origin(&self) -> Option<(f32, f32)> {
        None
    }

    /// Draws and presents a stopped scene through the editor's independent
    /// camera. Return false to use the normal game frame instead.
    fn draw_scene(&mut self, _renderer: &mut reign_render::Renderer, _view: SceneView) -> bool {
        false
    }

    /// The app's own toolbar menus and status, drawn after the GameObject
    /// menu every frame. It may also open windows through `ui.ctx()`.
    fn toolbar_ui(&mut self, _ui: &mut egui::Ui) -> EditorRequests {
        EditorRequests::default()
    }

    /// The app's own debug toggles and actions, shown in the Debug panel.
    fn debug_ui(&mut self, _ctx: &mut Ctx, _ui: &mut egui::Ui) {}

    /// The app's asset database, for the Project panel, the asset tools
    /// and hot reload. It lives as long as the program (a game keeps it in
    /// a static), so panels can hold it beside the app's scene.
    fn assets(&self) -> Option<&'static AssetDb> {
        None
    }

    /// Assets that changed on disk, or that an editor tool saved, were
    /// reloaded (at a frame boundary, before any ticks): refresh whatever
    /// was built from them.
    fn assets_changed(&mut self, _ctx: &mut Ctx, _changes: &[AssetChange]) {}

    /// What the Create menus (GameObject menu, Hierarchy right-click) offer
    /// besides an empty object, as menu paths such as
    /// `"2D Object/Sprites/Square"`.
    fn create_items(&self) -> Vec<String> {
        Vec::new()
    }

    /// Makes the object a [`EditorApp::create_items`] path names, under
    /// `parent` (or at the top level), with its world position at pixel
    /// `at` (the middle of the view).
    fn create(
        &mut self,
        _item: &str,
        _parent: Option<ObjectId>,
        _at: (i32, i32),
    ) -> Option<ObjectId> {
        None
    }

    /// An asset was dropped into the Game view at world pixel `at`: make
    /// an object for it in the scene, if this kind of asset makes one.
    /// `sprite` names the sprite when one sprite of a sheet was dragged.
    fn create_from_asset(
        &mut self,
        _asset: Guid,
        _sprite: Option<&str>,
        _at: (i32, i32),
    ) -> Option<ObjectId> {
        None
    }
}

/// What an app asked the editor to do from [`EditorApp::toolbar_ui`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorRequests {
    /// Save the open scene, as Ctrl+S does.
    pub save_scene: bool,
    /// Close the editor at the end of this frame.
    pub quit: bool,
}

/// How the editor starts.
#[derive(Clone, Debug)]
pub struct EditorConfig {
    /// The window. Its `exit_after` ends the editor too, for smoke tests.
    pub app: AppConfig,
    /// Start a run (as if Play were pressed) instead of edit mode.
    pub start_playing: bool,
}

impl Default for EditorConfig {
    fn default() -> Self {
        EditorConfig {
            app: AppConfig {
                title: "Reign Editor".into(),
                size: (1600, 900),
                min_size: (960, 540),
                ..AppConfig::default()
            },
            start_playing: false,
        }
    }
}

/// Opens the editor around the app `make` builds, once the window and
/// renderer exist. Call [`init_logger`] first to fill the Console.
pub fn run_editor<A, F>(config: EditorConfig, make: F)
where
    A: EditorApp,
    F: FnOnce(&mut Ctx) -> A,
{
    shell::run(config, make);
}
