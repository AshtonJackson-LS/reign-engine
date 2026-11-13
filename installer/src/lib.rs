//! Installing Reign Engine for the current user, the way Windows expects:
//! programs in `%LOCALAPPDATA%\Programs\Reign Engine`, a Start menu entry
//! (so Windows search finds "Reign Hub"), a Desktop shortcut, and an entry
//! in Settings → Apps that uninstalls it. No administrator rights needed.
//!
//! `ReignSetup.exe` uses all of it; the hub uses [`place_files`] (updates),
//! [`uninstall`] (`reign-hub.exe --uninstall`, which Settings → Apps runs)
//! and [`set_registered_version`].

use std::path::{Path, PathBuf};
use std::process::Command;

/// The programs an install holds.
pub const PROGRAMS: [&str; 2] = ["reign-hub.exe", "reign-engine.exe"];

/// The registry key of the Settings → Apps entry, under HKEY_CURRENT_USER.
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ReignEngine";

/// Where Reign is installed for this user.
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("USERPROFILE")
            .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            .unwrap_or_else(std::env::temp_dir)
    });
    base.join("Programs").join("Reign Engine")
}

/// Keeps a console program started from a windowed one from opening a
/// console window of its own (Windows).
pub fn no_console(mut command: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

/// A program that ships with the system. On Windows, by its full path in
/// System32, so a same-named program elsewhere on `PATH` never runs instead.
pub fn system_tool(name: &str) -> Command {
    #[cfg(windows)]
    {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        let exe = Path::new(&root).join("System32").join(format!("{name}.exe"));
        if exe.is_file() {
            return no_console(Command::new(exe));
        }
    }
    no_console(Command::new(name))
}

fn run(mut command: Command, what: &str) -> Result<(), String> {
    let output = command.output().map_err(|e| format!("{what}: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!("{what}: {}", String::from_utf8_lossy(&output.stderr).trim()))
}

/// A PowerShell script. Values go in as environment variables, so paths
/// with spaces or quotes can't break it.
fn powershell(script: &str, env: &[(&str, &str)]) -> Command {
    let mut command = system_tool("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
    command.arg(script).envs(env.iter().copied());
    command
}

/// Writes `files` (name, contents) into `dir`. A running program can't be
/// overwritten but can be renamed, so each old file is moved to `name.old`
/// first and deleted after; one still running stays until
/// [`clean_old_files`]. If any file fails, the old ones are put back.
pub fn place_files(dir: &Path, files: &[(&str, &[u8])]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let old = |name: &str| dir.join(format!("{name}.old"));
    let mut moved = Vec::new();
    let mut result = Ok(());
    for (name, bytes) in files {
        let target = dir.join(name);
        let _ = std::fs::remove_file(old(name));
        if target.exists() {
            if let Err(e) = std::fs::rename(&target, old(name)) {
                result = Err(format!("can't replace {name} (close Reign first): {e}"));
                break;
            }
            moved.push(*name);
        }
        if let Err(e) = std::fs::write(&target, bytes) {
            result = Err(format!("can't write {name}: {e}"));
            break;
        }
    }
    for name in moved {
        if result.is_err() {
            let _ = std::fs::remove_file(dir.join(name));
            let _ = std::fs::rename(old(name), dir.join(name));
        } else {
            let _ = std::fs::remove_file(old(name));
        }
    }
    result
}

/// Removes the programs [`place_files`] moved aside, where nothing still
/// runs them.
pub fn clean_old_files(dir: &Path) {
    for name in PROGRAMS {
        let _ = std::fs::remove_file(dir.join(format!("{name}.old")));
    }
}

/// Makes the Start menu shortcut and a Desktop one, both "Reign Hub".
pub fn add_shortcuts(dir: &Path) -> Result<(), String> {
    let hub = dir.join(PROGRAMS[0]).display().to_string();
    let script = r#"
        $shell = New-Object -ComObject WScript.Shell
        foreach ($folder in 'Programs', 'Desktop') {
            $s = $shell.CreateShortcut((Join-Path ([Environment]::GetFolderPath($folder)) 'Reign Hub.lnk'))
            $s.TargetPath = $env:REIGN_HUB
            $s.WorkingDirectory = Split-Path $env:REIGN_HUB
            $s.IconLocation = $env:REIGN_HUB + ',0'
            $s.Description = 'Reign Hub: create and open Reign Engine projects'
            $s.Save()
        }
    "#;
    run(powershell(script, &[("REIGN_HUB", &hub)]), "making shortcuts")
}

fn reg(args: &[&str], what: &str) -> Result<(), String> {
    let mut command = system_tool("reg");
    command.args(args);
    run(command, what)
}

fn reg_value(name: &str, kind: &str, data: &str) -> Result<(), String> {
    let args = ["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", data, "/f"];
    reg(&args, "registering with Settings → Apps")
}

/// Adds (or refreshes) the Settings → Apps entry.
pub fn register(dir: &Path, version: &str, repo_url: &str) -> Result<(), String> {
    let hub = dir.join(PROGRAMS[0]).display().to_string();
    let size_kb: u64 = PROGRAMS
        .iter()
        .filter_map(|p| std::fs::metadata(dir.join(p)).ok())
        .map(|m| m.len() / 1024)
        .sum();
    for (name, value) in [
        ("DisplayName", "Reign Engine".to_owned()),
        ("DisplayVersion", version.to_owned()),
        ("Publisher", "Reign Engine contributors".to_owned()),
        ("URLInfoAbout", repo_url.to_owned()),
        ("InstallLocation", dir.display().to_string()),
        ("DisplayIcon", format!("{hub},0")),
        ("UninstallString", format!("\"{hub}\" --uninstall")),
        ("QuietUninstallString", format!("\"{hub}\" --uninstall --silent")),
    ] {
        reg_value(name, "REG_SZ", &value)?;
    }
    reg_value("NoModify", "REG_DWORD", "1")?;
    reg_value("NoRepair", "REG_DWORD", "1")?;
    reg_value("EstimatedSize", "REG_DWORD", &size_kb.to_string())
}

/// Whether Reign has a Settings → Apps entry (it was installed by setup).
fn is_registered() -> bool {
    cfg!(windows) && reg(&["query", UNINSTALL_KEY], "").is_ok()
}

/// After an update: the version Settings → Apps shows.
pub fn set_registered_version(version: &str) {
    if is_registered()
        && let Err(e) = reg_value("DisplayVersion", "REG_SZ", version)
    {
        eprintln!("{e}");
    }
}

/// Removes the shortcuts and the Settings → Apps entry, then the install
/// folder once the calling program has quit. Projects and the hub's project
/// list are never touched. Only deletes a folder that holds the hub, so a
/// wrong path can't take anything else with it.
pub fn uninstall(dir: &Path) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("uninstalling is only automatic on Windows".into());
    }
    let script = r#"
        foreach ($folder in 'Programs', 'Desktop') {
            $link = Join-Path ([Environment]::GetFolderPath($folder)) 'Reign Hub.lnk'
            if (Test-Path -LiteralPath $link) { Remove-Item -LiteralPath $link -Force }
        }
    "#;
    run(powershell(script, &[]), "removing shortcuts")?;
    if is_registered() {
        reg(&["delete", UNINSTALL_KEY, "/f"], "removing the Settings → Apps entry")?;
    }
    if dir.join(PROGRAMS[0]).is_file() {
        // This program runs from that folder: a separate process waits for
        // it to quit, then deletes the folder.
        let script = "Start-Sleep -Seconds 2; Remove-Item -LiteralPath $env:REIGN_DIR -Recurse -Force -ErrorAction SilentlyContinue";
        let dir_text = dir.display().to_string();
        powershell(script, &[("REIGN_DIR", &dir_text)])
            .spawn()
            .map_err(|e| format!("removing {dir_text}: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_under_local_app_data_programs() {
        let dir = install_dir();
        assert!(dir.ends_with(Path::new("Programs").join("Reign Engine")), "{}", dir.display());
    }

    #[test]
    fn placing_files_replaces_them_and_undoes_a_failure() {
        let dir = std::env::temp_dir().join(format!("reign-place-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        place_files(&dir, &[("a.exe", b"one")]).unwrap();
        place_files(&dir, &[("a.exe", b"two")]).unwrap();
        assert_eq!(std::fs::read(dir.join("a.exe")).unwrap(), b"two");
        assert!(!dir.join("a.exe.old").exists(), "not running, so removed");
        // The second file can't be written (no such folder): a.exe is put back.
        assert!(place_files(&dir, &[("a.exe", b"three"), ("missing/b.exe", b"x")]).is_err());
        assert_eq!(std::fs::read(dir.join("a.exe")).unwrap(), b"two");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
