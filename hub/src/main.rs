//! Reign Hub: the launcher for Reign Engine projects.
//!
//! Lists the projects you've made or added (most recently opened first),
//! makes new ones, adds existing folders, and opens a project in the
//! `reign-engine` editor, which it finds next to its own executable (or at
//! `$REIGN_EDITOR`). The list lives in `hub.ron` in the platform's config
//! folder.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod intro;
mod snapshot;
mod theme;
mod update;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Id, Layout, Margin, Pos2, Rect, Sense, Stroke, Vec2,
};
use serde::{Deserialize, Serialize};

use intro::Intro;
use theme::{
    BG, FAINT, Face, GOOD, HAIRLINE, INK, MUTED, RED, RED_DEEP, RED_HOT, SURFACE, SURFACE_2, TEXT,
    eyebrow, text,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The file that marks a folder as a Reign project. The editor writes it too.
const PROJECT_FILE: &str = "ReignProject.ron";

// ------------------------------------------------------------------- storage

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    path: PathBuf,
    /// Seconds since the Unix epoch; 0 if never opened from the hub.
    last_opened: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    projects: Vec<Entry>,
    /// Where New Project puts projects unless told otherwise.
    default_location: Option<PathBuf>,
    /// Skip the launch intro.
    skip_intro: bool,
}

/// What `ReignProject.ron` holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ProjectFile {
    name: String,
    engine_version: String,
}

use reign_build::ide::{config_dir, home};
use reign_build::{ide, project};

fn settings_path() -> PathBuf {
    config_dir().join("hub.ron")
}

impl Settings {
    fn load() -> Settings {
        std::fs::read_to_string(settings_path())
            .ok()
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        let text = match ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default()) {
            Ok(text) => text,
            Err(e) => return log::error!("can't write the project list: {e}"),
        };
        let path = settings_path();
        let result =
            std::fs::create_dir_all(config_dir()).and_then(|_| std::fs::write(&path, text));
        if let Err(e) = result {
            log::error!("{}: {e}", path.display());
        }
    }

    fn location(&self) -> PathBuf {
        self.default_location.clone().unwrap_or_else(|| home().join("ReignProjects"))
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn read_project(path: &Path) -> Option<ProjectFile> {
    let text = std::fs::read_to_string(path.join(PROJECT_FILE)).ok()?;
    ron::from_str(&text).ok()
}

fn is_project(path: &Path) -> bool {
    path.join(PROJECT_FILE).is_file() || path.join("assets").is_dir()
}

fn project_name(path: &Path) -> String {
    read_project(path)
        .map(|p| p.name)
        .filter(|n| !n.is_empty())
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| path.display().to_string())
}

/// Makes the folder, its `assets/scenes/` and its project file. The editor
/// fills in the first scene when it opens the project.
fn create_project(path: &Path, name: &str) -> Result<(), String> {
    if path.exists() && std::fs::read_dir(path).map_or(true, |mut d| d.next().is_some()) {
        return Err(format!("{} already exists and isn't empty", path.display()));
    }
    std::fs::create_dir_all(path.join("assets/scenes")).map_err(|e| e.to_string())?;
    let file = ProjectFile { name: name.to_owned(), engine_version: VERSION.to_owned() };
    let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())?;
    std::fs::write(path.join(PROJECT_FILE), text).map_err(|e| e.to_string())
}

fn ago(secs: u64) -> String {
    if secs == 0 {
        return "never".into();
    }
    let d = now().saturating_sub(secs);
    match d {
        0..60 => "just now".into(),
        60..3600 => plural(d / 60, "minute"),
        3600..86_400 => plural(d / 3600, "hour"),
        86_400..2_592_000 => plural(d / 86_400, "day"),
        _ => plural(d / 2_592_000, "month"),
    }
}

fn plural(n: u64, unit: &str) -> String {
    if n == 1 { format!("1 {unit} ago") } else { format!("{n} {unit}s ago") }
}

/// On Windows the hub has no console, so a console program it starts would
/// open a black window of its own; this stops that.
fn no_console(mut command: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// The editor executable: `$REIGN_EDITOR`, or `reign-engine` beside the hub.
fn editor_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("REIGN_EDITOR") {
        return Some(PathBuf::from(path));
    }
    let name = if cfg!(windows) { "reign-engine.exe" } else { "reign-engine" };
    let exe = std::env::current_exe().ok()?;
    let beside = exe.parent()?.join(name);
    beside.is_file().then_some(beside)
}

fn reveal(path: &Path) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(path).spawn()
    } else if cfg!(windows) {
        Command::new("explorer").arg(path).spawn()
    } else {
        Command::new("xdg-open").arg(path).spawn()
    };
    if let Err(e) = result {
        log::error!("can't show {}: {e}", path.display());
    }
}

// ------------------------------------------------------------------------ ui

/// Space either side of the page content.
const GUTTER: f32 = 40.0;
const HERO_HEIGHT: f32 = 236.0;
const ROW_HEIGHT: f32 = 68.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Projects,
    About,
}

struct NewProject {
    name: String,
    location: String,
    /// Set the project up as a Rust crate for its own components.
    scripts: bool,
    error: Option<String>,
}

/// A project's editor being built before it opens.
struct HubBuild {
    rx: std::sync::mpsc::Receiver<BuildEvent>,
    started: std::time::Instant,
    first_error: Option<String>,
    hide_progress: bool,
}

enum BuildEvent {
    Line(String),
    Done(bool),
}

struct Hub {
    settings: Settings,
    page: Page,
    search: String,
    new_project: Option<NewProject>,
    /// Editors the hub started, by project path.
    running: HashMap<PathBuf, Child>,
    /// Projects whose editor is being built.
    building: HashMap<PathBuf, HubBuild>,
    /// Projects whose last build failed, with the first error.
    failed: HashMap<PathBuf, String>,
    /// Rust is installed.
    cargo: bool,
    /// A message for the bottom bar, and whether it's an error.
    status: Option<(String, bool)>,
    intro: Intro,
    grain: egui::TextureHandle,
    snapshots: Option<snapshot::Snapshots>,
    updater: update::Updater,
}

impl Hub {
    fn new(cc: &eframe::CreationContext) -> Self {
        theme::install(&cc.egui_ctx);
        let settings = Settings::load();
        let intro = if settings.skip_intro { Intro::finished() } else { Intro::new() };
        Hub {
            settings,
            page: Page::Projects,
            search: String::new(),
            new_project: None,
            running: HashMap::new(),
            building: HashMap::new(),
            failed: HashMap::new(),
            cargo: project::cargo_available(),
            status: None,
            intro,
            grain: theme::grain_texture(&cc.egui_ctx),
            snapshots: snapshot::Snapshots::from_env(),
            updater: update::Updater::new(),
        }
    }

    fn info(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), false));
    }

    fn error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::error!("{msg}");
        self.status = Some((msg, true));
    }

    fn remember(&mut self, path: &Path, opened: bool) {
        match self.settings.projects.iter_mut().find(|e| e.path == path) {
            Some(e) => {
                if opened {
                    e.last_opened = now();
                }
            }
            None => self
                .settings
                .projects
                .push(Entry { path: path.to_owned(), last_opened: if opened { now() } else { 0 } }),
        }
        self.settings.save();
    }

    fn open(&mut self, path: &Path) {
        if self.running.contains_key(path) {
            return self.info(format!("{} is already open", project_name(path)));
        }
        if !path.is_dir() {
            return self.error(format!("{} is missing", path.display()));
        }
        let mut command = if project::has_code(path) {
            let cached = project::read_crate_name(path).map(|n| project::editor_path(path, &n));
            let Some(editor) = cached.filter(|p| p.is_file()) else {
                return self.build(path);
            };
            let mut command = no_console(Command::new(editor));
            command.env("REIGN_CHECK_BUILD_ON_OPEN", "1");
            command
        } else if let Some(editor) = editor_path() {
            no_console(Command::new(editor))
        } else {
            return self.error(
                "Can't find the reign-engine editor next to the hub (set REIGN_EDITOR to its path)",
            );
        };
        match command.arg(path).spawn() {
            Ok(child) => {
                self.running.insert(path.to_owned(), child);
                self.remember(path, true);
                self.info(format!("Opened {}", project_name(path)));
            }
            Err(_) if project::has_code(path) => self.build(path),
            Err(e) => self.error(format!("Can't start the editor: {e}")),
        }
    }

    /// Builds a project's own editor, then opens it (see [`Self::poll_builds`]).
    fn build(&mut self, path: &Path) {
        if self.building.contains_key(path) {
            return;
        }
        if !self.cargo {
            return self
                .error("This project has code, and Rust isn't installed: get it from rustup.rs");
        }
        let Some(name) = project::read_crate_name(path) else {
            return self.error(format!("{}/Cargo.toml has no package name", path.display()));
        };
        let mut command = project::build_command(path, &name);
        command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
        let (tx, rx) = std::sync::mpsc::channel();
        match command.spawn() {
            Ok(mut child) => {
                let stderr = child.stderr.take();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    if let Some(stderr) = stderr {
                        for line in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                            let _ = tx.send(BuildEvent::Line(line));
                        }
                    }
                    let ok = child.wait().is_ok_and(|s| s.success());
                    let _ = tx.send(BuildEvent::Done(ok));
                });
                self.failed.remove(path);
                self.building.insert(
                    path.to_owned(),
                    HubBuild {
                        rx,
                        started: std::time::Instant::now(),
                        first_error: None,
                        hide_progress: false,
                    },
                );
                self.info(format!("Compiling {}…", project_name(path)));
            }
            Err(e) => self.error(format!("Can't run cargo: {e}")),
        }
    }

    /// Opens editors whose builds finished.
    fn poll_builds(&mut self) {
        let mut finished = Vec::new();
        for (path, build) in &mut self.building {
            while let Ok(event) = build.rx.try_recv() {
                match event {
                    BuildEvent::Line(line) => {
                        log::info!("{line}");
                        if build.first_error.is_none()
                            && line.contains("error")
                            && !line.starts_with("warning")
                        {
                            build.first_error = Some(line.trim().to_owned());
                        }
                    }
                    BuildEvent::Done(ok) => finished.push((path.clone(), ok)),
                }
            }
        }
        for (path, ok) in finished {
            let build = self.building.remove(&path).expect("listed");
            if !ok {
                let first = build.first_error.unwrap_or_else(|| "cargo build failed".to_owned());
                self.error(format!("{} didn't build: {first}", project_name(&path)));
                self.failed.insert(path, first);
                continue;
            }
            let Some(name) = project::read_crate_name(&path) else { continue };
            let exe = project::editor_path(&path, &name);
            match no_console(Command::new(&exe)).arg(&path).spawn() {
                Ok(child) => {
                    let secs = build.started.elapsed().as_secs();
                    self.running.insert(path.clone(), child);
                    self.remember(&path, true);
                    self.info(format!("Opened {} (built in {secs}s)", project_name(&path)));
                }
                Err(e) => self.error(format!("Can't start {}: {e}", exe.display())),
            }
        }
    }

    fn open_code(&mut self, path: &Path) {
        match ide::open_project(path, None) {
            Ok(name) => self.info(format!("Opened {} in {name}", project_name(path))),
            Err(e) => self.error(e),
        }
    }

    fn add_existing(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Add a Reign project")
            .set_directory(self.settings.location())
            .pick_folder()
        else {
            return;
        };
        if !is_project(&path) {
            return self.error(format!(
                "{} isn't a Reign project (no {PROJECT_FILE} or assets folder)",
                path.display()
            ));
        }
        self.remember(&path, false);
        self.info(format!("Added {}", project_name(&path)));
    }

    fn start_new_project(&mut self) {
        let location = self.settings.location().display().to_string();
        self.new_project = Some(NewProject {
            name: "My Project".into(),
            location,
            scripts: self.cargo,
            error: None,
        });
    }

    /// Forgets editors that have quit.
    fn reap(&mut self) {
        self.running.retain(|_, child| matches!(child.try_wait(), Ok(None)));
    }

    // ------------------------------------------------------------- top bar

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, BG);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, HAIRLINE));

        // Wordmark: REIGN, a red dot, HUB.
        let mut mark = text("REIGN", Face::Display, 30.0, TEXT, 0.6);
        mark.append(
            "HUB",
            8.0,
            egui::TextFormat {
                font_id: Face::Display.id(30.0),
                color: RED,
                extra_letter_spacing: 0.6,
                ..Default::default()
            },
        );
        let galley = painter.layout_job(mark);
        let pos = Pos2::new(rect.left() + GUTTER, rect.center().y - galley.size().y / 2.0 + 2.0);
        painter.galley(pos, galley, TEXT);

        // Page links, centred.
        let items = [(Page::Projects, "Projects"), (Page::About, "About")];
        let galleys: Vec<_> =
            items.iter().map(|(_, l)| painter.layout_job(eyebrow(l, TEXT))).collect();
        let gap = 44.0;
        let total: f32 =
            galleys.iter().map(|g| g.size().x).sum::<f32>() + gap * (items.len() - 1) as f32;
        let mut x = rect.center().x - total / 2.0;
        for ((page, label), galley) in items.iter().zip(galleys) {
            let size = galley.size();
            let hit = Rect::from_min_size(
                Pos2::new(x - 10.0, rect.top()),
                Vec2::new(size.x + 20.0, rect.height()),
            );
            let response = ui.interact(hit, Id::new(("nav", *label)), Sense::click());
            let active = self.page == *page;
            let color = if active || response.hovered() { TEXT } else { MUTED };
            let at = Pos2::new(x, rect.center().y - size.y / 2.0);
            ui.painter().galley_with_override_text_color(at, galley, color);
            if active {
                let y = rect.bottom() - 1.5;
                ui.painter().hline(x..=x + size.x, y, Stroke::new(2.0, RED));
            }
            if response.clicked() {
                self.page = *page;
            }
            x += size.x + gap;
        }

        // Version, right.
        let v = ui.painter().layout_job(text(
            format!("ENGINE {VERSION}"),
            Face::Display,
            20.0,
            FAINT,
            0.6,
        ));
        let at =
            Pos2::new(rect.right() - GUTTER - v.size().x, rect.center().y - v.size().y / 2.0 + 2.0);
        ui.painter().galley(at, v, FAINT);

        // A green arrow beside the version when a newer release is out.
        let busy = matches!(self.updater.state, update::State::Installing(_));
        if let update::State::Available(version) | update::State::Installing(version) =
            self.updater.state.clone()
        {
            let center = Pos2::new(at.x - 28.0, rect.center().y);
            let hit = Rect::from_center_size(center, Vec2::splat(30.0));
            let response = ui.interact(hit, Id::new("update"), Sense::click());
            let tip = if busy {
                format!("Installing {version}…")
            } else {
                format!("Update to {version}")
            };
            let response =
                response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand);
            let fill = if busy {
                GOOD.gamma_multiply(0.45)
            } else if response.hovered() {
                Color32::from_rgb(150, 232, 176)
            } else {
                GOOD
            };
            let painter = ui.painter();
            painter.circle_filled(center, 13.0, fill);
            let stroke = Stroke::new(2.2, BG);
            painter.line_segment(
                [center + Vec2::new(0.0, -6.0), center + Vec2::new(0.0, 5.0)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(-5.0, 0.5), center + Vec2::new(0.0, 5.5)],
                stroke,
            );
            painter
                .line_segment([center + Vec2::new(5.0, 0.5), center + Vec2::new(0.0, 5.5)], stroke);
            if response.clicked() && !busy {
                if !self.running.is_empty() {
                    self.info("Close open editors before updating");
                } else {
                    self.info(format!("Updating to {version}…"));
                    self.updater.install(ui.ctx());
                }
            }
        }
    }

    /// The grainy red band at the top of a page, with a huge title cropped
    /// by its bottom edge and a few lines of facts on the right.
    fn hero(&self, ui: &mut egui::Ui, label: &str, title: &str, facts: &[String]) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), HERO_HEIGHT), Sense::hover());
        let painter = ui.painter_at(rect);
        theme::gradient(
            &painter,
            rect,
            [
                INK,
                Color32::from_rgb(34, 10, 13),
                Color32::from_rgb(120, 20, 28),
                Color32::from_rgb(46, 11, 15),
            ],
        );
        // A hotter glow low on the right.
        let glow = Rect::from_min_max(Pos2::new(rect.center().x, rect.center().y), rect.max);
        theme::gradient(
            &painter,
            glow,
            [
                Color32::TRANSPARENT,
                RED_DEEP.gamma_multiply(0.0),
                RED.gamma_multiply(0.35),
                Color32::TRANSPARENT,
            ],
        );
        theme::grain(&painter, rect, &self.grain, 0.7);

        let inner = rect.shrink2(Vec2::new(GUTTER, 0.0));
        painter.galley(
            Pos2::new(inner.left(), rect.top() + 26.0),
            painter.layout_job(eyebrow(&format!("↳ {label}"), Color32::from_white_alpha(170))),
            TEXT,
        );

        let size = 150.0;
        let big = painter.layout_job(text(title, Face::Display, size, TEXT, 1.0));
        let at = Pos2::new(inner.left() - 4.0, rect.bottom() - big.size().y + size * 0.30);
        painter.galley_with_override_text_color(
            at,
            big,
            Color32::from_rgba_unmultiplied(248, 240, 240, 240),
        );

        let mut y = rect.bottom() - 26.0 - facts.len() as f32 * 20.0;
        for fact in facts {
            let g = painter.layout_job(text(fact, Face::Medium, 13.5, TEXT, -0.2));
            painter.galley(Pos2::new(inner.right() - g.size().x, y), g, TEXT);
            y += 20.0;
        }
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, HAIRLINE));
    }

    // -------------------------------------------------------------- projects

    fn projects_page(&mut self, ui: &mut egui::Ui) {
        let open_now = self.running.len();
        let last = self.settings.projects.iter().map(|e| e.last_opened).max().unwrap_or(0);
        let n = self.settings.projects.len();
        let facts = vec![
            format!("{n} project{}", if n == 1 { "" } else { "s" }),
            format!("{open_now} open now"),
            format!("Last opened {}", ago(last)),
        ];
        self.hero(ui, "Your workspace", "PROJECTS", &facts);

        egui::Frame::NONE.inner_margin(Margin::symmetric(GUTTER as i8, 22)).show(ui, |ui| {
            // Search on the left, actions on the right.
            ui.horizontal(|ui| {
                search_field(ui, &mut self.search);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(theme::primary("New project  +")).clicked() {
                        self.start_new_project();
                    }
                    if ui.add(theme::secondary("Add existing")).clicked() {
                        self.add_existing();
                    }
                });
            });
            ui.add_space(22.0);
            self.project_list(ui);
        });
    }

    fn project_list(&mut self, ui: &mut egui::Ui) {
        let mut projects = self.settings.projects.clone();
        projects.sort_by_key(|e| std::cmp::Reverse(e.last_opened));
        let needle = self.search.to_lowercase();
        let rows: Vec<(Entry, String)> = projects
            .into_iter()
            .map(|e| {
                let name = project_name(&e.path);
                (e, name)
            })
            .filter(|(e, name)| {
                needle.is_empty()
                    || name.to_lowercase().contains(&needle)
                    || e.path.to_string_lossy().to_lowercase().contains(&needle)
            })
            .collect();

        // Section label with a count.
        ui.horizontal(|ui| {
            ui.label(eyebrow("— Recent", MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(text(format!("{:02}", rows.len()), Face::Display, 18.0, FAINT, 0.5));
            });
        });
        let (line, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
        ui.painter().hline(line.x_range(), line.center().y, Stroke::new(1.0, HAIRLINE));

        if rows.is_empty() {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                let (title, body) = if self.settings.projects.is_empty() {
                    ("NOTHING HERE YET", "Make a project, or add a folder you already have.")
                } else {
                    ("NO MATCHES", "Nothing in your list matches that search.")
                };
                ui.label(text(title, Face::Display, 64.0, SURFACE_2, 1.0));
                ui.label(text(body, Face::Regular, 14.0, MUTED, -0.1));
                ui.add_space(14.0);
                if self.settings.projects.is_empty()
                    && ui.add(theme::primary("Create your first project")).clicked()
                {
                    self.start_new_project();
                }
            });
            return;
        }

        let mut open = None;
        let mut code = None;
        let mut remove = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (i, (entry, name)) in rows.iter().enumerate() {
                if let Some(action) = self.project_row(ui, i, entry, name) {
                    match action {
                        RowAction::Open => open = Some(entry.path.clone()),
                        RowAction::Code => code = Some(entry.path.clone()),
                        RowAction::Remove => remove = Some(entry.path.clone()),
                    }
                }
            }
        });
        if let Some(path) = open {
            self.open(&path);
        }
        if let Some(path) = code {
            self.open_code(&path);
        }
        if let Some(path) = remove {
            self.settings.projects.retain(|e| e.path != path);
            self.settings.save();
            self.info(format!(
                "Removed {} from the list. Its files are untouched.",
                project_name(&path)
            ));
        }
    }

    fn project_row(
        &self,
        ui: &mut egui::Ui,
        i: usize,
        entry: &Entry,
        name: &str,
    ) -> Option<RowAction> {
        let missing = !entry.path.is_dir();
        let running = self.running.contains_key(&entry.path);
        let building = self.building.get(&entry.path);
        let failed = self.failed.get(&entry.path);
        let openable = !missing && !running && building.is_none();
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let painter = ui.painter_at(rect.expand(1.0));
        if hovered {
            painter.rect_filled(rect, 0.0, SURFACE);
            painter.rect_filled(
                Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())),
                0.0,
                RED,
            );
        }
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, HAIRLINE));

        // Index, name, path.
        let index =
            painter.layout_job(text(format!("{:02}", i + 1), Face::Display, 24.0, TEXT, 0.5));
        let index_color = if hovered { RED } else { FAINT };
        painter.galley_with_override_text_color(
            Pos2::new(rect.left() + 18.0, rect.center().y - index.size().y / 2.0 + 2.0),
            index,
            index_color,
        );
        let left = rect.left() + 64.0;
        let right_block = 360.0;
        let max_text = (rect.width() - 64.0 - right_block).max(120.0);

        let mut title = text(name, Face::SemiBold, 17.0, TEXT, -0.4);
        title.wrap.max_width = max_text;
        title.wrap.max_rows = 1;
        title.wrap.break_anywhere = true;
        title.wrap.overflow_character = Some('…');
        let title = painter.layout_job(title);
        let title_w = title.size().x;
        let title_y = rect.center().y - 20.0;
        painter.galley(Pos2::new(left, title_y), title, TEXT);

        let compiling = building.map(|b| format!("Compiling {}s", b.started.elapsed().as_secs()));
        let badge = if running {
            Some(("● Open", GOOD))
        } else if let Some(label) = &compiling {
            Some((label.as_str(), RED_HOT))
        } else if failed.is_some() {
            Some(("Build failed", RED_HOT))
        } else if missing {
            Some(("Missing", RED_HOT))
        } else {
            None
        };
        if let Some((label, color)) = badge {
            let g = painter.layout_job(eyebrow(label, color));
            painter.galley(Pos2::new(left + title_w + 12.0, title_y + 5.0), g, color);
        }

        let mut path = text(entry.path.display().to_string(), Face::Regular, 12.0, MUTED, -0.1);
        path.wrap.max_width = max_text;
        path.wrap.max_rows = 1;
        path.wrap.break_anywhere = true;
        path.wrap.overflow_character = Some('…');
        painter.galley(Pos2::new(left, rect.center().y + 4.0), painter.layout_job(path), MUTED);

        // Right side: when, version, Open, more.
        let mut action = None;
        let controls = Rect::from_min_max(
            Pos2::new(rect.right() - right_block, rect.top()),
            Pos2::new(rect.right() - 12.0, rect.bottom()),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(controls)
                .layout(Layout::right_to_left(Align::Center))
                .id_salt(("row", &entry.path)),
        );
        child.menu_button(text("•••", Face::Bold, 12.0, MUTED, 1.0), |ui| {
            let ide_name = ide::preferred().map_or("code editor", |i| i.name);
            if ui.add_enabled(!missing, egui::Button::new(format!("Open in {ide_name}"))).clicked()
            {
                action = Some(RowAction::Code);
                ui.close();
            }
            if ui.add_enabled(!missing, egui::Button::new("Show in folder")).clicked() {
                reveal(&entry.path);
                ui.close();
            }
            if ui.button("Remove from list").clicked() {
                action = Some(RowAction::Remove);
                ui.close();
            }
        });
        let open_button = if hovered && openable {
            theme::primary("Open  ↗")
        } else {
            theme::secondary("Open  ↗")
        };
        if child.add_enabled(openable, open_button).clicked() {
            action = Some(RowAction::Open);
        }
        child.add_space(16.0);
        child.label(text(ago(entry.last_opened), Face::Medium, 12.5, MUTED, -0.1));
        if let Some(v) =
            read_project(&entry.path).map(|p| p.engine_version).filter(|v| !v.is_empty())
        {
            child.add_space(10.0);
            child.label(eyebrow(&format!("v{v}"), FAINT));
        }

        if let Some(error) = failed {
            response
                .on_hover_text(format!("{error}\n\nFix it in your code editor, then Open again."));
        } else if response.double_clicked() && openable {
            action = Some(RowAction::Open);
        }
        action
    }

    // ----------------------------------------------------------------- about

    fn about_page(&mut self, ui: &mut egui::Ui) {
        let facts = vec![
            "Deterministic 2D simulation".to_owned(),
            "Scene editor with hot reload".to_owned(),
            "Rollback netcode".to_owned(),
        ];
        self.hero(ui, "About", "REIGN ENGINE", &facts);

        egui::Frame::NONE.inner_margin(Margin::symmetric(GUTTER as i8, 28)).show(ui, |ui| {
            // Big numbers over hairlines.
            let stats = [
                ("13", "Engine crates", "One facade crate re-exports them all."),
                ("640×360", "Render target", "Pixel art, scaled up by whole numbers."),
                ("120 Hz", "Fixed tick", "The simulation steps at a fixed rate."),
            ];
            ui.columns(stats.len(), |cols| {
                for (col, (number, label, body)) in cols.iter_mut().zip(stats) {
                    col.label(text(number, Face::Bold, 52.0, TEXT, -2.2));
                    col.add_space(6.0);
                    let (line, _) = col.allocate_exact_size(
                        Vec2::new(col.available_width() - 24.0, 1.0),
                        Sense::hover(),
                    );
                    col.painter().hline(
                        line.x_range(),
                        line.center().y,
                        Stroke::new(1.0, HAIRLINE),
                    );
                    col.add_space(8.0);
                    col.label(text(label, Face::SemiBold, 15.0, TEXT, -0.3));
                    col.label(text(body, Face::Regular, 12.5, MUTED, -0.1));
                }
            });

            ui.add_space(36.0);
            ui.label(eyebrow("— Settings", MUTED));
            ui.add_space(6.0);
            let mut play = !self.settings.skip_intro;
            if ui
                .checkbox(
                    &mut play,
                    text("Play the intro on launch", Face::Medium, 14.0, TEXT, -0.2),
                )
                .changed()
            {
                self.settings.skip_intro = !play;
                self.settings.save();
            }
            ui.add_space(4.0);
            if ui.add(theme::secondary("Replay intro")).clicked() {
                self.intro = Intro::new();
            }
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                ui.label(text("Code editor", Face::Medium, 14.0, TEXT, -0.2));
                let installed = ide::installed();
                let current = ide::preferred().map(|i| i.name);
                egui::ComboBox::from_id_salt("ide")
                    .selected_text(current.unwrap_or("None found"))
                    .show_ui(ui, |ui| {
                        for choice in installed {
                            if ui
                                .selectable_label(current == Some(choice.name), choice.name)
                                .clicked()
                            {
                                let prefs = ide::Preferences { ide: Some(choice.name.to_owned()) };
                                if let Err(e) = prefs.save() {
                                    log::error!("can't save the code editor choice: {e}");
                                }
                            }
                        }
                    });
            });
            let rust = if self.cargo {
                "Rust is installed"
            } else {
                "Rust isn't installed: get it from rustup.rs to write components"
            };
            ui.label(text(
                rust,
                Face::Regular,
                12.5,
                if self.cargo { MUTED } else { RED_HOT },
                -0.1,
            ));

            ui.add_space(28.0);
            ui.label(eyebrow("— Paths", MUTED));
            ui.add_space(4.0);
            let editor = editor_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "not found".into());
            for (k, v) in
                [("Editor", editor), ("Project list", settings_path().display().to_string())]
            {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [110.0, 18.0],
                        egui::Label::new(text(k, Face::Medium, 12.5, MUTED, -0.1)),
                    );
                    ui.label(text(v, Face::Regular, 12.5, FAINT, -0.1));
                });
            }
        });
    }

    // ------------------------------------------------------------ new project

    fn new_project_dialog(&mut self, ctx: &egui::Context) {
        let Some(np) = &mut self.new_project else { return };
        let mut create = false;
        let mut cancel = false;
        let frame = egui::Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, HAIRLINE))
            .corner_radius(CornerRadius::same(18))
            .inner_margin(Margin::same(28));
        let modal = egui::Modal::new(Id::new("new project"))
            .frame(frame)
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| {
                ui.set_width(500.0);
                ui.label(eyebrow("↳ Create", RED_HOT));
                ui.label(text("NEW PROJECT", Face::Display, 56.0, TEXT, 0.8));
                ui.add_space(10.0);

                egui::Frame::new()
                    .fill(SURFACE_2)
                    .stroke(Stroke::new(1.5, RED))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::symmetric(16, 12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(text("EMPTY 2D", Face::Display, 24.0, TEXT, 0.6));
                                ui.label(text(
                                    "A scene with a camera. Drag art in from the Project panel.",
                                    Face::Regular,
                                    12.5,
                                    MUTED,
                                    -0.1,
                                ));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(text("●", Face::Bold, 14.0, RED, 0.0));
                            });
                        });
                    });

                ui.add_space(10.0);
                ui.add_enabled_ui(self.cargo, |ui| {
                    ui.checkbox(
                        &mut np.scripts,
                        text(
                            "Rust scripts: write your own components",
                            Face::Medium,
                            13.5,
                            TEXT,
                            -0.2,
                        ),
                    )
                    .on_hover_text(
                        "Makes the project a Rust crate. Its first build takes a few minutes.",
                    );
                });
                if !self.cargo {
                    ui.label(text(
                        "Install Rust (rustup.rs) to write components.",
                        Face::Regular,
                        12.0,
                        FAINT,
                        -0.1,
                    ));
                }
                ui.add_space(12.0);
                ui.label(eyebrow("Project name", MUTED));
                let name = text_field(ui, &mut np.name, "My Project", f32::INFINITY);
                if name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    create = true;
                }
                ui.add_space(10.0);
                ui.label(eyebrow("Location", MUTED));
                ui.horizontal(|ui| {
                    let browse = ui.add(theme::secondary("Browse…"));
                    text_field(ui, &mut np.location, "", f32::INFINITY);
                    if browse.clicked()
                        && let Some(dir) =
                            rfd::FileDialog::new().set_directory(&np.location).pick_folder()
                    {
                        np.location = dir.display().to_string();
                    }
                });
                let target = PathBuf::from(&np.location).join(np.name.trim());
                ui.add_space(2.0);
                let mut preview =
                    text(format!("→ {}", target.display()), Face::Regular, 12.0, FAINT, -0.1);
                preview.wrap.max_width = ui.available_width();
                preview.wrap.max_rows = 1;
                preview.wrap.break_anywhere = true;
                preview.wrap.overflow_character = Some('…');
                ui.label(preview);
                if let Some(e) = &np.error {
                    ui.add_space(4.0);
                    ui.label(text(e, Face::Medium, 12.5, RED_HOT, -0.1));
                }
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(!np.name.trim().is_empty(), theme::primary("Create project"))
                        .clicked()
                    {
                        create = true;
                    }
                    if ui.add(theme::secondary("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });
        if modal.should_close() {
            cancel = true;
        }
        if create {
            let name = np.name.trim().to_owned();
            let location = PathBuf::from(np.location.trim());
            let path = location.join(&name);
            let scripts = np.scripts;
            let made = create_project(&path, &name).and_then(|()| {
                if scripts {
                    project::init(&path, &name).map(drop).map_err(|e| e.to_string())
                } else {
                    Ok(())
                }
            });
            match made {
                Ok(()) => {
                    self.settings.default_location = Some(location);
                    self.new_project = None;
                    self.remember(&path, false);
                    self.open(&path);
                }
                Err(e) => np.error = Some(e),
            }
        } else if cancel {
            self.new_project = None;
        }
    }
}

enum RowAction {
    Open,
    Code,
    Remove,
}

/// A dark rounded text box.
fn text_field(ui: &mut egui::Ui, value: &mut String, hint: &str, width: f32) -> egui::Response {
    egui::Frame::new()
        .fill(INK)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(value)
                    .frame(egui::Frame::NONE)
                    .hint_text(text(hint, Face::Regular, 14.0, FAINT, -0.1))
                    .font(Face::Regular.id(14.0))
                    .text_color(TEXT)
                    .desired_width(width),
            )
        })
        .inner
}

/// The pill-shaped search box.
fn search_field(ui: &mut egui::Ui, value: &mut String) {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, HAIRLINE))
        .corner_radius(CornerRadius::same(200))
        .inner_margin(Margin::symmetric(16, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // A magnifier: ring and handle.
                let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
                let c = r.center() - Vec2::splat(1.5);
                ui.painter().circle_stroke(c, 5.0, Stroke::new(1.6, MUTED));
                ui.painter().line_segment(
                    [c + Vec2::splat(3.6), c + Vec2::splat(7.5)],
                    Stroke::new(1.8, MUTED),
                );
                ui.add(
                    egui::TextEdit::singleline(value)
                        .frame(egui::Frame::NONE)
                        .hint_text(text("Search projects", Face::Regular, 14.0, FAINT, -0.1))
                        .font(Face::Regular.id(14.0))
                        .text_color(TEXT)
                        .desired_width(260.0),
                );
            });
        });
}

impl eframe::App for Hub {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.reap();
        self.poll_builds();
        let ctx = ui.ctx().clone();
        if self.snapshots.is_none() || std::env::var_os("REIGN_UPDATE_FAKE").is_some() {
            self.updater.check(&ctx);
        }
        if let Some(error) = self.updater.poll() {
            self.error(error);
        }
        if let Some(hub) = self.updater.restart.take() {
            match Command::new(&hub).spawn() {
                Ok(_) => return ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.error(format!("Updated, but can't restart {}: {e}", hub.display())),
            }
        }
        if !self.building.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        for (path, build) in &mut self.building {
            if build.hide_progress {
                continue;
            }
            let name = project_name(path);
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("opening", path)),
                egui::ViewportBuilder::default()
                    .with_title(format!("{name} - Reign Engine"))
                    .with_inner_size([480.0, 180.0])
                    .with_resizable(false),
                |ui, _| {
                    if ui.input(|i| i.viewport().close_requested()) {
                        build.hide_progress = true;
                    }
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.vertical_centered(|ui| {
                            ui.add_space(24.0);
                            ui.heading(&name);
                            ui.spinner();
                            ui.label(format!(
                                "Building project scripts… {}s",
                                build.started.elapsed().as_secs()
                            ));
                            ui.weak("The editor will open when the first build finishes.");
                        });
                    });
                },
            );
        }
        if let Some(step) = self.snapshots.as_mut().and_then(|s| s.poll(&ctx)) {
            use snapshot::Step;
            match step {
                Step::Shot(_) => {}
                Step::About => self.page = Page::About,
                Step::Projects => self.page = Page::Projects,
                Step::NewProject => self.start_new_project(),
                Step::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
        // Notice editors closing even when nothing else happens.
        if !self.running.is_empty() {
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        egui::Panel::top("nav")
            .exact_size(64.0)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(GUTTER as i8, 8))
                    .stroke(Stroke::new(1.0, HAIRLINE)),
            )
            .show(ui, |ui| {
                let (msg, color) = match &self.status {
                    Some((msg, true)) => (msg.clone(), RED_HOT),
                    Some((msg, false)) => (msg.clone(), MUTED),
                    None => ("Ready".to_owned(), FAINT),
                };
                ui.horizontal(|ui| {
                    ui.label(text("●", Face::Bold, 9.0, color, 0.0));
                    ui.label(text(msg, Face::Medium, 12.0, color, -0.1));
                });
            });
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ui, |ui| {
            match self.page {
                Page::Projects => self.projects_page(ui),
                Page::About => self.about_page(ui),
            }
        });
        self.new_project_dialog(&ctx);
        self.intro.show(&ctx, &self.grain, VERSION);
    }
}

/// The window and taskbar icon (the same artwork as the .exe's icon).
fn window_icon() -> egui::IconData {
    let decoder =
        png::Decoder::new(std::io::Cursor::new(include_bytes!("../../assets/icon/reign-256.png")));
    let decoded = decoder.read_info().and_then(|mut reader| {
        let mut rgba = vec![0; reader.output_buffer_size().unwrap_or(0)];
        let info = reader.next_frame(&mut rgba)?;
        rgba.truncate(info.buffer_size());
        Ok((rgba, info.width, info.height))
    });
    match decoded {
        Ok((rgba, width, height)) => egui::IconData { rgba, width, height },
        Err(_) => egui::IconData::default(),
    }
}

fn main() -> eframe::Result {
    update::clean_old_files();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Reign Hub")
            .with_icon(window_icon())
            .with_inner_size([1080.0, 680.0])
            .with_min_inner_size([820.0, 520.0]),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native("Reign Hub", options, Box::new(|cc| Ok(Box::new(Hub::new(cc)))))
}
