//! `reign-assets`: asset folder chores from the command line.
//!
//! ```text
//! reign-assets metas DIR        make any missing .meta files
//! reign-assets check DIR        fail if an asset lacks a .meta, a .meta is
//!                               broken, or a handle points at nothing
//! reign-assets pack DIR OUT     write the packed archive
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["metas", dir] => reign_assets::AssetDb::loose(PathBuf::from(dir))
            .map(|db| println!("{} assets, every one with a .meta", db.assets().len())),
        ["check", dir] => check(&PathBuf::from(dir)),
        ["pack", dir, out] => reign_assets::pack_dir(&PathBuf::from(dir)).and_then(|blob| {
            std::fs::write(out, &blob)
                .map(|_| println!("{} bytes", blob.len()))
                .map_err(|e| reign_assets::AssetError(e.to_string()))
        }),
        _ => {
            eprintln!("usage: reign-assets metas DIR | check DIR | pack DIR OUT");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn check(dir: &std::path::Path) -> Result<(), reign_assets::AssetError> {
    // Packing refuses assets without metas and broken metas; it reads
    // without writing anything.
    let blob = reign_assets::pack_dir(dir)?;
    let blob: &'static [u8] = Box::leak(blob.into_boxed_slice());
    let db = reign_assets::AssetDb::packed(blob)?;
    let missing = db.missing_references();
    for (path, guid) in &missing {
        eprintln!("{path}: no asset has GUID {guid}");
    }
    if !missing.is_empty() {
        return Err(reign_assets::AssetError(format!("{} missing references", missing.len())));
    }
    println!("{} assets ok", db.assets().len());
    Ok(())
}
