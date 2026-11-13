//! Walking a loose asset folder.

use std::path::{Path, PathBuf};

use crate::{AssetError, META_EXT};

/// Files and folders skipped when scanning: hidden ones.
fn skipped(name: &str) -> bool {
    name.starts_with('.')
}

/// The file listing paths to leave out of the asset database (and so of
/// metas and packs): one pattern per line, `*` matching anything (slashes
/// included), `#` starting a comment. Documentation, previews and
/// build-time-only files belong here.
pub const IGNORE_FILE: &str = ".reignignore";

/// Every asset file under `root` (not `.meta` files, nor anything
/// [`IGNORE_FILE`] lists), as `/`-separated paths relative to it, sorted.
pub fn asset_files(root: &Path) -> Result<Vec<String>, AssetError> {
    let ignore = ignore_patterns(root);
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.retain(|path| !ignore.iter().any(|p| glob(p, path)));
    out.sort();
    Ok(out)
}

/// The patterns in `root`'s ignore file.
pub fn ignore_patterns(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(IGNORE_FILE))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Whether `path` matches `pattern`, where `*` matches any run of
/// characters.
pub fn glob(pattern: &str, path: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == path;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !path.starts_with(first) || !path[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &path[first.len()..path.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(i) => rest = &rest[i + part.len()..],
            None => return false,
        }
    }
    true
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), AssetError> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| AssetError(format!("{}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| AssetError(e.to_string()))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if skipped(&name) {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out)?;
        } else if path.extension().is_none_or(|e| e != META_EXT) {
            out.push(relative(root, &path));
        }
    }
    Ok(())
}

/// `path` relative to `root`, with `/` separators.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// The sidecar of an asset file.
pub fn meta_path(asset: &Path) -> PathBuf {
    let mut name = asset.as_os_str().to_owned();
    name.push(".");
    name.push(META_EXT);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::glob;

    #[test]
    fn globs() {
        assert!(glob("*.md", "ui/FONTS.md"));
        assert!(glob("*/previews/*", "veiled_king/previews/a.gif"));
        assert!(!glob("*/previews/*", "previews.png"));
        assert!(glob("icon/exreign.ico", "icon/exreign.ico"));
        assert!(!glob("icon/exreign.ico", "icon/exreign.icon"));
        assert!(glob("a*b*c", "a-x-b-y-c"));
        assert!(!glob("a*b*c", "a-x-c"));
    }
}
