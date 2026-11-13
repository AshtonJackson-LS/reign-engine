//! Where this build's releases come from. The release workflow sets
//! `REIGN_UPDATE_REPO` to the repository it runs in, so a fork's hub looks
//! for updates in the fork and its projects fetch the engine from it.

/// The GitHub repository (`owner/name`); unset or empty, the official one.
pub const REPO: &str = match option_env!("REIGN_UPDATE_REPO") {
    Some(repo) if !repo.is_empty() => repo,
    _ => "AshtonJackson-LS/reign-engine",
};

/// This build's version, the same in every crate of the workspace.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The repository's address, for `git = "…"` in a Cargo.toml.
pub fn git_url() -> String {
    format!("https://github.com/{REPO}")
}
