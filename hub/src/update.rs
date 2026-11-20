//! Updates from GitHub Releases. The hub checks for a newer release when it
//! starts, installs it in the background, then offers a restart. The Engine
//! version dialog uses the same release list and downloads.
//!
//! Releases are stable (`v1.2.3`) or beta (`v1.2.3-beta.4`, built from every
//! merge to `main`). A hub only takes betas when its Beta updates setting is
//! on.
//!
//! Each release has one archive per platform ([`PACKAGE`]) and a signature
//! made with the release key (`signing.rs`). Nothing is unpacked unless the
//! signature checks out. Downloads use the system's `curl` and `tar`, which
//! Windows 10+, macOS and Linux all ship.
//!
//! `REIGN_NO_UPDATE_CHECK=1` turns checking off; `REIGN_UPDATE_FAKE=1.2.3`
//! shows the arrow without a real release. `REIGN_DOWNLOAD_BASE=URL` takes
//! release files from `URL/<tag>/<file>` instead of GitHub (for testing the
//! download window against a local server; the signature check still applies,
//! so nothing unsigned is ever installed).
//!
//! Downloads report their progress (see `download.rs`): the hub shows a
//! window with a bar, the size so far and in all, and the speed.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use eframe::egui;
use reign_setup::{PROGRAMS, system_tool};

use super::download::{CANCELLED, Phase, Progress, download_file};
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
    /// Take beta releases too.
    beta: bool,
    rx: Option<Receiver<Message>>,
    /// The new hub, once an update is installed.
    new_hub: Option<PathBuf>,
    /// How the download in progress is going.
    progress: Option<Arc<Progress>>,
}

impl Updater {
    pub fn new(automatic: bool, beta: bool) -> Self {
        Updater { state: State::Idle, automatic, beta, rx: None, new_hub: None, progress: None }
    }

    /// The download in progress, while one is.
    pub fn progress(&self) -> Option<Arc<Progress>> {
        self.progress.clone()
    }

    /// Stops the download in progress. The state goes back to
    /// [`State::Available`] when it has stopped.
    pub fn cancel(&self) {
        if let Some(progress) = &self.progress {
            progress.cancel();
        }
    }

    /// Switches between stable and beta releases, checking again if
    /// nothing is on its way.
    pub fn set_beta(&mut self, beta: bool) {
        self.beta = beta;
        if self.rx.is_none() && matches!(self.state, State::Checked | State::Available(_)) {
            self.state = State::Idle;
        }
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
        let progress = Arc::new(Progress::new(format!("Reign {version}")));
        self.progress = Some(progress.clone());
        self.run(ctx, move || {
            let result = install(&version, &progress);
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
                let latest = releases.into_iter().find(|v| self.beta || !is_beta(v))?;
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
                self.progress = None;
                self.new_hub = Some(hub);
                self.state = State::Installed(version.clone());
                return Some(Ok(format!(
                    "Reign {version} is installed. Click the green arrow to restart the hub."
                )));
            }
            Message::Installed(version, Err(e)) => {
                self.progress = None;
                self.state = State::Available(version);
                if e == CANCELLED {
                    return Some(Ok("Update cancelled. The green arrow starts it again.".into()));
                }
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
    PACKAGE.is_some() && !cfg!(debug_assertions) && !in_cargo_target && !translocated(&exe)
}

/// Whether macOS is running the hub from a translocated copy: a quarantined
/// app opened where it was unzipped runs from a random read-only mount under
/// `.../AppTranslocation/...` until it is moved.
pub fn translocated(exe: &Path) -> bool {
    exe.components().any(|c| c.as_os_str() == "AppTranslocation")
}

/// Why `dir`, the hub's own folder, can't take new files, in words that
/// say what to do; `None` if it can. Writes and removes a small file to
/// find out.
pub fn cant_write_reason(dir: &Path) -> Option<String> {
    let fix = if cfg!(target_os = "macos") {
        "Quit Reign Hub, drag it into Applications in Finder, and open it from there."
    } else {
        "Move Reign Hub to a folder you can write to and open it from there."
    };
    if translocated(dir) {
        return Some(format!(
            "macOS is running Reign Hub from a read-only copy because it was opened \
             where it was downloaded. {fix}"
        ));
    }
    let probe = dir.join(format!(".reign-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(e) => {
            Some(format!("Reign Hub's folder ({}) can't be written to: {e}. {fix}", dir.display()))
        }
    }
}

/// A release version, ordered so that `1.2.3-beta.4` comes after
/// `1.2.3-beta.3` and before `1.2.3`.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    numbers: Vec<u64>,
    /// The beta number, or `u64::MAX` for a stable release.
    beta: u64,
}

/// Reads `1.2.3` or `1.2.3-beta.4`. Anything else, like `1.2.3-rc1`, gives
/// `None` and is never offered.
pub fn parse_version(v: &str) -> Option<Version> {
    let (numbers, beta) = match v.split_once("-beta.") {
        Some((numbers, beta)) => (numbers, beta.parse().ok()?),
        None => (v, u64::MAX),
    };
    let numbers = numbers.split('.').map(|part| part.parse().ok()).collect::<Option<_>>()?;
    Some(Version { numbers, beta })
}

pub fn is_beta(v: &str) -> bool {
    parse_version(v).is_some_and(|v| v.beta != u64::MAX)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

pub fn sort_newest_first(versions: &mut Vec<String>) {
    versions.sort_by_key(|v| Reverse(parse_version(v)));
    versions.dedup();
}

/// Release versions in [`REPO`], stable and beta, newest first.
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
/// returns the contents of [`PROGRAMS`], in that order. `progress` follows
/// the download and the steps after it, and can cancel it.
pub fn fetch_release(version: &str, progress: &Progress) -> Result<Vec<Vec<u8>>, String> {
    let package = PACKAGE.ok_or("there are no Reign releases for this platform")?;
    // One folder per download, not per process: two at once don't collide.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let work =
        std::env::temp_dir().join(format!("reign-release-{version}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let result = download_and_unpack(&package, version, &work, progress);
    let _ = std::fs::remove_dir_all(&work);
    result
}

fn download_and_unpack(
    package: &Package,
    version: &str,
    work: &Path,
    progress: &Progress,
) -> Result<Vec<Vec<u8>>, String> {
    let unpacked = work.join("unpacked");
    std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    let tag = format!("v{version}");
    let archive = work.join(package.asset);
    let sig = work.join(format!("{}.sig", package.asset));
    download(&tag, package.asset, &archive, Some(progress))?;
    // The signature is a few hundred bytes: nothing to show.
    download(&tag, &format!("{}.sig", package.asset), &sig, None)?;

    progress.set_phase(Phase::Verifying);
    let bytes = std::fs::read(&archive).map_err(|e| e.to_string())?;
    let signature = std::fs::read_to_string(&sig).map_err(|e| e.to_string())?;
    signing::verify(signing::PUBLIC_KEY, &tag, &bytes, &signature)
        .map_err(|e| format!("the download isn't signed by the Reign release key ({e})"))?;

    progress.set_phase(Phase::Unpacking);
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

fn download(tag: &str, name: &str, to: &Path, progress: Option<&Progress>) -> Result<(), String> {
    download_file(&release_url(tag, name), to, progress)
}

/// Where a release's file is: GitHub's, or `$REIGN_DOWNLOAD_BASE`'s.
fn release_url(tag: &str, name: &str) -> String {
    let base = std::env::var("REIGN_DOWNLOAD_BASE")
        .ok()
        .map(|b| b.trim_end_matches('/').to_owned())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| format!("https://github.com/{REPO}/releases/download"));
    format!("{base}/{tag}/{name}")
}

/// Downloads release `version` and swaps it in beside the running hub.
/// Returns the new hub's path.
fn install(version: &str, progress: &Progress) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("the hub has no folder")?;
    // Before downloading: there's no point fetching what can't be placed.
    if let Some(reason) = cant_write_reason(dir) {
        return Err(reason);
    }
    let programs = fetch_release(version, progress)?;
    progress.set_phase(Phase::Installing);
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
    fn knows_a_translocated_hub() {
        let moved = Path::new(
            "/private/var/folders/j3/x/T/AppTranslocation/C702/d/Reign Hub.app/Contents/MacOS/reign-hub",
        );
        assert!(translocated(moved));
        assert!(cant_write_reason(moved.parent().unwrap()).unwrap().contains("read-only copy"));
        assert!(!translocated(Path::new("/Applications/Reign Hub.app/Contents/MacOS/reign-hub")));
    }

    #[test]
    fn a_writable_folder_has_no_reason_and_keeps_no_probe() {
        let dir = std::env::temp_dir().join(format!("reign-hub-write-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(cant_write_reason(&dir), None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the probe is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-rc1", "0.1.0"), "only betas are pre-releases we know");
    }

    #[test]
    fn a_beta_comes_between_the_release_before_and_its_own() {
        assert!(is_newer("0.2.0-beta.1", "0.1.0"));
        assert!(is_newer("0.2.0-beta.10", "0.2.0-beta.9"));
        assert!(is_newer("0.2.0", "0.2.0-beta.10"));
        assert!(!is_newer("0.2.0-beta.3", "0.2.0"));
        assert!(is_beta("0.2.0-beta.3") && !is_beta("0.2.0"));
    }

    #[test]
    fn reads_release_tags_from_github_json() {
        let json = r#"[{"url":"x","tag_name":"v0.2.0","name":"Reign v0.2.0"},
            {"tag_name": "v0.10.1"}, {"tag_name":"v0.3.0-rc1"},
            {"tag_name":"v0.10.2-beta.3"}, {"tag_name":"v0.1.0"}]"#;
        let mut tags = tag_names(json);
        assert_eq!(tags, ["0.2.0", "0.10.1", "0.10.2-beta.3", "0.1.0"]);
        sort_newest_first(&mut tags);
        assert_eq!(tags, ["0.10.2-beta.3", "0.10.1", "0.2.0", "0.1.0"]);
    }

    /// A real download of a real, signed release from GitHub, with progress.
    /// It needs the network, so it only runs on request:
    /// `cargo test -p reign_hub -- --ignored a_real_release`. It downloads,
    /// checks the signature and unpacks; it installs nothing.
    #[test]
    #[ignore = "downloads a release from GitHub"]
    fn a_real_release_downloads_with_progress() {
        use crate::download::{Phase, Progress};
        let Some(package) = PACKAGE else { return };
        let progress = std::sync::Arc::new(Progress::new("Reign 0.2.0"));
        let watcher = {
            let progress = progress.clone();
            std::thread::spawn(move || {
                let mut seen = Vec::new();
                while seen.last().is_none_or(|s: &crate::download::Snapshot| {
                    !matches!(s.phase, Phase::Verifying | Phase::Unpacking)
                }) {
                    seen.push(progress.snapshot());
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                seen
            })
        };
        let programs = fetch_release("0.2.0", &progress).expect("a signed release downloads");
        let seen = watcher.join().unwrap();
        assert_eq!(programs.len(), PROGRAMS.len());
        assert!(programs.iter().all(|p| p.len() > 100_000), "real programs, not stubs");
        // The total is the archive's real size, known before it finished.
        let total = seen.iter().rev().find_map(|s| s.total).expect("a total was reported");
        assert!(total > 1_000_000, "{} is {total} bytes", package.asset);
        assert!(seen.iter().any(|s| s.total.is_some() && s.received < total), "total known early");
        let received: Vec<u64> = seen.iter().map(|s| s.received).collect();
        assert!(received.windows(2).all(|w| w[0] <= w[1]), "never goes backwards");
        assert_eq!(seen.last().unwrap().received, total, "the bar reaches the end");
        eprintln!(
            "{}: {} bytes, {} samples, fastest {:.1} MB/s",
            package.asset,
            total,
            seen.len(),
            seen.iter().filter_map(|s| s.speed).fold(0.0, f64::max) / 1e6
        );
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
