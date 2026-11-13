//! Where official builds of this engine come from.
//!
//! A fork that publishes its own releases builds with
//! `REIGN_UPDATE_REPO=owner/name`; its hub then looks for updates there and
//! its projects fetch the engine from there.

/// The GitHub repository (`owner/name`) this build's releases come from.
/// The release workflow sets it from the repository it runs in, so a fork's
/// releases follow the fork. Unset or empty: the official repository.
pub const REPO: &str = match option_env!("REIGN_UPDATE_REPO") {
    Some(repo) if !repo.is_empty() => repo,
    _ => OFFICIAL_REPO,
};

/// The repository official Reign releases come from.
pub const OFFICIAL_REPO: &str = "AshtonJackson-LS/reign-engine";

/// This build's version, the same in every crate of the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The git tag this build's release has, like `v0.2.0`.
pub fn tag() -> String {
    format!("v{VERSION}")
}

/// The repository's address, for `git = "…"` in a Cargo.toml.
pub fn git_url() -> String {
    format!("https://github.com/{REPO}")
}
