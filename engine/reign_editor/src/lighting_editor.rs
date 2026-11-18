//! The Lighting tab: makes, edits and previews the normal map and light mask
//! of a texture, so lights can shade a flat sprite as if it had shape.
//!
//! Open a texture (or a sprite sheet) from the Project panel. The tab shows
//! three maps: the **albedo** (the texture itself), the **normal map**
//! (which way each pixel faces) and the **light mask** (how much light each
//! pixel receives). Create makes one from the albedo, Modify edits one by
//! hand, and the lit preview shows one point light shining on the sprite as
//! you work (drag it, or let it follow the mouse). Save writes
//! `<texture>_n.png` and `<texture>_m.png` next to the texture and links
//! them from every sprite sheet that uses it, so the game's lights use them.
//!
//! The maths is in [`reign_assets::lighting`]; this is the interface.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use reign_assets::lighting::{
    MaskParams, NormalParams, Preset, PreviewLight, generate_light_mask, generate_normal_map,
    shade_image,
};
use reign_assets::{
    AssetDb, AssetKind, Guid, Handle, RgbaImage, SpriteSheet, Texture, encode_png, to_ron,
};

use crate::assets_ui::AssetDrag;
use crate::lighting_modify::{CanvasMap, ModifyDialog, Outcome, WandSources};
use crate::undo::Edits;

static REVISION: AtomicU64 = AtomicU64::new(1);

fn next_rev() -> u64 {
    REVISION.fetch_add(1, Ordering::Relaxed)
}

/// A shared value that changes only through [`Versioned::edit`], with a
/// revision number that says whether it did. Comparing two is comparing
/// revisions, which is cheap however big the image is, and cloning one
/// shares the pixels until one is edited.
pub(crate) struct Versioned<T> {
    pub value: Arc<T>,
    pub rev: u64,
}

impl<T> Clone for Versioned<T> {
    fn clone(&self) -> Self {
        Versioned { value: self.value.clone(), rev: self.rev }
    }
}

impl<T> PartialEq for Versioned<T> {
    fn eq(&self, other: &Self) -> bool {
        self.rev == other.rev
    }
}

impl<T: Clone> Versioned<T> {
    pub fn new(value: T) -> Self {
        Versioned { value: Arc::new(value), rev: next_rev() }
    }

    pub fn edit<R>(&mut self, f: impl FnOnce(&mut T) -> R) -> R {
        let out = f(Arc::make_mut(&mut self.value));
        self.rev = next_rev();
        out
    }
}

pub(crate) type Map = Versioned<RgbaImage>;

/// The two maps being edited. This is what undo steps back through.
#[derive(Clone, PartialEq)]
struct Work {
    normal: Option<Map>,
    mask: Option<Map>,
}

/// Which map a slot, dialog or tool is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MapKind {
    Normal,
    LightMask,
}

impl MapKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            MapKind::Normal => "Normal map",
            MapKind::LightMask => "Light mask",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            MapKind::Normal => "_n",
            MapKind::LightMask => "_m",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    Final,
    Albedo,
    Normal,
    LightMask,
}

impl ViewMode {
    const ALL: [ViewMode; 4] =
        [ViewMode::Final, ViewMode::Albedo, ViewMode::Normal, ViewMode::LightMask];

    fn label(self) -> &'static str {
        match self {
            ViewMode::Final => "Final",
            ViewMode::Albedo => "Albedo",
            ViewMode::Normal => "Normal",
            ViewMode::LightMask => "Light Mask",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorStyle {
    /// A dot where the light is.
    Simple,
    /// The dot, the circle where its light runs out and its height.
    Advanced,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ColorTab {
    Rgb,
    Hsl,
    Hex,
}

/// The preview light's settings, with the tool's defaults.
struct LightSettings {
    /// In image pixels; the middle of the image until it's moved.
    at: Option<(f32, f32)>,
    color: [u8; 3],
    brightness: f32,
    falloff: f32,
    height: f32,
    /// Percent.
    ambient: f32,
    /// How much the light mask counts, percent.
    influence: f32,
    follow: bool,
    cursor: CursorStyle,
    tab: ColorTab,
    hex: String,
}

impl Default for LightSettings {
    fn default() -> Self {
        LightSettings {
            at: None,
            color: [255, 255, 255],
            brightness: 2.5,
            falloff: 70.0,
            height: 25.0,
            ambient: 15.0,
            influence: 100.0,
            follow: false,
            cursor: CursorStyle::Simple,
            tab: ColorTab::Rgb,
            hex: "#FFFFFF".into(),
        }
    }
}

/// A result being worked out on another thread.
struct Pending<T> {
    rx: mpsc::Receiver<T>,
}

impl<T: Send + 'static> Pending<T> {
    fn spawn(ctx: &egui::Context, work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        Pending { rx }
    }

    fn poll(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }
}

struct NormalDialog {
    preset: Preset,
    params: NormalParams,
    per_sprite: bool,
    show_texture: bool,
    result: Option<Map>,
    /// What `result` and `job` were made from.
    made_with: Option<(NormalParams, bool, u64)>,
    job: Option<Pending<RgbaImage>>,
}

struct MaskDialog {
    params: MaskParams,
    show_texture: bool,
    result: Option<Map>,
    made_with: Option<(MaskParams, u64)>,
    job: Option<Pending<RgbaImage>>,
}

/// "This image's dimensions do not match" with Continue and Cancel.
struct Mismatch {
    kind: MapKind,
    file: Guid,
    image: RgbaImage,
}

enum Dialog {
    Normal(NormalDialog),
    Mask(MaskDialog),
    Modify(Box<ModifyDialog>),
    Mismatch(Mismatch),
}

struct OpenLighting {
    /// The texture.
    guid: Guid,
    path: String,
    albedo: Map,
    /// The sprite sheets that use this texture, to link the maps from.
    sheets: Vec<(Guid, String)>,
    /// The cells of the first sheet (a grid's, or its named sprites'), for
    /// making a map one sprite at a time.
    cells: Vec<[u32; 4]>,
    work: Work,
    saved: Work,
    edits: Edits<Work>,
    /// The PNG assets the maps were last saved to or loaded from.
    normal_file: Option<Guid>,
    mask_file: Option<Guid>,
}

/// What the preview texture was drawn from.
#[derive(Clone, Debug, PartialEq)]
struct PreviewKey {
    view: ViewMode,
    albedo: u64,
    normal: Option<u64>,
    mask: Option<u64>,
    light: [u32; 8],
}

pub struct LightingEditor {
    open: Option<OpenLighting>,
    status: Option<String>,
    view: ViewMode,
    zoom: f32,
    /// Where the image's middle is, from the middle of the canvas.
    pan: Vec2,
    fit: bool,
    /// The colour behind the image, if not the checkerboard.
    bg: Option<Color32>,
    hide_bg: bool,
    light: LightSettings,
    dialog: Option<Dialog>,
    /// The albedo with the background hidden, and what it was made from.
    albedo_view: Option<((u64, bool, Color32), Map)>,
    preview: Option<(PreviewKey, egui::TextureHandle)>,
    /// What the canvas shows, as pixels, for the exports.
    shown: Option<RgbaImage>,
    /// The canvas as last drawn: its size, and where the image's corner is.
    last_view: Option<(Vec2, Vec2)>,
    /// Where the image was last drawn on screen.
    last_image_rect: Option<Rect>,
}

impl Default for LightingEditor {
    fn default() -> Self {
        LightingEditor {
            open: None,
            status: None,
            view: ViewMode::Final,
            zoom: 4.0,
            pan: Vec2::ZERO,
            fit: true,
            bg: None,
            hide_bg: false,
            light: LightSettings::default(),
            dialog: None,
            albedo_view: None,
            preview: None,
            shown: None,
            last_view: None,
            last_image_rect: None,
        }
    }
}

// ---- Loading and saving ----------------------------------------------------

fn load_image(db: &AssetDb, guid: Guid) -> Option<RgbaImage> {
    db.load(Handle::<Texture>::new(guid)).ok().map(|t| t.image.clone())
}

/// `sprites/hero.png` and `_n` -> `sprites/hero_n.png`.
fn sibling_path(texture_path: &str, suffix: &str) -> String {
    match texture_path.rsplit_once('.') {
        Some((stem, ext)) if !stem.ends_with('/') => format!("{stem}{suffix}.{ext}"),
        _ => format!("{texture_path}{suffix}.png"),
    }
}

/// The rectangles of a sheet's cells: its grid's, else its named sprites'.
fn sheet_cells(sheet: &SpriteSheet) -> Vec<[u32; 4]> {
    if let Some(grid) = &sheet.grid {
        return (0..grid.rows)
            .flat_map(|row| (0..grid.columns).map(move |column| (column, row)))
            .map(|(column, row)| grid.cell_rect(column, row))
            .collect();
    }
    sheet.sprites.iter().map(|s| s.rect).collect()
}

/// Makes a map one cell at a time, so colours and outlines never reach
/// across the gap between sprites; pixels outside every cell keep the
/// whole-image result.
fn per_cell(
    albedo: &RgbaImage,
    cells: &[[u32; 4]],
    make: impl Fn(&RgbaImage) -> RgbaImage,
) -> RgbaImage {
    let mut out = make(albedo);
    for &[x, y, w, h] in cells {
        let (x1, y1) = ((x + w).min(albedo.width), (y + h).min(albedo.height));
        if x >= x1 || y >= y1 {
            continue;
        }
        let (cw, ch) = (x1 - x, y1 - y);
        let mut crop = RgbaImage { rgba: Vec::new(), width: cw, height: ch };
        for row in y..y1 {
            let start = ((row * albedo.width + x) * 4) as usize;
            crop.rgba.extend_from_slice(&albedo.rgba[start..start + cw as usize * 4]);
        }
        let made = make(&crop);
        for row in 0..ch {
            let to = (((y + row) * albedo.width + x) * 4) as usize;
            let from = (row * cw * 4) as usize;
            out.rgba[to..to + cw as usize * 4]
                .copy_from_slice(&made.rgba[from..from + cw as usize * 4]);
        }
    }
    out
}

/// The most common opaque colour on the image's border: the background of a
/// sprite pasted on a solid colour.
fn detect_background(image: &RgbaImage) -> Option<[u8; 3]> {
    let (w, h) = (image.width as usize, image.height as usize);
    let mut counts: Vec<([u8; 3], u32)> = Vec::new();
    for i in 0..w * h {
        let (x, y) = (i % w, i / w);
        if (x != 0 && y != 0 && x + 1 != w && y + 1 != h) || image.rgba[i * 4 + 3] < 128 {
            continue;
        }
        let c = [image.rgba[i * 4], image.rgba[i * 4 + 1], image.rgba[i * 4 + 2]];
        match counts.iter_mut().find(|(k, _)| *k == c) {
            Some((_, n)) => *n += 1,
            None => counts.push((c, 1)),
        }
    }
    counts.into_iter().max_by_key(|(c, n)| (*n, std::cmp::Reverse(*c))).map(|(c, _)| c)
}

fn hide_colour(image: &RgbaImage, bg: [u8; 3]) -> RgbaImage {
    let mut out = image.clone();
    for p in out.rgba.as_chunks_mut::<4>().0.iter_mut() {
        if p[..3] == bg {
            p[3] = 0;
        }
    }
    out
}

/// What a canvas of `view_size` pixels shows of `image` at `zoom`, with the
/// image's top-left corner `corner` pixels from the canvas's: nearest-pixel
/// scaling, see-through where the image isn't.
fn render_view(
    image: &RgbaImage,
    view_size: (u32, u32),
    corner: (f32, f32),
    zoom: f32,
) -> RgbaImage {
    let (vw, vh) = view_size;
    let mut out = RgbaImage { rgba: vec![0; (vw * vh * 4) as usize], width: vw, height: vh };
    for y in 0..vh {
        let sy = ((y as f32 + 0.5 - corner.1) / zoom).floor();
        if sy < 0.0 || sy >= image.height as f32 {
            continue;
        }
        for x in 0..vw {
            let sx = ((x as f32 + 0.5 - corner.0) / zoom).floor();
            if sx < 0.0 || sx >= image.width as f32 {
                continue;
            }
            let from = ((sy as u32 * image.width + sx as u32) * 4) as usize;
            let to = ((y * vw + x) * 4) as usize;
            out.rgba[to..to + 4].copy_from_slice(&image.rgba[from..from + 4]);
        }
    }
    out
}

fn to_color_image(image: &RgbaImage) -> egui::ColorImage {
    let pixels = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    egui::ColorImage::new([image.width as usize, image.height as usize], pixels)
}

fn rgb_to_hsl([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h * 60.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [u8; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let t = text.trim().trim_start_matches('#');
    if t.len() != 6 || !t.is_ascii() {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&t[i..i + 2], 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

pub(crate) fn reset_slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    default: f32,
    suffix: &str,
) {
    ui.horizontal(|ui| {
        ui.add(egui::Slider::new(value, range).text(label).suffix(suffix));
        if ui.small_button("↺").on_hover_text("Reset").clicked() {
            *value = default;
        }
    });
}

impl LightingEditor {
    /// Opens a texture, or a sprite sheet's texture.
    pub fn open(&mut self, db: &AssetDb, guid: Guid) {
        let texture = match db.kind(guid) {
            Some(AssetKind::SpriteSheet) => {
                db.load(Handle::<SpriteSheet>::new(guid)).ok().map(|s| s.texture.guid)
            }
            _ => Some(guid),
        };
        let (Some(texture), Some(path)) = (texture, texture.and_then(|t| db.path(t))) else {
            self.status = Some("That isn't a texture or a sprite sheet".into());
            return;
        };
        let Some(albedo) = load_image(db, texture) else {
            self.status = Some(format!("Couldn't load {path}"));
            return;
        };
        let (mut sheets, mut cells) = (Vec::new(), Vec::new());
        let (mut normal_file, mut mask_file) = (None, None);
        for info in db.assets().into_iter().filter(|i| i.kind == AssetKind::SpriteSheet) {
            let Ok(sheet) = db.load(Handle::<SpriteSheet>::new(info.guid)) else { continue };
            if sheet.texture.guid != texture {
                continue;
            }
            if sheets.is_empty() {
                cells = sheet_cells(&sheet);
            }
            normal_file =
                normal_file.or_else(|| (!sheet.normal.is_none()).then_some(sheet.normal.guid));
            mask_file = mask_file
                .or_else(|| (!sheet.light_mask.is_none()).then_some(sheet.light_mask.guid));
            sheets.push((info.guid, info.path));
        }
        // Without a link, a file named like the texture's will do.
        normal_file =
            normal_file.or_else(|| db.guid(&sibling_path(&path, MapKind::Normal.suffix())));
        mask_file =
            mask_file.or_else(|| db.guid(&sibling_path(&path, MapKind::LightMask.suffix())));
        let load = |file: Option<Guid>| file.and_then(|g| load_image(db, g)).map(Map::new);
        let (normal, mask) = (load(normal_file), load(mask_file));
        let (normal_file, mask_file) =
            (normal.as_ref().and(normal_file), mask.as_ref().and(mask_file));
        let work = Work { normal, mask };
        self.status = Some(format!("Opened {path}"));
        self.open = Some(OpenLighting {
            guid: texture,
            path,
            albedo: Map::new(albedo),
            sheets,
            cells,
            edits: Edits::new(&work),
            saved: work.clone(),
            work,
            normal_file,
            mask_file,
        });
        self.dialog = None;
        self.albedo_view = None;
        self.preview = None;
        self.light.at = None;
        self.fit = true;
    }

    /// (texture, unsaved changes), for the tab's title.
    pub fn editing(&self) -> Option<(Guid, bool)> {
        self.open.as_ref().map(|o| (o.guid, o.work != o.saved))
    }

    /// A file changed on disk: take it up, unless there are unsaved edits.
    pub fn reloaded(&mut self, db: &AssetDb, guid: Guid) {
        let Some(open) = &self.open else { return };
        let ours =
            guid == open.guid || Some(guid) == open.normal_file || Some(guid) == open.mask_file;
        if !ours || open.work != open.saved {
            return;
        }
        let same = |file: Option<Guid>, map: &Option<Map>| match (
            file.and_then(|g| load_image(db, g)),
            map,
        ) {
            (Some(disk), Some(have)) => disk == *have.value,
            (None, None) => true,
            _ => false,
        };
        let changed = load_image(db, open.guid).is_some_and(|d| d != *open.albedo.value)
            || !same(open.normal_file, &open.work.normal)
            || !same(open.mask_file, &open.work.mask);
        if changed {
            let (view, zoom, pan, light) = (self.view, self.zoom, self.pan, self.light.at);
            self.open(db, open.guid);
            (self.view, self.zoom, self.pan, self.light.at, self.fit) =
                (view, zoom, pan, light, false);
            self.status = Some("Reloaded from disk".into());
        }
    }

    pub fn undo(&mut self) -> bool {
        if let Some(Dialog::Modify(modify)) = &mut self.dialog {
            return modify.undo();
        }
        self.open.as_mut().is_some_and(|o| o.edits.undo(&mut o.work))
    }

    pub fn redo(&mut self) -> bool {
        if let Some(Dialog::Modify(modify)) = &mut self.dialog {
            return modify.redo();
        }
        self.open.as_mut().is_some_and(|o| o.edits.redo(&mut o.work))
    }

    /// The path a map is (or would be) saved at.
    fn map_path(open: &OpenLighting, db: &AssetDb, kind: MapKind) -> String {
        let file = match kind {
            MapKind::Normal => open.normal_file,
            MapKind::LightMask => open.mask_file,
        };
        file.and_then(|g| db.path(g)).unwrap_or_else(|| sibling_path(&open.path, kind.suffix()))
    }

    /// Writes one map's PNG, making the asset if it is new. Returns its GUID.
    fn write_map(
        open: &mut OpenLighting,
        db: &AssetDb,
        kind: MapKind,
    ) -> Result<Option<Guid>, String> {
        let map = match kind {
            MapKind::Normal => open.work.normal.clone(),
            MapKind::LightMask => open.work.mask.clone(),
        };
        let Some(map) = map else { return Ok(None) };
        let bytes = encode_png(&map.value);
        let path = Self::map_path(open, db, kind);
        let guid = match db.guid(&path) {
            Some(guid) => db.write(guid, &bytes).map(|()| guid),
            None => db.create(&path, &bytes),
        }
        .map_err(|e| e.to_string())?;
        match kind {
            MapKind::Normal => open.normal_file = Some(guid),
            MapKind::LightMask => open.mask_file = Some(guid),
        }
        Ok(Some(guid))
    }

    /// Points every sheet that uses the texture at the maps, or at none.
    fn link_sheets(open: &OpenLighting, db: &AssetDb) -> Result<usize, String> {
        let handle = |file: Option<Guid>, map: &Option<Map>| match (file, map) {
            (Some(g), Some(_)) => Handle::<Texture>::new(g),
            _ => Handle::<Texture>::NONE,
        };
        let (normal, mask) =
            (handle(open.normal_file, &open.work.normal), handle(open.mask_file, &open.work.mask));
        let mut changed = 0;
        for (guid, _) in &open.sheets {
            let Ok(sheet) = db.load(Handle::<SpriteSheet>::new(*guid)) else { continue };
            if sheet.normal == normal && sheet.light_mask == mask {
                continue;
            }
            let mut sheet = (*sheet).clone();
            (sheet.normal, sheet.light_mask) = (normal, mask);
            db.write(*guid, to_ron(&sheet).as_bytes()).map_err(|e| e.to_string())?;
            changed += 1;
        }
        Ok(changed)
    }

    /// Saves the maps (or just `only` one) and links them from the sheets.
    fn save(&mut self, db: &AssetDb, only: Option<MapKind>) {
        let Some(open) = &mut self.open else { return };
        if db.root().is_none() {
            self.status = Some("This project can't be saved to".into());
            return;
        }
        let mut written = Vec::new();
        for kind in [MapKind::Normal, MapKind::LightMask] {
            if only.is_some_and(|o| o != kind) {
                continue;
            }
            match Self::write_map(open, db, kind) {
                Ok(Some(_)) => written.push(Self::map_path(open, db, kind)),
                Ok(None) => {}
                Err(e) => {
                    self.status =
                        Some(format!("Couldn't save the {}: {e}", kind.label().to_lowercase()));
                    return;
                }
            }
        }
        let linked = match Self::link_sheets(open, db) {
            Ok(n) => n,
            Err(e) => {
                self.status = Some(format!("Saved, but couldn't update a sprite sheet: {e}"));
                return;
            }
        };
        open.saved = open.work.clone();
        self.status = Some(if open.sheets.is_empty() {
            format!(
                "Saved {}. No sprite sheet uses this texture, so nothing links to them yet: \
                 make one in the Sprites tab.",
                written.join(", ")
            )
        } else {
            format!("Saved {} and updated {linked} sprite sheet(s)", written.join(", "))
        });
    }

    /// Writes `image` to the project's `exports` folder (beside `assets`).
    fn export_png(&mut self, db: &AssetDb, name: &str, image: &RgbaImage) {
        let Some(root) = db.root() else {
            self.status = Some("This project can't be written to".into());
            return;
        };
        let dir = root.parent().unwrap_or(root).join("exports");
        let file = dir.join(name);
        self.status = Some(
            match std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&file, encode_png(image)))
            {
                Ok(()) => format!("Wrote {}", file.display()),
                Err(e) => format!("Couldn't write {}: {e}", file.display()),
            },
        );
    }

    // ---- The view's inputs ---------------------------------------------------

    /// The albedo, with the background hidden if that is on.
    fn albedo_now(&mut self) -> Option<Map> {
        let open = self.open.as_ref()?;
        if !self.hide_bg {
            return Some(open.albedo.clone());
        }
        let bg = self.bg.unwrap_or(Color32::BLACK);
        let key = (open.albedo.rev, true, bg);
        if self.albedo_view.as_ref().is_none_or(|(k, _)| *k != key) {
            let image = hide_colour(&open.albedo.value, [bg.r(), bg.g(), bg.b()]);
            self.albedo_view = Some((key, Map::new(image)));
        }
        self.albedo_view.as_ref().map(|(_, m)| m.clone())
    }

    /// The maps the canvas shows: the saved ones, or the candidate in an open
    /// dialog. The view to show instead of the chosen one, if the dialog's
    /// Show Texture is on.
    fn shown_maps(&mut self) -> (Option<Map>, Option<Map>, Option<ViewMode>) {
        let Some(open) = &self.open else { return (None, None, None) };
        let (mut normal, mut mask) = (open.work.normal.clone(), open.work.mask.clone());
        let mut force = None;
        match &mut self.dialog {
            Some(Dialog::Normal(d)) => {
                if let Some(r) = &d.result {
                    normal = Some(r.clone());
                }
                if d.show_texture {
                    force = Some(ViewMode::Normal);
                }
            }
            Some(Dialog::Mask(d)) => {
                if let Some(r) = &d.result {
                    mask = Some(r.clone());
                }
                if d.show_texture {
                    force = Some(ViewMode::LightMask);
                }
            }
            Some(Dialog::Modify(d)) => {
                let shown = d.shown();
                match d.kind {
                    MapKind::Normal => normal = Some(shown),
                    MapKind::LightMask => mask = Some(shown),
                }
                if d.show_texture {
                    force = Some(match d.kind {
                        MapKind::Normal => ViewMode::Normal,
                        MapKind::LightMask => ViewMode::LightMask,
                    });
                }
            }
            _ => {}
        }
        (normal, mask, force)
    }

    fn preview_light(&self, albedo: &RgbaImage) -> PreviewLight {
        let (cx, cy) =
            self.light.at.unwrap_or((albedo.width as f32 / 2.0, albedo.height as f32 / 2.0));
        let c = self.light.color;
        PreviewLight {
            x: cx,
            y: cy,
            height: self.light.height,
            color: [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0],
            brightness: self.light.brightness,
            falloff: self.light.falloff,
        }
    }

    /// What the canvas draws for `view`.
    fn render_shown(
        &self,
        view: ViewMode,
        albedo: &RgbaImage,
        normal: Option<&RgbaImage>,
        mask: Option<&RgbaImage>,
    ) -> RgbaImage {
        match view {
            ViewMode::Albedo => albedo.clone(),
            ViewMode::Normal => normal.cloned().unwrap_or_else(|| {
                let mut flat = albedo.clone();
                for p in flat.rgba.as_chunks_mut::<4>().0.iter_mut() {
                    p[..3].copy_from_slice(&reign_assets::lighting::FLAT);
                }
                flat
            }),
            ViewMode::LightMask => mask.cloned().unwrap_or_else(|| {
                let mut white = albedo.clone();
                for p in white.rgba.as_chunks_mut::<4>().0.iter_mut() {
                    p[..3].fill(255);
                }
                white
            }),
            ViewMode::Final => shade_image(
                albedo,
                normal,
                mask,
                &self.preview_light(albedo),
                self.light.ambient / 100.0,
                self.light.influence / 100.0,
            ),
        }
    }

    fn preview_key(
        &self,
        view: ViewMode,
        albedo: &Map,
        normal: &Option<Map>,
        mask: &Option<Map>,
    ) -> PreviewKey {
        let l = &self.light;
        let (x, y) = l.at.unwrap_or((-1.0, -1.0));
        let c = l.color;
        PreviewKey {
            view,
            albedo: albedo.rev,
            normal: normal.as_ref().map(|m| m.rev),
            mask: mask.as_ref().map(|m| m.rev),
            light: [
                x.to_bits(),
                y.to_bits(),
                l.brightness.to_bits(),
                l.falloff.to_bits(),
                l.height.to_bits(),
                l.ambient.to_bits(),
                l.influence.to_bits(),
                u32::from(c[0]) << 16 | u32::from(c[1]) << 8 | u32::from(c[2]),
            ],
        }
    }

    // ---- The tab -------------------------------------------------------------

    pub fn ui(&mut self, ui: &mut Ui, db: &AssetDb) {
        if self.open.is_none() {
            ui.weak(
                "Open a texture or a sprite sheet to make its normal map and light mask: \
                 right-click it in the Project panel > Open in Lighting.",
            );
            if let Some(status) = &self.status {
                ui.weak(status);
            }
            return;
        }
        self.poll_dialogs(ui.ctx());
        self.toolbar(ui, db);
        egui::Panel::right("lighting controls").resizable(true).default_size(310.0).show(
            ui,
            |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.controls(ui, db));
            },
        );
        self.canvas(ui);
        self.dialogs(ui.ctx(), db);
        let holding = ui.input(|i| i.pointer.any_down());
        if let Some(open) = &mut self.open {
            open.edits.track(&open.work, holding);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, db: &AssetDb) {
        let Some(open) = &mut self.open else { return };
        let dirty = open.work != open.saved;
        let (w, h) = (open.albedo.value.width, open.albedo.value.height);
        let mut save = false;
        let mut revert = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}{}", open.path, if dirty { "*" } else { "" })).monospace(),
            );
            ui.weak(format!("{w} × {h}"));
            ui.separator();
            if ui.add_enabled(open.edits.can_undo(), egui::Button::new("Undo")).clicked() {
                open.edits.undo(&mut open.work);
            }
            if ui.add_enabled(open.edits.can_redo(), egui::Button::new("Redo")).clicked() {
                open.edits.redo(&mut open.work);
            }
            save =
                ui.add_enabled(dirty && db.root().is_some(), egui::Button::new("Save")).clicked();
            revert = ui.add_enabled(dirty, egui::Button::new("Revert")).clicked();
            if let Some(status) = &self.status {
                ui.weak(status);
            }
        });
        if revert {
            open.work = open.saved.clone();
        }
        ui.separator();
        if save {
            self.save(db, None);
        }
    }

    fn controls(&mut self, ui: &mut Ui, db: &AssetDb) {
        ui.heading("Maps");
        ui.weak("The albedo is the texture itself.");
        let busy = self.dialog.is_some();
        for kind in [MapKind::Normal, MapKind::LightMask] {
            self.slot(ui, db, kind, busy);
        }
        ui.separator();

        ui.heading("View");
        egui::ComboBox::from_id_salt("lighting view").selected_text(self.view.label()).show_ui(
            ui,
            |ui| {
                for v in ViewMode::ALL {
                    ui.selectable_value(&mut self.view, v, v.label());
                }
            },
        );
        ui.horizontal(|ui| {
            let mut bg = self.bg.unwrap_or(Color32::from_gray(40));
            if ui.color_edit_button_srgba(&mut bg).changed() {
                self.bg = Some(bg);
            }
            ui.label("BG Color");
            if ui
                .button("Detect BG")
                .on_hover_text("Take the colour round the image's border")
                .clicked()
                && let Some(open) = &self.open
                && let Some(c) = detect_background(&open.albedo.value)
            {
                self.bg = Some(Color32::from_rgb(c[0], c[1], c[2]));
            }
            ui.checkbox(&mut self.hide_bg, "Hide BG")
                .on_hover_text("Treat pixels of the background colour as see-through");
            if ui.small_button("Clear").on_hover_text("Back to the checkerboard").clicked() {
                self.bg = None;
                self.hide_bg = false;
            }
        });
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.zoom, 0.1..=48.0).logarithmic(true).text("zoom"));
            if ui.button("%Fit").clicked() {
                self.fit = true;
            }
        });
        ui.weak("Wheel or Ctrl+wheel zooms; hold Space and drag, or middle-drag, to pan.");
        ui.separator();

        ui.heading("Light");
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("light cursor")
                .selected_text(match self.light.cursor {
                    CursorStyle::Simple => "Simple",
                    CursorStyle::Advanced => "Advanced",
                    CursorStyle::Off => "Off",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.light.cursor, CursorStyle::Simple, "Simple")
                        .on_hover_text("A dot where the light is");
                    ui.selectable_value(&mut self.light.cursor, CursorStyle::Advanced, "Advanced")
                        .on_hover_text("The dot, where its light runs out, and its height");
                    ui.selectable_value(&mut self.light.cursor, CursorStyle::Off, "Off");
                });
            ui.label("Light Cursor");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.light.follow, "Follow Mouse");
            if ui.button("Reset to centre").clicked() {
                self.light.at = None;
            }
        });
        self.color_ui(ui);
        let l = &mut self.light;
        reset_slider(ui, "Brightness", &mut l.brightness, 0.0..=8.0, 2.5, "");
        reset_slider(ui, "Falloff", &mut l.falloff, 1.0..=500.0, 70.0, " px");
        reset_slider(ui, "Height", &mut l.height, 0.0..=200.0, 25.0, " px");
        reset_slider(ui, "Ambient", &mut l.ambient, 0.0..=100.0, 15.0, "%");
        reset_slider(ui, "Light Mask", &mut l.influence, 0.0..=100.0, 100.0, "%");
        ui.weak("Drag on the image to move the light.");
        ui.separator();

        ui.heading("Export");
        ui.horizontal_wrapped(|ui| {
            if ui
                .button("Final lit PNG")
                .on_hover_text("The whole image, lit, into the project's exports folder")
                .clicked()
            {
                self.export_lit(db);
            }
            if ui.button("Canvas view PNG").on_hover_text("What is on the canvas now").clicked() {
                self.export_canvas(db);
            }
        });
    }

    fn color_ui(&mut self, ui: &mut Ui) {
        let l = &mut self.light;
        ui.horizontal(|ui| {
            ui.label("Light Color");
            for (tab, name) in
                [(ColorTab::Rgb, "RGB"), (ColorTab::Hsl, "HSL"), (ColorTab::Hex, "HEX")]
            {
                ui.selectable_value(&mut l.tab, tab, name);
            }
            let (r, g, b) = (l.color[0], l.color[1], l.color[2]);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
            ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(r, g, b));
        });
        match l.tab {
            ColorTab::Rgb => {
                for (i, name) in ["R", "G", "B"].into_iter().enumerate() {
                    ui.add(egui::Slider::new(&mut l.color[i], 0..=255).text(name));
                }
                l.hex = hex(l.color);
            }
            ColorTab::Hsl => {
                let (mut h, s, lum) = rgb_to_hsl(l.color);
                let (mut s, mut lum) = (s * 100.0, lum * 100.0);
                let before = (h, s, lum);
                ui.add(egui::Slider::new(&mut h, 0.0..=360.0).text("H"));
                ui.add(egui::Slider::new(&mut s, 0.0..=100.0).text("S").suffix("%"));
                ui.add(egui::Slider::new(&mut lum, 0.0..=100.0).text("L").suffix("%"));
                if (h, s, lum) != before {
                    l.color = hsl_to_rgb(h, s / 100.0, lum / 100.0);
                }
                l.hex = hex(l.color);
            }
            ColorTab::Hex => {
                let edit = ui.add(egui::TextEdit::singleline(&mut l.hex).desired_width(90.0));
                if edit.changed()
                    && let Some(c) = parse_hex(&l.hex)
                {
                    l.color = c;
                }
                if edit.lost_focus() {
                    l.hex = hex(l.color);
                }
            }
        }
    }

    fn slot(&mut self, ui: &mut Ui, db: &AssetDb, kind: MapKind, busy: bool) {
        let Some(open) = &mut self.open else { return };
        let has = match kind {
            MapKind::Normal => open.work.normal.is_some(),
            MapKind::LightMask => open.work.mask.is_some(),
        };
        let mut action = None;
        let group = ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(kind.label()).strong());
                ui.weak(if has { "made" } else { "none" });
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(!busy, egui::Button::new("Create…"))
                    .on_hover_text("Make one from the albedo")
                    .clicked()
                {
                    action = Some(Action::Create);
                }
                if ui
                    .add_enabled(!busy && has, egui::Button::new("Modify…"))
                    .on_hover_text("Edit it by hand")
                    .clicked()
                {
                    action = Some(Action::Modify);
                }
                if ui
                    .add_enabled(!busy && has, egui::Button::new("Remove"))
                    .on_hover_text("Unlink it (saving does not delete the file)")
                    .clicked()
                {
                    action = Some(Action::Remove);
                }
                if ui
                    .add_enabled(has && db.root().is_some(), egui::Button::new("Export"))
                    .on_hover_text("Save this map's PNG now")
                    .clicked()
                {
                    action = Some(Action::Export);
                }
                ui.add_enabled_ui(!busy, |ui| {
                    egui::ComboBox::from_id_salt(("link", kind as u8))
                        .selected_text("Link…")
                        .show_ui(ui, |ui| {
                            for info in db
                                .assets()
                                .into_iter()
                                .filter(|i| i.kind == AssetKind::Texture && i.guid != open.guid)
                            {
                                if ui.selectable_label(false, &info.path).clicked() {
                                    action = Some(Action::Link(info.guid));
                                }
                            }
                        });
                });
            });
        });
        // A texture dragged from the Project panel links as this map.
        if let Some(drag) = group.response.dnd_release_payload::<AssetDrag>()
            && drag.kind == AssetKind::Texture
            && !busy
        {
            action = Some(Action::Link(drag.guid));
        } else if group.response.dnd_hover_payload::<AssetDrag>().is_some() && !busy {
            ui.painter().rect_stroke(
                group.response.rect,
                4.0,
                Stroke::new(2.0, Color32::from_rgb(110, 170, 255)),
                egui::StrokeKind::Inside,
            );
        }
        match action {
            Some(Action::Create) => self.start_create(kind),
            Some(Action::Modify) => self.start_modify(kind),
            Some(Action::Remove) => {
                if let Some(open) = &mut self.open {
                    match kind {
                        MapKind::Normal => open.work.normal = None,
                        MapKind::LightMask => open.work.mask = None,
                    }
                }
            }
            Some(Action::Export) => self.save(db, Some(kind)),
            Some(Action::Link(guid)) => self.link(db, kind, guid),
            None => {}
        }
        ui.add_space(4.0);
    }

    /// Takes a texture from the project as a map. A different size asks first.
    fn link(&mut self, db: &AssetDb, kind: MapKind, file: Guid) {
        let Some(open) = &self.open else { return };
        let Some(image) = load_image(db, file) else {
            self.status = Some("Couldn't load that image".into());
            return;
        };
        let size = (open.albedo.value.width, open.albedo.value.height);
        if (image.width, image.height) != size {
            self.dialog = Some(Dialog::Mismatch(Mismatch { kind, file, image }));
            return;
        }
        self.set_map(kind, Some(file), image);
    }

    fn set_map(&mut self, kind: MapKind, file: Option<Guid>, image: RgbaImage) {
        let Some(open) = &mut self.open else { return };
        match kind {
            MapKind::Normal => {
                open.work.normal = Some(Map::new(image));
                open.normal_file = file.or(open.normal_file);
            }
            MapKind::LightMask => {
                open.work.mask = Some(Map::new(image));
                open.mask_file = file.or(open.mask_file);
            }
        }
    }

    fn start_create(&mut self, kind: MapKind) {
        self.dialog = Some(match kind {
            MapKind::Normal => Dialog::Normal(NormalDialog {
                preset: Preset::Object,
                params: Preset::Object.params(),
                per_sprite: false,
                show_texture: false,
                result: None,
                made_with: None,
                job: None,
            }),
            MapKind::LightMask => Dialog::Mask(MaskDialog {
                params: MaskParams::default(),
                show_texture: false,
                result: None,
                made_with: None,
                job: None,
            }),
        });
    }

    fn start_modify(&mut self, kind: MapKind) {
        let Some(open) = &self.open else { return };
        let map = match kind {
            MapKind::Normal => open.work.normal.clone(),
            MapKind::LightMask => open.work.mask.clone(),
        };
        if let Some(map) = map {
            let (w, h) = (open.albedo.value.width, open.albedo.value.height);
            self.dialog = Some(Dialog::Modify(Box::new(ModifyDialog::new(kind, map, w, h))));
        }
    }

    fn export_lit(&mut self, db: &AssetDb) {
        let Some(image) = self.shown.clone().filter(|_| self.view == ViewMode::Final) else {
            self.status = Some("Switch the View to Final to export the lit image".into());
            return;
        };
        let name = self
            .open
            .as_ref()
            .map_or("image".into(), |o| sibling_path(&o.path, "_lit").replace('/', "_"));
        self.export_png(db, &name, &image);
    }

    fn export_canvas(&mut self, db: &AssetDb) {
        let (Some(image), Some((size, corner))) = (self.shown.clone(), self.last_view) else {
            return;
        };
        let view =
            render_view(&image, (size.x as u32, size.y as u32), (corner.x, corner.y), self.zoom);
        let name = self
            .open
            .as_ref()
            .map_or("view".into(), |o| sibling_path(&o.path, "_view").replace('/', "_"));
        self.export_png(db, &name, &view);
    }

    // ---- Dialogs -------------------------------------------------------------

    /// Starts and collects the background work the Create dialogs wait on.
    fn poll_dialogs(&mut self, ctx: &egui::Context) {
        let Some(albedo) = self.albedo_now() else { return };
        let Some(open) = &self.open else { return };
        let cells = open.cells.clone();
        let image = albedo.value.clone();
        match &mut self.dialog {
            Some(Dialog::Normal(d)) => {
                if let Some(done) = d.job.as_ref().and_then(Pending::poll) {
                    d.result = Some(Map::new(done));
                    d.job = None;
                }
                let now = (d.params, d.per_sprite && !cells.is_empty(), albedo.rev);
                if d.made_with != Some(now) {
                    d.made_with = Some(now);
                    let params = d.params;
                    let split = now.1;
                    d.job = Some(Pending::spawn(ctx, move || {
                        let make = |a: &RgbaImage| generate_normal_map(a, &params);
                        if split { per_cell(&image, &cells, make) } else { make(&image) }
                    }));
                }
            }
            Some(Dialog::Mask(d)) => {
                if let Some(done) = d.job.as_ref().and_then(Pending::poll) {
                    d.result = Some(Map::new(done));
                    d.job = None;
                }
                let now = (d.params, albedo.rev);
                if d.made_with != Some(now) {
                    d.made_with = Some(now);
                    let params = d.params;
                    d.job = Some(Pending::spawn(ctx, move || generate_light_mask(&image, &params)));
                }
            }
            _ => {}
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context, db: &AssetDb) {
        let Some(dialog) = self.dialog.take() else { return };
        let (albedo, normal_now, mask_now) = match (self.albedo_now(), self.open.as_ref()) {
            (Some(a), Some(o)) => (a, o.work.normal.clone(), o.work.mask.clone()),
            _ => return,
        };
        let has_cells = self.open.as_ref().is_some_and(|o| !o.cells.is_empty());
        let mut keep = None;
        let mut apply: Option<(MapKind, Map)> = None;
        match dialog {
            Dialog::Normal(mut d) => {
                let mut close = None;
                egui::Window::new("Create Normal Map")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::LEFT_TOP, Vec2::new(12.0, 80.0))
                    .show(ctx, |ui| {
                        ui.set_width(280.0);
                        egui::ComboBox::from_label("Preset")
                            .selected_text(d.preset.name())
                            .show_ui(ui, |ui| {
                                for p in Preset::ALL {
                                    if ui.selectable_label(d.preset == p, p.name()).clicked() {
                                        d.preset = p;
                                        d.params = p.params();
                                    }
                                }
                            });
                        let p = &mut d.params;
                        let pct = |ui: &mut Ui, label: &str, v: &mut f32, max: f32, def: f32| {
                            let mut x = *v * 100.0;
                            reset_slider(ui, label, &mut x, 0.0..=max, def * 100.0, "%");
                            *v = x / 100.0;
                        };
                        let defaults = d.preset.params();
                        pct(ui, "Shape Strength", &mut p.shape_strength, 100.0, defaults.shape_strength);
                        pct(ui, "Normal Strength", &mut p.normal_strength, 150.0, defaults.normal_strength);
                        let mut edge = p.edge_width as f32;
                        reset_slider(ui, "Edge Width", &mut edge, 0.0..=8.0, defaults.edge_width as f32, " px");
                        p.edge_width = edge.round() as u32;
                        pct(ui, "Facet Splitting", &mut p.facet_splitting, 100.0, defaults.facet_splitting);
                        ui.checkbox(&mut p.merge_outlines, "Merge Outlines")
                            .on_hover_text("Thin dark outlines take the surface beside them");
                        if has_cells {
                            ui.checkbox(&mut d.per_sprite, "One sprite at a time")
                                .on_hover_text("Use the sprite sheet's cells, so nothing reaches across between sprites");
                        }
                        ui.checkbox(&mut d.show_texture, "Show Texture")
                            .on_hover_text("Show the map itself instead of the lit result");
                        if d.job.is_some() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.weak("Working…");
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui.add_enabled(d.result.is_some(), egui::Button::new("Apply")).clicked() {
                                close = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                close = Some(false);
                            }
                        });
                    });
                match close {
                    Some(true) => apply = d.result.take().map(|m| (MapKind::Normal, m)),
                    Some(false) => {}
                    None => keep = Some(Dialog::Normal(d)),
                }
            }
            Dialog::Mask(mut d) => {
                let mut close = None;
                egui::Window::new("Create Light Mask")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::LEFT_TOP, Vec2::new(12.0, 80.0))
                    .show(ctx, |ui| {
                        ui.set_width(280.0);
                        let defaults = MaskParams::default();
                        let p = &mut d.params;
                        let pct = |ui: &mut Ui, label: &str, v: &mut f32, def: f32| {
                            let mut x = *v * 100.0;
                            reset_slider(ui, label, &mut x, 0.0..=100.0, def * 100.0, "%");
                            *v = x / 100.0;
                        };
                        pct(
                            ui,
                            "Shading Strength",
                            &mut p.shading_strength,
                            defaults.shading_strength,
                        );
                        pct(ui, "Darkest Value", &mut p.darkest, defaults.darkest);
                        pct(ui, "Lightest Value", &mut p.lightest, defaults.lightest);
                        pct(
                            ui,
                            "Cavity Darkening",
                            &mut p.cavity_darkening,
                            defaults.cavity_darkening,
                        );
                        ui.weak("Made from the albedo's brightness. It does not cast shadows.");
                        ui.checkbox(&mut d.show_texture, "Show Texture")
                            .on_hover_text("Show the mask itself instead of the lit result");
                        if d.job.is_some() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.weak("Working…");
                            });
                        }
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(d.result.is_some(), egui::Button::new("Apply"))
                                .clicked()
                            {
                                close = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                close = Some(false);
                            }
                        });
                    });
                match close {
                    Some(true) => apply = d.result.take().map(|m| (MapKind::LightMask, m)),
                    Some(false) => {}
                    None => keep = Some(Dialog::Mask(d)),
                }
            }
            Dialog::Modify(mut d) => {
                let sources = WandSources {
                    albedo: &albedo.value,
                    normal: normal_now.as_ref().map(|m| &*m.value),
                    mask: mask_now.as_ref().map(|m| &*m.value),
                };
                match d.window(ctx, &sources) {
                    Outcome::Apply => apply = Some((d.kind, d.shown())),
                    Outcome::Cancel => {}
                    Outcome::Open => keep = Some(Dialog::Modify(d)),
                }
            }
            Dialog::Mismatch(m) => {
                let mut choice = None;
                let size =
                    self.open.as_ref().map(|o| (o.albedo.value.width, o.albedo.value.height));
                egui::Window::new("New Image Detected")
                    .collapsible(false)
                    .resizable(false)
                    .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.label(format!(
                            "This image's dimensions ({} × {}) do not match the texture ({} × {}).",
                            m.image.width,
                            m.image.height,
                            size.map_or(0, |s| s.0),
                            size.map_or(0, |s| s.1)
                        ));
                        ui.label("Loading this image will clear the previous maps.");
                        ui.horizontal(|ui| {
                            if ui.button("Continue").clicked() {
                                choice = Some(true);
                            }
                            if ui.button("Cancel").clicked() {
                                choice = Some(false);
                            }
                        });
                    });
                match choice {
                    Some(true) => {
                        if let Some(open) = &mut self.open {
                            open.work.normal = None;
                            open.work.mask = None;
                        }
                        self.set_map(m.kind, Some(m.file), m.image);
                    }
                    Some(false) => {}
                    None => keep = Some(Dialog::Mismatch(m)),
                }
            }
        }
        if let Some((kind, map)) = apply {
            self.set_map(kind, None, (*map.value).clone());
        }
        self.dialog = keep;
        let _ = db;
    }

    // ---- The canvas ----------------------------------------------------------

    fn canvas(&mut self, ui: &mut Ui) {
        let Some(albedo) = self.albedo_now() else { return };
        let (normal, mask, force) = self.shown_maps();
        let view = force.unwrap_or(self.view);
        let key = self.preview_key(view, &albedo, &normal, &mask);
        if self.preview.as_ref().is_none_or(|(k, _)| *k != key) {
            let image = self.render_shown(
                view,
                &albedo.value,
                normal.as_ref().map(|m| &*m.value),
                mask.as_ref().map(|m| &*m.value),
            );
            let color = to_color_image(&image);
            match &mut self.preview {
                Some((k, texture)) => {
                    texture.set(color, egui::TextureOptions::NEAREST);
                    *k = key;
                }
                None => {
                    let texture = ui.ctx().load_texture(
                        "lighting preview",
                        color,
                        egui::TextureOptions::NEAREST,
                    );
                    self.preview = Some((key, texture));
                }
            }
            self.shown = Some(image);
        }
        let Some((_, texture)) = self.preview.clone() else { return };
        let (w, h) = (albedo.value.width, albedo.value.height);

        let space = ui.available_rect_before_wrap();
        let response = ui.interact(space, ui.id().with("lighting canvas"), Sense::click_and_drag());
        ui.allocate_rect(space, Sense::hover());
        let painter = ui.painter_at(space);
        if self.fit {
            let fit = (space.width() / w as f32).min(space.height() / h as f32) * 0.92;
            self.zoom = fit.clamp(0.1, 48.0);
            self.pan = Vec2::ZERO;
            self.fit = false;
        }
        let space_held = ui.input(|i| i.key_down(egui::Key::Space));
        if response.dragged_by(egui::PointerButton::Middle)
            || (space_held && response.dragged_by(egui::PointerButton::Primary))
        {
            self.pan += response.drag_delta();
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            let pinch = ui.input(|i| i.zoom_delta());
            let factor = (scroll * 0.003).exp() * pinch;
            if factor != 1.0
                && let Some(p) = response.hover_pos()
            {
                let new = (self.zoom * factor).clamp(0.1, 48.0);
                // Keep the pixel under the cursor where it is.
                let centre = space.center();
                let under = (p - (centre + self.pan)) / self.zoom;
                self.pan = p - centre - under * new;
                self.zoom = new;
            }
        }
        let size = Vec2::new(w as f32, h as f32) * self.zoom;
        let image_rect = Rect::from_center_size(space.center() + self.pan, size);
        self.last_view = Some((space.size(), image_rect.min - space.min));
        self.last_image_rect = Some(image_rect);

        // The background: a colour, or a checkerboard under see-through pixels.
        match self.bg {
            Some(c) => {
                painter.rect_filled(space, 0.0, c);
            }
            None => {
                painter.rect_filled(space, 0.0, Color32::from_gray(44));
                let cell = 12.0;
                let visible = image_rect.intersect(space);
                if visible.is_positive() {
                    let (x0, y0) = (
                        ((visible.min.x - image_rect.min.x) / cell) as i32,
                        ((visible.min.y - image_rect.min.y) / cell) as i32,
                    );
                    let (x1, y1) = (
                        ((visible.max.x - image_rect.min.x) / cell) as i32 + 1,
                        ((visible.max.y - image_rect.min.y) / cell) as i32 + 1,
                    );
                    for cy in y0..y1 {
                        for cx in x0..x1 {
                            if (cx + cy) % 2 == 0 {
                                continue;
                            }
                            let r = Rect::from_min_size(
                                image_rect.min + Vec2::new(cx as f32, cy as f32) * cell,
                                Vec2::splat(cell),
                            );
                            painter.rect_filled(
                                r.intersect(image_rect).intersect(space),
                                0.0,
                                Color32::from_gray(58),
                            );
                        }
                    }
                }
            }
        }
        painter.image(
            texture.id(),
            image_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );
        painter.rect_stroke(
            image_rect,
            0.0,
            Stroke::new(1.0, Color32::from_gray(90)),
            egui::StrokeKind::Outside,
        );

        let map = CanvasMap { image_rect, zoom: self.zoom, width: w, height: h };
        let hover_image =
            response.hover_pos().map(|p| ((p - image_rect.min) / self.zoom).to_pos2());

        // A Modify tool takes the pointer; otherwise it moves the light.
        let tool_active = matches!(self.dialog, Some(Dialog::Modify(_)));
        if let Some(Dialog::Modify(d)) = &mut self.dialog {
            let sources = WandSources {
                albedo: &albedo.value,
                normal: normal.as_ref().map(|m| &*m.value),
                mask: mask.as_ref().map(|m| &*m.value),
            };
            d.canvas(ui, &response, &map, &sources, space_held);
            d.overlay(&painter, &map, ui.input(|i| i.time));
            ui.ctx().request_repaint();
        } else if !space_held {
            let moving = response.dragged_by(egui::PointerButton::Primary) || response.clicked();
            if moving && let Some(p) = response.interact_pointer_pos().or(response.hover_pos()) {
                let at = (p - image_rect.min) / self.zoom;
                self.light.at = Some((at.x, at.y));
            } else if self.light.follow
                && let Some(p) = hover_image
            {
                self.light.at = Some((p.x, p.y));
            }
        }
        if !tool_active || self.light.cursor != CursorStyle::Off {
            self.draw_light_cursor(&painter, &image_rect, w, h);
        }
    }

    fn draw_light_cursor(&self, painter: &egui::Painter, image_rect: &Rect, w: u32, h: u32) {
        if self.light.cursor == CursorStyle::Off || self.view != ViewMode::Final {
            return;
        }
        let (x, y) = self.light.at.unwrap_or((w as f32 / 2.0, h as f32 / 2.0));
        let centre = image_rect.min + Vec2::new(x, y) * self.zoom;
        let c = self.light.color;
        let tint = Color32::from_rgb(c[0], c[1], c[2]);
        painter.circle_filled(centre, 5.0, tint);
        painter.circle_stroke(centre, 6.0, Stroke::new(1.5, Color32::BLACK));
        if self.light.cursor == CursorStyle::Advanced {
            painter.circle_stroke(
                centre,
                self.light.falloff * self.zoom,
                Stroke::new(1.0, tint.gamma_multiply(0.6)),
            );
            painter.text(
                centre + Vec2::new(9.0, -9.0),
                egui::Align2::LEFT_BOTTOM,
                format!("h {:.0}", self.light.height),
                egui::FontId::monospace(11.0),
                Color32::WHITE,
            );
        }
    }
}

/// Whether a texture is itself a generated map or export, which batch
/// generation leaves alone.
fn is_generated(path: &str) -> bool {
    let stem = path.rsplit_once('.').map_or(path, |(s, _)| s);
    ["_n", "_m", "_lit", "_view"].iter().any(|suffix| stem.ends_with(suffix))
}

/// What generating one texture's maps came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Generated {
    Made,
    AlreadyHad,
    Failed,
}

/// Makes the normal map and light mask (the Object preset and the default
/// mask settings) a texture is missing, one sprite at a time when a sheet
/// has cells, and links them from the sheets.
fn generate_one(db: &AssetDb, texture: Guid) -> Generated {
    let mut editor = LightingEditor::default();
    editor.open(db, texture);
    let Some(open) = &editor.open else { return Generated::Failed };
    if open.work.normal.is_some() && open.work.mask.is_some() {
        return Generated::AlreadyHad;
    }
    let (albedo, cells) = (open.albedo.value.clone(), open.cells.clone());
    let (need_normal, need_mask) = (open.work.normal.is_none(), open.work.mask.is_none());
    let (params, mask) = (Preset::Object.params(), MaskParams::default());
    let make = |image: &RgbaImage, normal: bool| {
        let one = |a: &RgbaImage| {
            if normal { generate_normal_map(a, &params) } else { generate_light_mask(a, &mask) }
        };
        if cells.is_empty() { one(image) } else { per_cell(image, &cells, one) }
    };
    if need_normal {
        editor.set_map(MapKind::Normal, None, make(&albedo, true));
    }
    if need_mask {
        editor.set_map(MapKind::LightMask, None, make(&albedo, false));
    }
    editor.save(db, None);
    if editor.editing().is_some_and(|(_, dirty)| dirty) {
        Generated::Failed
    } else {
        Generated::Made
    }
}

fn generated_summary(made: usize, had: usize, failed: &[String]) -> String {
    let mut text = format!("Made lighting maps for {made} texture(s)");
    if had > 0 {
        text.push_str(&format!(", {had} already had both"));
    }
    if !failed.is_empty() {
        text.push_str(&format!("; couldn't do {}", failed.join(", ")));
    }
    text
}

/// Makes the missing lighting maps for every texture in `folder` (`""` for
/// the whole project), leaving the maps and exports themselves alone.
/// Says what it did.
pub(crate) fn generate_for_folder(db: &AssetDb, folder: &str) -> String {
    let prefix = if folder.is_empty() { String::new() } else { format!("{folder}/") };
    let (mut made, mut had, mut failed) = (0, 0, Vec::new());
    for info in db.assets() {
        if info.kind != AssetKind::Texture
            || !info.path.starts_with(&prefix)
            || is_generated(&info.path)
        {
            continue;
        }
        match generate_one(db, info.guid) {
            Generated::Made => made += 1,
            Generated::AlreadyHad => had += 1,
            Generated::Failed => failed.push(info.path),
        }
    }
    generated_summary(made, had, &failed)
}

/// The same for one texture (or a sprite sheet's texture).
pub(crate) fn generate_for_texture(db: &AssetDb, guid: Guid) -> String {
    let texture = match db.kind(guid) {
        Some(AssetKind::SpriteSheet) => {
            db.load(Handle::<SpriteSheet>::new(guid)).ok().map(|s| s.texture.guid)
        }
        _ => Some(guid),
    };
    let Some(texture) = texture else { return "That isn't a texture or a sprite sheet".into() };
    let path = db.path(texture).unwrap_or_default();
    match generate_one(db, texture) {
        Generated::Made => generated_summary(1, 0, &[]),
        Generated::AlreadyHad => generated_summary(0, 1, &[]),
        Generated::Failed => generated_summary(0, 0, &[path]),
    }
}

enum Action {
    Create,
    Modify,
    Remove,
    Export,
    Link(Guid),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        RgbaImage { rgba, width: w, height: h }
    }

    #[test]
    fn map_files_sit_beside_the_texture() {
        assert_eq!(sibling_path("sprites/hero.png", "_n"), "sprites/hero_n.png");
        assert_eq!(sibling_path("hero.png", "_m"), "hero_m.png");
        assert_eq!(sibling_path("a.b/hero.png", "_n"), "a.b/hero_n.png");
        assert_eq!(sibling_path("noext", "_n"), "noext_n.png");
    }

    #[test]
    fn colours_convert_between_rgb_hsl_and_hex() {
        for c in [[255, 0, 0], [12, 200, 90], [255, 255, 255], [0, 0, 0], [128, 64, 200]] {
            let (h, s, l) = rgb_to_hsl(c);
            let back = hsl_to_rgb(h, s, l);
            for i in 0..3 {
                assert!((i32::from(c[i]) - i32::from(back[i])).abs() <= 1, "{c:?} -> {back:?}");
            }
        }
        assert_eq!(rgb_to_hsl([255, 0, 0]).0, 0.0);
        assert!((rgb_to_hsl([0, 255, 0]).0 - 120.0).abs() < 0.01);
        assert_eq!(hex([255, 128, 0]), "#FF8000");
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("00FF7f"), Some([0, 255, 127]));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("#gg0000"), None);
    }

    #[test]
    fn the_background_is_the_commonest_opaque_colour_on_the_border() {
        let image = img(5, 5, |x, y| {
            if x == 0 || y == 0 || x == 4 || y == 4 {
                [10, 200, 10, 255]
            } else {
                [200, 10, 10, 255]
            }
        });
        assert_eq!(detect_background(&image), Some([10, 200, 10]));
        let clear = img(3, 3, |_, _| [0, 0, 0, 0]);
        assert_eq!(detect_background(&clear), None);
        let hidden = hide_colour(&image, [10, 200, 10]);
        assert_eq!(hidden.rgba[3], 0, "the border is see-through");
        assert_eq!(hidden.rgba[(2 * 5 + 2) * 4 + 3], 255, "the middle is not");
    }

    #[test]
    fn making_a_map_one_cell_at_a_time_gives_each_cell_the_map_it_would_get_alone() {
        // Two blobs of one colour that touch across the line between cells.
        let sheet = img(8, 6, |x, y| {
            if (1..=6).contains(&x) && (1..=4).contains(&y) {
                [200, 90 + x as u8 * 10, 60, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        let params = Preset::Character.params();
        let make = |a: &RgbaImage| generate_normal_map(a, &params);
        let cells = [[0, 0, 4, 6], [4, 0, 4, 6]];
        let out = per_cell(&sheet, &cells, make);
        assert_eq!((out.width, out.height), (8, 6));
        for [x, _, w, h] in cells {
            let mut crop = RgbaImage { rgba: Vec::new(), width: w, height: h };
            for row in 0..h {
                let start = ((row * 8 + x) * 4) as usize;
                crop.rgba.extend_from_slice(&sheet.rgba[start..start + w as usize * 4]);
            }
            let alone = make(&crop);
            for row in 0..h {
                let from = (row * w * 4) as usize;
                let to = ((row * 8 + x) * 4) as usize;
                assert_eq!(
                    out.rgba[to..to + w as usize * 4],
                    alone.rgba[from..from + w as usize * 4],
                    "cell at {x}, row {row}"
                );
            }
        }
        // Cells larger than the image are clipped, not a crash.
        let _ = per_cell(&sheet, &[[6, 2, 100, 100]], make);
    }

    #[test]
    fn a_view_of_an_image_scales_it_with_nearest_pixels() {
        let image = img(2, 1, |x, _| if x == 0 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        // Zoom 2, the image's corner 1 pixel in from the canvas's left.
        let view = render_view(&image, (6, 3), (1.0, 0.0), 2.0);
        let px = |x: usize, y: usize| view.rgba[(y * 6 + x) * 4..(y * 6 + x) * 4 + 4].to_vec();
        assert_eq!(px(0, 0), [0, 0, 0, 0], "left of the image");
        assert_eq!(px(1, 0), [255, 0, 0, 255]);
        assert_eq!(px(2, 1), [255, 0, 0, 255], "the pixel is two tall at zoom 2");
        assert_eq!(px(3, 0), [0, 0, 255, 255]);
        assert_eq!(px(5, 0), [0, 0, 0, 0], "right of it");
        assert_eq!(px(1, 2), [0, 0, 0, 0], "below it");
    }

    #[test]
    fn revisions_say_whether_a_shared_image_changed() {
        let mut a = Map::new(img(2, 2, |_, _| [1, 2, 3, 4]));
        let b = a.clone();
        assert!(a == b);
        a.edit(|i| i.rgba[0] = 99);
        assert!(a != b, "an edit is a new revision");
        assert_eq!(b.value.rgba[0], 1, "and the clone still has the old pixels");
        assert_eq!(a.value.rgba[0], 99);
    }

    /// A project in a temp folder with a sprite and a sheet that uses it.
    struct Project {
        dir: std::path::PathBuf,
        db: AssetDb,
        texture: Guid,
        sheet: Guid,
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn orb(size: u32) -> RgbaImage {
        let c = (size as f32 - 1.0) / 2.0;
        img(size, size, |x, y| {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            if d <= c - 1.0 { [200, 120 + (x * 3) as u8, 70, 255] } else { [0, 0, 0, 0] }
        })
    }

    fn project(name: &str) -> Project {
        let dir =
            std::env::temp_dir().join(format!("reign-lighting-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hero.png"), encode_png(&orb(24))).unwrap();
        std::fs::write(dir.join("other.png"), encode_png(&orb(16))).unwrap();
        let texture = AssetDb::loose(&dir).unwrap().guid("hero.png").unwrap();
        let sheet =
            SpriteSheet { texture: Handle::new(texture), density: 1, ..SpriteSheet::default() };
        std::fs::write(dir.join("hero.sheet.ron"), to_ron(&sheet)).unwrap();
        let db = AssetDb::loose(&dir).unwrap();
        let sheet = db.guid("hero.sheet.ron").unwrap();
        Project { dir, db, texture, sheet }
    }

    fn frame(editor: &mut LightingEditor, db: &AssetDb) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(input, |ui| editor.ui(ui, db));
        output.textures_delta.clear();
    }

    /// Runs frames until a Create dialog has its result.
    fn wait_for_result(editor: &mut LightingEditor, db: &AssetDb) -> Map {
        for _ in 0..500 {
            frame(editor, db);
            let result = match &editor.dialog {
                Some(Dialog::Normal(d)) => d.result.clone(),
                Some(Dialog::Mask(d)) => d.result.clone(),
                _ => None,
            };
            if let Some(result) = result {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the dialog never produced a map");
    }

    #[test]
    fn creating_and_saving_maps_writes_the_pngs_and_links_the_sheet() {
        let p = project("save");
        let mut editor = LightingEditor::default();
        editor.open(&p.db, p.texture);
        assert_eq!(editor.editing(), Some((p.texture, false)));
        assert_eq!(editor.open.as_ref().unwrap().sheets.len(), 1);

        editor.start_create(MapKind::Normal);
        let normal = wait_for_result(&mut editor, &p.db);
        assert_eq!((normal.value.width, normal.value.height), (24, 24));
        editor.set_map(MapKind::Normal, None, (*normal.value).clone());
        editor.dialog = None;
        editor.start_create(MapKind::LightMask);
        let mask = wait_for_result(&mut editor, &p.db);
        editor.set_map(MapKind::LightMask, None, (*mask.value).clone());
        editor.dialog = None;
        assert_eq!(editor.editing(), Some((p.texture, true)), "unsaved");

        editor.save(&p.db, None);
        assert_eq!(editor.editing(), Some((p.texture, false)), "{:?}", editor.status);
        assert!(p.dir.join("hero_n.png").exists() && p.dir.join("hero_m.png").exists());
        let n_guid = p.db.guid("hero_n.png").expect("a registered asset");
        let m_guid = p.db.guid("hero_m.png").unwrap();
        let sheet = p.db.load(Handle::<SpriteSheet>::new(p.sheet)).unwrap();
        assert_eq!(sheet.normal.guid, n_guid);
        assert_eq!(sheet.light_mask.guid, m_guid);
        assert_eq!(sheet.texture.guid, p.texture, "the rest of the sheet is untouched");
        // What was written is the map.
        let on_disk = load_image(&p.db, n_guid).unwrap();
        assert_eq!(on_disk, *normal.value);

        // A fresh editor finds them through the sheet and is not dirty.
        let mut again = LightingEditor::default();
        again.open(&p.db, p.sheet);
        let open = again.open.as_ref().unwrap();
        assert_eq!(open.guid, p.texture, "opening the sheet opens its texture");
        assert!(open.work.normal.is_some() && open.work.mask.is_some());
        assert_eq!(again.editing(), Some((p.texture, false)));

        // Removing a map and saving unlinks it (the file stays).
        editor.open.as_mut().unwrap().work.normal = None;
        editor.save(&p.db, None);
        let sheet = p.db.load(Handle::<SpriteSheet>::new(p.sheet)).unwrap();
        assert!(sheet.normal.is_none() && !sheet.light_mask.is_none());
        assert!(p.dir.join("hero_n.png").exists());
    }

    #[test]
    fn a_map_of_the_wrong_size_asks_before_it_clears_the_others() {
        let p = project("mismatch");
        let mut editor = LightingEditor::default();
        editor.open(&p.db, p.texture);
        let good = orb(24);
        editor.set_map(MapKind::LightMask, None, good);
        let other = p.db.guid("other.png").unwrap();
        editor.link(&p.db, MapKind::Normal, other);
        assert!(matches!(editor.dialog, Some(Dialog::Mismatch(_))));
        frame(&mut editor, &p.db);
        assert!(matches!(editor.dialog, Some(Dialog::Mismatch(_))), "it waits for an answer");
        assert!(editor.open.as_ref().unwrap().work.mask.is_some(), "nothing cleared yet");
        // A map of the right size just links.
        editor.dialog = None;
        let same = p.db.guid("hero.png").unwrap();
        editor.link(&p.db, MapKind::Normal, same);
        assert!(editor.dialog.is_none());
        assert!(editor.open.as_ref().unwrap().work.normal.is_some());
    }

    #[test]
    fn every_view_and_dialog_draws() {
        let p = project("views");
        let mut editor = LightingEditor::default();
        editor.open(&p.db, p.texture);
        editor.start_create(MapKind::Normal);
        let normal = wait_for_result(&mut editor, &p.db);
        editor.set_map(MapKind::Normal, None, (*normal.value).clone());
        editor.dialog = None;
        for view in ViewMode::ALL {
            editor.view = view;
            for cursor in [CursorStyle::Simple, CursorStyle::Advanced, CursorStyle::Off] {
                editor.light.cursor = cursor;
                frame(&mut editor, &p.db);
            }
        }
        editor.view = ViewMode::Final;
        editor.hide_bg = true;
        editor.bg = Some(Color32::from_rgb(1, 2, 3));
        frame(&mut editor, &p.db);
        editor.start_modify(MapKind::Normal);
        frame(&mut editor, &p.db);
        assert!(matches!(editor.dialog, Some(Dialog::Modify(_))));
        editor.dialog = None;
        editor.start_create(MapKind::LightMask);
        let _ = wait_for_result(&mut editor, &p.db);
        assert!(editor.shown.is_some(), "the canvas drew");
    }

    /// One frame on a context that lives on, so the pointer's state does too.
    fn run(
        ctx: &egui::Context,
        editor: &mut LightingEditor,
        db: &AssetDb,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        // Held keys reach egui as an event of their own.
        let mut all = vec![egui::Event::ModifiersChanged(modifiers)];
        all.extend(events);
        // Time moves on between frames, as it does for a person clicking.
        let time = ctx.input(|i| i.time) + 0.2;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
            time: Some(time),
            events: all,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| editor.ui(ui, db));
        output.textures_delta.clear();
    }

    /// A click at `at` with `modifiers` held, as three frames.
    fn click(
        ctx: &egui::Context,
        editor: &mut LightingEditor,
        db: &AssetDb,
        at: Pos2,
        modifiers: egui::Modifiers,
    ) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        run(ctx, editor, db, vec![egui::Event::PointerMoved(at)], modifiers);
        run(ctx, editor, db, vec![button(true)], modifiers);
        run(ctx, editor, db, vec![button(false)], modifiers);
    }

    fn selected(editor: &LightingEditor) -> usize {
        match &editor.dialog {
            Some(Dialog::Modify(d)) => d.selected(),
            _ => panic!("no Modify dialog"),
        }
    }

    #[test]
    fn clicking_the_canvas_with_the_wand_selects_a_colour_cluster_and_shift_adds() {
        let p = project("wand");
        let mut editor = LightingEditor::default();
        editor.open(&p.db, p.texture);
        let normal = generate_normal_map(&orb(24), &Preset::Object.params());
        editor.set_map(MapKind::Normal, None, normal);
        editor.start_modify(MapKind::Normal);
        let ctx = egui::Context::default();
        let none = egui::Modifiers::NONE;
        run(&ctx, &mut editor, &p.db, vec![], none);
        run(&ctx, &mut editor, &p.db, vec![], none);
        let rect = editor.last_image_rect.expect("the canvas drew");
        assert_eq!(selected(&editor), 0);

        // The orb's colour changes with x, so a cluster is a column.
        let zoom = editor.zoom;
        let pixel = |x: f32, y: f32| rect.min + Vec2::new(x + 0.5, y + 0.5) * zoom;
        click(&ctx, &mut editor, &p.db, pixel(12.0, 12.0), none);
        let one = selected(&editor);
        assert!(one > 5 && one < 24, "one column: {one}");
        // Shift adds another column; a plain click replaces.
        click(&ctx, &mut editor, &p.db, pixel(14.0, 12.0), egui::Modifiers::SHIFT);
        assert!(selected(&editor) > one, "added to it");
        click(&ctx, &mut editor, &p.db, pixel(12.0, 12.0), none);
        assert_eq!(selected(&editor), one, "replaced it");
        // Alt takes the column out again.
        click(&ctx, &mut editor, &p.db, pixel(12.0, 12.0), egui::Modifiers::ALT);
        assert_eq!(selected(&editor), 0);
    }

    #[test]
    fn generating_for_a_folder_makes_the_missing_maps_once_and_links_sheets() {
        let p = project("batch");
        let text = generate_for_folder(&p.db, "");
        assert_eq!(text, "Made lighting maps for 2 texture(s)", "hero and other");
        for name in ["hero_n.png", "hero_m.png", "other_n.png", "other_m.png"] {
            assert!(p.dir.join(name).exists(), "{name}");
        }
        let sheet = p.db.load(Handle::<SpriteSheet>::new(p.sheet)).unwrap();
        assert_eq!(sheet.normal.guid, p.db.guid("hero_n.png").unwrap());
        assert_eq!(sheet.light_mask.guid, p.db.guid("hero_m.png").unwrap());
        // The maps are textures too, but are left alone, and a second run finds both made.
        let again = generate_for_folder(&p.db, "");
        assert_eq!(again, "Made lighting maps for 0 texture(s), 2 already had both");
        assert!(!p.dir.join("hero_n_n.png").exists());
        // One texture by itself, through its sheet.
        assert_eq!(
            generate_for_texture(&p.db, p.sheet),
            "Made lighting maps for 0 texture(s), 1 already had both"
        );
    }

    #[test]
    fn generated_files_are_recognised_by_name() {
        assert!(is_generated("sprites/hero_n.png"));
        assert!(is_generated("hero_m.png"));
        assert!(is_generated("hero_lit.png"));
        assert!(!is_generated("sprites/hero.png"));
        assert!(!is_generated("sprites/normal_book.png"));
    }

    #[test]
    fn light_defaults_match_the_tool() {
        let l = LightSettings::default();
        assert_eq!(
            (l.brightness, l.falloff, l.height, l.ambient, l.influence),
            (2.5, 70.0, 25.0, 15.0, 100.0)
        );
    }
}
