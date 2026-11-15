//! Engine versions, like Unity Hub's installs.
//!
//! The hub's own editor is one version (the hub's). Others are downloaded
//! from GitHub Releases (signed, like updates) into `versions/<version>/`
//! beside the hub, and a project without code opens in the editor of the
//! version its `ReignProject.ron` records. A project with code needs none of
//! this: its Cargo.toml names its engine, and its own editor is built from it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;

use super::download::{Phase, Progress};
use super::update::{PACKAGE, fetch_release, is_beta, parse_version, releases, sort_newest_first};

/// Where downloaded versions go: `versions/<version>/` beside the hub.
fn versions_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("versions"))
}

/// Versions downloaded into [`versions_dir`].
pub fn installed() -> Vec<String> {
    versions_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().join(super::editor_name()).is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|v| parse_version(v).is_some())
        .collect()
}

/// The editor for `version`: the hub's own for its version (or an unknown
/// one), else a downloaded one. `None` when that version isn't here.
pub fn editor_for(version: &str) -> Option<PathBuf> {
    if version.is_empty() || version == super::VERSION {
        return super::editor_path();
    }
    let exe = versions_dir()?.join(version).join(super::editor_name());
    exe.is_file().then_some(exe)
}

/// Whether this platform can download versions.
pub fn can_download() -> bool {
    PACKAGE.is_some()
}

/// The release list (fetched when first asked for) and downloads.
pub struct Versions {
    /// Release versions on GitHub, newest first; `None` until fetched.
    pub releases: Option<Result<Vec<String>, String>>,
    /// A version being downloaded.
    pub installing: Option<String>,
    /// How that download is going.
    pub progress: Option<Arc<Progress>>,
    listing: Option<Receiver<Result<Vec<String>, String>>>,
    download: Option<Receiver<Result<(), String>>>,
}

impl Versions {
    pub fn new() -> Self {
        Versions { releases: None, installing: None, progress: None, listing: None, download: None }
    }

    /// Fetches the release list in the background, if it isn't already.
    pub fn fetch(&mut self, ctx: &egui::Context) {
        if self.releases.is_none() && self.listing.is_none() {
            // Projects pin stable versions; old betas get deleted.
            let stable = || releases().map(|all| all.into_iter().filter(|v| !is_beta(v)).collect());
            self.listing = Some(background(ctx, stable));
        }
    }

    /// Downloads `version` into [`versions_dir`] in the background.
    pub fn install(&mut self, ctx: &egui::Context, version: &str) {
        if self.installing.is_none() {
            self.installing = Some(version.to_owned());
            let progress = Arc::new(Progress::new(format!("Reign engine {version}")));
            self.progress = Some(progress.clone());
            let version = version.to_owned();
            self.download = Some(background(ctx, move || install_version(&version, &progress)));
        }
    }

    /// Stops the download in progress, if there is one.
    pub fn cancel(&self) {
        if let Some(progress) = &self.progress {
            progress.cancel();
        }
    }

    /// Takes in the release list when it arrives, and returns a finished
    /// download: its version and how it went.
    pub fn poll(&mut self) -> Option<(String, Result<(), String>)> {
        if let Some(list) = self.listing.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.releases = Some(list);
            self.listing = None;
        }
        let result = self.download.as_ref()?.try_recv().ok()?;
        self.download = None;
        self.progress = None;
        Some((self.installing.take()?, result))
    }

    /// Every version worth listing, newest first: releases, downloaded
    /// ones and the hub's own.
    pub fn all(&self) -> Vec<String> {
        let mut all = installed();
        all.push(super::VERSION.to_owned());
        if let Some(Ok(releases)) = &self.releases {
            all.extend(releases.iter().cloned());
        }
        sort_newest_first(&mut all);
        all
    }
}

/// Runs `job` on another thread; its result arrives on the receiver, and the
/// hub repaints to pick it up.
pub fn background<T: Send + 'static>(
    ctx: &egui::Context,
    job: impl FnOnce() -> T + Send + 'static,
) -> Receiver<T> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(job());
        ctx.request_repaint();
    });
    rx
}

fn install_version(version: &str, progress: &Progress) -> Result<(), String> {
    let dir = versions_dir().ok_or("the hub has no folder")?.join(version);
    let programs = fetch_release(version, progress)?;
    progress.set_phase(Phase::Installing);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)
        .and_then(|()| reign_setup::write_program(&dir.join(super::editor_name()), &programs[1]))
        .map_err(|e| e.to_string())
}
