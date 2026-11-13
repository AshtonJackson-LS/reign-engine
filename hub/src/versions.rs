//! Engine versions, like Unity Hub's installs.
//!
//! The hub's own editor is one version (the hub's). Other versions are
//! downloaded from GitHub Releases (signed, like updates) into `versions/`
//! beside the hub, one folder each, and a project without code opens in the
//! editor of the version it records in `ReignProject.ron`.
//!
//! A project with code doesn't need any of that: its Cargo.toml says which
//! engine it builds against (a release's git tag, or a local checkout), and
//! its own editor is built from that.
//!
//! Downloading versions is Windows-only for now, like self-updates.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use eframe::egui;

use super::update::{REPO, curl_json, fetch_release, is_newer, parse_version};

/// Where downloaded versions go: `versions/<version>/` beside the hub.
pub fn versions_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("versions"))
}

/// Versions downloaded into [`versions_dir`], newest first.
pub fn installed() -> Vec<String> {
    let mut found: Vec<String> = versions_dir()
        .and_then(|dir| std::fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.path().join(super::editor_name()).is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .filter(|v| parse_version(v).is_some())
        .collect();
    sort_newest_first(&mut found);
    found
}

pub fn sort_newest_first(versions: &mut Vec<String>) {
    versions.sort_by(|a, b| {
        if is_newer(a, b) {
            std::cmp::Ordering::Less
        } else if is_newer(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    versions.dedup();
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
    cfg!(windows)
}

enum Message {
    Releases(Result<Vec<String>, String>),
    Installed(String, Result<(), String>),
}

/// The release list (fetched when first asked for) and downloads.
pub struct Versions {
    /// Release versions on GitHub, newest first; `None` until fetched.
    pub releases: Option<Result<Vec<String>, String>>,
    /// A version being downloaded.
    pub installing: Option<String>,
    rx: Option<Receiver<Message>>,
}

impl Versions {
    pub fn new() -> Self {
        Versions { releases: None, installing: None, rx: None }
    }

    /// Fetches the release list in the background, if it isn't already.
    pub fn fetch(&mut self, ctx: &egui::Context) {
        if self.releases.is_some() || self.rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Message::Releases(fetch_releases()));
            ctx.request_repaint();
        });
    }

    /// Downloads `version` into [`versions_dir`] in the background.
    pub fn install(&mut self, ctx: &egui::Context, version: &str) {
        if self.installing.is_some() || self.rx.is_some() {
            return;
        }
        self.installing = Some(version.to_owned());
        let (tx, rx) = channel();
        self.rx = Some(rx);
        let ctx = ctx.clone();
        let version = version.to_owned();
        std::thread::spawn(move || {
            let result = install_version(&version);
            let _ = tx.send(Message::Installed(version, result));
            ctx.request_repaint();
        });
    }

    /// A finished download: its version and how it went.
    pub fn poll(&mut self) -> Option<(String, Result<(), String>)> {
        let message = self.rx.as_ref()?.try_recv().ok()?;
        self.rx = None;
        match message {
            Message::Releases(result) => {
                self.releases = Some(result);
                None
            }
            Message::Installed(version, result) => {
                self.installing = None;
                Some((version, result))
            }
        }
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

/// Release versions in the repository, newest first. Pre-releases (a tag
/// like `v0.3.0-beta`) are left out.
fn fetch_releases() -> Result<Vec<String>, String> {
    let json = curl_json(&format!("https://api.github.com/repos/{REPO}/releases?per_page=100"))?;
    let mut versions = tag_names(&json);
    sort_newest_first(&mut versions);
    Ok(versions)
}

/// Every `"tag_name": "vX.Y.Z"` in GitHub's JSON, without the `v`.
fn tag_names(json: &str) -> Vec<String> {
    json.split("\"tag_name\"")
        .skip(1)
        .filter_map(|rest| {
            let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
            let tag = rest.split('"').next()?;
            let version = tag.trim_start_matches('v');
            parse_version(version).map(|_| version.to_owned())
        })
        .collect()
}

fn install_version(version: &str) -> Result<(), String> {
    if !can_download() {
        return Err("downloading engine versions is Windows-only for now".into());
    }
    let dir = versions_dir().ok_or("the hub has no folder")?.join(version);
    let (work, stage) = fetch_release(version)?;
    let result = (|| {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let editor = super::editor_name();
        std::fs::copy(stage.join(editor), dir.join(editor)).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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
