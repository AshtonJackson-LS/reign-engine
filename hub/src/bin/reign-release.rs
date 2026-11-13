//! Signs Reign releases so the hub's updater trusts them.
//!
//! ```text
//! reign-release keygen KEY_FILE      make a key pair: the secret goes in KEY_FILE,
//!                                    the public key is printed
//! reign-release sign ZIP TAG         sign ZIP as release TAG (like v0.2.0) with the
//!                                    secret in $REIGN_RELEASE_KEY; writes ZIP.sig
//! reign-release verify ZIP TAG       check ZIP.sig against the public key built
//!                                    into this build (the one the hub trusts)
//! ```
//!
//! The release workflow signs and verifies; you only run `keygen`, once.
//! Keep the secret out of the repository: it goes in the GitHub secret
//! `REIGN_RELEASE_KEY`, and anyone holding it can ship updates.

#[path = "../signing.rs"]
mod signing;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["keygen", file] => keygen(Path::new(file)),
        ["sign", zip, tag] => sign(Path::new(zip), tag),
        ["verify", zip, tag] => verify(Path::new(zip), tag),
        _ => Err("usage: reign-release keygen KEY_FILE | sign ZIP TAG | verify ZIP TAG".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("reign-release: {e}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(file: &Path) -> Result<(), String> {
    if file.exists() {
        return Err(format!("{} exists; not overwriting a key", file.display()));
    }
    let (secret, public) = signing::generate()?;
    std::fs::write(file, format!("{secret}\n")).map_err(|e| e.to_string())?;
    println!("Secret key written to {}", file.display());
    println!("Public key: {public}");
    Ok(())
}

fn signature_path(zip: &Path) -> PathBuf {
    let mut name = zip.as_os_str().to_owned();
    name.push(".sig");
    PathBuf::from(name)
}

fn sign(zip: &Path, tag: &str) -> Result<(), String> {
    let secret = std::env::var("REIGN_RELEASE_KEY")
        .map_err(|_| "set REIGN_RELEASE_KEY to the secret key".to_owned())?;
    let bytes = std::fs::read(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let signature = signing::sign(&secret, tag, &bytes)?;
    let out = signature_path(zip);
    std::fs::write(&out, format!("{signature}\n")).map_err(|e| e.to_string())?;
    println!("Signed {} as {tag}: {}", zip.display(), out.display());
    Ok(())
}

fn verify(zip: &Path, tag: &str) -> Result<(), String> {
    let bytes = std::fs::read(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let sig_file = signature_path(zip);
    let signature =
        std::fs::read_to_string(&sig_file).map_err(|e| format!("{}: {e}", sig_file.display()))?;
    signing::verify(signing::PUBLIC_KEY, tag, &bytes, &signature)?;
    println!("{} is signed by the key this build trusts", zip.display());
    Ok(())
}
