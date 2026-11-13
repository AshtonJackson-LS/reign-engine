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

/// The engine source the project builds against: `$REIGN_ENGINE_SOURCE`,
/// or the checkout these tools were built from.
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
    root.join("target").join("debug").join(format!("{}{exe}", editor_bin(crate_name)))
}

/// The project game binary's name for a crate: the game without the
/// editor, which Export Game builds.
pub fn game_bin(crate_name: &str) -> String {
    format!("{crate_name}-game")
}

/// Where `cargo build --release` puts the project's game.
pub fn game_path(root: &Path, crate_name: &str) -> PathBuf {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    root.join("target").join("release").join(format!("{}{exe}", game_bin(crate_name)))
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

/// The command that builds the project's editor.
pub fn build_command(root: &Path, crate_name: &str) -> Command {
    build_bin(root, &editor_bin(crate_name), false)
}

/// The command that builds the project's game, optimised, for export.
pub fn game_build_command(root: &Path, crate_name: &str) -> Command {
    build_bin(root, &game_bin(crate_name), true)
}

fn build_bin(root: &Path, bin: &str, release: bool) -> Command {
    let mut c = cargo();
    c.current_dir(root).args(["build", "--bin", bin, "--message-format=short", "--color=never"]);
    if release {
        c.arg("--release");
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
    let engine = engine_root();
    let path = |p: &str| engine.join(p).display().to_string().replace('\\', "/");
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
reign_engine = {{ path = "{app}" }}
reign = {{ path = "{reign}" }}
serde = {{ version = "1", features = ["derive"] }}

[build-dependencies]
reign_build = {{ path = "{build}" }}

[profile.dev]
opt-level = 1

[profile.dev.package."*"]
opt-level = 3

# A project of its own, not part of any workspace around it.
[workspace]
"#,
            bin = editor_bin(&name),
            game = game_bin_table(&name),
            app = path("app"),
            reign = path("engine/reign"),
            build = path("engine/reign_build"),
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
    for file in ["rust-toolchain.toml", "Cargo.lock"] {
        let from = engine.join(file);
        if from.is_file() && !root.join(file).exists() {
            std::fs::copy(from, root.join(file))?;
        }
    }
    Ok(name)
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
