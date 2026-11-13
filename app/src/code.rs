//! The Scripts menu: making components, opening the project in a code
//! editor, rebuilding this project's editor when its code changes, and
//! exporting the game.
//!
//! A project's editor is a Rust binary built from the project's own crate
//! (see `reign_build::project`). When the code changes, the editor builds
//! the new one in the background, saves the scene, starts it on the same
//! scene and closes itself, like Unity's script reload.
//!
//! Export Game builds the project's other binary, the game without the
//! editor, with `--release`, then packs the assets beside it (see
//! `export.rs`). That needs the project to be a crate, so a project without
//! code is set up as one first.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant, SystemTime};

use reign::editor::{EditorRequests, PlayMode, egui};
use reign_build::ide;
use reign_build::project::{self, Template};

use crate::export::{self, Export, Layout};

enum BuildEvent {
    Line(String),
    Done(bool),
    /// An export finished packing and copying: the app or folder it made.
    Exported(Result<PathBuf, String>),
}

struct Build {
    rx: Receiver<BuildEvent>,
    started: Instant,
    /// The last line cargo printed, for the toolbar.
    last: String,
    /// Building the game for Export Game, not this project's editor.
    export: bool,
}

/// After a good build: save, then start the new editor and close.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Relaunch {
    /// Waiting for Stop before reloading.
    Waiting,
    /// The save was asked for this frame; start the new editor next frame.
    Saved,
}

struct Dialog {
    name: String,
    template: Template,
    error: Option<String>,
    focus: bool,
}

/// The Export Game dialog.
struct ExportDialog {
    out_dir: PathBuf,
    error: Option<String>,
}

pub struct Code {
    root: PathBuf,
    project_name: String,
    /// The crate this editor was built from, if any.
    compiled_for: Option<&'static str>,
    cargo: bool,
    build: Option<Build>,
    relaunch: Option<Relaunch>,
    failed: bool,
    /// Code on disk is newer than this editor.
    changed: bool,
    last_check: Option<Instant>,
    /// When this editor's binary was built.
    built_at: Option<SystemTime>,
    auto_reload: bool,
    dialog: Option<Dialog>,
    /// Scripts → Input Actions, while open.
    input_dialog: Option<crate::input::dialog::InputDialog>,
    export_dialog: Option<ExportDialog>,
    /// Where the last export went, for the next one.
    export_dir: Option<PathBuf>,
    /// Export Game just made this project a crate: once the game is out,
    /// build its editor and switch to it, as New Component does.
    switch_after_export: bool,
    /// Windows: this editor's .exe, moved aside so a build can replace it
    /// (where it went, where it was).
    moved_aside: Option<(PathBuf, PathBuf)>,
    mode: PlayMode,
}

impl Code {
    pub fn new(root: PathBuf, project_name: String, compiled_for: Option<&'static str>) -> Self {
        let built_at =
            std::env::current_exe().ok().and_then(|p| p.metadata().ok()?.modified().ok());
        let mut code = Code {
            root,
            project_name,
            compiled_for,
            cargo: project::cargo_available(),
            build: None,
            relaunch: None,
            failed: false,
            changed: false,
            last_check: None,
            built_at,
            auto_reload: true,
            dialog: None,
            input_dialog: None,
            export_dialog: None,
            export_dir: None,
            switch_after_export: false,
            moved_aside: None,
            mode: PlayMode::Edit,
        };
        // Opened in an editor without this project's code (the stock one):
        // build it now and switch over.
        if code.needs_build() && code.cargo {
            log::info!("this project has code: building its editor");
            code.start_build();
        }
        code
    }

    pub fn set_mode(&mut self, mode: PlayMode) {
        self.mode = mode;
    }

    fn crate_name(&self) -> Option<String> {
        project::read_crate_name(&self.root)
    }

    /// The project has code this editor wasn't built from.
    fn needs_build(&self) -> bool {
        match (self.crate_name(), self.compiled_for) {
            (Some(name), Some(built)) => name != built,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    fn start_build(&mut self) {
        if self.build.is_some() {
            return;
        }
        let Some(name) = self.crate_name() else { return };
        self.moved_aside = move_running_editor_aside(&project::editor_path(&self.root, &name));
        let mut command = project::build_command(&self.root, &name);
        command.stdout(Stdio::null()).stderr(Stdio::piped());
        let (tx, rx) = channel();
        match command.spawn() {
            Ok(mut child) => {
                let stderr = child.stderr.take();
                std::thread::spawn(move || {
                    if let Some(stderr) = stderr {
                        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                            let _ = tx.send(BuildEvent::Line(line));
                        }
                    }
                    let ok = child.wait().is_ok_and(|s| s.success());
                    let _ = tx.send(BuildEvent::Done(ok));
                });
                self.build = Some(Build {
                    rx,
                    started: Instant::now(),
                    last: "Starting cargo".into(),
                    export: false,
                });
                self.failed = false;
            }
            Err(e) => {
                log::error!("can't run cargo: {e}");
                self.put_editor_back();
            }
        }
    }

    /// After a build that made no new editor: the running one goes back
    /// where it was, so the Hub can still open it.
    fn put_editor_back(&mut self) {
        let Some((aside, original)) = self.moved_aside.take() else { return };
        if !original.exists()
            && let Err(e) = std::fs::rename(&aside, &original)
        {
            log::warn!("couldn't put {} back: {e}", original.display());
        }
    }

    /// Sets the project up to build its game (a crate with the game
    /// binary), then builds it with `--release` and exports it into
    /// `out_dir` in the background.
    fn start_export(&mut self, out_dir: PathBuf) -> Result<(), String> {
        if self.build.is_some() {
            return Err("A build is running: wait for it to finish.".into());
        }
        if !self.cargo {
            return Err("Couldn't find cargo (looked on PATH, in ~/.cargo/bin and Homebrew). Install Rust from rustup.rs, then try again.".into());
        }
        let (name, had_code) = export::prepare(&self.root, &self.project_name)?;
        std::fs::create_dir_all(&out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
        let root = self.root.clone();
        let mut export = Export {
            title: self.project_name.clone(),
            crate_name: name,
            exe: PathBuf::new(),
            assets: self.root.join("assets"),
            out_dir,
            layout: Layout::native(),
        };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let built = export::build(&root, &export.crate_name, |l| {
                let _ = tx.send(BuildEvent::Line(l));
            });
            let result = built.and_then(|exe| {
                export.exe = exe;
                // Packed after the build, so the scene saved when Export
                // was pressed is on disk by now.
                export::assemble(&export)
            });
            let _ = tx.send(BuildEvent::Exported(result));
        });
        log::info!("exporting {}: building the game (release)", self.project_name);
        self.build = Some(Build {
            rx,
            started: Instant::now(),
            last: "Starting cargo".into(),
            export: true,
        });
        self.failed = false;
        self.switch_after_export = !had_code;
        Ok(())
    }

    /// After an export: says where it went and shows it.
    fn exported(&mut self, result: Result<PathBuf, String>, secs: u64) {
        match result {
            Ok(path) => {
                log::info!("exported the game in {secs}s: {}", path.display());
                reveal(&path);
            }
            Err(e) => {
                log::error!("couldn't export the game: {e}");
                self.failed = true;
            }
        }
        // A project set up just now switches to its own editor either way:
        // its code is there now.
        if std::mem::take(&mut self.switch_after_export) {
            log::info!("this project has code now: building its editor");
            self.start_build();
        }
    }

    /// Reads build output into the Console, and notices when it's done.
    fn poll_build(&mut self) {
        let Some(build) = &mut self.build else { return };
        let mut done = None;
        let mut exported = None;
        while let Ok(event) = build.rx.try_recv() {
            match event {
                BuildEvent::Line(line) => {
                    let text = line.trim();
                    if text.contains("error") && !text.starts_with("Compiling") {
                        log::error!("{text}");
                    } else if text.contains("warning:") {
                        log::warn!("{text}");
                    } else if !text.is_empty() {
                        log::info!("{text}");
                    }
                    if !text.is_empty() {
                        build.last = text.to_owned();
                    }
                }
                BuildEvent::Done(ok) => done = Some(ok),
                BuildEvent::Exported(result) => exported = Some(result),
            }
        }
        if let Some(result) = exported {
            let secs = build.started.elapsed().as_secs();
            self.build = None;
            self.exported(result, secs);
            return;
        }
        if let Some(ok) = done {
            let secs = build.started.elapsed().as_secs();
            self.build = None;
            if ok {
                self.moved_aside = None;
                let exe = self.crate_name().map(|n| project::editor_path(&self.root, &n));
                let built = exe.and_then(|p| p.metadata().ok()?.modified().ok());
                let same_editor = self.compiled_for.is_some()
                    && self.compiled_for.map(str::to_owned) == self.crate_name();
                if same_editor && built.is_some() && built <= self.built_at {
                    // Nothing new to load: don't restart, and stop asking.
                    log::info!("scripts are up to date");
                    self.built_at = Some(SystemTime::now());
                    self.changed = false;
                } else {
                    log::info!("scripts built in {secs}s: reloading");
                    self.relaunch = Some(Relaunch::Waiting);
                }
            } else {
                log::error!(
                    "the build failed: fix the errors above, then Scripts → Recompile (Ctrl+R)"
                );
                self.put_editor_back();
                self.failed = true;
            }
        }
    }

    /// Whether any code is newer than this editor.
    fn check_changes(&mut self) {
        if self.last_check.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.last_check = Some(Instant::now());
        let (Some(built), Some(_)) = (self.built_at, self.compiled_for) else { return };
        let newest = newest_source(&self.root);
        self.changed = newest.is_some_and(|t| t > built);
    }

    fn new_component(&mut self) {
        let Some(dialog) = &mut self.dialog else { return };
        if !self.cargo {
            dialog.error = Some("Couldn't find cargo (looked on PATH, in ~/.cargo/bin and Homebrew). Install Rust from rustup.rs, then try again.".into());
            return;
        }
        if let Err(e) = project::init(&self.root, &self.project_name) {
            dialog.error = Some(format!("couldn't set up the project's code: {e}"));
            return;
        }
        let name = effective_name(&dialog.name, dialog.template);
        match project::new_component(&self.root, &name, dialog.template) {
            Ok(path) => {
                log::info!("made {}", path.display());
                match ide::open_project(&self.root, Some(&path)) {
                    Ok(name) => log::info!("opened it in {name}"),
                    Err(e) => log::warn!("{e}"),
                }
                self.dialog = None;
                self.start_build();
            }
            Err(e) => dialog.error = Some(e),
        }
    }

    fn open_ide(&self) {
        match ide::open_project(&self.root, None) {
            Ok(name) => log::info!("opened the project in {name}"),
            Err(e) => log::error!("{e}"),
        }
    }

    /// The Scripts menu and build status, in the editor's toolbar.
    pub fn toolbar_ui(&mut self, ui: &mut egui::Ui, scene_file: Option<&Path>) -> EditorRequests {
        let mut requests = EditorRequests::default();
        self.poll_build();
        self.check_changes();

        // Reload when code changes and the editor is in front again.
        let focused = ui.ctx().input(|i| i.focused);
        if self.changed
            && self.auto_reload
            && focused
            && self.build.is_none()
            && self.relaunch.is_none()
            && !self.failed
        {
            log::info!("scripts changed: rebuilding");
            self.start_build();
        }
        let typing = ui.ctx().egui_wants_keyboard_input();
        let recompile = !typing
            && ui.input_mut(|i| {
                i.consume_shortcut(&egui::KeyboardShortcut::new(
                    egui::Modifiers::COMMAND,
                    egui::Key::R,
                ))
            });
        if recompile && self.crate_name().is_some() {
            self.start_build();
        }

        ui.menu_button("Scripts", |ui| {
            if ui.button("New Component…").clicked() {
                self.dialog = Some(Dialog {
                    name: String::new(),
                    template: Template::Behaviour,
                    error: None,
                    focus: true,
                });
                ui.close();
            }
            if ui.button("Input Actions…").clicked() {
                let file = self.root.join("assets").join(crate::input::INPUT_FILE);
                self.input_dialog = Some(crate::input::dialog::InputDialog::open(file));
                ui.close();
            }
            let ide_name = ide::preferred().map_or("folder", |i| i.name);
            if ui.button(format!("Open in {ide_name}")).clicked() {
                self.open_ide();
                ui.close();
            }
            ui.menu_button("Code editor", |ui| {
                let installed = ide::installed();
                if installed.is_empty() {
                    ui.weak("None found (VS Code, RustRover, Cursor, Zed, Sublime Text)");
                }
                let current = ide::preferred().map(|i| i.name);
                for ide in installed {
                    if ui.radio(current == Some(ide.name), ide.name).clicked() {
                        let prefs = ide::Preferences { ide: Some(ide.name.to_owned()) };
                        if let Err(e) = prefs.save() {
                            log::error!("couldn't save the code editor choice: {e}");
                        }
                        ui.close();
                    }
                }
            });
            ui.separator();
            let can_build = self.cargo && self.crate_name().is_some() && self.build.is_none();
            if ui
                .add_enabled(can_build, egui::Button::new("Recompile and reload  (Ctrl+R)"))
                .clicked()
            {
                self.start_build();
                ui.close();
            }
            ui.checkbox(&mut self.auto_reload, "Reload when code changes");
            ui.separator();
            let can_export = self.cargo && self.build.is_none();
            if ui.add_enabled(can_export, egui::Button::new("Export Game…")).clicked() {
                let out_dir = self.export_dir.clone().unwrap_or_else(|| self.root.join("Builds"));
                self.export_dialog = Some(ExportDialog { out_dir, error: None });
                ui.close();
            }
            if !self.cargo {
                ui.weak("Rust isn't installed: get it from rustup.rs");
            }
        });

        // Status, right of the menu.
        if let Some(build) = &self.build {
            ui.add(egui::Spinner::new());
            let secs = build.started.elapsed().as_secs();
            let mut last = build.last.clone();
            if last.len() > 60 {
                last.truncate(60);
                last.push('…');
            }
            let doing = if build.export { "Exporting" } else { "Compiling" };
            ui.weak(format!("{doing} {secs}s · {last}")).on_hover_text(
                "The first build of a project takes a few minutes; later ones are quick.",
            );
            ui.ctx().request_repaint_after(Duration::from_millis(200));
        } else if self.failed {
            ui.colored_label(egui::Color32::LIGHT_RED, "Build failed: see Console");
        } else if self.relaunch == Some(Relaunch::Waiting) && self.mode != PlayMode::Edit {
            ui.weak("Scripts ready: Stop to reload");
        } else if self.needs_build() && !self.cargo {
            ui.colored_label(
                egui::Color32::LIGHT_RED,
                "This project has code, but Rust isn't installed",
            );
        }

        // Reload once it's safe: save first, then start the new editor.
        match self.relaunch {
            Some(Relaunch::Waiting) if self.mode == PlayMode::Edit => {
                requests.save_scene = true;
                self.relaunch = Some(Relaunch::Saved);
            }
            Some(Relaunch::Saved) => {
                self.relaunch = None;
                if self.spawn_editor(scene_file) {
                    requests.quit = true;
                }
            }
            _ => {}
        }

        self.dialog_ui(ui.ctx());
        if let Some(dialog) = &mut self.input_dialog
            && !dialog.show(ui.ctx())
        {
            self.input_dialog = None;
        }
        if self.export_dialog_ui(ui.ctx()) {
            // Export what's in the editor: save it first (Play keeps the
            // saved scene; the editor says so if asked to save then).
            if self.mode == PlayMode::Edit {
                requests.save_scene = true;
            } else {
                log::warn!("exporting the saved scene: changes made while playing aren't in it");
            }
        }
        requests
    }

    /// The Export Game dialog. Returns whether an export started.
    fn export_dialog_ui(&mut self, ctx: &egui::Context) -> bool {
        let Some(dialog) = &mut self.export_dialog else { return false };
        let has_code = project::has_code(&self.root);
        let title = export::file_title(&self.project_name);
        let mut go = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("export game")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.heading("Export Game");
            ui.add_space(6.0);
            ui.label("Output folder");
            ui.horizontal(|ui| {
                let mut text = dialog.out_dir.display().to_string();
                if ui.add(egui::TextEdit::singleline(&mut text).desired_width(320.0)).changed() {
                    dialog.out_dir = PathBuf::from(text);
                }
                if ui.button("Choose…").clicked()
                    && let Some(dir) = rfd::FileDialog::new().set_directory(&dialog.out_dir).pick_folder()
                {
                    dialog.out_dir = dir;
                }
            });
            let made = match Layout::native() {
                Layout::MacApp => format!("{title}.app"),
                Layout::Folder => format!("{title}/ (the game and assets.rpak)"),
            };
            ui.weak(format!("Makes {made} in this folder, replacing an earlier export."));
            ui.add_space(6.0);
            if !has_code {
                ui.label(
                    "This project has no code yet. A game is built from the project's own Rust \
                     crate, so exporting sets one up first (Cargo.toml, build.rs and src/, as \
                     Scripts → New Component does), then reopens the editor on it.",
                );
                ui.add_space(4.0);
            }
            ui.weak("The game is built with --release: the first time takes a few minutes. It plays the start scene (scenes/main.scene.ron, else the first scene).");
            if let Some(e) = &dialog.error {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let label = if has_code { "Export" } else { "Set up code and export" };
                if ui.button(label).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        if go {
            let out_dir = dialog.out_dir.clone();
            match self.start_export(out_dir.clone()) {
                Ok(()) => {
                    self.export_dir = Some(out_dir);
                    self.export_dialog = None;
                    return true;
                }
                Err(e) => {
                    if let Some(dialog) = &mut self.export_dialog {
                        dialog.error = Some(e);
                    }
                }
            }
        } else if cancel {
            self.export_dialog = None;
        }
        false
    }

    fn spawn_editor(&self, scene_file: Option<&Path>) -> bool {
        let Some(name) = self.crate_name() else { return false };
        let exe = project::editor_path(&self.root, &name);
        let mut command = Command::new(&exe);
        command.arg(&self.root);
        if let Some(scene) = scene_file {
            command.arg("--scene").arg(scene);
        }
        match command.spawn() {
            Ok(_) => true,
            Err(e) => {
                log::error!("couldn't start {}: {e}", exe.display());
                false
            }
        }
    }

    fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.dialog else { return };
        let mut create = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("new component")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("New Component");
            ui.add_space(6.0);
            ui.label("Name");
            let field = ui.add(
                egui::TextEdit::singleline(&mut dialog.name)
                    .hint_text(dialog.template.suggested_name())
                    .desired_width(f32::INFINITY),
            );
            if dialog.focus {
                field.request_focus();
                dialog.focus = false;
            }
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                create = true;
            }
            let name = effective_name(&dialog.name, dialog.template);
            if let Some((ty, file)) = project::component_names(&name) {
                ui.weak(format!("struct {ty} in src/components/{file}.rs"));
            }
            ui.add_space(6.0);
            ui.label("Start from");
            for template in Template::ALL {
                ui.radio_value(&mut dialog.template, template, template.label())
                    .on_hover_text(template.description());
            }
            ui.weak(dialog.template.description());
            if let Some(e) = &dialog.error {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ok = project::component_names(&effective_name(&dialog.name, dialog.template))
                    .is_some();
                if ui.add_enabled(ok, egui::Button::new("Create and open")).clicked() {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
            ui.add_space(4.0);
            ui.weak("It opens in your code editor; the editor reloads with it once it builds.");
        });
        if modal.should_close() {
            cancel = true;
        }
        if create {
            self.new_component();
        } else if cancel {
            self.dialog = None;
        }
    }
}

/// The newest modification time among the project's code files.
/// Windows won't let cargo replace a running program's file, but it will
/// let it be renamed. When `target` is this editor, it's moved to
/// `<name>.old-<pid>.exe` (cleared up by a later build, once nothing runs
/// it) so the build can write a new one. Returns (moved to, moved from).
fn move_running_editor_aside(target: &Path) -> Option<(PathBuf, PathBuf)> {
    if !cfg!(windows) {
        return None;
    }
    let dir = target.parent()?;
    let stem = target.file_stem()?.to_string_lossy().into_owned();
    // Old ones from earlier reloads: their editors have closed by now
    // (one still running stays locked and is left for next time).
    let prefix = format!("{stem}.old-");
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let running = std::env::current_exe().ok()?.canonicalize().ok()?;
    if target.canonicalize().ok()? != running {
        return None;
    }
    let aside = dir.join(format!("{prefix}{}.exe", std::process::id()));
    match std::fs::rename(target, &aside) {
        Ok(()) => Some((aside, target.to_owned())),
        Err(e) => {
            log::warn!("couldn't move the running editor aside: {e}");
            None
        }
    }
}

fn newest_source(root: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut consider = |path: &Path| {
        if let Ok(t) = path.metadata().and_then(|m| m.modified()) {
            newest = Some(newest.map_or(t, |n| n.max(t)));
        }
    };
    consider(&root.join("Cargo.toml"));
    consider(&root.join("build.rs"));
    let mut dirs = vec![root.join("src")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                consider(&path);
            }
        }
    }
    newest
}

/// Shows a file in the system's file browser.
fn reveal(path: &Path) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("explorer").arg(format!("/select,{}", path.display())).spawn()
    } else {
        Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn()
    };
    if let Err(e) = result {
        log::warn!("couldn't show {}: {e}", path.display());
    }
}

/// The typed name, or the template's suggestion when it's left empty.
fn effective_name(typed: &str, template: Template) -> String {
    if typed.trim().is_empty() { template.suggested_name().to_owned() } else { typed.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_running_editor_is_moved_aside() {
        let dir = std::env::temp_dir().join(format!("reign-aside-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("game-editor.exe");
        let stale = dir.join("game-editor.old-1.exe");
        std::fs::write(&target, "").unwrap();
        std::fs::write(&stale, "").unwrap();
        // Not this process's exe: left where it is.
        assert_eq!(move_running_editor_aside(&target), None);
        assert!(target.exists());
        if cfg!(windows) {
            assert!(!stale.exists(), "leftovers from earlier reloads are cleared");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
