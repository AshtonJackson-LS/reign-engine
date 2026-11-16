//! Project Settings: the dialog behind the toolbar's **Project** menu. It
//! edits the game's name (`ReignProject.ron`), its start scene, window,
//! version and author (`ProjectSettings.ron`, see `settings.rs`), and its app
//! icon (`icon.png`, see `icon.rs` for the rules). It also shows the input
//! actions (`assets/input.ron`), with a button for the Input Actions dialog.
//!
//! What the dialog does is plain functions ([`Draft`], [`choose_icon`],
//! [`reset_icon`]) so it can be tested without a window.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use reign::assets::RgbaImage;
use reign::editor::egui;

use crate::editor::Project;
use crate::icon::{self, IconError};
use crate::input::{self, InputMap};
use crate::settings::{self, MAX_WINDOW, Settings, Window};

/// Takes the image at `from` as the project's icon: it must pass the icon
/// rules (see [`icon::check`]), and is then copied to `icon.png` in the
/// project folder, replacing the one there. Returns the image. A file that
/// is plainly too big is refused without reading it.
pub fn choose_icon(root: &Path, from: &Path) -> Result<RgbaImage, String> {
    let size =
        std::fs::metadata(from).map_err(|e| format!("Can't read {}: {e}", from.display()))?.len();
    if size > icon::MAX_BYTES as u64 {
        return Err(IconError::TooHeavy { bytes: size as usize }.to_string());
    }
    let bytes = std::fs::read(from).map_err(|e| format!("Can't read {}: {e}", from.display()))?;
    let image = icon::check(&bytes).map_err(|e| e.to_string())?;
    // Written beside the old one first, so a failure leaves the old icon.
    let target = root.join(icon::FILE);
    let temp = root.join(format!("{}.new", icon::FILE));
    std::fs::write(&temp, &bytes).and_then(|()| std::fs::rename(&temp, &target)).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Can't save {}: {e}", target.display())
    })?;
    Ok(image)
}

/// Goes back to the default icon by removing the project's `icon.png`.
/// Returns whether there was one to remove.
pub fn reset_icon(root: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(root.join(icon::FILE)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// What the project's icon is right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconState {
    /// No icon.png: the Reign default.
    Default,
    /// The project's own, with its size in pixels.
    Own { side: u32 },
    /// An icon.png that can't be used, and why: the default is used.
    Unusable(String),
}

/// Looks at the project's `icon.png`.
pub fn icon_state(root: &Path) -> IconState {
    match std::fs::read(root.join(icon::FILE)) {
        Ok(bytes) => match icon::check(&bytes) {
            Ok(image) => IconState::Own { side: image.width },
            Err(e) => IconState::Unusable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => IconState::Default,
        Err(e) => IconState::Unusable(format!("It can't be read: {e}")),
    }
}

/// What Project Settings saves: the game's name (in `ReignProject.ron`) and
/// the rest (in `ProjectSettings.ron`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub settings: Settings,
}

impl Draft {
    /// The project's settings as they are on disk, and why the settings file
    /// couldn't be used if it couldn't (its values are then the defaults).
    pub fn load(root: &Path) -> (Draft, Option<String>) {
        let name = Project { root: root.to_owned() }.title();
        match Settings::load(root) {
            Ok(settings) => (Draft { name, settings }, None),
            Err(e) => (Draft { name, settings: Settings::default() }, Some(e)),
        }
    }

    /// What's wrong with the draft, in words for the person editing it.
    /// `scenes` are the project's scene paths: the start scene must be one.
    pub fn problems(&self, scenes: &[String]) -> Vec<String> {
        let mut problems = Vec::new();
        if let Err(e) = settings::check_text("The name", &self.name, false) {
            problems.push(e);
        }
        problems.extend(self.settings.problems());
        let start = &self.settings.start_scene;
        if !start.is_empty() && !scenes.contains(start) {
            problems.push(format!("There is no scene {start} in the project."));
        }
        problems
    }

    /// Saves the draft over what `before` was: the name only if it changed.
    /// Nothing is written if the draft has a problem. Returns whether the
    /// name changed.
    pub fn save(&self, root: &Path, before: &Draft, scenes: &[String]) -> Result<bool, String> {
        if let Some(problem) = self.problems(scenes).into_iter().next() {
            return Err(problem);
        }
        let renamed = self.name.trim() != before.name;
        if renamed {
            Project { root: root.to_owned() }.set_title(&self.name)?;
        }
        self.settings.save(root)?;
        Ok(renamed)
    }
}

/// The side of the icon preview in the dialog, in points.
const PREVIEW: f32 = 96.0;

/// The dialog's state.
#[derive(Default)]
pub struct ProjectSettings {
    open: bool,
    /// What the icon is, and how it looks, refreshed when the dialog opens
    /// and after each change.
    state: Option<(IconState, egui::TextureHandle)>,
    /// The settings being edited, and as they were when last saved or read.
    draft: Option<(Draft, Draft)>,
    /// Why `ProjectSettings.ron` couldn't be used, when the dialog opened.
    load_error: Option<String>,
    /// What to tell the user after the last action, and whether it's an error.
    message: Option<(String, bool)>,
    /// "Remove icon.png?" is being asked.
    confirm_reset: bool,
    /// A new window icon, for the editor to show once.
    new_icon: Option<RgbaImage>,
    /// A new window title, for the editor to show once.
    new_title: Option<String>,
    /// Settings just saved, for the editor to take up once.
    saved: Option<Settings>,
    /// The input actions as last read, and the file's time then: read
    /// again when the file changes (the Input Actions dialog saved it).
    input: Option<(Option<SystemTime>, Result<InputMap, String>)>,
    /// Edit bindings… was pressed, for the editor to open the dialog once.
    edit_input: bool,
}

/// The editor window's title for a game called `name`.
pub fn editor_title(name: &str) -> String {
    format!("{name} - Reign Engine")
}

impl ProjectSettings {
    /// The toolbar's **Project** menu.
    pub fn menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Project", |ui| {
            if ui.button("Project Settings…").clicked() {
                self.open = true;
                self.state = None;
                self.draft = None;
                self.message = None;
                self.confirm_reset = false;
                ui.close();
            }
        });
    }

    /// A new window icon, once, for `EditorApp::take_window_icon`.
    pub fn take_new_icon(&mut self) -> Option<RgbaImage> {
        self.new_icon.take().map(|image| icon::window_icon(&image))
    }

    /// A new window title, once, for `EditorApp::take_window_title`.
    pub fn take_new_title(&mut self) -> Option<String> {
        self.new_title.take()
    }

    /// Edit bindings… was pressed: true once, for the editor to open the
    /// Input Actions dialog.
    pub fn take_edit_input(&mut self) -> bool {
        std::mem::take(&mut self.edit_input)
    }

    /// The settings, once, after they're saved: for the editor's views.
    pub fn take_saved(&mut self) -> Option<Settings> {
        self.saved.take()
    }

    /// Draws the dialog while it is open. `scenes` are the project's scene
    /// paths.
    pub fn dialog(&mut self, ctx: &egui::Context, root: &Path, scenes: &[String]) {
        if !self.open {
            return;
        }
        if self.state.is_none() {
            self.refresh(ctx, root);
        }
        if self.draft.is_none() {
            let (draft, error) = Draft::load(root);
            self.draft = Some((draft.clone(), draft));
            self.load_error = error;
        }
        let input_file = root.join("assets").join(input::INPUT_FILE);
        let modified = std::fs::metadata(&input_file).and_then(|m| m.modified()).ok();
        if self.input.as_ref().is_none_or(|(time, _)| *time != modified) {
            self.input = Some((modified, input::read(&input_file)));
        }
        let mut choose = false;
        let mut ask_reset = false;
        let mut do_reset = false;
        let mut cancel_reset = false;
        let mut save = false;
        let mut close = false;
        let max_height = (ctx.content_rect().height() - 160.0).max(200.0);
        let modal = egui::Modal::new(egui::Id::new("project settings")).show(ctx, |ui| {
            ui.set_width(480.0);
            ui.heading("Project Settings");
            ui.add_space(4.0);
            let Some((draft, saved)) = &mut self.draft else { return };
            let problems = draft.problems(scenes);
            let dirty = draft != saved;
            egui::ScrollArea::vertical().max_height(max_height).show(ui, |ui| {
                if let Some(e) = &self.load_error {
                    ui.colored_label(
                        egui::Color32::LIGHT_RED,
                        format!("{e}. The defaults are shown; saving replaces the file."),
                    );
                    ui.add_space(4.0);
                }
                game_section(ui, draft);
                ui.separator();
                start_scene_section(ui, &mut draft.settings, scenes);
                ui.separator();
                window_section(ui, &mut draft.settings);
                ui.separator();
                layers_section(ui, &mut draft.settings.layers);
                ui.separator();
                tags_section(ui, &mut draft.settings.tags);
                ui.separator();
                if let Some((_, map)) = &self.input
                    && input_section(ui, map)
                {
                    self.edit_input = true;
                }
                ui.separator();
                let Some((state, texture)) = &self.state else { return };
                ui.strong("App icon");
                ui.weak(
                    "Shown on this project's windows and on the game you export. \
                     A change here is saved straight away.",
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(texture).fit_to_exact_size(egui::Vec2::splat(PREVIEW)));
                    ui.vertical(|ui| {
                        icon_words(ui, state);
                        ui.add_space(6.0);
                        if self.confirm_reset {
                            ui.label(format!("Remove {} from this project?", icon::FILE));
                            ui.horizontal(|ui| {
                                if ui.button("Remove it").clicked() {
                                    do_reset = true;
                                }
                                if ui.button("Keep it").clicked() {
                                    cancel_reset = true;
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                if ui.button("Choose image…").clicked() {
                                    choose = true;
                                }
                                let has_file = !matches!(state, IconState::Default);
                                if ui
                                    .add_enabled(has_file, egui::Button::new("Use the default"))
                                    .on_hover_text(format!(
                                        "Removes {} from the project",
                                        icon::FILE
                                    ))
                                    .clicked()
                                {
                                    ask_reset = true;
                                }
                            });
                        }
                    });
                });
                ui.add_space(8.0);
                ui.weak(format!(
                    "A PNG, square, from {min} to {max} pixels a side (256 or 512 is a good \
                     choice), at most {mb} MB. It is copied into the project as {file}.",
                    min = icon::MIN_SIZE,
                    max = icon::MAX_SIZE,
                    mb = icon::MAX_BYTES / (1024 * 1024),
                    file = icon::FILE,
                ));
            });
            for problem in &problems {
                ui.colored_label(egui::Color32::LIGHT_RED, problem);
            }
            if let Some((message, is_error)) = &self.message {
                ui.add_space(6.0);
                if *is_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, message);
                } else {
                    ui.label(message);
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if dirty { "Cancel" } else { "Close" };
                let hover = if dirty { "Closes without saving the changes" } else { "" };
                if ui.button(label).on_hover_text(hover).clicked() {
                    close = true;
                }
                if ui.add_enabled(dirty && problems.is_empty(), egui::Button::new("Save")).clicked()
                {
                    save = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }

        if save && let Some((draft, saved)) = &mut self.draft {
            match draft.save(root, saved, scenes) {
                Ok(renamed) => {
                    if renamed {
                        self.new_title = Some(editor_title(draft.name.trim()));
                    }
                    draft.name = draft.name.trim().to_owned();
                    *saved = draft.clone();
                    self.saved = Some(draft.settings.clone());
                    self.load_error = None;
                    self.message = Some((
                        "Saved. Play, Export Game and the game use these from now on.".into(),
                        false,
                    ));
                }
                Err(e) => self.message = Some((e, true)),
            }
        }
        if choose
            && let Some(from) = rfd::FileDialog::new()
                .set_title("Choose the project's icon")
                .add_filter("PNG image", &["png"])
                .pick_file()
        {
            self.apply_choice(ctx, root, &from);
        }
        if ask_reset {
            self.confirm_reset = true;
        }
        if cancel_reset {
            self.confirm_reset = false;
        }
        if do_reset {
            self.confirm_reset = false;
            match reset_icon(root) {
                Ok(_) => {
                    self.new_icon = Some(icon::default_icon().clone());
                    self.message = Some(("Back to the default icon.".into(), false));
                }
                Err(e) => self.message = Some((format!("Can't remove {}: {e}", icon::FILE), true)),
            }
            self.refresh(ctx, root);
        }
        if close {
            self.open = false;
            self.draft = None;
        }
    }

    /// Takes `from` as the icon and updates what the dialog shows.
    fn apply_choice(&mut self, ctx: &egui::Context, root: &Path, from: &Path) {
        match choose_icon(root, from) {
            Ok(image) => {
                self.message = Some((
                    format!(
                        "Saved as {} ({} × {} pixels). The editor's window shows it now; \
                         the game and exports use it from now on.",
                        icon::FILE,
                        image.width,
                        image.height
                    ),
                    false,
                ));
                self.new_icon = Some(image);
            }
            Err(e) => self.message = Some((e, true)),
        }
        self.refresh(ctx, root);
    }

    /// Reads the project's icon again, for the preview and the words.
    fn refresh(&mut self, ctx: &egui::Context, root: &Path) {
        let state = icon_state(root);
        let image = icon::resize(&icon::project_icon(root), (PREVIEW * 2.0) as u32);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.rgba,
        );
        let texture = ctx.load_texture("project icon", color, egui::TextureOptions::LINEAR);
        self.state = Some((state, texture));
    }
}

/// The game's name, version and author.
fn game_section(ui: &mut egui::Ui, draft: &mut Draft) {
    ui.strong("Game");
    ui.add_space(4.0);
    egui::Grid::new("project settings game").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Name");
        ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(300.0)).on_hover_text(
            "The game's window title and the name of what Export Game makes. \
                 The project's folder keeps its name.",
        );
        ui.end_row();
        ui.label("Version");
        ui.add(
            egui::TextEdit::singleline(&mut draft.settings.version)
                .desired_width(120.0)
                .hint_text("1.0.0"),
        )
        .on_hover_text("One to four numbers with dots between, such as 1.0.0");
        ui.end_row();
        ui.label("Author");
        ui.add(
            egui::TextEdit::singleline(&mut draft.settings.author)
                .desired_width(300.0)
                .hint_text("You or your studio (optional)"),
        );
        ui.end_row();
    });
    ui.weak(
        "The version and author go in the exported game's file details (Windows) and app (macOS).",
    );
}

/// Which scene the game starts on.
fn start_scene_section(ui: &mut egui::Ui, settings: &mut Settings, scenes: &[String]) {
    ui.strong("Start scene");
    ui.add_space(4.0);
    let automatic = "Automatic: main.scene.ron, else the first";
    let shown = if settings.start_scene.is_empty() { automatic } else { &settings.start_scene };
    egui::ComboBox::from_id_salt("project settings start scene")
        .width(320.0)
        .selected_text(shown.to_owned())
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut settings.start_scene, String::new(), automatic);
            let mut sorted: Vec<&String> = scenes.iter().collect();
            sorted.sort();
            for scene in sorted {
                ui.selectable_value(&mut settings.start_scene, scene.clone(), scene.as_str());
            }
        });
    ui.weak("What the exported game and --simulate play first, and what the editor opens on.");
}

/// The window the game starts in, and the colour behind everything.
fn window_section(ui: &mut egui::Ui, settings: &mut Settings) {
    let window = &mut settings.window;
    ui.strong("Game window");
    ui.add_space(4.0);
    let (min_w, min_h) = Window::MIN;
    ui.horizontal(|ui| {
        ui.label("Size");
        ui.add(egui::DragValue::new(&mut window.width).range(min_w..=MAX_WINDOW).suffix(" px"));
        ui.label("×");
        ui.add(egui::DragValue::new(&mut window.height).range(min_h..=MAX_WINDOW).suffix(" px"));
        for scale in [2, 3] {
            let size = (min_w * scale, min_h * scale);
            if ui
                .selectable_label((window.width, window.height) == size, format!("{scale}×"))
                .on_hover_text(format!("{} × {}: the view, {scale} times over", size.0, size.1))
                .clicked()
            {
                (window.width, window.height) = size;
            }
        }
    });
    ui.checkbox(&mut window.fullscreen, "Start fullscreen");
    ui.weak("Players can switch fullscreen with F11 while the game runs.");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("Background");
        ui.color_edit_button_srgb(&mut settings.background)
            .on_hover_text("What shows where nothing is drawn, in the game and the editor");
        if ui
            .add_enabled(
                settings.background != settings::DEFAULT_BACKGROUND,
                egui::Button::new("Default"),
            )
            .clicked()
        {
            settings.background = settings::DEFAULT_BACKGROUND;
        }
    });
}

/// Names for collision layers: one row per named layer, its number and its
/// name, and a button to add one.
fn layers_section(ui: &mut egui::Ui, layers: &mut BTreeMap<u8, String>) {
    ui.strong("Collision layers");
    ui.weak(
        "Name the layers your colliders use: the Inspector shows these names for a \
         collider's layer and mask. Unnamed layers still work.",
    );
    ui.add_space(4.0);
    let mut rows: Vec<(u8, String)> = std::mem::take(layers).into_iter().collect();
    let used: Vec<u8> = rows.iter().map(|(l, _)| *l).collect();
    let mut remove = None;
    egui::Grid::new("project settings layers").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
        for (index, (layer, name)) in rows.iter_mut().enumerate() {
            let own = *layer;
            egui::ComboBox::from_id_salt(("layer number", index))
                .width(48.0)
                .selected_text(own.to_string())
                .show_ui(ui, |ui| {
                    for l in (0..settings::LAYERS).filter(|l| *l == own || !used.contains(l)) {
                        ui.selectable_value(layer, l, l.to_string());
                    }
                });
            ui.add(egui::TextEdit::singleline(name).desired_width(220.0).hint_text("Name"));
            if ui.small_button("✕").on_hover_text("Forget this name").clicked() {
                remove = Some(index);
            }
            ui.end_row();
        }
    });
    if let Some(index) = remove {
        rows.remove(index);
    }
    let free = (0..settings::LAYERS).find(|l| !rows.iter().any(|(r, _)| r == l));
    if ui.add_enabled(free.is_some(), egui::Button::new("Add layer")).clicked()
        && let Some(l) = free
    {
        rows.push((l, String::new()));
    }
    *layers = rows.into_iter().collect();
}

/// The project's tags: one row each, a button to add one.
fn tags_section(ui: &mut egui::Ui, tags: &mut Vec<String>) {
    ui.strong("Tags");
    ui.weak(
        "Labels for objects, picked under an object's name in the Inspector. Scripts find \
         objects by them: ctx.find_with_tag(\"Enemy\").",
    );
    ui.add_space(4.0);
    let mut remove = None;
    egui::Grid::new("project settings tags").num_columns(2).spacing([8.0, 4.0]).show(ui, |ui| {
        for (index, tag) in tags.iter_mut().enumerate() {
            ui.add(egui::TextEdit::singleline(tag).desired_width(220.0).hint_text("Tag"));
            let forget = ui
                .small_button("✕")
                .on_hover_text("Take it off the list. Objects with it keep it.");
            if forget.clicked() {
                remove = Some(index);
            }
            ui.end_row();
        }
    });
    if let Some(index) = remove {
        tags.remove(index);
    }
    if ui.button("Add tag").clicked() {
        tags.push(String::new());
    }
}

/// What the icon is, in words.
/// The input actions and their bindings. Returns whether Edit bindings…
/// was pressed.
fn input_section(ui: &mut egui::Ui, map: &Result<InputMap, String>) -> bool {
    ui.strong("Input");
    ui.weak(format!(
        "The actions scripts read with ctx.action(\"Jump\"), from assets/{}. \
         Changes to them are saved straight away.",
        input::INPUT_FILE
    ));
    ui.add_space(4.0);
    match map {
        Ok(map) => {
            egui::Grid::new("project settings input").num_columns(2).spacing([12.0, 4.0]).show(
                ui,
                |ui| {
                    for action in &map.actions {
                        ui.label(&action.name);
                        let labels = action.labels();
                        if labels.is_empty() {
                            ui.weak("nothing bound");
                        } else {
                            ui.add(egui::Label::new(labels.join(", ")).wrap());
                        }
                        ui.end_row();
                    }
                },
            );
        }
        Err(e) => {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("{}: {e}. The game uses the defaults.", input::INPUT_FILE),
            );
        }
    }
    ui.add_space(4.0);
    ui.button("Edit bindings…")
        .on_hover_text("Listen for keys, mouse buttons and pad inputs, and swap them")
        .clicked()
}

fn icon_words(ui: &mut egui::Ui, state: &IconState) {
    match state {
        IconState::Default => {
            ui.label("The Reign default icon.");
        }
        IconState::Own { side } => {
            ui.label(format!("Your icon: {icon} ({side} × {side} pixels).", icon = icon::FILE));
        }
        IconState::Unusable(why) => {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                format!("{} in the project can't be used, so the default is shown.", icon::FILE),
            );
            ui.weak(why);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("reign-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(side: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = RgbaImage {
            rgba: rgba.iter().copied().cycle().take((side * side * 4) as usize).collect(),
            width: side,
            height: side,
        };
        icon::encode_png(&image)
    }

    #[test]
    fn saving_a_draft_renames_the_game_and_writes_the_settings() {
        let root = temp("draft");
        Project::open(root.clone()).unwrap();
        let scenes = vec!["scenes/main.scene.ron".to_owned(), "scenes/title.scene.ron".to_owned()];
        let (before, error) = Draft::load(&root);
        assert_eq!(error, None);
        assert_eq!(before.settings, Settings::default());

        let mut draft = before.clone();
        draft.name = " Space Rocks ".into();
        draft.settings.start_scene = "scenes/title.scene.ron".into();
        draft.settings.window = Window { width: 1920, height: 1080, fullscreen: true };
        draft.settings.author = "Me".into();
        draft.settings.background = [200, 100, 50];
        draft.settings.layers.insert(5, "Coins".into());
        draft.settings.tags.push("Coin".into());
        assert_eq!(draft.save(&root, &before, &scenes), Ok(true), "renamed");
        let (after, _) = Draft::load(&root);
        assert_eq!(after.name, "Space Rocks");
        assert_eq!(after.settings, draft.settings);

        // The same name again: not a rename.
        let mut again = after.clone();
        again.settings.version = "1.1".into();
        assert_eq!(again.save(&root, &after, &scenes), Ok(false));
        assert_eq!(Settings::load(&root).unwrap().version, "1.1");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_draft_with_a_problem_writes_nothing() {
        let root = temp("draft-bad");
        Project::open(root.clone()).unwrap();
        let scenes = vec!["scenes/main.scene.ron".to_owned()];
        let (before, _) = Draft::load(&root);
        let project_file = std::fs::read(root.join(crate::editor::PROJECT_FILE)).unwrap();

        let mut gone = before.clone();
        gone.name = "New name".into();
        gone.settings.start_scene = "scenes/gone.scene.ron".into();
        assert!(gone.save(&root, &before, &scenes).unwrap_err().contains("no scene"));
        let mut nameless = before.clone();
        nameless.name = "  ".into();
        assert!(nameless.save(&root, &before, &scenes).unwrap_err().contains("empty"));
        let mut tiny = before.clone();
        tiny.settings.window.width = 10;
        assert_eq!(tiny.problems(&scenes).len(), 1);
        assert!(tiny.save(&root, &before, &scenes).is_err());

        assert!(!root.join(settings::FILE).exists());
        assert_eq!(std::fs::read(root.join(crate::editor::PROJECT_FILE)).unwrap(), project_file);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_settings_file_that_cant_be_read_is_reported_when_the_dialog_opens() {
        let root = temp("draft-broken");
        std::fs::write(root.join(settings::FILE), "(window: nope)").unwrap();
        let (draft, error) = Draft::load(&root);
        assert!(error.unwrap().starts_with(settings::FILE));
        assert_eq!(draft.settings, Settings::default());
        let _ = std::fs::remove_dir_all(root);
    }

    /// The dialog draws, with every section, in a context with no window.
    #[test]
    fn the_dialog_draws() {
        let root = temp("dialog");
        Project::open(root.clone()).unwrap();
        let ctx = egui::Context::default();
        let mut dialog = ProjectSettings { open: true, ..ProjectSettings::default() };
        let scenes = vec!["scenes/main.scene.ron".to_owned()];
        for _ in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 800.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| dialog.dialog(ui.ctx(), &root, &scenes));
            // Nothing draws these frames; egui panics if a texture it made is
            // dropped without being uploaded or cleared.
            output.textures_delta.clear();
        }
        assert!(dialog.open);
        assert!(dialog.state.is_some(), "the icon preview is loaded");
        let (_, input) = dialog.input.as_ref().expect("the input actions are read");
        assert_eq!(input.as_ref().unwrap(), &InputMap::default(), "a new project's");
        assert!(!dialog.take_edit_input(), "nothing pressed");
        let (draft, saved) = dialog.draft.as_ref().expect("the settings are loaded");
        assert_eq!(draft, saved, "drawing changes nothing");
        assert_eq!(dialog.take_new_title(), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn choosing_a_good_image_copies_it_into_the_project() {
        let root = temp("choose");
        let from = root.join("art.png");
        std::fs::write(&from, png(256, [10, 20, 30, 255])).unwrap();
        let image = choose_icon(&root, &from).unwrap();
        assert_eq!((image.width, &image.rgba[..4]), (256, &[10, 20, 30, 255][..]));
        // The file is the same bytes, named icon.png, and nothing else is left.
        assert_eq!(std::fs::read(root.join(icon::FILE)).unwrap(), std::fs::read(&from).unwrap());
        assert!(!root.join("icon.png.new").exists());
        assert_eq!(icon_state(&root), IconState::Own { side: 256 });
        // The project now uses it.
        assert_eq!(icon::project_icon(&root), image);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn choosing_again_replaces_the_icon() {
        let root = temp("again");
        let first = root.join("one.png");
        let second = root.join("two.png");
        std::fs::write(&first, png(128, [1, 1, 1, 255])).unwrap();
        std::fs::write(&second, png(512, [9, 9, 9, 255])).unwrap();
        choose_icon(&root, &first).unwrap();
        choose_icon(&root, &second).unwrap();
        assert_eq!(icon_state(&root), IconState::Own { side: 512 });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_choice_says_why_and_changes_nothing() {
        let root = temp("bad");
        let good = root.join("good.png");
        std::fs::write(&good, png(128, [5, 5, 5, 255])).unwrap();
        choose_icon(&root, &good).unwrap();
        let before = std::fs::read(root.join(icon::FILE)).unwrap();

        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("tiny.png", png(32, [1, 2, 3, 255]), "at least 64"),
            ("huge.png", png(2048, [1, 2, 3, 255]), "at most 1024"),
            ("photo.jpg", b"\xff\xd8\xff\xe0 jpeg bytes".to_vec(), "must be a PNG"),
            ("clear.png", png(64, [1, 2, 3, 0]), "transparent"),
        ];
        for (name, bytes, words) in cases {
            let path = root.join(name);
            std::fs::write(&path, bytes).unwrap();
            let error = choose_icon(&root, &path).unwrap_err();
            assert!(error.contains(words), "{name}: {error}");
            assert_eq!(
                std::fs::read(root.join(icon::FILE)).unwrap(),
                before,
                "{name} changed the icon"
            );
        }
        // Not square.
        let wide = root.join("wide.png");
        let mut encoder_out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut encoder_out, 128, 64);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&vec![7; 128 * 64 * 4]).unwrap();
        }
        std::fs::write(&wide, encoder_out).unwrap();
        assert!(choose_icon(&root, &wide).unwrap_err().contains("must be square"));
        // A file that isn't there.
        assert!(choose_icon(&root, &root.join("nope.png")).unwrap_err().starts_with("Can't read"));
        assert!(!root.join("icon.png.new").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_over_the_limit_is_refused_without_being_read() {
        let root = temp("heavy");
        let big = root.join("big.png");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(icon::MAX_BYTES as u64 + 1).unwrap(); // sparse: not actually written
        let error = choose_icon(&root, &big).unwrap_err();
        assert!(error.contains("at most 8 MB"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resetting_removes_the_icon_and_is_safe_to_repeat() {
        let root = temp("reset");
        assert_eq!(icon_state(&root), IconState::Default);
        assert!(!reset_icon(&root).unwrap(), "nothing to remove");
        let from = root.join("a.png");
        std::fs::write(&from, png(64, [3, 3, 3, 255])).unwrap();
        choose_icon(&root, &from).unwrap();
        assert!(reset_icon(&root).unwrap());
        assert_eq!(icon_state(&root), IconState::Default);
        assert!(from.is_file(), "the picked image itself is never touched");
        assert_eq!(icon::project_icon(&root), *icon::default_icon());
        assert!(!reset_icon(&root).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unusable_icon_file_is_reported_not_hidden() {
        let root = temp("unusable");
        std::fs::write(root.join(icon::FILE), b"GIF89a not a png").unwrap();
        match icon_state(&root) {
            IconState::Unusable(why) => assert!(why.contains("PNG"), "{why}"),
            other => panic!("{other:?}"),
        }
        // The default is what the project actually uses meanwhile.
        assert_eq!(icon::project_icon(&root), *icon::default_icon());
        let _ = std::fs::remove_dir_all(root);
    }
}
