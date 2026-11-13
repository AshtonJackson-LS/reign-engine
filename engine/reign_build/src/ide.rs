//! Finding and opening a code editor, and the preference both Reign Hub and
//! the editor read: `preferences.ron` in the Reign Engine config folder.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A code editor Reign knows how to open.
#[derive(Debug, PartialEq, Eq)]
pub struct Ide {
    pub name: &'static str,
    /// The macOS app bundle name, without `.app`.
    mac_app: &'static str,
    /// The command-line launcher on Windows and Linux.
    cli: &'static str,
}

pub const KNOWN: &[Ide] = &[
    Ide { name: "VS Code", mac_app: "Visual Studio Code", cli: "code" },
    Ide { name: "RustRover", mac_app: "RustRover", cli: "rustrover" },
    Ide { name: "Cursor", mac_app: "Cursor", cli: "cursor" },
    Ide { name: "Zed", mac_app: "Zed", cli: "zed" },
    Ide { name: "Sublime Text", mac_app: "Sublime Text", cli: "subl" },
];

impl Ide {
    pub fn installed(&self) -> bool {
        if cfg!(target_os = "macos") {
            let app = format!("{}.app", self.mac_app);
            [PathBuf::from("/Applications"), home().join("Applications")]
                .iter()
                .any(|dir| dir.join(&app).exists())
        } else {
            on_path(self.cli)
        }
    }

    /// Opens `root` as a project, and `file` in it if given.
    pub fn open(&self, root: &Path, file: Option<&Path>) -> Result<(), String> {
        let mut command = if cfg!(target_os = "macos") {
            let mut c = Command::new("open");
            c.arg("-a").arg(self.mac_app);
            c
        } else {
            Command::new(self.cli)
        };
        command.arg(root);
        if let Some(file) = file {
            command.arg(file);
        }
        command.spawn().map(drop).map_err(|e| format!("can't start {}: {e}", self.name))
    }
}

/// The installed editors, in [`KNOWN`] order.
pub fn installed() -> Vec<&'static Ide> {
    KNOWN.iter().filter(|i| i.installed()).collect()
}

pub fn by_name(name: &str) -> Option<&'static Ide> {
    KNOWN.iter().find(|i| i.name == name)
}

/// The chosen editor if it's installed, else the first installed one.
pub fn preferred() -> Option<&'static Ide> {
    Preferences::load()
        .ide
        .as_deref()
        .and_then(by_name)
        .filter(|i| i.installed())
        .or_else(|| installed().first().copied())
}

/// Opens the project (and a file in it) in the preferred editor, or the
/// system's file browser when none is installed.
pub fn open_project(root: &Path, file: Option<&Path>) -> Result<String, String> {
    match preferred() {
        Some(ide) => ide.open(root, file).map(|_| ide.name.to_owned()),
        None => {
            let target = file.unwrap_or(root);
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else if cfg!(windows) {
                "explorer"
            } else {
                "xdg-open"
            };
            Command::new(opener)
                .arg(target)
                .spawn()
                .map(|_| "the system default".to_owned())
                .map_err(|e| format!("no code editor found, and {opener} failed: {e}"))
        }
    }
}

/// Settings shared by the hub and the editor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    /// An [`Ide::name`], or `None` for the first installed.
    pub ide: Option<String>,
}

impl Preferences {
    pub fn path() -> PathBuf {
        config_dir().join("preferences.ron")
    }

    pub fn load() -> Preferences {
        Self::parse(&std::fs::read_to_string(Self::path()).unwrap_or_default())
    }

    /// A tiny reader for `(ide: Some("Name"))`, to stay dependency-free.
    fn parse(text: &str) -> Preferences {
        let ide = text
            .split_once("ide:")
            .and_then(|(_, rest)| rest.trim_start().strip_prefix("Some(\""))
            .and_then(|rest| rest.split_once('"'))
            .map(|(name, _)| name.to_owned());
        Preferences { ide }
    }

    fn to_text(&self) -> String {
        let ide = match &self.ide {
            Some(name) => format!("Some({name:?})"),
            None => "None".to_owned(),
        };
        format!("(\n    ide: {ide},\n)\n")
    }

    pub fn save(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir())?;
        std::fs::write(Self::path(), self.to_text())
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Where Reign Engine keeps its settings.
pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/Reign Engine")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(home).join("Reign Engine")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("reign-engine")
    }
}

fn on_path(cli: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else { return false };
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{cli}.exe"), format!("{cli}.cmd"), format!("{cli}.bat")]
    } else {
        vec![cli.to_owned()]
    };
    std::env::split_paths(&paths).any(|dir| names.iter().any(|n| dir.join(n).is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip() {
        for prefs in [Preferences { ide: Some("RustRover".into()) }, Preferences { ide: None }] {
            assert_eq!(Preferences::parse(&prefs.to_text()), prefs);
        }
        assert!(by_name("VS Code").is_some());
    }
}
