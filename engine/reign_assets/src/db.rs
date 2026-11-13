//! The asset database: GUID ↔ path and type, typed loading with a cache,
//! and hot reload.

use std::any::Any;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use crate::scan::{asset_files, meta_path, relative};
use crate::{Asset, AssetError, AssetKind, Guid, Handle, Meta, read_pack};

/// What the database knows about one asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetInfo {
    pub guid: Guid,
    /// Relative to the asset root, `/`-separated.
    pub path: String,
    pub kind: AssetKind,
}

/// An asset that changed on disk and was reloaded (or removed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetChange {
    pub guid: Guid,
    pub path: String,
    pub kind: AssetKind,
    pub removed: bool,
}

impl AssetChange {
    /// Whether the change touches simulation data (see
    /// [`AssetKind::sim_affecting`]).
    pub fn sim_affecting(&self) -> bool {
        self.kind.sim_affecting()
    }
}

struct Entry {
    path: String,
    meta: Meta,
    /// The bytes, for a packed database.
    packed: Option<&'static [u8]>,
}

#[derive(Default)]
struct State {
    entries: BTreeMap<Guid, Entry>,
    by_path: BTreeMap<String, Guid>,
    cache: BTreeMap<Guid, Arc<dyn Any + Send + Sync>>,
    /// Bumped each time an asset reloads, so users can spot it.
    versions: BTreeMap<Guid, u32>,
    folders: BTreeSet<String>,
}

/// Where the assets come from.
enum Source {
    /// Files under a folder, with hot reload: development and the editor.
    Loose(PathBuf),
    /// One blob embedded in the executable: release builds.
    Packed,
}

/// Assets by GUID.
///
/// Both backends answer the same calls. Loading is lazy and cached; a
/// reload replaces the cached value and bumps [`AssetDb::version`]. All
/// methods take `&self`, so one database can be shared.
pub struct AssetDb {
    source: Source,
    state: RwLock<State>,
    /// Paths changed on disk, waiting for [`AssetDb::apply_changes`].
    pending: Arc<Mutex<BTreeSet<String>>>,
    /// How long a file must have been left alone before a change to it is
    /// applied, so a file still being written isn't read half done.
    settle: Mutex<std::time::Duration>,
    /// Changes the editing calls applied right away, for the next
    /// [`AssetDb::apply_changes`] to report.
    applied: Mutex<Vec<AssetChange>>,
    #[cfg(feature = "watch")]
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

impl AssetDb {
    /// A database over a packed archive (see [`crate::pack_dir`]).
    pub fn packed(blob: &'static [u8]) -> Result<AssetDb, AssetError> {
        let mut state = State::default();
        for e in read_pack(blob)? {
            let meta =
                Meta::from_ron(e.meta).map_err(|m| AssetError(format!("{}: {m}", e.path)))?;
            state.add(e.path.to_owned(), meta, Some(e.data))?;
        }
        Ok(AssetDb::new(Source::Packed, state))
    }

    /// A database over the files under `root`. Creates a `.meta` (with a
    /// fresh GUID) for every asset that lacks one, and a new GUID for any
    /// copy that duplicates another's.
    pub fn loose(root: impl Into<PathBuf>) -> Result<AssetDb, AssetError> {
        let root = root.into();
        let mut state = State::default();
        for path in asset_files(&root)? {
            let meta = read_or_create_meta(&root, &path)?;
            let meta = if state.entries.contains_key(&meta.guid) {
                log::warn!("{path}: its GUID duplicates another asset's; giving it a new one");
                let fresh = Meta { guid: Guid::new(), ..meta };
                write_meta(&root, &path, &fresh)?;
                fresh
            } else {
                meta
            };
            state.add(path, meta, None)?;
        }
        state.folders = folders(&root);
        Ok(AssetDb::new(Source::Loose(root), state))
    }

    fn new(source: Source, state: State) -> Self {
        AssetDb {
            source,
            state: RwLock::new(state),
            pending: Arc::default(),
            settle: Mutex::new(std::time::Duration::ZERO),
            applied: Mutex::default(),
            #[cfg(feature = "watch")]
            watcher: Mutex::new(None),
        }
    }

    /// The folder a loose database reads, if it is one.
    pub fn root(&self) -> Option<&Path> {
        match &self.source {
            Source::Loose(root) => Some(root),
            Source::Packed => None,
        }
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write_state(&self) -> std::sync::RwLockWriteGuard<'_, State> {
        self.state.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn guid(&self, path: &str) -> Option<Guid> {
        self.read().by_path.get(path).copied()
    }

    pub fn path(&self, guid: Guid) -> Option<String> {
        self.read().entries.get(&guid).map(|e| e.path.clone())
    }

    pub fn kind(&self, guid: Guid) -> Option<AssetKind> {
        self.read().entries.get(&guid).map(|e| e.meta.importer.kind())
    }

    pub fn meta(&self, guid: Guid) -> Option<Meta> {
        self.read().entries.get(&guid).map(|e| e.meta.clone())
    }

    /// Every asset, by path.
    pub fn assets(&self) -> Vec<AssetInfo> {
        let state = self.read();
        let mut all: Vec<AssetInfo> = state
            .entries
            .iter()
            .map(|(guid, e)| AssetInfo {
                guid: *guid,
                path: e.path.clone(),
                kind: e.meta.importer.kind(),
            })
            .collect();
        all.sort_by(|a, b| a.path.cmp(&b.path));
        all
    }

    /// Every folder (loose databases also list empty ones), `/`-separated.
    pub fn folders(&self) -> Vec<String> {
        let state = self.read();
        let mut all: BTreeSet<String> = state.folders.clone();
        for e in state.entries.values() {
            let mut p = e.path.as_str();
            while let Some((dir, _)) = p.rsplit_once('/') {
                all.insert(dir.to_owned());
                p = dir;
            }
        }
        all.into_iter().collect()
    }

    /// The handle for the asset at `path`, if it is a `T`.
    pub fn handle<T: Asset>(&self, path: &str) -> Option<Handle<T>> {
        let guid = self.guid(path)?;
        (self.kind(guid)? == T::KIND).then_some(Handle::new(guid))
    }

    /// The asset's file bytes.
    pub fn bytes(&self, guid: Guid) -> Result<Cow<'static, [u8]>, AssetError> {
        let state = self.read();
        let entry =
            state.entries.get(&guid).ok_or_else(|| AssetError(format!("no asset {guid}")))?;
        match (entry.packed, &self.source) {
            (Some(data), _) => Ok(Cow::Borrowed(data)),
            (None, Source::Loose(root)) => std::fs::read(root.join(&entry.path))
                .map(Cow::Owned)
                .map_err(|e| AssetError(format!("{}: {e}", entry.path))),
            (None, Source::Packed) => Err(AssetError(format!("{} is not in the pack", entry.path))),
        }
    }

    /// The bytes of the asset at `path`.
    pub fn bytes_at(&self, path: &str) -> Result<Cow<'static, [u8]>, AssetError> {
        let guid = self.guid(path).ok_or_else(|| AssetError(format!("no asset at {path}")))?;
        self.bytes(guid)
    }

    /// Loads (or returns the cached) asset.
    pub fn load<T: Asset>(&self, handle: Handle<T>) -> Result<Arc<T>, AssetError> {
        if let Some(cached) = self.read().cache.get(&handle.guid)
            && let Ok(value) = cached.clone().downcast::<T>()
        {
            return Ok(value);
        }
        let meta = self
            .meta(handle.guid)
            .ok_or_else(|| AssetError(format!("no asset {}", handle.guid)))?;
        if meta.importer.kind() != T::KIND {
            return Err(AssetError(format!(
                "{} is a {}, not a {}",
                self.path(handle.guid).unwrap_or_default(),
                meta.importer.kind().name(),
                T::KIND.name()
            )));
        }
        let bytes = self.bytes(handle.guid)?;
        let value = Arc::new(T::load(&bytes, &meta).map_err(|e| {
            AssetError(format!("{}: {}", self.path(handle.guid).unwrap_or_default(), e.0))
        })?);
        self.write_state().cache.insert(handle.guid, value.clone());
        Ok(value)
    }

    /// Loads the asset at `path`.
    pub fn load_path<T: Asset>(&self, path: &str) -> Result<Arc<T>, AssetError> {
        let handle = self
            .handle::<T>(path)
            .ok_or_else(|| AssetError(format!("no {} at {path}", T::KIND.name())))?;
        self.load(handle)
    }

    /// How many times the asset has been reloaded.
    pub fn version(&self, guid: Guid) -> u32 {
        self.read().versions.get(&guid).copied().unwrap_or(0)
    }

    /// A hash of every asset that can change the simulation (see
    /// [`AssetKind::sim_affecting`]): GUID, import settings and bytes, in
    /// GUID order. Online peers compare it, so textures and sounds can
    /// differ but collision, clips and scenes can't.
    pub fn content_hash(&self) -> u64 {
        let sim: Vec<Guid> = self
            .read()
            .entries
            .iter()
            .filter(|(_, e)| e.meta.importer.kind().sim_affecting())
            .map(|(g, _)| *g)
            .collect();
        let mut parts = Vec::with_capacity(sim.len());
        for guid in sim {
            let meta = self.meta(guid).expect("listed");
            let bytes = self.bytes(guid).map(|b| reign_core::checksum(&*b)).unwrap_or(0);
            parts.push((guid, meta, bytes));
        }
        reign_core::checksum(&parts)
    }

    // Hot reload.

    /// Notes that the file at `path` (an asset or its `.meta`, relative to
    /// the root) changed. Nothing reloads until [`AssetDb::apply_changes`].
    pub fn queue_change(&self, path: &str) {
        let asset = path.strip_suffix(".meta").unwrap_or(path);
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).insert(asset.to_owned());
    }

    /// Whether changes are waiting.
    pub fn has_pending_changes(&self) -> bool {
        !self.pending.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }

    /// Watches a loose database's folder and queues every change. The
    /// watcher only queues; call [`AssetDb::apply_changes`] at a frame
    /// boundary to reload.
    #[cfg(feature = "watch")]
    pub fn watch(&self) -> Result<(), AssetError> {
        use notify::Watcher;
        let Source::Loose(root) = &self.source else {
            return Err(AssetError("only loose assets can be watched".into()));
        };
        let pending = self.pending.clone();
        let base = root.canonicalize().unwrap_or_else(|_| root.clone());
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let Ok(event) = event else { return };
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    return;
                }
                let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
                for path in event.paths {
                    let path = path.canonicalize().unwrap_or(path);
                    let rel = relative(&base, &path);
                    let name = rel.rsplit('/').next().unwrap_or("");
                    if rel.is_empty() || name.starts_with('.') || path.is_dir() {
                        continue;
                    }
                    pending.insert(rel.strip_suffix(".meta").unwrap_or(&rel).to_owned());
                }
            })
            .map_err(|e| AssetError(e.to_string()))?;
        watcher
            .watch(root, notify::RecursiveMode::Recursive)
            .map_err(|e| AssetError(e.to_string()))?;
        *self.watcher.lock().unwrap_or_else(|e| e.into_inner()) = Some(watcher);
        self.set_settle_time(std::time::Duration::from_millis(150));
        Ok(())
    }

    /// How long a changed file must stay untouched before
    /// [`AssetDb::apply_changes`] takes it (zero by default; watching sets
    /// 150 ms). Younger changes stay queued for a later frame.
    pub fn set_settle_time(&self, settle: std::time::Duration) {
        *self.settle.lock().unwrap_or_else(|e| e.into_inner()) = settle;
    }

    /// Reloads everything queued since the last call: re-reads metas (making
    /// any that are missing), drops cached values so the next load reads the
    /// new bytes, bumps versions and forgets removed files. Call it between
    /// frames; it returns what changed so dependents can refresh, including
    /// what the editing calls ([`AssetDb::write`] and so on) applied since.
    pub fn apply_changes(&self) -> Vec<AssetChange> {
        let mut changes =
            std::mem::take(&mut *self.applied.lock().unwrap_or_else(|e| e.into_inner()));
        changes.extend(self.apply_settled());
        changes
    }

    fn apply_settled(&self) -> Vec<AssetChange> {
        let Source::Loose(root) = &self.source else {
            self.pending.lock().unwrap_or_else(|e| e.into_inner()).clear();
            return Vec::new();
        };
        let settle = *self.settle.lock().unwrap_or_else(|e| e.into_inner());
        let paths: Vec<String> = {
            let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::SystemTime::now();
            let settled = |path: &String| {
                [root.join(path), meta_path(&root.join(path))].iter().all(|p| {
                    std::fs::metadata(p)
                        .and_then(|m| m.modified())
                        .map_or(true, |t| now.duration_since(t).unwrap_or_default() >= settle)
                })
            };
            let ready: Vec<String> = pending.iter().filter(|p| settled(p)).cloned().collect();
            for p in &ready {
                pending.remove(p);
            }
            ready
        };
        self.apply_paths(paths)
    }

    fn apply_paths(&self, paths: Vec<String>) -> Vec<AssetChange> {
        let Source::Loose(root) = &self.source else { return Vec::new() };
        let mut changes = Vec::new();
        for path in paths {
            let exists = root.join(&path).is_file();
            let mut state = self.write_state();
            if !exists {
                if let Some(guid) = state.by_path.remove(&path) {
                    let entry = state.entries.remove(&guid).expect("indexed");
                    state.cache.remove(&guid);
                    changes.push(AssetChange {
                        guid,
                        path,
                        kind: entry.meta.importer.kind(),
                        removed: true,
                    });
                }
                continue;
            }
            drop(state);
            let meta = match read_or_create_meta(root, &path) {
                Ok(meta) => meta,
                Err(e) => {
                    log::warn!("{path}: {e}");
                    continue;
                }
            };
            let mut state = self.write_state();
            if let Some(old) = state.by_path.get(&path).copied()
                && old != meta.guid
            {
                state.entries.remove(&old);
            }
            let guid = meta.guid;
            let kind = meta.importer.kind();
            state.by_path.insert(path.clone(), guid);
            state.entries.insert(guid, Entry { path: path.clone(), meta, packed: None });
            state.cache.remove(&guid);
            *state.versions.entry(guid).or_default() += 1;
            changes.push(AssetChange { guid, path, kind, removed: false });
        }
        if !changes.is_empty() {
            self.write_state().folders = folders(root);
        }
        changes
    }

    /// Handles in text assets (scenes, sheets, clips, maps...) that point
    /// at no asset: `(the asset holding it, the GUID)`. A handle is a
    /// quoted 32-digit hex string.
    pub fn missing_references(&self) -> Vec<(String, Guid)> {
        let mut missing = Vec::new();
        for info in self.assets().into_iter().filter(|i| i.path.ends_with(".ron")) {
            let Ok(bytes) = self.bytes(info.guid) else { continue };
            for guid in quoted_guids(&bytes) {
                if guid != Guid::NIL && self.path(guid).is_none() {
                    missing.push((info.path.clone(), guid));
                }
            }
        }
        missing
    }

    // Editing (loose databases only).

    fn loose_root(&self) -> Result<&Path, AssetError> {
        self.root().ok_or_else(|| AssetError("packed assets are read-only".into()))
    }

    /// Replaces an asset's file and reloads it now.
    pub fn write(&self, guid: Guid, bytes: &[u8]) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        let path = self.path(guid).ok_or_else(|| AssetError(format!("no asset {guid}")))?;
        std::fs::write(root.join(&path), bytes).map_err(|e| AssetError(format!("{path}: {e}")))?;
        self.reload_now(&path);
        Ok(())
    }

    /// Replaces an asset's import settings (keeping its GUID) and reloads it.
    pub fn write_meta(&self, guid: Guid, meta: &Meta) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        let path = self.path(guid).ok_or_else(|| AssetError(format!("no asset {guid}")))?;
        let meta = Meta { guid, ..meta.clone() };
        write_meta(root, &path, &meta)?;
        self.reload_now(&path);
        Ok(())
    }

    /// Writes a new asset at `path` with a fresh `.meta`, and returns its GUID.
    pub fn create(&self, path: &str, bytes: &[u8]) -> Result<Guid, AssetError> {
        let root = self.loose_root()?;
        let full = root.join(path);
        if full.exists() {
            return Err(AssetError(format!("{path} already exists")));
        }
        if let Some(dir) = full.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AssetError(e.to_string()))?;
        }
        std::fs::write(&full, bytes).map_err(|e| AssetError(format!("{path}: {e}")))?;
        let meta = Meta::new_for(path);
        write_meta(root, path, &meta)?;
        self.reload_now(path);
        Ok(meta.guid)
    }

    pub fn create_folder(&self, path: &str) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        std::fs::create_dir_all(root.join(path)).map_err(|e| AssetError(e.to_string()))?;
        self.write_state().folders = folders(root);
        Ok(())
    }

    /// Moves or renames an asset, taking its `.meta` (and so its GUID) along.
    pub fn rename(&self, guid: Guid, new_path: &str) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        let old = self.path(guid).ok_or_else(|| AssetError(format!("no asset {guid}")))?;
        let (from, to) = (root.join(&old), root.join(new_path));
        if to.exists() {
            return Err(AssetError(format!("{new_path} already exists")));
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AssetError(e.to_string()))?;
        }
        std::fs::rename(&from, &to).map_err(|e| AssetError(e.to_string()))?;
        std::fs::rename(meta_path(&from), meta_path(&to)).map_err(|e| AssetError(e.to_string()))?;
        let mut state = self.write_state();
        state.by_path.remove(&old);
        state.by_path.insert(new_path.to_owned(), guid);
        state.entries.get_mut(&guid).expect("listed").path = new_path.to_owned();
        state.folders = folders(root);
        Ok(())
    }

    /// Moves or renames a folder and everything in it, metas included.
    pub fn rename_folder(&self, old: &str, new: &str) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        let to = root.join(new);
        if to.exists() {
            return Err(AssetError(format!("{new} already exists")));
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| AssetError(e.to_string()))?;
        }
        std::fs::rename(root.join(old), &to).map_err(|e| AssetError(e.to_string()))?;
        let mut state = self.write_state();
        let prefix = format!("{old}/");
        let moved: Vec<(String, Guid)> = state
            .by_path
            .iter()
            .filter(|(p, _)| p.starts_with(&prefix))
            .map(|(p, g)| (p.clone(), *g))
            .collect();
        for (path, guid) in moved {
            let new_path = format!("{new}/{}", &path[prefix.len()..]);
            state.by_path.remove(&path);
            state.by_path.insert(new_path.clone(), guid);
            state.entries.get_mut(&guid).expect("listed").path = new_path;
        }
        state.folders = folders(root);
        Ok(())
    }

    /// Deletes an asset and its `.meta`.
    pub fn delete(&self, guid: Guid) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        let path = self.path(guid).ok_or_else(|| AssetError(format!("no asset {guid}")))?;
        let full = root.join(&path);
        std::fs::remove_file(&full).map_err(|e| AssetError(e.to_string()))?;
        let _ = std::fs::remove_file(meta_path(&full));
        let mut state = self.write_state();
        state.by_path.remove(&path);
        let entry = state.entries.remove(&guid).expect("listed");
        state.cache.remove(&guid);
        drop(state);
        let kind = entry.meta.importer.kind();
        let change = AssetChange { guid, path, kind, removed: true };
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).push(change);
        Ok(())
    }

    /// Deletes an empty folder.
    pub fn delete_folder(&self, path: &str) -> Result<(), AssetError> {
        let root = self.loose_root()?;
        std::fs::remove_dir(root.join(path)).map_err(|e| AssetError(format!("{path}: {e}")))?;
        self.write_state().folders = folders(root);
        Ok(())
    }

    /// Reloads `path` now, without waiting for it to settle (the editor
    /// just wrote it whole), and keeps the change for the next
    /// [`AssetDb::apply_changes`].
    fn reload_now(&self, path: &str) {
        self.pending.lock().unwrap_or_else(|e| e.into_inner()).remove(path);
        let changes = self.apply_paths(vec![path.to_owned()]);
        self.applied.lock().unwrap_or_else(|e| e.into_inner()).extend(changes);
    }
}

impl State {
    fn add(
        &mut self,
        path: String,
        meta: Meta,
        packed: Option<&'static [u8]>,
    ) -> Result<(), AssetError> {
        if let Some(other) = self.entries.get(&meta.guid) {
            return Err(AssetError(format!("{path} and {} have the same GUID", other.path)));
        }
        self.by_path.insert(path.clone(), meta.guid);
        self.entries.insert(meta.guid, Entry { path, meta, packed });
        Ok(())
    }
}

fn read_or_create_meta(root: &Path, path: &str) -> Result<Meta, AssetError> {
    let file = meta_path(&root.join(path));
    match std::fs::read_to_string(&file) {
        Ok(text) => Meta::from_ron(&text).map_err(|e| AssetError(format!("{path}.meta: {e}"))),
        Err(_) => {
            let meta = Meta::new_for(path);
            write_meta(root, path, &meta)?;
            log::info!("made {path}.meta");
            Ok(meta)
        }
    }
}

fn write_meta(root: &Path, path: &str, meta: &Meta) -> Result<(), AssetError> {
    std::fs::write(meta_path(&root.join(path)), meta.to_ron())
        .map_err(|e| AssetError(format!("{path}.meta: {e}")))
}

fn folders(root: &Path) -> BTreeSet<String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
                out.insert(relative(root, &path));
                walk(root, &path, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

/// Every `"` + 32 hex digits + `"` in `text`, as GUIDs.
fn quoted_guids(text: &[u8]) -> Vec<Guid> {
    text.windows(34)
        .filter(|w| w[0] == b'"' && w[33] == b'"' && w[1..33].iter().all(u8::is_ascii_hexdigit))
        .filter_map(|w| std::str::from_utf8(&w[1..33]).ok())
        .filter_map(|hex| u128::from_str_radix(hex, 16).ok())
        .map(Guid)
        .collect()
}
