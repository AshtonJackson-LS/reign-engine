//! Making scenes and moving between them: the New Scene box (Scene → New
//! Scene, or the Project panel's Create → Scene) and the "Save changes?"
//! question asked before opening another scene would lose unsaved edits.

use std::path::{Path, PathBuf};

use egui::{Context, Key};

pub const SCENE_SUFFIX: &str = ".scene.ron";

/// Where the editor is going next once the current scene is dealt with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Open this scene file.
    Open(PathBuf),
    /// Make a new scene file here, then open it.
    Create(PathBuf),
}

/// The file name for a scene called `name`, with `.scene.ron` added if
/// missing.
pub fn scene_file_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let name = name.strip_suffix(SCENE_SUFFIX).unwrap_or(name).trim();
    if name.is_empty() {
        return Err("Give the scene a name".into());
    }
    if name.starts_with('.') || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        return Err("A scene name can't start with a dot or contain / \\ : * ? \" < > |".into());
    }
    Ok(format!("{name}{SCENE_SUFFIX}"))
}

/// The asset path of a new scene called `name` in the asset folder
/// `folder` (`""` is the top).
pub fn new_scene_path(folder: &str, name: &str) -> Result<String, String> {
    let file = scene_file_name(name)?;
    let folder = folder.trim_matches('/');
    Ok(if folder.is_empty() { file } else { format!("{folder}/{file}") })
}

/// The New Scene box, while it's open.
pub struct NewScene {
    /// The folder the scene goes in.
    pub dir: PathBuf,
    /// The folder as the box shows it, like `assets/scenes`.
    label: String,
    name: String,
    error: Option<String>,
    focus: bool,
}

impl NewScene {
    pub fn new(dir: PathBuf, label: impl Into<String>) -> Self {
        let label = label.into();
        NewScene { dir, label, name: "New Scene".into(), error: None, focus: true }
    }
}

/// How a folder reads in the New Scene box: from the asset folder's parent
/// (`assets/scenes`) when it's inside it, else in full.
pub fn folder_label(dir: &Path, assets: Option<&Path>) -> String {
    let inside = assets.and_then(|root| Some((root.file_name()?, dir.strip_prefix(root).ok()?)));
    match inside {
        Some((root, rest)) if rest.as_os_str().is_empty() => root.to_string_lossy().into_owned(),
        Some((root, rest)) => Path::new(root).join(rest).to_string_lossy().replace('\\', "/"),
        None => dir.display().to_string(),
    }
}

/// What a box decided this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer<T> {
    Waiting,
    Cancelled,
    Done(T),
}

/// Draws the New Scene box. Done gives the new scene's file, which doesn't
/// exist yet.
pub fn new_scene_window(ctx: &Context, state: &mut NewScene) -> Answer<PathBuf> {
    let mut answer = Answer::Waiting;
    egui::Modal::new(egui::Id::new("new scene")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.heading("New Scene");
        ui.weak(format!("In {}/", state.label));
        let field =
            ui.add(egui::TextEdit::singleline(&mut state.name).desired_width(f32::INFINITY));
        if std::mem::take(&mut state.focus) {
            field.request_focus();
        }
        if let Some(error) = &state.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        ui.horizontal(|ui| {
            if ui.button("Create").clicked() || enter {
                match scene_file_name(&state.name).map(|file| state.dir.join(file)) {
                    Ok(path) if path.exists() => {
                        state.error = Some(format!("{} already exists", file_name(&path)));
                        state.focus = true;
                    }
                    Ok(path) => answer = Answer::Done(path),
                    Err(e) => {
                        state.error = Some(e);
                        state.focus = true;
                    }
                }
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                answer = Answer::Cancelled;
            }
        });
    });
    answer
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into())
}

/// What to do with unsaved changes before switching scenes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
}

/// Asks whether to save `scene` (its file name) first. Done gives the
/// choice; Cancelled means stay.
pub fn save_prompt(ctx: &Context, scene: &str) -> Answer<SaveChoice> {
    let mut answer = Answer::Waiting;
    egui::Modal::new(egui::Id::new("save changes")).show(ctx, |ui| {
        ui.set_width(320.0);
        ui.heading("Save changes?");
        ui.label(format!("{scene} has unsaved changes. They're lost if you don't save them."));
        ui.horizontal(|ui| {
            if ui.button("Save").clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                answer = Answer::Done(SaveChoice::Save);
            }
            if ui.button("Don't Save").clicked() {
                answer = Answer::Done(SaveChoice::DontSave);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                answer = Answer::Cancelled;
            }
        });
    });
    answer
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{folder_label, new_scene_path};

    #[test]
    fn names_become_scene_files_in_the_folder() {
        assert_eq!(new_scene_path("scenes", "Level 2"), Ok("scenes/Level 2.scene.ron".into()));
        assert_eq!(new_scene_path("", " boss.scene.ron "), Ok("boss.scene.ron".into()));
        assert_eq!(new_scene_path("/levels/", "a"), Ok("levels/a.scene.ron".into()));
    }

    #[test]
    fn folders_read_from_the_asset_folder() {
        let assets = Path::new("/game/assets");
        assert_eq!(folder_label(&assets.join("scenes"), Some(assets)), "assets/scenes");
        assert_eq!(folder_label(assets, Some(assets)), "assets");
        let elsewhere = Path::new("/game/levels");
        assert_eq!(folder_label(elsewhere, Some(assets)), elsewhere.display().to_string());
    }

    /// Draws the New Scene box for one frame with `events` as input.
    fn frame(
        ctx: &egui::Context,
        state: &mut super::NewScene,
        events: Vec<egui::Event>,
    ) -> super::Answer<std::path::PathBuf> {
        let input = egui::RawInput { events, ..Default::default() };
        let mut answer = super::Answer::Waiting;
        let mut output = ctx.run_ui(input, |ui| {
            answer = super::new_scene_window(ui.ctx(), state);
        });
        output.textures_delta.clear();
        answer
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn enter() -> egui::Event {
        key(egui::Key::Enter)
    }

    #[test]
    fn the_create_button_makes_the_scene() {
        let dir = std::env::temp_dir().join("reign-new-scene-button");
        let ctx = egui::Context::default();
        let mut state = super::NewScene::new(dir.clone(), "assets/scenes");
        frame(&ctx, &mut state, vec![]);
        // Tab from the name to Create, then press it.
        assert_eq!(frame(&ctx, &mut state, vec![key(egui::Key::Tab)]), super::Answer::Waiting);
        let answer = frame(&ctx, &mut state, vec![key(egui::Key::Space)]);
        assert_eq!(answer, super::Answer::Done(dir.join("New Scene.scene.ron")));
    }

    #[test]
    fn enter_in_the_new_scene_box_makes_the_scene() {
        let dir = std::env::temp_dir().join(format!("reign-new-scene-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = egui::Context::default();
        let mut state = super::NewScene::new(dir.clone(), "assets/scenes");
        assert_eq!(frame(&ctx, &mut state, vec![]), super::Answer::Waiting);
        let answer = frame(&ctx, &mut state, vec![enter()]);
        assert_eq!(answer, super::Answer::Done(dir.join("New Scene.scene.ron")));
        // A name that's taken is refused, and the box stays open.
        std::fs::write(dir.join("New Scene.scene.ron"), "").unwrap();
        frame(&ctx, &mut state, vec![]);
        assert_eq!(frame(&ctx, &mut state, vec![enter()]), super::Answer::Waiting);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_names_are_refused() {
        for name in ["", "   ", ".scene.ron", ".hidden", "a/b", "a\\b", "what?"] {
            assert!(new_scene_path("scenes", name).is_err(), "{name:?}");
        }
    }
}
