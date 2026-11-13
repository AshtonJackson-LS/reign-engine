//! Installing Reign Engine for the current user, the way Windows expects:
//! programs in `%LOCALAPPDATA%\Programs\Reign Engine`, a Start menu entry
//! (so Windows search finds "Reign Hub"), a Desktop shortcut, and an entry
//! in Settings → Apps that uninstalls it. No administrator rights needed.
//!
//! `ReignSetup.exe` uses all of it; the hub uses [`uninstall`] (for
//! `reign-hub.exe --uninstall`, which Settings → Apps runs) and
//! [`set_registered_version`] (after it updates itself).
//!
//! On other platforms these do nothing and say so.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The programs an install holds.
pub const PROGRAMS: [&str; 2] = ["reign-hub.exe", "reign-engine.exe"];

/// What Settings → Apps lists it as.
pub const DISPLAY_NAME: &str = "Reign Engine";

/// The registry key of the Settings → Apps entry, under HKEY_CURRENT_USER.
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\ReignEngine";

/// Where Reign is installed for this user.
pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| {
        std::env::var_os("USERPROFILE")
            .map(|home| PathBuf::from(home).join("AppData").join("Local"))
            .unwrap_or_else(std::env::temp_dir)
    });
    base.join("Programs").join(DISPLAY_NAME)
}

/// Runs a console program without flashing a console window.
fn quiet(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

fn run(mut command: Command, what: &str) -> Result<(), String> {
    let output = command.output().map_err(|e| format!("{what}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        Err(format!("{what}: {}", err.trim()))
    }
}

/// Runs a PowerShell script. Values go in as environment variables, so
/// paths with spaces or quotes can't break the script.
fn powershell(script: &str, env: &[(&str, &str)], what: &str) -> Result<(), String> {
    let mut command = quiet("powershell");
    command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
    command.arg(script);
    for (key, value) in env {
        command.env(key, value);
    }
    run(command, what)
}

/// Copies `files` (name → source) into `dir`. A program that's running
/// can't be overwritten but can be renamed, so each old file is moved to
/// `name.old` first (removed on the next install or hub start).
pub fn place_files(dir: &Path, files: &[(&str, &[u8])]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, bytes) in files {
        let target = dir.join(name);
        let old = dir.join(format!("{name}.old"));
        let _ = std::fs::remove_file(&old);
        if target.exists() {
            std::fs::rename(&target, &old)
                .map_err(|e| format!("can't replace {name} (close Reign first): {e}"))?;
        }
        std::fs::write(&target, bytes).map_err(|e| format!("can't write {name}: {e}"))?;
    }
    Ok(())
}

/// Makes the Start menu shortcut and, if `desktop`, a Desktop one, both
/// called "Reign Hub".
pub fn add_shortcuts(dir: &Path, desktop: bool) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("shortcuts are only made on Windows".into());
    }
    let hub = dir.join(PROGRAMS[0]).display().to_string();
    let dir = dir.display().to_string();
    let script = r#"
        $shell = New-Object -ComObject WScript.Shell
        $links = @(Join-Path ([Environment]::GetFolderPath('Programs')) 'Reign Hub.lnk')
        if ($env:REIGN_DESKTOP -eq '1') {
            $links += Join-Path ([Environment]::GetFolderPath('Desktop')) 'Reign Hub.lnk'
        }
        foreach ($link in $links) {
            $s = $shell.CreateShortcut($link)
            $s.TargetPath = $env:REIGN_HUB
            $s.WorkingDirectory = $env:REIGN_DIR
            $s.IconLocation = $env:REIGN_HUB + ',0'
            $s.Description = 'Reign Hub: create and open Reign Engine projects'
            $s.Save()
        }
    "#;
    let desktop = if desktop { "1" } else { "0" };
    powershell(
        script,
        &[("REIGN_HUB", &hub), ("REIGN_DIR", &dir), ("REIGN_DESKTOP", desktop)],
        "making shortcuts",
    )
}

fn remove_shortcuts() -> Result<(), String> {
    let script = r#"
        foreach ($folder in 'Programs', 'Desktop') {
            $link = Join-Path ([Environment]::GetFolderPath($folder)) 'Reign Hub.lnk'
            if (Test-Path -LiteralPath $link) { Remove-Item -LiteralPath $link -Force }
        }
    "#;
    powershell(script, &[], "removing shortcuts")
}

fn reg_value(name: &str, kind: &str, data: &str) -> Result<(), String> {
    let mut command = quiet("reg");
    command.args(["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", data, "/f"]);
    run(command, "registering with Settings → Apps")
}

/// Adds (or refreshes) the Settings → Apps entry.
pub fn register(dir: &Path, version: &str, repo_url: &str) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("only Windows has Settings → Apps".into());
    }
    let hub = dir.join(PROGRAMS[0]);
    let size_kb = PROGRAMS
        .iter()
        .filter_map(|p| std::fs::metadata(dir.join(p)).ok())
        .map(|m| m.len() / 1024)
        .sum::<u64>();
    reg_value("DisplayName", "REG_SZ", DISPLAY_NAME)?;
    reg_value("DisplayVersion", "REG_SZ", version)?;
    reg_value("Publisher", "REG_SZ", "Reign Engine contributors")?;
    reg_value("URLInfoAbout", "REG_SZ", repo_url)?;
    reg_value("InstallLocation", "REG_SZ", &dir.display().to_string())?;
    reg_value("DisplayIcon", "REG_SZ", &format!("{},0", hub.display()))?;
    reg_value("UninstallString", "REG_SZ", &format!("\"{}\" --uninstall", hub.display()))?;
    reg_value(
        "QuietUninstallString",
        "REG_SZ",
        &format!("\"{}\" --uninstall --silent", hub.display()),
    )?;
    reg_value("NoModify", "REG_DWORD", "1")?;
    reg_value("NoRepair", "REG_DWORD", "1")?;
    reg_value("EstimatedSize", "REG_DWORD", &size_kb.to_string())
}

/// Whether Reign has a Settings → Apps entry (it was installed by setup).
pub fn is_registered() -> bool {
    cfg!(windows)
        && quiet("reg").args(["query", UNINSTALL_KEY]).output().is_ok_and(|o| o.status.success())
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
    remove_shortcuts()?;
    if is_registered() {
        let mut command = quiet("reg");
        command.args(["delete", UNINSTALL_KEY, "/f"]);
        run(command, "removing the Settings → Apps entry")?;
    }
    if dir.join(PROGRAMS[0]).is_file() {
        // This program is running from that folder, so a separate process
        // waits for it to quit, then deletes it.
        let script = r#"
            Start-Sleep -Seconds 2
            Remove-Item -LiteralPath $env:REIGN_DIR -Recurse -Force -ErrorAction SilentlyContinue
        "#;
        let mut command = quiet("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command"]);
        command.arg(script).env("REIGN_DIR", dir);
        command.spawn().map_err(|e| format!("removing {}: {e}", dir.display()))?;
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
    fn placing_files_moves_the_old_ones_aside() {
        let dir = std::env::temp_dir().join(format!("reign-place-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        place_files(&dir, &[("a.exe", b"one")]).unwrap();
        place_files(&dir, &[("a.exe", b"two")]).unwrap();
        assert_eq!(std::fs::read(dir.join("a.exe")).unwrap(), b"two");
        assert_eq!(std::fs::read(dir.join("a.exe.old")).unwrap(), b"one");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
