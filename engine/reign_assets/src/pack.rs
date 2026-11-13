//! The packed archive: every asset and its `.meta` in one blob, for
//! release builds that ship as a single executable.
//!
//! Layout (little-endian): `RPAK`, a `u32` version, a `u32` entry count,
//! then per entry the path (`u16` length + UTF-8), the meta text (`u32`
//! length + UTF-8) and the data (`u32` length + bytes), entries sorted by
//! path. Reading it borrows straight from the blob; nothing is copied.

use std::path::Path;

use crate::{AssetError, Meta};

const MAGIC: &[u8; 4] = b"RPAK";
const VERSION: u32 = 1;

/// One packed asset.
#[derive(Clone, Copy, Debug)]
pub struct PackEntry<'a> {
    pub path: &'a str,
    pub meta: &'a str,
    pub data: &'a [u8],
}

/// Packs every file under `root` that has a `.meta`, sorted by path, so the
/// same files always give the same bytes. Fails on an asset without a
/// `.meta` (run the editor or `reign-assets metas` first) or a broken one.
pub fn pack_dir(root: &Path) -> Result<Vec<u8>, AssetError> {
    let mut files = Vec::new();
    for path in crate::scan::asset_files(root)? {
        let full = root.join(&path);
        let meta_path = crate::scan::meta_path(&full);
        let meta = std::fs::read_to_string(&meta_path).map_err(|_| {
            AssetError(format!(
                "{path} has no .meta (open the folder in the editor, or run `reign-assets metas`)"
            ))
        })?;
        Meta::from_ron(&meta).map_err(|e| AssetError(format!("{path}.meta: {e}")))?;
        let data = std::fs::read(&full).map_err(|e| AssetError(format!("{path}: {e}")))?;
        files.push((path, meta, data));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (path, meta, data) in &files {
        out.extend_from_slice(&(path.len() as u16).to_le_bytes());
        out.extend_from_slice(path.as_bytes());
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend_from_slice(meta.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    Ok(out)
}

/// Reads a pack's entries, borrowing from it.
pub fn read_pack(blob: &[u8]) -> Result<Vec<PackEntry<'_>>, AssetError> {
    let bad = |what: &str| AssetError(format!("asset pack is corrupt: {what}"));
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], AssetError> {
        let slice = blob.get(at..at + n).ok_or_else(|| bad("truncated"))?;
        at += n;
        Ok(slice)
    };
    if take(4)? != MAGIC {
        return Err(bad("not a Reign asset pack"));
    }
    let u32_at = |b: &[u8]| u32::from_le_bytes(b.try_into().expect("4 bytes"));
    if u32_at(take(4)?) != VERSION {
        return Err(bad("unknown version"));
    }
    let count = u32_at(take(4)?) as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let len = u16::from_le_bytes(take(2)?.try_into().expect("2 bytes")) as usize;
        let path = std::str::from_utf8(take(len)?).map_err(|_| bad("path"))?;
        let len = u32_at(take(4)?) as usize;
        let meta = std::str::from_utf8(take(len)?).map_err(|_| bad("meta"))?;
        let len = u32_at(take(4)?) as usize;
        let data = take(len)?;
        entries.push(PackEntry { path, meta, data });
    }
    Ok(entries)
}
