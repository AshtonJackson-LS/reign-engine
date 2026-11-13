//! Talking to GitHub Releases: the hub's update arrow, the release list the
//! Engine version dialog shows, and downloading a release.
//!
//! Which repository is built in ([`REPO`]), so a fork that ships its own
//! releases only hears about its own. No HTTP library: the hub runs the
//! `curl` that ships with Windows 10+, macOS and most Linux distributions.
//!
//! On Windows a release carries [`ASSET`], a zip with the hub and editor, and
//! `ASSET.sig`, its signature by the release key (`signing.rs`). A download
//! whose signature doesn't check out is never unpacked. Other platforms open
//! the release page instead of replacing themselves.
//!
//! `REIGN_NO_UPDATE_CHECK=1` turns checking off; `REIGN_UPDATE_FAKE=1.2.3`
//! shows the arrow without a real release.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;

use eframe::egui;
use reign_setup::{PROGRAMS, system_tool};

use super::signing;

pub use reign_build::release::REPO;

/// The release file a Windows install is replaced from.
pub const ASSET: &str = "reign-windows-x64.zip";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Not checked yet.
    Idle,
    /// Checked (or checking is off): nothing to offer.
    Checked,
    /// A newer release exists, like `0.2.0`.
    Available(String),
    Installing(String),
}

enum Message {
    Latest(Result<Vec<String>, String>),
    Installed(String, Result<PathBuf, String>),
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
        self.state = State::Checked;
        if std::env::var_os("REIGN_NO_UPDATE_CHECK").is_some() {
            return;
        }
        if let Ok(fake) = std::env::var("REIGN_UPDATE_FAKE") {
            self.state = State::Available(fake);
            return;
        }
        self.run(ctx, || Message::Latest(releases()));
    }

    /// Downloads the release and swaps the programs in place. The result
    /// arrives through [`Self::poll`]; on success `restart` is set.
    pub fn install(&mut self, ctx: &egui::Context) {
        let State::Available(version) = self.state.clone() else { return };
        if !cfg!(windows) {
            return super::reveal(Path::new(&format!("https://github.com/{REPO}/releases/latest")));
        }
        self.state = State::Installing(version.clone());
        self.run(ctx, move || {
            let result = install_windows(&version);
            Message::Installed(version, result)
        });
    }

    fn run(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Message + Send + 'static) {
        self.rx = Some(super::versions::background(ctx, job));
    }

    /// Takes in a finished background job. Returns an error to show, if any.
    pub fn poll(&mut self) -> Option<String> {
        let message = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        match message {
            Message::Latest(Ok(releases)) => {
                if let Some(latest) = releases.first().filter(|v| is_newer(v, super::VERSION)) {
                    self.state = State::Available(latest.clone());
                }
            }
            // Offline is normal; keep quiet and try again next launch.
            Message::Latest(Err(e)) => log::info!("update check failed: {e}"),
            Message::Installed(_, Ok(hub)) => self.restart = Some(hub),
            Message::Installed(version, Err(e)) => {
                self.state = State::Available(version);
                return Some(format!("Update failed: {e}"));
            }
        }
        None
    }
}

/// Numeric parts of `1.2.3`; a pre-release suffix like `-beta` makes it `None`
/// so a pre-release is never offered.
pub fn parse_version(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|part| part.parse().ok()).collect()
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

pub fn sort_newest_first(versions: &mut Vec<String>) {
    versions.sort_by_key(|v| Reverse(parse_version(v)));
    versions.dedup();
}

/// Release versions in [`REPO`], newest first, pre-releases left out.
pub fn releases() -> Result<Vec<String>, String> {
    let output = system_tool("curl")
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json"])
        .arg(format!("https://api.github.com/repos/{REPO}/releases?per_page=100"))
        .output()
        .map_err(|e| format!("can't run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!("couldn't reach GitHub ({})", output.status));
    }
    let mut versions = tag_names(&String::from_utf8_lossy(&output.stdout));
    sort_newest_first(&mut versions);
    Ok(versions)
}

/// Every `"tag_name": "vX.Y.Z"` in GitHub's JSON, without the `v`.
fn tag_names(json: &str) -> Vec<String> {
    json.split("\"tag_name\"")
        .skip(1)
        .filter_map(|rest| {
            let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
            let version = rest.split('"').next()?.trim_start_matches('v');
            parse_version(version).map(|_| version.to_owned())
        })
        .collect()
}

/// Downloads release `version`'s [`ASSET`] and its signature, checks the
/// signature, unpacks it and returns the programs' contents, in
/// [`PROGRAMS`] order. Nothing is unpacked unless the signature matches.
pub fn fetch_release(version: &str) -> Result<Vec<Vec<u8>>, String> {
    let work = std::env::temp_dir().join(format!("reign-release-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let result = download_and_unpack(version, &work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn download_and_unpack(version: &str, work: &Path) -> Result<Vec<Vec<u8>>, String> {
    let stage = work.join("new");
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    let tag = format!("v{version}");
    let sig = format!("{ASSET}.sig");
    for name in [ASSET, &sig] {
        let url = format!("https://github.com/{REPO}/releases/download/{tag}/{name}");
        let status = system_tool("curl")
            .args(["-fsSL", "--max-time", "600", "-o"])
            .arg(work.join(name))
            .arg(&url)
            .status()
            .map_err(|e| format!("can't run curl: {e}"))?;
        if !status.success() {
            return Err(format!("couldn't download {url}"));
        }
    }
    let zip = work.join(ASSET);
    let bytes = std::fs::read(&zip).map_err(|e| e.to_string())?;
    let signature = std::fs::read_to_string(work.join(&sig)).map_err(|e| e.to_string())?;
    signing::verify(signing::PUBLIC_KEY, &tag, &bytes, &signature)
        .map_err(|e| format!("the download isn't signed by the Reign release key ({e})"))?;
    // Windows 10+ ships bsdtar as tar.exe, which reads zips.
    let status = system_tool("tar")
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&stage)
        .status()
        .map_err(|e| format!("can't run tar: {e}"))?;
    if !status.success() {
        return Err("couldn't unpack the download".to_owned());
    }
    PROGRAMS
        .iter()
        .map(|name| {
            std::fs::read(stage.join(name)).map_err(|_| format!("the download has no {name}"))
        })
        .collect()
}

/// Downloads release `version` and swaps it in beside the running hub.
/// Returns the path of the new hub to start.
fn install_windows(version: &str) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the hub has no folder")?;
    let programs = fetch_release(version)?;
    let files: Vec<(&str, &[u8])> =
        PROGRAMS.iter().copied().zip(programs.iter().map(Vec::as_slice)).collect();
    reign_setup::place_files(dir, &files)?;
    reign_setup::set_registered_version(version);
    Ok(dir.join(PROGRAMS[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-beta", "0.1.0"));
    }

    #[test]
    fn reads_release_tags_from_github_json() {
        let json = r#"[{"url":"x","tag_name":"v0.2.0","name":"Reign v0.2.0"},
            {"tag_name": "v0.10.1"}, {"tag_name":"v0.3.0-beta"}, {"tag_name":"v0.1.0"}]"#;
        let mut tags = tag_names(json);
        assert_eq!(tags, ["0.2.0", "0.10.1", "0.1.0"]);
        sort_newest_first(&mut tags);
        assert_eq!(tags, ["0.10.1", "0.2.0", "0.1.0"]);
    }
}
