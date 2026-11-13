//! ReignSetup.exe: the one file a Windows user downloads. It holds the hub
//! and the editor, installs them for the current user (no administrator
//! rights), adds the Start menu entry, a Desktop shortcut and an entry in
//! Settings → Apps, then offers to open Reign Hub.
//!
//! Running a newer setup over an older install upgrades it in place.
//! `--silent` installs without asking anything and doesn't open the hub.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::{Command, ExitCode};

use reign_setup::{PROGRAMS, add_shortcuts, install_dir, place_files, register};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const HUB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/reign-hub.exe"));
const EDITOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/reign-engine.exe"));
const LICENSE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/LICENSE"));
const TITLE: &str = "Reign Engine Setup";

fn main() -> ExitCode {
    let silent = std::env::args().any(|a| a == "--silent");
    let tell = |level: MessageLevel, text: &str| {
        if silent {
            eprintln!("{text}");
        } else {
            MessageDialog::new()
                .set_title(TITLE)
                .set_level(level)
                .set_description(text)
                .set_buttons(MessageButtons::Ok)
                .show();
        }
    };
    let ask = |text: &str| {
        silent
            || MessageDialog::new()
                .set_title(TITLE)
                .set_description(text)
                .set_buttons(MessageButtons::YesNo)
                .show()
                == MessageDialogResult::Yes
    };

    if HUB.is_empty() || EDITOR.is_empty() {
        tell(
            MessageLevel::Error,
            "This setup was built without Reign inside it (a development build).\n\n\
             Download ReignSetup.exe from the project's GitHub Releases page instead.",
        );
        return ExitCode::FAILURE;
    }
    if !cfg!(windows) {
        tell(MessageLevel::Error, "ReignSetup is for Windows.");
        return ExitCode::FAILURE;
    }

    let dir = install_dir();
    let question = format!(
        "Install Reign Engine {VERSION}?\n\n\
         It installs for your Windows user only, in:\n{}\n\n\
         You'll get Reign Hub in the Start menu and on the Desktop. \
         Your projects are never touched.",
        dir.display()
    );
    if !ask(&question) {
        return ExitCode::SUCCESS;
    }

    let files: [(&str, &[u8]); 3] =
        [(PROGRAMS[0], HUB), (PROGRAMS[1], EDITOR), ("LICENSE", LICENSE)];
    let result = place_files(&dir, &files)
        .and_then(|()| add_shortcuts(&dir))
        .and_then(|()| register(&dir, VERSION, &reign_build::release::git_url()));
    if let Err(e) = result {
        tell(MessageLevel::Error, &format!("Reign Engine couldn't be installed:\n\n{e}"));
        return ExitCode::FAILURE;
    }

    if !silent && ask(&format!("Reign Engine {VERSION} is installed.\n\nOpen Reign Hub now?")) {
        let _ = Command::new(dir.join(PROGRAMS[0])).current_dir(&dir).spawn();
    }
    ExitCode::SUCCESS
}
