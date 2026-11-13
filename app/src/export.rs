//! Export Game: putting a built game and its packed assets together into
//! something to hand to players. On macOS that's `<Title>.app`; elsewhere a
//! `<Title>` folder with the executable, the pack and [`GAME_FILE`].
//!
//! Building the game is the editor's job (`code.rs` runs cargo); this is
//! the part after, which needs no window.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use reign::assets::{AssetKind, Meta, pack_dir, read_pack};
use reign_build::project;

use crate::player::{GAME_FILE, GameInfo, PACK_FILE, choose_scene};

/// How the export is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// `<Title>.app/Contents/MacOS/<exe>`, with the data in
    /// `Contents/Resources`.
    MacApp,
    /// `<Title>/<exe>`, with the data beside it.
    Folder,
}

impl Layout {
    /// The layout for the platform the editor runs on (games are built for
    /// the same one).
    pub fn native() -> Layout {
        if cfg!(target_os = "macos") { Layout::MacApp } else { Layout::Folder }
    }
}

/// What to export.
pub struct Export {
    /// The game's name: the window title and the app or folder name.
    pub title: String,
    /// The project crate's name, which the executable is named after.
    pub crate_name: String,
    /// The built game executable.
    pub exe: PathBuf,
    /// The project's `assets/` folder.
    pub assets: PathBuf,
    /// Where `<Title>.app` or `<Title>/` goes.
    pub out_dir: PathBuf,
    pub layout: Layout,
}

/// Gets a project ready to build its game: makes it a crate if it isn't
/// one (a project without code has nothing to build the game from) and
/// adds the game binary if it was made before there was one. Returns the
/// crate's name and whether the project had code already.
pub fn prepare(root: &Path, title: &str) -> Result<(String, bool), String> {
    let had_code = project::has_code(root);
    let name = project::init(root, title)
        .map_err(|e| format!("couldn't set up the project's code: {e}"))?;
    let added = project::ensure_game_bin(root, title)
        .map_err(|e| format!("couldn't add the game to Cargo.toml: {e}"))?;
    if added && had_code {
        log::info!("added the game binary ({}) to Cargo.toml", project::game_bin(&name));
    }
    Ok((name, had_code))
}

/// Builds the game with `--release`, handing each line cargo prints to
/// `line`. Blocks until cargo is done.
pub fn build(
    root: &Path,
    crate_name: &str,
    mut line: impl FnMut(String),
) -> Result<PathBuf, String> {
    let mut command = project::game_build_command(root, crate_name);
    command.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("can't run cargo: {e}"))?;
    if let Some(stderr) = child.stderr.take() {
        for l in BufReader::new(stderr).lines().map_while(Result::ok) {
            line(l);
        }
    }
    if child.wait().is_ok_and(|s| s.success()) {
        Ok(project::game_path(root, crate_name))
    } else {
        Err("the game didn't build: fix the errors above, then export again".into())
    }
}

/// The whole export, start to end, blocking: what `--export` does.
pub fn export_project(root: &Path, title: &str, out_dir: &Path) -> Result<PathBuf, String> {
    let (name, _) = prepare(root, title)?;
    let exe = build(root, &name, |l| eprintln!("{l}"))?;
    assemble(&Export {
        title: title.to_owned(),
        crate_name: name,
        exe,
        assets: root.join("assets"),
        out_dir: out_dir.to_owned(),
        layout: Layout::native(),
    })
}

/// A file name from a title: characters that can't be in a file name on
/// some system become spaces.
pub fn file_title(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { ' ' } else { c })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let joined = words.join(" ").trim_matches('.').to_owned();
    if joined.is_empty() { "Game".to_owned() } else { joined }
}

/// Packs the assets and picks the start scene from what's in the pack.
/// Returns the pack and that scene's path.
pub fn pack(assets: &Path) -> Result<(Vec<u8>, String), String> {
    let blob = pack_dir(assets).map_err(|e| e.to_string())?;
    let mut scenes = Vec::new();
    for entry in read_pack(&blob).map_err(|e| e.to_string())? {
        let meta = Meta::from_ron(entry.meta).map_err(|e| format!("{}: {e}", entry.path))?;
        if meta.importer.kind() == AssetKind::Scene {
            scenes.push(entry.path.to_owned());
        }
    }
    let scene = choose_scene(&scenes, None).ok_or("the project has no scenes")?;
    Ok((blob, scene))
}

/// Writes the export and returns the app or folder it made. Files already
/// there are replaced; nothing else is removed.
pub fn assemble(export: &Export) -> Result<PathBuf, String> {
    let (blob, scene) = pack(&export.assets)?;
    let name = file_title(&export.title);
    let exe_name = format!("{}{}", export.crate_name, std::env::consts::EXE_SUFFIX);
    let (top, bin_dir, data_dir) = match export.layout {
        Layout::MacApp => {
            let app = export.out_dir.join(format!("{name}.app"));
            let contents = app.join("Contents");
            (app, contents.join("MacOS"), contents.join("Resources"))
        }
        Layout::Folder => {
            let dir = export.out_dir.join(&name);
            (dir.clone(), dir.clone(), dir)
        }
    };
    let io = |what: &Path, e: std::io::Error| format!("{}: {e}", what.display());
    for dir in [&bin_dir, &data_dir] {
        std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
    }
    let exe = bin_dir.join(&exe_name);
    std::fs::copy(&export.exe, &exe).map_err(|e| io(&export.exe, e))?;
    let pack = data_dir.join(PACK_FILE);
    std::fs::write(&pack, &blob).map_err(|e| io(&pack, e))?;
    let info = GameInfo { title: export.title.clone(), scene };
    let game = data_dir.join(GAME_FILE);
    std::fs::write(&game, info.to_ron()).map_err(|e| io(&game, e))?;
    if export.layout == Layout::MacApp {
        let plist = top.join("Contents").join("Info.plist");
        std::fs::write(&plist, info_plist(&export.title, &export.crate_name, &exe_name))
            .map_err(|e| io(&plist, e))?;
        sign(&top);
    }
    Ok(top)
}

/// The macOS app's `Info.plist`.
pub fn info_plist(title: &str, crate_name: &str, exe_name: &str) -> String {
    let escape = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let id: String =
        crate_name.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let version = env!("CARGO_PKG_VERSION");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>{title}</string>
  <key>CFBundleDisplayName</key><string>{title}</string>
  <key>CFBundleIdentifier</key><string>com.reign-engine.game.{id}</string>
  <key>CFBundleExecutable</key><string>{exe}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#,
        title = escape(title),
        exe = escape(exe_name),
    )
}

/// Signs the app ad hoc so its files are sealed together, which macOS
/// wants before it runs a bundle copied from elsewhere. Not having
/// `codesign` (no Xcode tools) only costs that.
fn sign(app: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let out = std::process::Command::new("codesign")
        .args(["--force", "--deep", "--sign", "-"])
        .arg(app)
        .output();
    match out {
        Ok(o) if o.status.success() => {}
        Ok(o) => log::warn!(
            "couldn't sign {}: {}",
            app.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log::warn!("couldn't sign {}: {e}", app.display()),
    }
}
