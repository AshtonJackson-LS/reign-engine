//! Packs the programs ReignSetup installs into it, from the folder in
//! `$REIGN_SETUP_PAYLOAD` (the release workflow sets it). Without it the
//! setup is built empty and says so when run.

use std::path::{Path, PathBuf};

/// What a payload folder holds.
const FILES: [&str; 3] = ["reign-hub.exe", "reign-engine.exe", "LICENSE"];

fn main() {
    println!("cargo:rerun-if-env-changed=REIGN_SETUP_PAYLOAD");
    println!("cargo:rerun-if-changed=../assets/icon/reign.ico");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    let payload = std::env::var_os("REIGN_SETUP_PAYLOAD").map(PathBuf::from);
    for name in FILES {
        let to = out.join(name);
        match &payload {
            Some(dir) => {
                let from = dir.join(name);
                println!("cargo:rerun-if-changed={}", from.display());
                copy(&from, &to);
            }
            None => std::fs::write(&to, []).expect("write an empty payload file"),
        }
    }

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../assets/icon/reign.ico");
        if let Err(e) = resource.compile() {
            println!("cargo:warning=couldn't embed the icon: {e}");
        }
    }
}

fn copy(from: &Path, to: &Path) {
    if let Err(e) = std::fs::copy(from, to) {
        panic!("REIGN_SETUP_PAYLOAD: can't read {}: {e}", from.display());
    }
}
