//! The database over real folders: metas, GUID stability, hot reload,
//! packing and the content hash.

use std::path::PathBuf;

use reign_assets::{
    AnimationClip, AssetDb, AssetKind, ClipFrame, Guid, Handle, Importer, Meta, Texture, Tilemap,
    pack_dir, scan, to_ron,
};

/// A fresh folder under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("reign-assets-{name}-{}", Guid::new()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let full = self.0.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn png(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let pixels = vec![shade; (w * h * 4) as usize];
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
    out
}

fn sample(dir: &TempDir) {
    dir.write("art/hero.png", &png(4, 2, 200));
    dir.write("levels/one.tilemap.ron", to_ron(&Tilemap::from_ascii("..C\n###\n")).as_bytes());
    let clip = AnimationClip {
        frames: vec![ClipFrame { ms: 100, ..ClipFrame::default() }; 3],
        ..AnimationClip::default()
    };
    dir.write("anims/idle.anim.ron", to_ron(&clip).as_bytes());
    dir.write("audio/hit.wav", b"RIFF....WAVE");
}

#[test]
fn opening_a_folder_writes_metas_that_round_trip() {
    let dir = TempDir::new("metas");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    for info in db.assets() {
        let text = std::fs::read_to_string(scan::meta_path(&dir.0.join(&info.path))).unwrap();
        let meta = Meta::from_ron(&text).unwrap();
        assert_eq!(meta.guid, info.guid, "{}", info.path);
        assert_eq!(meta.to_ron(), text, "{} is written canonically", info.path);
    }
    assert_eq!(db.kind(db.guid("art/hero.png").unwrap()), Some(AssetKind::Texture));
    assert_eq!(db.kind(db.guid("audio/hit.wav").unwrap()), Some(AssetKind::AudioClip));
    // Opening again keeps every GUID.
    let again = AssetDb::loose(&dir.0).unwrap();
    assert_eq!(again.assets(), db.assets());
    // Import settings load with the asset.
    let hero = db.guid("art/hero.png").unwrap();
    let mut meta = db.meta(hero).unwrap();
    meta.importer = Importer::Texture(reign_assets::TextureSettings { density: 3 });
    db.write_meta(hero, &meta).unwrap();
    let texture = db.load(Handle::<Texture>::new(hero)).unwrap();
    assert_eq!((texture.density, texture.image.width, texture.image.height), (3, 4, 2));
}

#[test]
fn guids_survive_renames_and_moves() {
    let dir = TempDir::new("rename");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    let hero: Handle<Texture> = db.handle("art/hero.png").unwrap();
    db.rename(hero.guid, "art/characters/knight.png").unwrap();
    assert_eq!(db.path(hero.guid).as_deref(), Some("art/characters/knight.png"));
    assert!(db.load(hero).is_ok(), "the old handle still loads");
    db.rename_folder("art", "graphics").unwrap();
    assert_eq!(db.path(hero.guid).as_deref(), Some("graphics/characters/knight.png"));
    // A fresh database finds it at the new path under the same GUID.
    let reopened = AssetDb::loose(&dir.0).unwrap();
    assert_eq!(reopened.guid("graphics/characters/knight.png"), Some(hero.guid));
    assert!(reopened.load(hero).is_ok());
}

#[test]
fn copies_with_duplicate_guids_get_new_ones() {
    let dir = TempDir::new("dupe");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    let hero = db.guid("art/hero.png").unwrap();
    std::fs::copy(dir.0.join("art/hero.png"), dir.0.join("art/copy.png")).unwrap();
    std::fs::copy(dir.0.join("art/hero.png.meta"), dir.0.join("art/copy.png.meta")).unwrap();
    let again = AssetDb::loose(&dir.0).unwrap();
    let a = again.guid("art/hero.png").unwrap();
    let b = again.guid("art/copy.png").unwrap();
    assert_ne!(a, b);
    assert!(a == hero || b == hero);
}

#[test]
fn changes_apply_only_at_the_frame_boundary() {
    let dir = TempDir::new("reload");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    let hero: Handle<Texture> = db.handle("art/hero.png").unwrap();
    assert_eq!(db.load(hero).unwrap().image.rgba[0], 200);
    dir.write("art/hero.png", &png(4, 2, 50));
    db.queue_change("art/hero.png");
    // Queued, not applied: the cached texture is still served.
    assert!(db.has_pending_changes());
    assert_eq!(db.load(hero).unwrap().image.rgba[0], 200);
    assert_eq!(db.version(hero.guid), 0);
    let changes = db.apply_changes();
    assert_eq!(changes.len(), 1);
    assert_eq!((changes[0].guid, changes[0].sim_affecting()), (hero.guid, false));
    assert_eq!(db.load(hero).unwrap().image.rgba[0], 50);
    assert_eq!(db.version(hero.guid), 1);
    // A queued meta change is the asset's change; a new file gets a meta.
    dir.write("art/new.png", &png(1, 1, 9));
    db.queue_change("art/new.png");
    db.queue_change("levels/one.tilemap.ron.meta");
    let changes = db.apply_changes();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().any(|c| c.path == "levels/one.tilemap.ron" && c.sim_affecting()));
    assert!(db.guid("art/new.png").is_some());
    // Removing a file removes the asset.
    std::fs::remove_file(dir.0.join("art/new.png")).unwrap();
    db.queue_change("art/new.png");
    assert!(db.apply_changes()[0].removed);
    assert!(db.guid("art/new.png").is_none());
}

#[cfg(feature = "watch")]
#[test]
fn the_watcher_queues_changes_without_applying_them() {
    let dir = TempDir::new("watch");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    db.watch().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    dir.write("art/hero.png", &png(4, 2, 77));
    let hero: Handle<Texture> = db.handle("art/hero.png").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !db.has_pending_changes() {
        assert!(std::time::Instant::now() < deadline, "the watcher saw nothing");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(db.version(hero.guid), 0, "queued only");
    // Applied once the file has been left alone for the settle time.
    std::thread::sleep(std::time::Duration::from_millis(300));
    db.apply_changes();
    assert_eq!(db.load(hero).unwrap().image.rgba[0], 77);
}

#[test]
fn changes_wait_until_the_file_settles() {
    let dir = TempDir::new("settle");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    db.set_settle_time(std::time::Duration::from_secs(3600));
    dir.write("art/hero.png", &png(4, 2, 9));
    db.queue_change("art/hero.png");
    assert!(db.apply_changes().is_empty(), "just written: not yet");
    assert!(db.has_pending_changes(), "still queued");
    db.set_settle_time(std::time::Duration::ZERO);
    assert_eq!(db.apply_changes().len(), 1);
}

#[test]
fn the_pack_holds_exactly_the_loose_bytes() {
    let dir = TempDir::new("pack");
    sample(&dir);
    let loose = AssetDb::loose(&dir.0).unwrap();
    let blob: &'static [u8] = Box::leak(pack_dir(&dir.0).unwrap().into_boxed_slice());
    assert_eq!(pack_dir(&dir.0).unwrap(), blob, "packing is reproducible");
    let packed = AssetDb::packed(blob).unwrap();
    assert_eq!(packed.assets(), loose.assets());
    for info in loose.assets() {
        assert_eq!(
            packed.bytes(info.guid).unwrap(),
            loose.bytes(info.guid).unwrap(),
            "{}",
            info.path
        );
        assert_eq!(packed.meta(info.guid), loose.meta(info.guid));
    }
    assert_eq!(packed.content_hash(), loose.content_hash());
    assert!(packed.write(loose.assets()[0].guid, b"x").is_err(), "packs are read-only");
}

#[test]
fn packing_refuses_assets_without_metas() {
    let dir = TempDir::new("nometa");
    dir.write("a.png", &png(1, 1, 1));
    assert!(pack_dir(&dir.0).unwrap_err().0.contains("no .meta"));
}

#[test]
fn the_content_hash_follows_sim_data_only() {
    let dir = TempDir::new("hash");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    let start = db.content_hash();
    dir.write("art/hero.png", &png(4, 2, 1));
    dir.write("audio/hit.wav", b"RIFF....WAVE!");
    db.queue_change("art/hero.png");
    db.queue_change("audio/hit.wav");
    db.apply_changes();
    assert_eq!(db.content_hash(), start, "textures and sounds don't change it");
    dir.write("levels/one.tilemap.ron", to_ron(&Tilemap::from_ascii("...\n###\n")).as_bytes());
    db.queue_change("levels/one.tilemap.ron");
    db.apply_changes();
    assert_ne!(db.content_hash(), start, "collision does");
}

#[test]
fn editing_creates_moves_and_deletes_with_metas() {
    let dir = TempDir::new("edit");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    db.create_folder("prefabs/empty").unwrap();
    assert!(db.folders().contains(&"prefabs/empty".to_string()));
    let guid =
        db.create("prefabs/one.tilemap.ron", to_ron(&Tilemap::default()).as_bytes()).unwrap();
    assert!(dir.0.join("prefabs/one.tilemap.ron.meta").exists());
    assert_eq!(db.kind(guid), Some(AssetKind::Tilemap));
    assert!(db.create("prefabs/one.tilemap.ron", b"").is_err());
    db.delete(guid).unwrap();
    assert!(!dir.0.join("prefabs/one.tilemap.ron.meta").exists());
    assert!(db.path(guid).is_none());
    db.delete_folder("prefabs/empty").unwrap();
    assert!(!db.folders().contains(&"prefabs/empty".to_string()));
}

#[test]
fn editor_writes_reload_at_once_and_are_reported_at_the_boundary() {
    let dir = TempDir::new("write");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    // As when watching: changes on disk wait to settle, but the editor's
    // own writes don't.
    db.set_settle_time(std::time::Duration::from_secs(60));
    let guid = db.create("maps/a.tilemap.ron", to_ron(&Tilemap::default()).as_bytes()).unwrap();
    db.apply_changes();
    let handle = Handle::<Tilemap>::new(guid);
    let edited = Tilemap { collision: vec!["#.".into()], ..Tilemap::default() };
    db.write(guid, to_ron(&edited).as_bytes()).unwrap();
    assert_eq!(*db.load(handle).unwrap(), edited, "reloaded at once");
    let changes = db.apply_changes();
    assert_eq!(changes.len(), 1);
    assert_eq!((changes[0].guid, changes[0].removed), (guid, false));
    assert!(db.apply_changes().is_empty());
    db.delete(guid).unwrap();
    let changes = db.apply_changes();
    assert_eq!((changes.len(), changes[0].removed), (1, true));
}

#[test]
fn missing_references_are_found() {
    let dir = TempDir::new("refs");
    sample(&dir);
    let db = AssetDb::loose(&dir.0).unwrap();
    let real = db.assets()[0].guid;
    let ghost = reign_assets::Guid(0x1234);
    let text = format!("(a: \"{real}\", b: \"{ghost}\", none: \"{}\")", reign_assets::Guid::NIL);
    db.create("refs.data.ron", text.as_bytes()).unwrap();
    assert_eq!(db.missing_references(), [("refs.data.ron".to_string(), ghost)]);
}
