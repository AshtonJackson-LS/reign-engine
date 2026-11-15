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

/// Where a project with code gets its engine, read from its Cargo.toml: the
/// `reign` dependency, inline (`reign = { path = ".." }`) or as a table
/// (`[dependencies.reign]`).
pub fn project_engine(root: &Path) -> Option<EngineSource> {
    let text = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let entries = manifest_dependency(&text, "reign")?;
    let value = |key: &str| entries.iter().find(|(k, _)| k == key).map(|(_, v)| unquote(v));
    if let Some(path) = value("path") {
        let path = PathBuf::from(path);
        let root = path.parent().and_then(Path::parent).map(Path::to_owned).unwrap_or(path);
        return Some(EngineSource::Local(root));
    }
    Some(EngineSource::Git { url: value("git")?, tag: value("tag")? })
}

/// Points a project's engine dependencies at `source`: every engine crate
/// (see `ENGINE_CRATES`) in any dependency table, written inline
/// (`reign = { .. }`) or as its own table (`[dependencies.reign]`), keeping
/// each one's other settings (features, optional, ..), comments and line
/// endings. A crate inherited from a workspace (`workspace = true`) is left
/// to the workspace. Rebuilding the project's editor afterwards uses that
/// engine.
pub fn set_project_engine(root: &Path, source: &EngineSource) -> std::io::Result<()> {
    let file = root.join("Cargo.toml");
    let text = std::fs::read_to_string(&file)?;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 2);
    let mut changed = 0;
    let mut table = Table::Other;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        let (body, end) = match line.strip_suffix('\r') {
            Some(body) => (body, "\r"),
            None => (line, ""),
        };
        if let Some(header) = table_header(body) {
            table = Table::of(header);
            out.push(line.to_owned());
            let Table::Crate(name) = &table else { continue };
            let Some(dir) = engine_dir(name) else { continue };
            // The table's own lines, up to the next header.
            let start = i;
            while i < lines.len() && table_header(lines[i].trim_end_matches('\r')).is_none() {
                i += 1;
            }
            let own = &lines[start..i];
            let key_of = |l: &&str| key_value(l.trim_end_matches('\r')).map(|(k, _, _)| k);
            let inherits = own.iter().any(|l| {
                key_value(l.trim_end_matches('\r'))
                    .is_some_and(|(k, v, _)| k == "workspace" && unquote(&v) == "true")
            });
            if !inherits {
                changed += 1;
                let new = source.dependency(dir);
                for entry in inline_body(&new).map(split_top_level).unwrap_or_default() {
                    out.push(format!("{}{end}", entry.trim()));
                }
            }
            for l in own {
                let source_key = key_of(l).is_some_and(|k| SOURCE_KEYS.contains(&k.as_str()));
                if inherits || !source_key {
                    out.push((*l).to_owned());
                }
            }
            continue;
        }
        if matches!(table, Table::Dependencies)
            && let Some((key, value, eq)) = key_value(body)
            && let Some(dir) = engine_dir(&key)
            && let Some(new) = with_source(&value, &source.dependency(dir))
        {
            changed += 1;
            out.push(format!("{}= {new}{end}", &body[..eq]));
            continue;
        }
        out.push(line.to_owned());
    }
    if changed == 0 {
        return Err(std::io::Error::other("Cargo.toml has no Reign engine dependencies"));
    }
    std::fs::write(file, out.join("\n"))
}

/// The kind of Cargo.toml table a header opens.
enum Table {
    /// `[dependencies]`, `[build-dependencies]`, `[dev-dependencies]`,
    /// `[target.'cfg(..)'.dependencies]`, `[workspace.dependencies]`.
    Dependencies,
    /// One dependency as a table: `[dependencies.reign]`.
    Crate(String),
    Other,
}

impl Table {
    fn of(header: &str) -> Table {
        if header.ends_with("dependencies") {
            return Table::Dependencies;
        }
        match header.rsplit_once('.') {
            Some((tables, name)) if tables.trim().ends_with("dependencies") => {
                Table::Crate(unquote(name))
            }
            _ => Table::Other,
        }
    }
}

/// The table name a header line opens (`[dependencies] # x` →
/// `dependencies`, `[[bin]]` → `bin`), or `None` if it isn't a header.
fn table_header(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix('[')?.trim_start_matches('[');
    Some(inner.split(']').next()?.trim())
}

fn engine_dir(name: &str) -> Option<&'static str> {
    ENGINE_CRATES.iter().find(|(crate_name, _)| *crate_name == name).map(|(_, dir)| *dir)
}

/// The engine dependency `name` in a manifest's dependency tables, as its
/// key/value entries (`path`, `git`, `tag`, `features`, ..).
fn manifest_dependency(text: &str, name: &str) -> Option<Vec<(String, String)>> {
    let mut table = Table::Other;
    let mut found: Option<Vec<(String, String)>> = None;
    for line in text.lines() {
        if let Some(header) = table_header(line) {
            if found.is_some() {
                return found;
            }
            table = Table::of(header);
            if matches!(&table, Table::Crate(c) if c == name) {
                found = Some(Vec::new());
            }
            continue;
        }
        let Some((key, value, _)) = key_value(line) else { continue };
        match (&table, found.as_mut()) {
            (Table::Crate(_), Some(entries)) => entries.push((key, value)),
            (Table::Dependencies, None) if key == name => {
                let body = inline_body(&value)?;
                return Some(
                    split_top_level(body)
                        .into_iter()
                        .filter_map(|e| key_value(e).map(|(k, v, _)| (k, v)))
                        .collect(),
                );
            }
            _ => {}
        }
    }
    found
}

/// A dependency's value (`"1"` or `{ path = "..", features = [..] }`, and
/// any comment after it) with its source swapped for `source` (an inline
/// table, as [`EngineSource::dependency`] writes) and every other setting
/// kept. `None` when it inherits from the workspace (`workspace = true`).
fn with_source(value: &str, source: &str) -> Option<String> {
    let new = inline_body(source)?.trim();
    let value = value.trim();
    let Some(body) = value.strip_prefix('{') else {
        // A bare version string: nothing else to keep, but keep a comment.
        let comment = comment_start(value).map_or("", |at| &value[at..]);
        let gap = if comment.is_empty() { "" } else { "  " };
        return Some(format!("{{ {new} }}{gap}{comment}"));
    };
    let close = top_level_end(body)?;
    let (inner, after) = (&body[..close], &body[close + 1..]);
    let entries = split_top_level(inner);
    let inherits = entries
        .iter()
        .any(|e| key_value(e).is_some_and(|(k, v, _)| k == "workspace" && unquote(&v) == "true"));
    if inherits {
        return None;
    }
    let kept = entries.into_iter().map(str::trim).filter(|entry| {
        let key = key_value(entry).map(|(k, _, _)| k).unwrap_or_default();
        !entry.is_empty() && !SOURCE_KEYS.contains(&key.as_str())
    });
    let all: Vec<&str> = std::iter::once(new).chain(kept).collect();
    Some(format!("{{ {} }}{after}", all.join(", ")))
}

/// Calls `f(index, char, depth)` for each character of TOML text outside a
/// string (basic `".."` with escapes, or literal `'..'`), with the bracket
/// depth before it, until `f` returns true.
fn scan_toml(text: &str, mut f: impl FnMut(usize, char, i32) -> bool) {
    let mut depth = 0;
    let mut chars = text.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => {
                while let Some((_, d)) = chars.next() {
                    match d {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '\'' => {
                for (_, d) in chars.by_ref() {
                    if d == '\'' {
                        break;
                    }
                }
            }
            _ => {
                if f(i, c, depth) {
                    return;
                }
                match c {
                    '[' | '{' => depth += 1,
                    ']' | '}' => depth -= 1,
                    _ => {}
                }
            }
        }
    }
}

/// Where the inline table whose body starts `body` closes: its `}`.
fn top_level_end(body: &str) -> Option<usize> {
    let mut end = None;
    scan_toml(body, |i, c, depth| {
        let found = c == '}' && depth == 0;
        if found {
            end = Some(i);
        }
        found
    });
    end
}

/// Splits an inline table's body at its top-level commas.
fn split_top_level(body: &str) -> Vec<&str> {
    let mut cuts = Vec::new();
    scan_toml(body, |i, c, depth| {
        if c == ',' && depth == 0 {
            cuts.push(i);
        }
        false
    });
    let mut parts = Vec::with_capacity(cuts.len() + 1);
    let mut start = 0;
    for cut in cuts {
        parts.push(&body[start..cut]);
        start = cut + 1;
    }
    parts.push(&body[start..]);
    parts
}

/// The body of an inline table value (`{ a = 1 }` → ` a = 1 `).
fn inline_body(value: &str) -> Option<&str> {
    let body = value.trim().strip_prefix('{')?;
    Some(&body[..top_level_end(body)?])
}

/// Where a `#` comment starts in a line, outside strings.
fn comment_start(text: &str) -> Option<usize> {
    let mut at = None;
    scan_toml(text, |i, c, _| {
        let found = c == '#';
        if found {
            at = Some(i);
        }
        found
    });
    at
}

/// A `key = value` line or entry: the key (unquoted), the value (as written,
/// with any comment), and where the `=` is. `None` for anything else.
fn key_value(text: &str) -> Option<(String, String, usize)> {
    if text.trim_start().starts_with('#') || text.trim_start().starts_with('[') {
        return None;
    }
    let mut eq = None;
    scan_toml(text, |i, c, depth| {
        let found = c == '=' && depth == 0;
        if found {
            eq = Some(i);
        }
        found
    });
    let eq = eq?;
    let key = unquote(&text[..eq]);
    (!key.is_empty()).then(|| (key, text[eq + 1..].trim().to_owned(), eq))
}

/// A TOML scalar as text: a basic or literal string unquoted (escapes
/// resolved), anything else trimmed with its comment dropped.
fn unquote(text: &str) -> String {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => out.push(other),
                    None => break,
                },
                c => out.push(c),
            }
        }
        out
    } else if let Some(rest) = text.strip_prefix('\'') {
        rest.split('\'').next().unwrap_or("").to_owned()
    } else {
        let end = comment_start(text).unwrap_or(text.len());
        text[..end].trim().to_owned()
    }
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
/// once per project per run (about 50 ms), and `<project>/target` is the
/// answer when it can't say (no Rust installed, no Cargo.toml yet, a broken
/// manifest). Only Cargo's answers are remembered, so a project that gains
/// its Cargo.toml later is asked again.
pub fn target_dir(root: &Path) -> PathBuf {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(dir) = cache.lock().ok().and_then(|c| c.get(root).cloned()) {
        return dir;
    }
    match query_target_dir(root) {
        Some(dir) => {
            if let Ok(mut cache) = cache.lock() {
                cache.insert(root.to_owned(), dir.clone());
            }
            dir
        }
        None => root.join("target"),
    }
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
    // A project's rust-toolchain.toml may name a toolchain that isn't
    // installed: don't let rustup download it here, where a GUI may be
    // waiting on the answer. The build (on its own thread) installs it.
    c.env("RUSTUP_AUTO_INSTALL", "0");
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
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if let Some(header) = table_header(line) {
            in_table = header == "package.metadata.reign";
            continue;
        }
        let Some((k, value, _)) = key_value(line).filter(|_| in_table) else { continue };
        if k != key {
            continue;
        }
        // An array may run over several lines, with comments: read up to its
        // closing `]`, leaving the comments out.
        let code = |l: &str| l[..comment_start(l).unwrap_or(l.len())].to_owned();
        let mut array = code(&value);
        while !array_closed(&array) {
            let Some(next) = lines.next() else { break };
            array.push(' ');
            array.push_str(&code(next));
        }
        let Some(inner) = array.trim().strip_prefix('[') else { return Vec::new() };
        let mut close = inner.len();
        scan_toml(inner, |i, c, depth| {
            let found = c == ']' && depth == 0;
            if found {
                close = i;
            }
            found
        });
        return split_top_level(&inner[..close])
            .into_iter()
            .map(unquote)
            .filter(|f| !f.is_empty())
            .collect();
    }
    Vec::new()
}

/// Whether TOML text that starts an array has reached its closing `]`.
/// Comments must already be left out.
fn array_closed(text: &str) -> bool {
    let mut closed = false;
    scan_toml(text, |_, c, depth| {
        closed = c == ']' && depth == 1;
        closed
    });
    closed
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
    // A game binary the project already lists keeps its own source: only
    // the template's src/game.rs is ever written, never a stray one beside
    // a game whose main is elsewhere.
    let listed = bin_path(&text, &game_bin(&name));
    let writes_game_rs = match &listed {
        None => true,
        Some(path) => {
            path.as_deref().map(|p| p.replace('\\', "/")).as_deref() == Some("src/game.rs")
        }
    };
    if listed.is_none() {
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
    if writes_game_rs && !main.exists() {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(main, game_main(project_name, &name))?;
        changed = true;
    }
    Ok(changed)
}

/// Whether the manifest lists a `[[bin]]` named `bin`: `None` if not, else
/// its `path` (`None` inside when it has none and Cargo's default applies).
fn bin_path(text: &str, bin: &str) -> Option<Option<String>> {
    let mut in_bin = false;
    let (mut name, mut path) = (None::<String>, None::<String>);
    let mut found = None;
    let mut finish = |name: &mut Option<String>, path: &mut Option<String>| {
        if name.as_deref() == Some(bin) && found.is_none() {
            found = Some(path.clone());
        }
        *name = None;
        *path = None;
    };
    for line in text.lines() {
        if table_header(line).is_some() {
            if in_bin {
                finish(&mut name, &mut path);
            }
            in_bin = line.trim().starts_with("[[bin]]");
            continue;
        }
        if !in_bin {
            continue;
        }
        match key_value(line) {
            Some((k, v, _)) if k == "name" => name = Some(unquote(&v)),
            Some((k, v, _)) if k == "path" => path = Some(unquote(&v)),
            _ => {}
        }
    }
    if in_bin {
        finish(&mut name, &mut path);
    }
    found
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

        // Written over several lines, with comments, under a commented header.
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"g\"\n\n[package.metadata.reign]   # for the hub\n\
             editor-features = [\n    \"editor\",   # the editor shell, egui ]\n\
             \x20   'tools/x', # another\n]\ngame-features = [] # none\n\n[dependencies]\n",
        )
        .unwrap();
        assert_eq!(build_features(&root, "editor-features"), ["editor", "tools/x"]);
        assert!(build_features(&root, "game-features").is_empty());
        // No table, no Cargo.toml: nothing asked for.
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"g\"\n").unwrap();
        assert!(build_features(&root, "editor-features").is_empty());
        let _ = std::fs::remove_dir_all(&root);
        assert!(build_features(&root, "editor-features").is_empty());
    }

    /// A folder that isn't a crate yet gets the usual place, and is asked
    /// again once it becomes one (the fallback isn't remembered).
    #[test]
    fn a_project_that_gains_code_is_asked_again() {
        let root =
            std::env::temp_dir().join(format!("reign-target-later-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        assert_eq!(target_dir(&root), root.join("target"));
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::write(root.join(".cargo/config.toml"), "[build]\ntarget-dir = \"elsewhere\"\n")
            .unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
        )
        .unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let dir = target_dir(&root).to_string_lossy().replace('\\', "/");
        assert!(dir.ends_with("/elsewhere"), "{dir}");
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

    /// Writes `manifest` as a project's Cargo.toml, points it at release
    /// 0.3.1, and returns the result.
    fn moved_to_release(test: &str, manifest: &str) -> String {
        let root =
            std::env::temp_dir().join(format!("reign-engine-edge-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        set_project_engine(&root, &EngineSource::release("0.3.1")).unwrap();
        let text = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        text
    }

    const TAG: &str = r#"git = "https://github.com/AshtonJackson-LS/reign-engine", tag = "v0.3.1""#;

    #[test]
    fn workspace_inherited_engine_crates_are_left_to_the_workspace() {
        let text = moved_to_release(
            "inherit",
            "[package]\nname = \"g\"\n\n[dependencies]\n\
             reign = { workspace = true, features = [\"editor\"] }\n\
             reign_assets.workspace = true\n\
             reign_core = { path = \"../e/engine/reign_core\" }\n",
        );
        assert!(text.contains("reign = { workspace = true, features = [\"editor\"] }\n"), "{text}");
        assert!(text.contains("reign_assets.workspace = true\n"), "{text}");
        assert!(text.contains(&format!("reign_core = {{ {TAG} }}")), "{text}");
    }

    #[test]
    fn a_commented_table_header_still_counts() {
        let text = moved_to_release(
            "comment",
            "[package]\nname = \"g\"\n\n[dependencies]   # the engine\n\
             reign = { path = \"../e/engine/reign\" }  # keep me\n",
        );
        assert!(text.contains(&format!("reign = {{ {TAG} }}  # keep me")), "{text}");
    }

    #[test]
    fn table_form_dependencies_move_too() {
        let text = moved_to_release(
            "table",
            "[package]\nname = \"g\"\n\n[dependencies.reign]\npath = \"../e/engine/reign\"\n\
             features = [\"editor\"]\n\n[build-dependencies.reign_assets]\n\
             git = \"https://example.com/old\"\nbranch = \"dev\"\n\n[dependencies.serde]\nversion = \"1\"\n",
        );
        assert!(!text.contains("../e/engine"), "{text}");
        assert!(!text.contains("branch = \"dev\"") && !text.contains("example.com/old"), "{text}");
        assert_eq!(text.matches("tag = \"v0.3.1\"").count(), 2, "{text}");
        assert!(text.contains("features = [\"editor\"]"), "{text}");
        assert!(
            text.contains("[dependencies.serde]\nversion = \"1\""),
            "not an engine crate:\n{text}"
        );
        // And project_engine reads the table form.
        let root =
            std::env::temp_dir().join(format!("reign-engine-edge-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), &text).unwrap();
        assert_eq!(project_engine(&root), Some(EngineSource::release("0.3.1")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn quotes_and_brackets_inside_settings_are_kept_whole() {
        let text = moved_to_release(
            "quotes",
            "[package]\nname = \"g\"\n\n[dependencies]\n\
             reign = { path = 'C:\\old\\engine\\reign', features = [\"a,b}\", 'c]'], package = \"reign\" }\n\
             reign_core = { path = \"../x\\\"y\", optional = true }\n",
        );
        assert!(
            text.contains(&format!(
                "reign = {{ {TAG}, features = [\"a,b}}\", 'c]'], package = \"reign\" }}"
            )),
            "{text}"
        );
        assert!(text.contains(&format!("reign_core = {{ {TAG}, optional = true }}")), "{text}");
    }

    #[test]
    fn versions_quoted_keys_and_target_tables() {
        let text = moved_to_release(
            "misc",
            "[package]\nname = \"g\"\n\n[dependencies]\nreign = \"0.1\"  # from crates.io one day\n\
             \"reign_core\" = { path = \"../e/engine/reign_core\" }\n\n\
             [target.'cfg(target_os = \"macos\")'.dependencies]\nreign_audio = { path = \"../e/engine/reign_audio\" }\n\n\
             [features]\nreign = []\n",
        );
        assert!(text.contains(&format!("reign = {{ {TAG} }}  # from crates.io one day")), "{text}");
        assert!(text.contains(&format!("\"reign_core\" = {{ {TAG} }}")), "{text}");
        assert!(text.contains(&format!("reign_audio = {{ {TAG} }}")), "{text}");
        assert!(
            text.contains("[features]\nreign = []"),
            "a feature named reign isn't a dependency:\n{text}"
        );
    }

    #[test]
    fn no_engine_crates_is_an_error_and_changes_nothing() {
        let root =
            std::env::temp_dir().join(format!("reign-engine-edge-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let manifest = "[package]\nname = \"g\"\n\n[dependencies]\nserde = \"1\"\n";
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        assert!(set_project_engine(&root, &EngineSource::release("0.3.1")).is_err());
        assert_eq!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap(), manifest);
        assert_eq!(project_engine(&root), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn crlf_manifests_stay_crlf() {
        let text = moved_to_release(
            "crlf",
            "[package]\r\nname = \"g\"\r\n\r\n[dependencies]\r\nreign = { path = \"../e/engine/reign\" }\r\nserde = \"1\"\r\n",
        );
        assert!(text.contains(&format!("reign = {{ {TAG} }}\r\n")), "{text:?}");
        assert!(!text.replace("\r\n", "").contains('\n'), "only CRLF line ends: {text:?}");
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
        // Deleted later: the listed src/game.rs comes back.
        std::fs::remove_file(root.join("src/game.rs")).unwrap();
        assert!(ensure_game_bin(&root, "Old").unwrap());
        assert!(root.join("src/game.rs").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A hand-made project (Exreign) lists its game binary with its own
    /// source: Export must not add a template src/game.rs beside it.
    #[test]
    fn a_game_binary_with_its_own_source_is_left_alone() {
        let root = std::env::temp_dir().join(format!("reign-own-game-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let manifest = "[package]\nname = \"exreign\"\nversion = \"0.1.0\"\n\n\
             [[bin]]\nname = \"exreign\"\npath = \"src/main.rs\"\n\n\
             [[bin]]   # what Export Game builds\nname = \"exreign-game\"\npath = 'src/main.rs'\n\n\
             [[bin]]\nname = \"exreign-editor\"\npath = \"src/bin/editor.rs\"\n";
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        assert!(!ensure_game_bin(&root, "Exreign").unwrap());
        assert!(!root.join("src/game.rs").exists());
        assert_eq!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap(), manifest);
        assert_eq!(bin_path(manifest, "exreign-game"), Some(Some("src/main.rs".into())));
        assert_eq!(bin_path(manifest, "exreign-editor"), Some(Some("src/bin/editor.rs".into())));
        assert_eq!(bin_path("[[bin]]\nname = \"a\"\n", "a"), Some(None));
        assert_eq!(bin_path(manifest, "nope"), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
