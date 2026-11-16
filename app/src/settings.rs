//! A project's settings for its game, kept in [`FILE`] beside
//! `ReignProject.ron`: the start scene, the game's window and background
//! colour, its version and author, the names of its collision layers and
//! its tags. The game's name is the one in `ReignProject.ron` (see
//! [`crate::editor::Project::title`]) and its icon is `icon.png` (see
//! `icon.rs`). Project Settings edits all of them.
//!
//! They live in their own file so the hub, which rewrites
//! `ReignProject.ron` when a project changes engine version, can't drop them.

use std::collections::BTreeMap;
use std::path::Path;

use reign::render::{VIEW_HEIGHT, VIEW_WIDTH};
use serde::{Deserialize, Serialize};

/// The settings file, in the project folder.
pub const FILE: &str = "ProjectSettings.ron";

/// The largest window side, in pixels: bigger than any screen, small enough
/// to be a mistake if it's asked for.
pub const MAX_WINDOW: u32 = 16384;

/// The longest name, version or author, in characters.
pub const MAX_TEXT: usize = 100;

/// How many collision layers there are (0 to 31): one bit each in a mask.
pub const LAYERS: u8 = 32;

/// The longest layer name, in characters.
pub const MAX_LAYER_NAME: usize = 32;

/// The longest tag, in characters.
pub const MAX_TAG: usize = 32;

/// The background colour when a project sets none: a dark blue-grey.
pub const DEFAULT_BACKGROUND: [u8; 3] = [24, 24, 32];

/// What [`FILE`] holds. A field missing from the file has its default, so
/// files from older engines load.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The scene the game starts on, as an asset path such as
    /// `scenes/main.scene.ron`. Empty starts on `scenes/main.scene.ron`, else
    /// the first scene.
    pub start_scene: String,
    /// The game's window when it starts.
    pub window: Window,
    /// The game's version: one to four numbers with dots between, such as
    /// `1.0.0`. It goes in the exported game's file (Windows) and app
    /// (macOS).
    pub version: String,
    /// Who made the game: a person or a company. Empty leaves it out.
    pub author: String,
    /// The colour behind everything, as red, green and blue from 0 to 255:
    /// what shows where nothing is drawn, in the game and in the editor.
    pub background: [u8; 3],
    /// Names for collision layers, by number (0 to 31), shown in the
    /// Inspector instead of the number. Unnamed layers still work.
    pub layers: BTreeMap<u8, String>,
    /// The tags the Inspector offers for objects, in the order shown.
    /// Scripts find objects by them: `ctx.find_with_tag("Enemy")`.
    pub tags: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            start_scene: String::new(),
            window: Window::default(),
            version: "1.0.0".into(),
            author: String::new(),
            background: DEFAULT_BACKGROUND,
            layers: BTreeMap::from([(0, "Default".to_owned())]),
            tags: vec!["Player".to_owned(), "Enemy".to_owned()],
        }
    }
}

/// The game's window when it starts. F11 switches fullscreen while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Window {
    /// The inside of the window, in pixels.
    pub width: u32,
    pub height: u32,
    /// Start fullscreen instead of in a window.
    pub fullscreen: bool,
}

impl Default for Window {
    fn default() -> Self {
        Window { width: VIEW_WIDTH * 2, height: VIEW_HEIGHT * 2, fullscreen: false }
    }
}

impl Window {
    /// The smallest window: the game's view, one pixel per pixel.
    pub const MIN: (u32, u32) = (VIEW_WIDTH, VIEW_HEIGHT);
}

impl Settings {
    /// Reads the project's settings: the defaults if it has no [`FILE`], an
    /// error if the file can't be read or understood.
    pub fn load(root: &Path) -> Result<Settings, String> {
        let path = root.join(FILE);
        match std::fs::read_to_string(&path) {
            Ok(text) => ron::from_str(&text).map_err(|e| format!("{FILE}: {e}")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(format!("can't read {}: {e}", path.display())),
        }
    }

    /// [`Settings::load`], with the defaults (and a warning in the log) for a
    /// file that can't be used: for the game and the editor, which run anyway.
    pub fn read(root: &Path) -> Settings {
        Settings::load(root).unwrap_or_else(|e| {
            log::warn!("{e}: using the default settings");
            Settings::default()
        })
    }

    /// Writes the settings to the project's [`FILE`], if they are valid (see
    /// [`Settings::problems`]). Written beside the old file first, so a
    /// failure leaves it as it was.
    pub fn save(&self, root: &Path) -> Result<(), String> {
        if let Some(problem) = self.problems().into_iter().next() {
            return Err(problem);
        }
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| e.to_string())?;
        write_replacing(&root.join(FILE), &text)
    }

    /// What's wrong with the settings, in words for the person who made them:
    /// empty when they can be saved. Whether the start scene exists is up to
    /// the caller, which knows the project's scenes.
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if let Err(e) = check_version(&self.version) {
            problems.push(e);
        }
        if let Err(e) = check_text("The author", &self.author, true) {
            problems.push(e);
        }
        let (min_w, min_h) = Window::MIN;
        if !(min_w..=MAX_WINDOW).contains(&self.window.width) {
            problems.push(format!("The window's width must be {min_w} to {MAX_WINDOW} pixels."));
        }
        if !(min_h..=MAX_WINDOW).contains(&self.window.height) {
            problems.push(format!("The window's height must be {min_h} to {MAX_WINDOW} pixels."));
        }
        problems.extend(self.layer_problems());
        problems.extend(self.tag_problems());
        if self.start_scene.contains('\\') || self.start_scene.starts_with('/') {
            problems.push(
                "The start scene is an asset path with / between folders, such as \
                 scenes/main.scene.ron."
                    .into(),
            );
        }
        problems
    }

    /// What's wrong with the layer names: numbers past 31, names that are
    /// empty, too long or used twice.
    fn layer_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for (&layer, name) in &self.layers {
            if layer >= LAYERS {
                problems
                    .push(format!("There is no layer {layer}: layers are 0 to {}.", LAYERS - 1));
            }
            let what = format!("Layer {layer}'s name");
            if let Err(e) = check_text(&what, name, false) {
                problems.push(e);
            } else if name.trim().chars().count() > MAX_LAYER_NAME {
                problems.push(format!("{what} can be at most {MAX_LAYER_NAME} characters."));
            }
            let key = name.trim().to_lowercase();
            if !key.is_empty() && seen.contains(&key) {
                problems.push(format!("Two layers are called {}.", name.trim()));
            }
            seen.push(key);
        }
        problems
    }

    /// What's wrong with the tags: empty, too long or used twice.
    fn tag_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        for (i, tag) in self.tags.iter().enumerate() {
            if let Err(e) = check_tag(tag) {
                problems.push(e);
            } else if self.tags[..i].iter().any(|t| same_tag(t, tag)) {
                problems.push(format!("There are two tags called {}.", tag.trim()));
            }
        }
        problems
    }

    /// Adds `tag` (trimmed) to the end of the list. False if it was there
    /// already; an error if it can't be a tag or differs only in case from
    /// one that is, which would be easy to mix up.
    pub fn add_tag(&mut self, tag: &str) -> Result<bool, String> {
        let tag = tag.trim();
        check_tag(tag)?;
        if self.tags.iter().any(|t| t == tag) {
            return Ok(false);
        }
        if let Some(t) = self.tags.iter().find(|t| same_tag(t, tag)) {
            return Err(format!("There is already a tag called {t}."));
        }
        self.tags.push(tag.to_owned());
        Ok(true)
    }

    /// The tags, trimmed: what the Inspector shows.
    pub fn tag_names(&self) -> Vec<String> {
        self.tags.iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect()
    }

    /// The name of every layer, by number, 32 of them: empty for an unnamed
    /// one. What the editor's Inspector shows.
    pub fn layer_names(&self) -> Vec<String> {
        (0..LAYERS)
            .map(|l| self.layers.get(&l).map(|n| n.trim().to_owned()).unwrap_or_default())
            .collect()
    }

    /// The version as the four numbers Windows keeps in a program, missing
    /// ones 0. Only meaningful for a version [`check_version`] accepts.
    pub fn version_numbers(&self) -> [u16; 4] {
        let mut out = [0; 4];
        for (slot, part) in out.iter_mut().zip(self.version.trim().split('.')) {
            *slot = part.parse().unwrap_or(0);
        }
        out
    }
}

/// Checks a game version: one to four whole numbers up to 65535 with dots
/// between, such as `1.0.0` (what Windows and macOS both take).
pub fn check_version(version: &str) -> Result<(), String> {
    let parts: Vec<&str> = version.trim().split('.').collect();
    let numbers = parts
        .iter()
        .all(|p| !p.is_empty() && p.len() <= 5 && p.bytes().all(|b| b.is_ascii_digit()))
        && parts.iter().all(|p| p.parse::<u32>().is_ok_and(|n| n <= u32::from(u16::MAX)));
    if parts.len() <= 4 && numbers {
        Ok(())
    } else {
        Err("The version must be one to four numbers with dots between, such as 1.0.0 \
             (each at most 65535)."
            .into())
    }
}

/// Checks a name or an author: one line, at most [`MAX_TEXT`] characters,
/// and not empty unless `may_be_empty`. `what` starts the message.
pub fn check_text(what: &str, text: &str, may_be_empty: bool) -> Result<(), String> {
    if !may_be_empty && text.trim().is_empty() {
        return Err(format!("{what} can't be empty."));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("{what} can be at most {MAX_TEXT} characters."));
    }
    if text.chars().any(char::is_control) {
        return Err(format!("{what} must be one line of text."));
    }
    Ok(())
}

/// Whether `tag` can be a tag: one line, not empty, at most [`MAX_TAG`]
/// characters.
pub fn check_tag(tag: &str) -> Result<(), String> {
    check_text("A tag", tag, false)?;
    if tag.trim().chars().count() > MAX_TAG {
        return Err(format!("A tag can be at most {MAX_TAG} characters."));
    }
    Ok(())
}

/// Tags that differ only in case or spaces around them.
fn same_tag(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Writes `text` to `path` through a file beside it, so a failure leaves
/// the old file whole.
pub(crate) fn write_replacing(path: &Path, text: &str) -> Result<(), String> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".new");
    let temp = std::path::PathBuf::from(temp);
    std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("can't save {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("reign-game-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_project_without_the_file_has_the_defaults() {
        let root = temp("none");
        let settings = Settings::load(&root).unwrap();
        assert_eq!(settings, Settings::default());
        assert_eq!(settings.window, Window { width: 1280, height: 720, fullscreen: false });
        assert_eq!(settings.version, "1.0.0");
        assert_eq!(settings.background, DEFAULT_BACKGROUND);
        assert_eq!(settings.layer_names()[0], "Default");
        assert_eq!(settings.layer_names()[1..], vec![String::new(); 31][..]);
        assert_eq!(settings.tag_names(), ["Player", "Enemy"]);
        assert!(settings.problems().is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn settings_save_and_load_back() {
        let root = temp("round");
        let settings = Settings {
            start_scene: "scenes/title.scene.ron".into(),
            window: Window { width: 1920, height: 1080, fullscreen: true },
            version: "2.3".into(),
            author: "Small Studio".into(),
            background: [135, 206, 235],
            layers: BTreeMap::from([
                (0, "Default".into()),
                (3, "Enemies".into()),
                (31, "UI".into()),
            ]),
            tags: vec!["Player".into(), "Coin".into()],
        };
        settings.save(&root).unwrap();
        assert_eq!(Settings::load(&root).unwrap(), settings);
        assert!(!root.join(format!("{FILE}.new")).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let root = temp("partial");
        std::fs::write(root.join(FILE), "(author: \"Me\", window: (fullscreen: true))").unwrap();
        let settings = Settings::load(&root).unwrap();
        assert_eq!(settings.author, "Me");
        assert_eq!(settings.version, "1.0.0");
        assert_eq!(settings.window, Window { fullscreen: true, ..Window::default() });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_broken_file_is_an_error_to_load_and_the_defaults_to_read() {
        let root = temp("broken");
        std::fs::write(root.join(FILE), "(window: oops)").unwrap();
        assert!(Settings::load(&root).unwrap_err().starts_with(FILE));
        assert_eq!(Settings::read(&root), Settings::default());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn bad_settings_are_refused_and_say_why() {
        let root = temp("bad");
        let cases: Vec<(Settings, &str)> = vec![
            (Settings { version: "one".into(), ..Settings::default() }, "version"),
            (Settings { version: "1.2.3.4.5".into(), ..Settings::default() }, "version"),
            (Settings { version: "1..2".into(), ..Settings::default() }, "version"),
            (Settings { version: "70000".into(), ..Settings::default() }, "65535"),
            (Settings { author: "a\nb".into(), ..Settings::default() }, "one line"),
            (Settings { author: "x".repeat(101), ..Settings::default() }, "100"),
            (
                Settings {
                    window: Window { width: 100, ..Window::default() },
                    ..Settings::default()
                },
                "width",
            ),
            (
                Settings {
                    window: Window { height: 20000, ..Window::default() },
                    ..Settings::default()
                },
                "height",
            ),
            (
                Settings { start_scene: "scenes\\a.scene.ron".into(), ..Settings::default() },
                "asset path",
            ),
        ];
        for (settings, words) in cases {
            let error = settings.save(&root).unwrap_err();
            assert!(error.contains(words), "{settings:?}: {error}");
        }
        assert!(!root.join(FILE).exists(), "nothing was written");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn versions_become_four_numbers() {
        let numbers =
            |v: &str| Settings { version: v.into(), ..Settings::default() }.version_numbers();
        assert_eq!(numbers("1.0.0"), [1, 0, 0, 0]);
        assert_eq!(numbers("2"), [2, 0, 0, 0]);
        assert_eq!(numbers(" 1.2.3.4 "), [1, 2, 3, 4]);
        assert!(check_version("0.1").is_ok());
        assert!(check_version("").is_err());
        assert!(check_version("1.0-beta").is_err());
    }

    #[test]
    fn layer_names_are_checked() {
        let with = |layers: &[(u8, &str)]| Settings {
            layers: layers.iter().map(|(l, n)| (*l, (*n).to_owned())).collect(),
            ..Settings::default()
        };
        assert!(with(&[(0, "Default"), (1, "Player"), (31, "UI")]).problems().is_empty());
        assert!(with(&[]).problems().is_empty(), "no names at all is fine");
        let first = |layers: &[(u8, &str)]| with(layers).problems().join(" ");
        assert!(first(&[(32, "Too far")]).contains("no layer 32"));
        assert!(first(&[(1, "  ")]).contains("empty"));
        assert!(first(&[(1, &"x".repeat(33))]).contains("32 characters"));
        assert!(first(&[(1, "Enemy"), (2, " enemy ")]).contains("Two layers"));
        assert!(first(&[(1, "a\tb")]).contains("one line"));
        let names = with(&[(2, " Coins ")]).layer_names();
        assert_eq!((names.len(), names[2].as_str(), names[0].as_str()), (32, "Coins", ""));
    }

    #[test]
    fn tags_are_checked_and_added_once() {
        let with = |tags: &[&str]| Settings {
            tags: tags.iter().map(|t| (*t).to_owned()).collect(),
            ..Settings::default()
        };
        assert!(with(&[]).problems().is_empty(), "no tags at all is fine");
        let first = |tags: &[&str]| with(tags).problems().join(" ");
        assert!(first(&["  "]).contains("empty"));
        assert!(first(&[&"x".repeat(33)]).contains("32 characters"));
        assert!(first(&["Enemy", " enemy "]).contains("two tags"));

        let mut settings = with(&["Player"]);
        assert_eq!(settings.add_tag(" Coin "), Ok(true));
        assert_eq!(settings.add_tag("Coin"), Ok(false), "already there");
        assert!(settings.add_tag("coin").unwrap_err().contains("already a tag called Coin"));
        assert!(settings.add_tag("").is_err());
        assert_eq!(settings.tag_names(), ["Player", "Coin"]);
    }
}
