//! Updates from GitHub Releases. The hub checks for a newer release when it
//! starts, installs it in the background, then offers a restart. The Engine
//! version dialog uses the same release list and downloads.
//!
//! Each release has one archive per platform ([`PACKAGE`]) and a signature
//! made with the release key (`signing.rs`). Nothing is unpacked unless the
//! signature checks out. Downloads use the system's `curl` and `tar`, which
//! Windows 10+, macOS and Linux all ship.
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

/// A platform's archive in a release.
pub struct Package {
    /// The file name, the same in every release.
    pub asset: &'static str,
    /// The folder inside the archive that holds [`PROGRAMS`].
    pub programs_in: &'static str,
}

/// This platform's archive, or `None` where releases aren't built.
pub const PACKAGE: Option<Package> = if cfg!(target_os = "macos") {
    Some(Package { asset: "reign-macos.zip", programs_in: "Reign Hub.app/Contents/MacOS" })
} else if cfg!(all(windows, target_arch = "x86_64")) {
    Some(Package { asset: "reign-windows-x64.zip", programs_in: "" })
} else if cfg!(all(windows, target_arch = "aarch64")) {
    Some(Package { asset: "reign-windows-arm64.zip", programs_in: "" })
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some(Package { asset: "reign-linux-x64.tar.gz", programs_in: "" })
} else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
    Some(Package { asset: "reign-linux-arm64.tar.gz", programs_in: "" })
} else {
    None
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    /// Not checked yet.
    Idle,
    /// Checked (or checking is off): nothing to offer.
    Checked,
    /// A newer release exists but isn't installed, like `0.2.0`.
    Available(String),
    Installing(String),
    /// Installed beside the running hub; a restart switches to it.
    Installed(String),
}

enum Message {
    Latest(Result<Vec<String>, String>),
    Installed(String, Result<PathBuf, String>),
}

pub struct Updater {
    pub state: State,
    /// Install a newer release as soon as it's found.
    pub automatic: bool,
    rx: Option<Receiver<Message>>,
    /// The new hub, once an update is installed.
    new_hub: Option<PathBuf>,
}

impl Updater {
    pub fn new(automatic: bool) -> Self {
        Updater { state: State::Idle, automatic, rx: None, new_hub: None }
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

    /// Downloads the available release and puts it beside the running hub.
    /// Where this platform has no release, opens the releases page instead.
    pub fn install(&mut self, ctx: &egui::Context) {
        let State::Available(version) = self.state.clone() else { return };
        if PACKAGE.is_none() {
            return super::reveal(Path::new(&format!("https://github.com/{REPO}/releases/latest")));
        }
        self.state = State::Installing(version.clone());
        self.run(ctx, move || {
            let result = install(&version);
            Message::Installed(version, result)
        });
    }

    /// Starts the installed hub. The caller then closes this one.
    pub fn restart(&self) -> Result<(), String> {
        let hub = self.new_hub.as_ref().ok_or("no update is installed")?;
        std::process::Command::new(hub)
            .spawn()
            .map(drop)
            .map_err(|e| format!("Updated, but can't restart {}: {e}", hub.display()))
    }

    fn run(&mut self, ctx: &egui::Context, job: impl FnOnce() -> Message + Send + 'static) {
        self.rx = Some(super::versions::background(ctx, job));
    }

    /// Takes in a finished background job. Returns news or an error to show.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Result<String, String>> {
        let message = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        match message {
            Message::Latest(Ok(releases)) => {
                let latest = releases.into_iter().next()?;
                if is_newer(&latest, super::VERSION) {
                    self.state = State::Available(latest);
                    if self.automatic && can_install_here() {
                        self.install(ctx);
                    }
                }
            }
            // Offline is normal; try again next launch.
            Message::Latest(Err(e)) => log::info!("update check failed: {e}"),
            Message::Installed(version, Ok(hub)) => {
                self.new_hub = Some(hub);
                self.state = State::Installed(version.clone());
                return Some(Ok(format!(
                    "Reign {version} is installed. Click the green arrow to restart the hub."
                )));
            }
            Message::Installed(version, Err(e)) => {
                self.state = State::Available(version);
                return Some(Err(format!("Update failed: {e}")));
            }
        }
        None
    }
}

/// Whether updates may replace the running hub without being asked. Not a
/// debug build or one in a Cargo `target` folder: updating those would
/// overwrite what a developer just built.
fn can_install_here() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let in_cargo_target = exe.ancestors().any(|dir| {
        dir.file_name().is_some_and(|name| name == "target")
            && dir.parent().is_some_and(|root| root.join("Cargo.toml").is_file())
    });
    PACKAGE.is_some() && !cfg!(debug_assertions) && !in_cargo_target
}

/// Numeric parts of `1.2.3`. A pre-release like `1.2.3-beta` gives `None`,
/// so it's never offered.
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

/// Release versions in [`REPO`], newest first, without pre-releases.
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

/// Downloads release `version` for this platform, checks its signature and
/// returns the contents of [`PROGRAMS`], in that order.
pub fn fetch_release(version: &str) -> Result<Vec<Vec<u8>>, String> {
    let package = PACKAGE.ok_or("there are no Reign releases for this platform")?;
    let work = std::env::temp_dir().join(format!("reign-release-{version}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let result = download_and_unpack(&package, version, &work);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn download_and_unpack(
    package: &Package,
    version: &str,
    work: &Path,
) -> Result<Vec<Vec<u8>>, String> {
    let unpacked = work.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let tag = format!("v{version}");
    let archive = work.join(package.asset);
    let sig = work.join(format!("{}.sig", package.asset));
    download(&tag, package.asset, &archive)?;
    download(&tag, &format!("{}.sig", package.asset), &sig)?;

    let bytes = std::fs::read(&archive).map_err(|e| e.to_string())?;
    let signature = std::fs::read_to_string(&sig).map_err(|e| e.to_string())?;
    signing::verify(signing::PUBLIC_KEY, &tag, &bytes, &signature)
        .map_err(|e| format!("the download isn't signed by the Reign release key ({e})"))?;

    let status = system_tool("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&unpacked)
        .status()
        .map_err(|e| format!("can't run tar: {e}"))?;
    if !status.success() {
        return Err("couldn't unpack the download".to_owned());
    }
    let programs = unpacked.join(package.programs_in);
    PROGRAMS
        .iter()
        .map(|name| {
            std::fs::read(programs.join(name)).map_err(|_| format!("the download has no {name}"))
        })
        .collect()
}

fn download(tag: &str, name: &str, to: &Path) -> Result<(), String> {
    let url = format!("https://github.com/{REPO}/releases/download/{tag}/{name}");
    let status = system_tool("curl")
        .args(["-fsSL", "--max-time", "600", "-o"])
        .arg(to)
        .arg(&url)
        .status()
        .map_err(|e| format!("can't run curl: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("couldn't download {url}")) }
}

/// Downloads release `version` and swaps it in beside the running hub.
/// Returns the new hub's path.
fn install(version: &str) -> Result<PathBuf, String> {
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

    #[test]
    fn this_platform_has_a_release_archive() {
        let supported = cfg!(any(
            target_os = "macos",
            all(
                any(windows, target_os = "linux"),
                any(target_arch = "x86_64", target_arch = "aarch64")
            )
        ));
        assert_eq!(PACKAGE.is_some(), supported);
    }
}
