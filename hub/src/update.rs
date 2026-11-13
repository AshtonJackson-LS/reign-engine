//! Update checks for the hub: a green arrow in the top bar when the GitHub
//! repository this build comes from has a newer release.
//!
//! Which repository is baked in at build time, so a fork that ships its own
//! releases only hears about its own: build with `REIGN_UPDATE_REPO=owner/name`
//! (or change [`DEFAULT_REPO`]) and the hub follows that repository.
//!
//! No HTTP library: the hub asks the `curl` that ships with Windows 10+,
//! macOS and most Linux distributions. The latest version comes from where
//! `github.com/<repo>/releases/latest` redirects to, which needs no API key
//! and has no rate limit.
//!
//! A release is expected to carry [`ASSET`] on Windows: a zip with
//! `reign-hub.exe` and `reign-engine.exe` at its root. Other platforms open
//! the release page instead of replacing themselves.
//!
//! Set `REIGN_NO_UPDATE_CHECK=1` to turn checking off, or
//! `REIGN_UPDATE_FAKE=1.2.3` to show the arrow without a real release.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;

use super::no_console;

/// The repository official builds follow.
const DEFAULT_REPO: &str = "AshtonJackson-LS/reign-engine";

/// The repository this build follows for updates.
pub const REPO: &str = match option_env!("REIGN_UPDATE_REPO") {
    Some(repo) => repo,
    None => DEFAULT_REPO,
};

/// The release file a Windows install is replaced from.
pub const ASSET: &str = "reign-windows-x64.zip";

/// The programs an update replaces, beside the hub.
const PROGRAMS: [&str; 2] = ["reign-hub.exe", "reign-engine.exe"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Not asked yet, or checking is off.
    Idle,
    Checking,
    UpToDate,
    /// A newer release exists; the tag is its version, like `0.2.0`.
    Available(String),
    Installing(String),
    Failed(String),
}

enum Message {
    Latest(Result<Option<String>, String>),
    Installed(Result<PathBuf, String>),
}

pub struct Updater {
    pub state: State,
    rx: Option<Receiver<Message>>,
    /// Set once an install finished: the new hub to start before quitting.
    pub restart: Option<PathBuf>,
}

impl Updater {
    pub fn new() -> Self {
        Updater { state: State::Idle, rx: None, restart: None }
    }

    /// Asks GitHub for the latest release in the background (once).
    pub fn check(&mut self, ctx: &egui::Context) {
        if self.state != State::Idle {
            return;
        }
        if std::env::var_os("REIGN_NO_UPDATE_CHECK").is_some() {
            return;
        }
        if let Ok(fake) = std::env::var("REIGN_UPDATE_FAKE") {
            self.state = State::Available(fake);
            return;
        }
        self.state = State::Checking;
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Latest(latest_version()));
            ctx.request_repaint();
        });
    }

    /// Downloads the release and swaps the programs in place. The result
    /// arrives through [`Self::poll`]; on Windows `restart` is then set.
    pub fn install(&mut self, ctx: &egui::Context) {
        let State::Available(version) = self.state.clone() else { return };
        if !cfg!(windows) {
            open_release_page();
            return;
        }
        self.state = State::Installing(version.clone());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Installed(install_windows(&version)));
            ctx.request_repaint();
        });
    }

    /// Takes in a finished background job. Returns an error to show, if any.
    pub fn poll(&mut self) -> Option<String> {
        let message = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        match message {
            Message::Latest(Ok(Some(tag))) if is_newer(&tag, super::VERSION) => {
                self.state = State::Available(tag);
            }
            Message::Latest(Ok(_)) => self.state = State::UpToDate,
            Message::Latest(Err(e)) => {
                // Offline is normal; keep quiet and try again next launch.
                log::info!("update check failed: {e}");
                self.state = State::UpToDate;
            }
            Message::Installed(Ok(hub)) => {
                self.restart = Some(hub);
            }
            Message::Installed(Err(e)) => {
                self.state = State::Failed(e.clone());
                return Some(format!("Update failed: {e}"));
            }
        }
        None
    }
}

/// Removes programs an earlier update moved aside.
pub fn clean_old_files() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_owned))
    else {
        return;
    };
    for name in PROGRAMS {
        let _ = std::fs::remove_file(dir.join(format!("{name}.old")));
    }
}

pub fn open_release_page() {
    super::reveal(Path::new(&format!("https://github.com/{REPO}/releases/latest")));
}

fn curl() -> Command {
    no_console(Command::new("curl"))
}

/// The newest release's version, `None` when the repository has none.
fn latest_version() -> Result<Option<String>, String> {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let output = curl()
        .args(["-sIL", "--max-time", "15", "-o", null, "-w", "%{url_effective}"])
        .arg(format!("https://github.com/{REPO}/releases/latest"))
        .output()
        .map_err(|e| format!("can't run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!("curl exited with {}", output.status));
    }
    Ok(tag_from_url(String::from_utf8_lossy(&output.stdout).trim()))
}

/// `https://github.com/o/r/releases/tag/v0.2.0` → `0.2.0`.
fn tag_from_url(url: &str) -> Option<String> {
    let tag = url.split_once("/releases/tag/")?.1;
    let tag = tag.split(['?', '#', '/']).next()?;
    Some(tag.trim_start_matches('v').to_owned()).filter(|t| !t.is_empty())
}

/// Numeric parts of `1.2.3`; a pre-release suffix like `-beta` makes it `None`
/// so a pre-release never offers itself as an update.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|part| part.parse().ok()).collect()
}

fn is_newer(candidate: &str, current: &str) -> bool {
    let (Some(mut a), Some(mut b)) = (parse_version(candidate), parse_version(current)) else {
        return false;
    };
    let len = a.len().max(b.len());
    a.resize(len, 0);
    b.resize(len, 0);
    a > b
}

/// Downloads `ASSET` for `version`, unpacks it and swaps it in beside the
/// running hub. Returns the path of the new hub to start.
fn install_windows(version: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the hub has no folder")?.to_owned();
    let work = std::env::temp_dir().join(format!("reign-update-{version}"));
    let _ = std::fs::remove_dir_all(&work);
    let stage = work.join("new");
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;

    let zip = work.join(ASSET);
    let url = format!("https://github.com/{REPO}/releases/download/v{version}/{ASSET}");
    let status = curl()
        .args(["-fsSL", "--max-time", "600", "-o"])
        .arg(&zip)
        .arg(&url)
        .status()
        .map_err(|e| format!("can't run curl: {e}"))?;
    if !status.success() {
        return Err(format!("couldn't download {url}"));
    }
    // Windows 10+ ships bsdtar as tar.exe, which reads zips.
    let status = no_console(Command::new("tar"))
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&stage)
        .status()
        .map_err(|e| format!("can't run tar: {e}"))?;
    if !status.success() {
        return Err("couldn't unpack the download".to_owned());
    }
    for name in PROGRAMS {
        if !stage.join(name).is_file() {
            return Err(format!("the download has no {name}"));
        }
    }

    swap_in(&stage, &dir)?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(dir.join(PROGRAMS[0]))
}

/// A running `.exe` can't be overwritten but can be renamed, so each program
/// is moved to `name.old` and the new one copied in. Any failure puts the
/// old ones back.
fn swap_in(stage: &Path, dir: &Path) -> Result<(), String> {
    let mut moved: Vec<&str> = Vec::new();
    let undo = |moved: &[&str]| {
        for name in moved {
            let target = dir.join(name);
            let _ = std::fs::remove_file(&target);
            let _ = std::fs::rename(dir.join(format!("{name}.old")), target);
        }
    };
    for name in PROGRAMS {
        let target = dir.join(name);
        let old = dir.join(format!("{name}.old"));
        let _ = std::fs::remove_file(&old);
        if target.exists() {
            if let Err(e) = std::fs::rename(&target, &old) {
                undo(&moved);
                return Err(format!("can't replace {name} (is the editor still open?): {e}"));
            }
            moved.push(name);
        }
        if let Err(e) = std::fs::copy(stage.join(name), &target) {
            undo(&moved);
            return Err(format!("can't write {name}: {e}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_tag_from_the_redirect() {
        assert_eq!(
            tag_from_url("https://github.com/o/r/releases/tag/v0.2.0"),
            Some("0.2.0".to_owned())
        );
        assert_eq!(tag_from_url("https://github.com/o/r/releases"), None);
        assert_eq!(tag_from_url("https://github.com/o/r/releases/latest"), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta", "0.1.0"));
    }

    #[test]
    fn a_failed_swap_restores_the_old_programs() {
        let root = std::env::temp_dir().join(format!("reign-swap-test-{}", std::process::id()));
        let (dir, stage) = (root.join("dir"), root.join("stage"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&stage).unwrap();
        for name in PROGRAMS {
            std::fs::write(dir.join(name), "old").unwrap();
        }
        std::fs::write(stage.join(PROGRAMS[0]), "new").unwrap();
        // The second program is missing from the stage, so the copy fails.
        assert!(swap_in(&stage, &dir).is_err());
        for name in PROGRAMS {
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), "old");
        }
        std::fs::write(stage.join(PROGRAMS[1]), "new").unwrap();
        swap_in(&stage, &dir).unwrap();
        for name in PROGRAMS {
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), "new");
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
