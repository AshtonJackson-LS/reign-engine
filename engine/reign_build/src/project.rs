//! A project's code: setting a Reign project up as a Rust crate, writing
//! new components from a template, and building the project's editor.
//!
//! A project with code looks like this:
//!
//! ```text
//! MyGame/
//!   ReignProject.ron
//!   Cargo.toml          the crate; its binaries are MyGame's own editor and game
//!   build.rs            reign_build::components()
//!   src/main.rs         runs the editor with your components
//!   src/game.rs         plays the game without the editor (Export Game)
//!   src/lib.rs          includes the generated registration
//!   src/components/     one file per component: just add files here
//!   assets/
//! ```

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where a project with code gets the engine's crates from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineSource {
    /// A checkout on this machine: `$REIGN_ENGINE_SOURCE`, or the one these
    /// tools were built from.
    Local(PathBuf),
    /// No checkout here (a downloaded release, built on another machine): the
    /// release's tag in the GitHub repository, which Cargo fetches itself.
    Git { url: String, tag: String },
}

impl EngineSource {
    /// A Cargo.toml dependency on the engine crate in `dir` (like
    /// `engine/reign`), as the inline table after `name = `.
    pub fn dependency(&self, dir: &str) -> String {
        match self {
            EngineSource::Local(root) => {
                let path = root.join(dir).display().to_string().replace('\\', "/");
                format!("{{ path = \"{path}\" }}")
            }
            EngineSource::Git { url, tag } => format!("{{ git = \"{url}\", tag = \"{tag}\" }}"),
        }
    }

    /// Release `version` (like `0.2.0`) from this build's GitHub repository.
    pub fn release(version: &str) -> EngineSource {
        EngineSource::Git {
            url: crate::release::git_url(),
            tag: format!("v{}", version.trim_start_matches('v')),
        }
    }

    /// The engine checkout on this machine, if there is one.
    pub fn local() -> Option<EngineSource> {
        let root = engine_root();
        root.join("engine/reign/Cargo.toml").is_file().then_some(EngineSource::Local(root))
    }

    /// The release version this source is, like `0.2.0`; `None` for a checkout.
    pub fn version(&self) -> Option<&str> {
        match self {
            EngineSource::Local(_) => None,
            EngineSource::Git { tag, .. } => Some(tag.trim_start_matches('v')),
        }
    }
}

/// Every crate in the engine's repository a project may depend on, and
/// where each lives there. A new project uses the first three; a hand-made
/// one (like Exreign) may name any of them. Kept in step with the
/// workspace's members by a test.
const ENGINE_CRATES: [(&str, &str); 15] = [
    ("reign_engine", "app"),
    ("reign", "engine/reign"),
    ("reign_build", "engine/reign_build"),
    ("reign_app", "engine/reign_app"),
    ("reign_assets", "engine/reign_assets"),
    ("reign_audio", "engine/reign_audio"),
    ("reign_behaviour", "engine/reign_behaviour"),
    ("reign_combat", "engine/reign_combat"),
    ("reign_core", "engine/reign_core"),
    ("reign_derive", "engine/reign_derive"),
    ("reign_editor", "engine/reign_editor"),
    ("reign_net", "engine/reign_net"),
    ("reign_physics", "engine/reign_physics"),
    ("reign_render", "engine/reign_render"),
    ("reign_scene", "engine/reign_scene"),
];

/// The keys of a dependency that say where it comes from; the rest
/// (`features`, `optional`, `default-features`, ...) are kept when the
/// engine moves.
const SOURCE_KEYS: [&str; 6] = ["path", "git", "tag", "branch", "rev", "version"];

/// Where a project with code gets its engine, read from its Cargo.toml.
pub fn project_engine(root: &Path) -> Option<EngineSource> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let line =
        text.lines().find(|l| l.split_once('=').is_some_and(|(k, _)| k.trim() == "reign"))?;
    let value = |key: &str| -> Option<String> {
        let rest = line.split(&format!("{key} = \"")[..]).nth(1)?;
        Some(rest.split('"').next()?.to_owned())
    };
    if let Some(path) = value("path") {
        let path = PathBuf::from(path);
        let root = path.parent().and_then(Path::parent).map(Path::to_owned).unwrap_or(path);
        return Some(EngineSource::Local(root));
    }
    Some(EngineSource::Git { url: value("git")?, tag: value("tag")? })
}

/// Points a project's engine dependencies at `source`: every engine crate
/// (see [`ENGINE_CRATES`]) in any dependency table, keeping each one's other
/// settings. Rebuilding the project's editor afterwards uses that engine.
pub fn set_project_engine(root: &Path, source: &EngineSource) -> std::io::Result<()> {
    let file = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&file)?;
    let mut changed = 0;
    let mut in_dependencies = false;
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                // [dependencies], [build-dependencies], [dev-dependencies],
                // [target.'cfg(..)'.dependencies], [workspace.dependencies].
                in_dependencies = trimmed.trim_end_matches(']').ends_with("dependencies");
                return line.to_owned();
            }
            let Some((key, value)) = line.split_once('=') else { return line.to_owned() };
            let crate_dir = ENGINE_CRATES.iter().find(|(name, _)| *name == key.trim());
            match crate_dir.filter(|_| in_dependencies) {
                Some((_, dir)) => {
                    changed += 1;
                    let end = if line.ends_with('\r') { "\r" } else { "" };
                    let value = value.trim_end_matches('\r');
                    format!("{key}= {}{end}", with_source(value, &source.dependency(dir)))
                }
                None => line.to_owned(),
            }
        })
        .collect();
    if changed == 0 {
        return Err(std::io::Error::other("Cargo.toml has no Reign engine dependencies"));
    }
    std::fs::write(file, lines.join("\n"))
}

/// A dependency's value (`"1"` or `{ path = "...", features = [...] }`, and
/// any comment after it) with its source swapped for `source` (an inline
/// table, as [`EngineSource::dependency`] writes) and every other setting
/// kept.
fn with_source(value: &str, source: &str) -> String {
    let value = value.trim();
    let new = source.trim().trim_start_matches('{').trim_end_matches('}').trim();
    let Some(body) = value.strip_prefix('{') else {
        // A bare version string: nothing else to keep.
        return source.to_owned();
    };
    // The table ends at its closing brace; anything after is a comment.
    let Some(close) = top_level_end(body) else { return source.to_owned() };
    let (inner, after) = (&body[..close], &body[close + 1..]);
    let kept = split_top_level(inner).into_iter().map(str::trim).filter(|entry| {
        let key = entry.split('=').next().unwrap_or("").trim();
        !entry.is_empty() && !SOURCE_KEYS.contains(&key)
    });
    let entries: Vec<&str> = std::iter::once(new).chain(kept).collect();
    format!("{{ {} }}{after}", entries.join(", "))
}

/// Where the inline table whose body starts `body` closes: the index of its
/// `}`, outside strings and arrays.
fn top_level_end(body: &str) -> Option<usize> {
    let (mut depth, mut quoted) = (0, false);
    for (i, c) in body.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' | '{' if !quoted => depth += 1,
            ']' if !quoted => depth -= 1,
            '}' if !quoted && depth == 0 => return Some(i),
            '}' if !quoted => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Splits an inline table's body at its top-level commas.
fn split_top_level(body: &str) -> Vec<&str> {
    let (mut parts, mut start, mut depth, mut quoted) = (Vec::new(), 0, 0, false);
    for (i, c) in body.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' | '{' if !quoted => depth += 1,
            ']' | '}' if !quoted => depth -= 1,
            ',' if !quoted && depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&body[start..]);
    parts
}

/// The engine source a new project builds against: the local checkout when
/// there is one, else this version's release on GitHub.
pub fn engine_source() -> EngineSource {
    EngineSource::local().unwrap_or_else(|| EngineSource::release(crate::release::VERSION))
}

/// The engine checkout the project builds against: `$REIGN_ENGINE_SOURCE`,
/// or the checkout these tools were built from. On a downloaded release it
/// may not exist; [`engine_source`] says what to use then.
pub fn engine_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("REIGN_ENGINE_SOURCE") {
        return PathBuf::from(dir);
    }
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    here.canonicalize().map(strip_verbatim).unwrap_or(here)
}

/// Drops the `\\?\` prefix `canonicalize` adds on Windows, which Cargo
/// can't read as a dependency path (`\\?\UNC\server\share` → `\\server\share`).
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path
    }
}

/// Whether the project has code (a `Cargo.toml`).
pub fn has_code(root: &Path) -> bool {
    root.join("Cargo.toml").is_file()
}

/// Whether `cargo` can be run.
pub fn cargo_available() -> bool {
    cargo().arg("--version").output().is_ok_and(|o| o.status.success())
}

/// Where `cargo` is. Apps opened from Finder, the Dock or a launcher don't
/// get the shell's `PATH`, so besides `PATH` this looks where rustup and
/// package managers install it.
pub fn cargo_path() -> PathBuf {
    let exe = if cfg!(windows) { "cargo.exe" } else { "cargo" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(cargo) = std::env::var_os("CARGO") {
        candidates.push(PathBuf::from(cargo));
    }
    if let Some(paths) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&paths).map(|d| d.join(exe)));
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::ide::home().join(".cargo"));
    candidates.push(cargo_home.join("bin").join(exe));
    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        candidates.push(Path::new(dir).join(exe));
    }
    candidates.into_iter().find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(exe))
}

/// A `cargo` command that works without the shell's `PATH`: cargo's own
/// folder goes first on the child's `PATH`, so rustup's `rustc` and the
/// tools a build runs are found the same way.
pub fn cargo() -> Command {
    let cargo = cargo_path();
    let mut c = Command::new(&cargo);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no black window from a GUI host
    }
    if let Some(dir) = cargo.parent().filter(|d| !d.as_os_str().is_empty()) {
        let mut paths = vec![dir.to_path_buf()];
        if let Some(existing) = std::env::var_os("PATH") {
            paths.extend(std::env::split_paths(&existing));
        }
        for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
            let extra = PathBuf::from(extra);
            if !paths.contains(&extra) {
                paths.push(extra);
            }
        }
        if let Ok(joined) = std::env::join_paths(paths) {
            c.env("PATH", joined);
        }
    }
    c
}

/// A crate name from a project name: `My Game!` → `my_game`.
pub fn crate_name(project: &str) -> String {
    let mut name = String::new();
    for c in project.trim().chars() {
        if c.is_ascii_alphanumeric() {
            name.push(c.to_ascii_lowercase());
        } else if !name.ends_with('_') && !name.is_empty() {
            name.push('_');
        }
    }
    let name = name.trim_end_matches('_').to_owned();
    match name.chars().next() {
        None => "game".to_owned(),
        Some(c) if c.is_ascii_digit() => format!("game_{name}"),
        Some(_) => name,
    }
}

/// The project editor binary's name for a crate.
pub fn editor_bin(crate_name: &str) -> String {
    format!("{crate_name}-editor")
}

/// Where `cargo build` puts the project's editor.
pub fn editor_path(root: &Path, crate_name: &str) -> PathBuf {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    target_dir(root).join("debug").join(format!("{}{exe}", editor_bin(crate_name)))
}

/// Where Cargo puts the project's build output. Usually `<project>/target`,
/// but a project inside a larger workspace, or one whose `.cargo/config.toml`
/// sets `target-dir`, builds elsewhere: Cargo is asked (`cargo metadata`),
/// once per project per run, and `<project>/target` is the answer when it
/// can't say (no Rust installed, a broken manifest).
pub fn target_dir(root: &Path) -> PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(dir) = cache.lock().ok().and_then(|c| c.get(root).cloned()) {
        return dir;
    }
    let dir = query_target_dir(root).unwrap_or_else(|| root.join("target"));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(root.to_owned(), dir.clone());
    }
    dir
}

/// The `target_directory` `cargo metadata` reports for the project.
fn query_target_dir(root: &Path) -> Option<PathBuf> {
    if !root.join("Cargo.toml").is_file() {
        return None;
    }
    let mut c = cargo();
    c.current_dir(root).args(["metadata", "--no-deps", "--format-version", "1", "--offline"]);
    // The same environment the build gets (see build_bin).
    c.env_remove("CARGO_TARGET_DIR").env_remove("CARGO_MANIFEST_DIR");
    c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    let output = c.output().ok().filter(|o| o.status.success())?;
    json_string(&String::from_utf8_lossy(&output.stdout), "target_directory").map(PathBuf::from)
}

/// A top-level string field of a JSON document, unescaped. Enough for
/// `cargo metadata`'s paths without a JSON parser in this crate.
fn json_string(json: &str, key: &str) -> Option<String> {
    let start = json.find(&format!("\"{key}\":\""))? + key.len() + 4;
    let mut out = String::new();
    let mut chars = json[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The features a project asks for in its `[package.metadata.reign]` table:
/// `editor-features` for the editor the hub and Reload build, and
/// `game-features` for the game Export Game builds. A project can keep its
/// editor (and egui) behind a feature this way and still open from the hub:
///
/// ```toml
/// [package.metadata.reign]
/// editor-features = ["editor"]
/// ```
pub fn build_features(root: &Path, key: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(root.join("Cargo.toml")) else { return Vec::new() };
    let mut in_table = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_table = line == "[package.metadata.reign]";
            continue;
        }
        let Some((k, value)) = line.split_once('=') else { continue };
        if in_table && k.trim() == key {
            let list = value.trim().trim_start_matches('[');
            let list = list.split(']').next().unwrap_or("");
            return list
                .split(',')
                .map(|f| f.trim().trim_matches('"').to_owned())
                .filter(|f| !f.is_empty())
                .collect();
        }
    }
    Vec::new()
}

/// The project game binary's name for a crate: the game without the
/// editor, which Export Game builds.
pub fn game_bin(crate_name: &str) -> String {
    format!("{crate_name}-game")
}

/// Where `cargo build --release` puts the project's game.
pub fn game_path(root: &Path, crate_name: &str) -> PathBuf {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    target_dir(root).join("release").join(format!("{}{exe}", game_bin(crate_name)))
}

/// The `[package] name` in the project's `Cargo.toml`.
pub fn read_crate_name(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package && let Some(rest) = line.strip_prefix("name") {
            let value = rest.trim_start().strip_prefix('=')?.trim().trim_matches('"');
            return Some(value.to_owned());
        }
    }
    None
}

/// The command that builds the project's editor, with its
/// `editor-features` (see [`build_features`]).
pub fn build_command(root: &Path, crate_name: &str) -> Command {
    build_bin(root, &editor_bin(crate_name), false, &build_features(root, "editor-features"))
}

/// The command that builds the project's game, optimised, for export, with
/// its `game-features` (see [`build_features`]).
pub fn game_build_command(root: &Path, crate_name: &str) -> Command {
    build_bin(root, &game_bin(crate_name), true, &build_features(root, "game-features"))
}

fn build_bin(root: &Path, bin: &str, release: bool, features: &[String]) -> Command {
    let mut c = cargo();
    c.current_dir(root).args(["build", "--bin", bin, "--message-format=short", "--color=never"]);
    if release {
        c.arg("--release");
    }
    if !features.is_empty() {
        c.arg("--features").arg(features.join(","));
    }
    // Don't inherit a parent build's settings when started from one.
    c.env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_PKG_NAME");
    c
}

/// The `[[bin]]` table for the game, as `Cargo.toml` lists it.
fn game_bin_table(crate_name: &str) -> String {
    format!("[[bin]]\nname = \"{}\"\npath = \"src/game.rs\"\n", game_bin(crate_name))
}

/// `src/game.rs`: the game without the editor.
fn game_main(project_name: &str, crate_name: &str) -> String {
    format!(
        "//! {project_name}'s game: the start scene with this project's components,\n\
         //! no editor. Scripts → Export Game in the editor builds it with --release\n\
         //! and packs the assets beside it.\n\
         \n\
         // No console window behind the game on Windows.\n\
         #![cfg_attr(not(debug_assertions), windows_subsystem = \"windows\")]\n\
         \n\
         fn main() {{\n    reign_engine::play({crate_name}::scripts());\n}}\n"
    )
}

/// Gives a project made before games could be exported its game binary:
/// the `[[bin]]` in `Cargo.toml` and `src/game.rs`. Returns whether it
/// changed anything. Never overwrites `src/game.rs` if it exists.
pub fn ensure_game_bin(root: &Path, project_name: &str) -> std::io::Result<bool> {
    let manifest = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)?;
    let name = read_crate_name(root)
        .ok_or_else(|| std::io::Error::other("Cargo.toml has no [package] name"))?;
    let mut changed = false;
    let listed = text.lines().any(|l| {
        l.trim()
            .strip_prefix("name")
            .and_then(|r| r.trim_start().strip_prefix('='))
            .is_some_and(|v| v.trim().trim_matches('"') == game_bin(&name))
    });
    if !listed {
        // Next to the other binaries, before [dependencies], else at the end.
        let table = game_bin_table(&name);
        let text = match text.find("\n[dependencies]") {
            Some(at) => format!("{}{table}\n{}", &text[..at + 1], &text[at + 1..]),
            None => format!("{}\n{table}", text.trim_end()),
        };
        std::fs::write(&manifest, text)?;
        changed = true;
    }
    let main = root.join("src").join("game.rs");
    if !main.exists() {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(main, game_main(project_name, &name))?;
        changed = true;
    }
    Ok(changed)
}

/// Makes the project a crate, if it isn't already. Returns its crate name.
/// Never overwrites a file that exists.
pub fn init(root: &Path, project_name: &str) -> std::io::Result<String> {
    if let Some(name) = read_crate_name(root) {
        return Ok(name);
    }
    let name = crate_name(project_name);
    let source = engine_source();
    let write = |rel: &str, text: String| -> std::io::Result<()> {
        let file = root.join(rel);
        if file.exists() {
            return Ok(());
        }
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(file, text)
    };

    write(
        "Cargo.toml",
        format!(
            r#"# {project_name}: a Reign Engine project. Its binaries are this project's own
# editor, with your components built in (Reign Hub builds it for you), and
# the game on its own (the editor's Scripts → Export Game builds that).
[package]
name = "{name}"
version = "0.1.0"
edition = "2024"
publish = false

[[bin]]
name = "{bin}"
path = "src/main.rs"

{game}
[dependencies]
reign_engine = {app}
reign = {reign}
serde = {{ version = "1", features = ["derive"] }}

[build-dependencies]
reign_build = {build}

[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3

# A project of its own, not part of any workspace around it.
[workspace]
"#,
            bin = editor_bin(&name),
            game = game_bin_table(&name),
            app = source.dependency("app"),
            reign = source.dependency("engine/reign"),
            build = source.dependency("engine/reign_build"),
        ),
    )?;
    write(
        "build.rs",
        "// Registers every component in src/components/. Nothing to change here.\n\
         fn main() {\n    reign_build::components();\n}\n"
            .to_owned(),
    )?;
    write(
        "src/lib.rs",
        format!(
            "//! {project_name}'s code.\n\
             //!\n\
             //! Put components in `src/components/`, one per file (Scripts → New Component\n\
             //! in the editor writes one for you). They're registered automatically: the\n\
             //! line below includes the list that `build.rs` generates.\n\
             \n\
             include!(concat!(env!(\"OUT_DIR\"), \"/reign_components.rs\"));\n"
        ),
    )?;
    write(
        "src/main.rs",
        format!(
            "//! {project_name}'s editor: the Reign editor with this project's components.\n\
             \n\
             fn main() {{\n    reign_engine::run({name}::scripts());\n}}\n"
        ),
    )?;
    write("src/game.rs", game_main(project_name, &name))?;
    std::fs::create_dir_all(root.join("src/components"))?;
    write(".gitignore", "/target\n.DS_Store\n".to_owned())?;
    write(
        ".vscode/extensions.json",
        "{\n    \"recommendations\": [\"rust-lang.rust-analyzer\"]\n}\n".to_owned(),
    )?;
    // Same toolchain and dependency versions as the engine, so the first
    // build resolves the way the engine does.
    if let EngineSource::Local(engine) = &source {
        for file in ["rust-toolchain.toml", "Cargo.lock"] {
            let from = engine.join(file);
            if from.is_file() && !root.join(file).exists() {
                std::fs::copy(from, root.join(file))?;
            }
        }
    }
    Ok(name)
}

/// The guide for AI agents that every project gets.
pub const AGENT_GUIDE: &str = "AGENTS.md";

/// [`AGENT_GUIDE`]'s text for a project: how it's laid out, the commands
/// that check and play it without a window, and how to write scenes and
/// components.
pub fn agent_guide(project_name: &str) -> String {
    let (engine, cli) = match engine_source() {
        EngineSource::Local(root) => {
            let root = root.display().to_string().replace('\\', "/");
            let cli = format!("cargo run --manifest-path \"{root}/Cargo.toml\" -p reign_engine --");
            (root, cli)
        }
        // A downloaded release: the source is on GitHub, and the editor is
        // the installed one beside the program writing this guide.
        EngineSource::Git { url, tag } => {
            let name = if cfg!(windows) { "reign-engine.exe" } else { "reign-engine" };
            let exe = std::env::current_exe()
                .ok()
                .and_then(|exe| Some(exe.parent()?.join(name)))
                .filter(|exe| exe.is_file())
                .map_or_else(|| name.to_owned(), |exe| exe.display().to_string());
            (format!("{url}/tree/{tag}"), format!("\"{}\"", exe.replace('\\', "/")))
        }
    };
    include_str!("../templates/AGENTS.md")
        .replace("{example}", include_str!("../templates/agents_example.rs"))
        .replace("{project}", project_name)
        .replace("{engine_cli}", &cli)
        .replace("{engine}", &engine)
}

/// Writes [`AGENT_GUIDE`] into a project that has none. One the project
/// already has (edited, perhaps) is left alone. Returns whether it wrote.
pub fn write_agent_guide(root: &Path, project_name: &str) -> std::io::Result<bool> {
    let file = root.join(AGENT_GUIDE);
    if file.exists() {
        return Ok(false);
    }
    std::fs::write(file, agent_guide(project_name))?;
    Ok(true)
}

/// What kind of component to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Template {
    /// Fields plus `start` and `fixed_update`: runs every tick in Play.
    Behaviour,
    /// Fields only, for other code to read.
    Data,
    /// Run and jump, driving a Rigidbody: coyote time, jump buffering and
    /// variable jump height.
    PlatformerMovement,
    /// Eight-way movement with acceleration, with or without a Rigidbody.
    TopDownMovement,
}

impl Template {
    pub const ALL: [Template; 4] = [
        Template::Behaviour,
        Template::Data,
        Template::PlatformerMovement,
        Template::TopDownMovement,
    ];

    /// The template's source, and the type name in it.
    fn source(self) -> (&'static str, &'static str) {
        match self {
            Template::Behaviour => (include_str!("../templates/behaviour.rs"), "MyBehaviour"),
            Template::Data => (include_str!("../templates/data.rs"), "MyData"),
            Template::PlatformerMovement => {
                (include_str!("../templates/platformer_movement.rs"), "PlatformerMovement")
            }
            Template::TopDownMovement => {
                (include_str!("../templates/top_down_movement.rs"), "TopDownMovement")
            }
        }
    }

    /// Its name in the New Component dialog.
    pub fn label(self) -> &'static str {
        match self {
            Template::Behaviour => "Behaviour: runs code every tick",
            Template::Data => "Data: just fields",
            Template::PlatformerMovement => "Platformer Movement: run and jump",
            Template::TopDownMovement => "Top-down Movement: eight directions",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Template::Behaviour => {
                "start() once, then fixed_update() 120 times a second while playing"
            }
            Template::Data => "Values for other code to read; no code of its own",
            Template::PlatformerMovement => {
                "Needs a Rigidbody and BoxCollider. Acceleration, coyote time, jump buffering and short hops"
            }
            Template::TopDownMovement => {
                "Smooth eight-way movement; walls stop it if it has a Rigidbody (gravity scale 0)"
            }
        }
    }

    /// A name to suggest when the dialog's name is empty.
    pub fn suggested_name(self) -> &'static str {
        match self {
            Template::Behaviour => "My Behaviour",
            Template::Data => "My Data",
            Template::PlatformerMovement => "Platformer Movement",
            Template::TopDownMovement => "Top Down Movement",
        }
    }

    /// The template's code with its type renamed to `ty`.
    pub fn render(self, ty: &str) -> String {
        let (source, name) = self.source();
        rename(source, name, ty)
    }
}

/// Replaces `from` with `to` where it's a whole word.
fn rename(source: &str, from: &str, to: &str) -> String {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find(from) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + from.len()..].chars().next();
        out.push_str(&rest[..at]);
        if before.is_some_and(word) || after.is_some_and(word) {
            out.push_str(from);
        } else {
            out.push_str(to);
        }
        rest = &rest[at + from.len()..];
    }
    out.push_str(rest);
    out
}

/// `player controller` → (`PlayerController`, `player_controller`).
pub fn component_names(name: &str) -> Option<(String, String)> {
    let words: Vec<String> = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .flat_map(split_camel)
        .collect();
    let first = words.first()?;
    if first.chars().next()?.is_ascii_digit() {
        return None;
    }
    let pascal: String = words
        .iter()
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase())
                .unwrap_or_default()
        })
        .collect();
    let snake = words.iter().map(|w| w.to_ascii_lowercase()).collect::<Vec<_>>().join("_");
    Some((pascal, snake))
}

/// `PlayerController` → `Player`, `Controller`.
fn split_camel(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = word.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
        let prev_lower =
            i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
        if c.is_ascii_uppercase() && !current.is_empty() && (prev_lower || next_lower) {
            out.push(std::mem::take(&mut current));
        }
        current.push(c);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Writes `src/components/<snake>.rs` from the template and returns its
/// path. Fails if it already exists.
pub fn new_component(root: &Path, name: &str, template: Template) -> Result<PathBuf, String> {
    let (ty, file) =
        component_names(name).ok_or_else(|| format!("{name:?} isn't a usable name"))?;
    let path = root.join("src").join("components").join(format!("{file}.rs"));
    if path.exists() {
        return Err(format!("{} already exists", path.display()));
    }
    std::fs::create_dir_all(path.parent().expect("has a parent")).map_err(|e| e.to_string())?;
    let body = template.render(&ty);
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_paths_have_no_verbatim_prefix() {
        assert_eq!(strip_verbatim(r"\\?\C:\x\y".into()), PathBuf::from(r"C:\x\y"));
        assert_eq!(strip_verbatim(r"\\?\UNC\srv\share\x".into()), PathBuf::from(r"\\srv\share\x"));
        assert_eq!(strip_verbatim("/home/x".into()), PathBuf::from("/home/x"));
        assert!(!engine_root().to_string_lossy().starts_with(r"\\?\"));
    }

    #[test]
    fn a_project_without_a_local_engine_fetches_the_release_tag() {
        // These tests run inside the checkout, so it's the local one.
        assert!(matches!(engine_source(), EngineSource::Local(_)));
        let local = EngineSource::Local(PathBuf::from(r"C:\src\reign"));
        assert_eq!(local.dependency("engine/reign"), r#"{ path = "C:/src/reign/engine/reign" }"#);
        let git = EngineSource::Git { url: "https://github.com/o/r".into(), tag: "v1.2.3".into() };
        assert_eq!(
            git.dependency("engine/reign"),
            r#"{ git = "https://github.com/o/r", tag = "v1.2.3" }"#
        );
    }

    #[test]
    fn the_agent_guide_is_filled_in_and_never_overwritten() {
        let guide = agent_guide("Space Rocks");
        assert!(guide.starts_with("# Space Rocks: a Reign Engine project"));
        assert!(guide.contains("impl Behaviour<World> for Coin"));
        assert!(!guide.contains("{example}") && !guide.contains("{engine}"));
        assert!(!guide.contains(r"\\?\"), "engine path Cargo can read");

        let root = std::env::temp_dir().join(format!("reign-guide-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(write_agent_guide(&root, "Space Rocks").unwrap());
        std::fs::write(root.join(AGENT_GUIDE), "mine").unwrap();
        assert!(!write_agent_guide(&root, "Space Rocks").unwrap());
        assert_eq!(std::fs::read_to_string(root.join(AGENT_GUIDE)).unwrap(), "mine");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rename_only_touches_whole_words() {
        assert_eq!(
            rename("MyData MyDataX xMyData MyData::new", "MyData", "Hp"),
            "Hp MyDataX xMyData Hp::new"
        );
    }

    #[test]
    fn names() {
        assert_eq!(crate_name("My Game!"), "my_game");
        assert_eq!(crate_name("2D Fun"), "game_2d_fun");
        assert_eq!(crate_name("!!"), "game");
        assert_eq!(
            component_names("player controller"),
            Some(("PlayerController".into(), "player_controller".into()))
        );
        assert_eq!(
            component_names("PlayerController"),
            Some(("PlayerController".into(), "player_controller".into()))
        );
        assert_eq!(component_names("HP bar"), Some(("HpBar".into(), "hp_bar".into())));
        assert_eq!(component_names("9lives"), None);
        assert_eq!(component_names("  "), None);
    }

    #[test]
    fn init_and_new_component_write_a_crate() {
        let root = std::env::temp_dir().join(format!("reign-init-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let name = init(&root, "Test Game").unwrap();
        assert_eq!(name, "test_game");
        assert_eq!(read_crate_name(&root).as_deref(), Some("test_game"));
        assert!(has_code(&root));
        assert_eq!(init(&root, "Other").unwrap(), "test_game", "keeps the existing crate");
        let file = new_component(&root, "Player Controller", Template::Behaviour).unwrap();
        assert!(file.ends_with("src/components/player_controller.rs"));
        let scan = crate::scan(&std::fs::read_to_string(&file).unwrap());
        assert_eq!(scan.components, ["PlayerController"]);
        assert_eq!(scan.behaviours, ["PlayerController"]);
        assert!(new_component(&root, "player_controller", Template::Data).is_err());
        for template in Template::ALL {
            let name = format!("Hero {template:?}");
            let file = new_component(&root, &name, template).unwrap();
            let text = std::fs::read_to_string(&file).unwrap();
            let (ty, _) = component_names(&name).unwrap();
            let scan = crate::scan(&text);
            assert_eq!(scan.components, std::slice::from_ref(&ty), "{template:?}");
            let (_, original) = template.source();
            assert_eq!(rename(&text, original, "Gone"), text, "{template:?} still says {original}");
            assert_eq!(scan.behaviours.is_empty(), template == Template::Data, "{template:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_projects_engine_version_can_be_read_and_changed() {
        let root =
            std::env::temp_dir().join(format!("reign-engine-version-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        init(&root, "Versions").unwrap();
        // Made inside the checkout, so it builds against the checkout.
        let local = project_engine(&root).unwrap();
        assert_eq!(Some(&local), EngineSource::local().as_ref());

        let release = EngineSource::release("0.3.1");
        set_project_engine(&root, &release).unwrap();
        assert_eq!(project_engine(&root), Some(release.clone()));
        assert_eq!(release.version(), Some("0.3.1"));
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert_eq!(manifest.matches("tag = \"v0.3.1\"").count(), 3, "{manifest}");
        assert!(manifest.contains("serde = { version = \"1\""), "other lines are kept");

        set_project_engine(&root, &local).unwrap();
        assert_eq!(project_engine(&root), Some(local));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn json_strings_are_unescaped() {
        let json =
            r#"{"packages":[],"target_directory":"C:\\Games\\My \"Game\"\\target","version":1}"#;
        assert_eq!(json_string(json, "target_directory").unwrap(), r#"C:\Games\My "Game"\target"#);
        assert_eq!(json_string(r#"{"a":"\u00e9x"}"#, "a").unwrap(), "éx");
        assert_eq!(json_string(json, "missing"), None);
    }

    #[test]
    fn build_features_come_from_package_metadata() {
        let root =
            std::env::temp_dir().join(format!("reign-build-features-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\nversion = \"0.1.0\"\n\n\
             [features]\neditor-features = [\"not-this-table\"]\n\n\
             [package.metadata.reign]\neditor-features = [\"editor\", \"debug-ui\"]\n",
        )
        .unwrap();
        assert_eq!(build_features(&root, "editor-features"), ["editor", "debug-ui"]);
        assert!(build_features(&root, "game-features").is_empty());

        let args = |c: &Command| {
            c.get_args().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>()
        };
        let editor = args(&build_command(&root, "g"));
        assert!(editor.windows(2).any(|w| w == ["--features", "editor,debug-ui"]), "{editor:?}");
        let game = args(&game_build_command(&root, "g"));
        assert!(!game.contains(&"--features".to_owned()), "{game:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Cargo is asked where the build goes: `<project>/target` normally,
    /// elsewhere when `.cargo/config.toml` says so.
    #[test]
    fn the_target_dir_is_cargos() {
        let base =
            std::env::temp_dir().join(format!("reign-target-dir-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let manifest =
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n";
        let plain = base.join("plain");
        std::fs::create_dir_all(plain.join("src")).unwrap();
        std::fs::write(plain.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(plain.join("src/main.rs"), "fn main() {}\n").unwrap();
        let moved = base.join("moved");
        std::fs::create_dir_all(moved.join("src")).unwrap();
        std::fs::create_dir_all(moved.join(".cargo")).unwrap();
        std::fs::write(moved.join("Cargo.toml"), manifest).unwrap();
        std::fs::write(moved.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(moved.join(".cargo/config.toml"), "[build]\ntarget-dir = \"out\"\n")
            .unwrap();

        let same = |a: &Path, b: &Path| {
            let canon =
                |p: &Path| strip_verbatim(p.canonicalize().unwrap_or_else(|_| p.to_owned()));
            assert_eq!(
                canon(a).to_string_lossy().to_lowercase(),
                canon(b).to_string_lossy().to_lowercase()
            );
        };
        std::fs::create_dir_all(plain.join("target")).unwrap();
        std::fs::create_dir_all(moved.join("out")).unwrap();
        same(&target_dir(&plain), &plain.join("target"));
        same(&target_dir(&moved), &moved.join("out"));
        assert!(editor_path(&moved, "t").starts_with(target_dir(&moved)));
        // No manifest: the usual place, without asking.
        let none = base.join("none");
        assert_eq!(target_dir(&none), none.join("target"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// ENGINE_CRATES lists every library crate in the engine's workspace, so a
    /// new engine crate can't be left behind when a project changes version.
    #[test]
    fn engine_crates_match_the_workspace() {
        let root = engine_root();
        let workspace = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let members: Vec<&str> = workspace
            .split("members = [")
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .unwrap()
            .split(',')
            .map(|m| m.trim().trim_matches('"'))
            .filter(|m| m.starts_with("engine/") || *m == "app")
            .collect();
        assert!(!members.is_empty());
        for dir in &members {
            let manifest = root.join(dir).join("Cargo.toml");
            let name = read_crate_name(manifest.parent().unwrap()).unwrap();
            assert!(
                ENGINE_CRATES.contains(&(name.as_str(), *dir)),
                "{name} ({dir}) is missing from ENGINE_CRATES"
            );
        }
        for (name, dir) in ENGINE_CRATES {
            assert!(members.contains(&dir), "{name} ({dir}) is not a workspace member");
        }
    }

    /// A hand-made project (like Exreign) depends on more engine crates than
    /// a new one, some as build dependencies, some with features: switching
    /// versions moves every one of them and keeps their other settings.
    #[test]
    fn every_engine_crate_moves_with_the_engine_version() {
        let root =
            std::env::temp_dir().join(format!("reign-engine-crates-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let manifest = "[package]
name = \"game\"
version = \"0.1.0\"

            [features]
editor = [\"reign/editor\"]

            [dependencies]
            reign = { path = \"../reign-engine/engine/reign\" }
            reign_editor = { path = \"../reign-engine/engine/reign_editor\", optional = true, features = [\"a\", \"b\"] }
            reign_of_terror = \"1\"
            serde = { version = \"1\", features = [\"derive\"] }

            [build-dependencies]
            reign_assets = { path = \"../reign-engine/engine/reign_assets\" }
";
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();

        let release = EngineSource::release("0.3.1");
        set_project_engine(&root, &release).unwrap();
        let text = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert_eq!(project_engine(&root), Some(release.clone()));
        assert_eq!(text.matches("tag = \"v0.3.1\"").count(), 3, "{text}");
        assert!(
            !text.contains("reign-engine/engine"),
            "no crate left on the old path:
{text}"
        );
        assert!(
            text.contains("optional = true, features = [\"a\", \"b\"] }"),
            "other settings are kept:
{text}"
        );
        assert!(
            text.contains(
                "reign_of_terror = \"1\"
"
            ),
            "not an engine crate:
{text}"
        );
        assert!(text.contains("serde = { version = \"1\", features = [\"derive\"] }"));
        assert!(text.contains("editor = [\"reign/editor\"]"), "features table untouched");

        // And back to a checkout: paths into it, settings still kept.
        let local = EngineSource::Local(PathBuf::from("/src/reign"));
        set_project_engine(&root, &local).unwrap();
        let text = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(text.contains("reign_assets = { path = \"/src/reign/engine/reign_assets\" }"));
        assert!(text.contains(
            "reign_editor = { path = \"/src/reign/engine/reign_editor\", optional = true, features = [\"a\", \"b\"] }"
        ), "{text}");
        assert!(!text.contains("tag ="), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn init_writes_the_game_binary() {
        let root =
            std::env::temp_dir().join(format!("reign-init-game-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let name = init(&root, "Space Rocks").unwrap();
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("name = \"space_rocks-editor\""));
        assert!(
            manifest.contains("[[bin]]\nname = \"space_rocks-game\"\npath = \"src/game.rs\""),
            "{manifest}"
        );
        let game = std::fs::read_to_string(root.join("src/game.rs")).unwrap();
        assert!(game.contains("reign_engine::play(space_rocks::scripts())"), "{game}");
        let exe = if cfg!(windows) { ".exe" } else { "" };
        assert!(game_path(&root, &name).ends_with(format!("target/release/space_rocks-game{exe}")));
        assert!(!ensure_game_bin(&root, "Space Rocks").unwrap(), "already there");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn older_projects_get_the_game_binary_added() {
        let root = std::env::temp_dir().join(format!("reign-old-game-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let old = "[package]\nname = \"old\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"old-editor\"\npath = \"src/main.rs\"\n\n[dependencies]\nserde = \"1\"\n\n[workspace]\n";
        std::fs::write(root.join("Cargo.toml"), old).unwrap();
        assert!(ensure_game_bin(&root, "Old").unwrap());
        let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert_eq!(
            manifest,
            "[package]\nname = \"old\"\nversion = \"0.1.0\"\n\n[[bin]]\nname = \"old-editor\"\npath = \"src/main.rs\"\n\n[[bin]]\nname = \"old-game\"\npath = \"src/game.rs\"\n\n[dependencies]\nserde = \"1\"\n\n[workspace]\n"
        );
        assert_eq!(read_crate_name(&root).as_deref(), Some("old"));
        assert!(
            std::fs::read_to_string(root.join("src/game.rs")).unwrap().contains("old::scripts()")
        );
        assert!(!ensure_game_bin(&root, "Old").unwrap(), "only once");
        let _ = std::fs::remove_dir_all(&root);
    }
}
