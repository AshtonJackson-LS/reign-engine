//! The Project panel and the asset side of the Inspector.
//!
//! The panel shows the asset folder as a tree beside a grid of thumbnails,
//! with search and a type filter. Folders and assets can be created,
//! renamed, moved (drag onto a folder) and deleted, always together with
//! their `.meta` files, so GUIDs and every reference survive. Deleting an
//! asset that other assets or scenes refer to asks first and lists them.
//!
//! Assets drag out of the panel as [`AssetDrag`]: onto a handle field in
//! the Inspector to set it, or into the Game view to create an object
//! ([`crate::EditorApp::create_from_asset`]). Selecting an asset shows its
//! import settings in the Inspector, with Apply and Revert. Reloaded
//! assets flash in the grid.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, Rect, RichText, Sense, Stroke, TextureHandle, Ui, Vec2};
use reign_assets::{
    AssetChange, AssetDb, AssetInfo, AssetKind, Guid, Handle, Importer, Meta, SpriteSheet, Texture,
    Tileset,
};

use crate::inspector::{Pick, fields, file_label};

/// An asset being dragged out of the Project panel. `sprite` names one
/// sprite of a sheet when that is what was dragged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetDrag {
    pub guid: Guid,
    pub kind: AssetKind,
    pub sprite: Option<String>,
}

/// How long a reloaded asset stays highlighted.
const FLASH: Duration = Duration::from_secs(3);
/// Thumbnail edge in points.
const THUMB: f32 = 64.0;

/// Asset images as egui textures, by GUID and version, so a reload shows.
#[derive(Default)]
pub struct Thumbs {
    images: HashMap<Guid, (u32, Option<TextureHandle>)>,
}

impl Thumbs {
    /// The texture's image, if `guid` is a texture that loads.
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        db: &AssetDb,
        guid: Guid,
    ) -> Option<TextureHandle> {
        let version = db.version(guid);
        if let Some((seen, image)) = self.images.get(&guid)
            && *seen == version
        {
            return image.clone();
        }
        let image = db.load(Handle::<Texture>::new(guid)).ok().map(|t| {
            let size = [t.image.width as usize, t.image.height as usize];
            let pixels = egui::ColorImage::from_rgba_unmultiplied(size, &t.image.rgba);
            ctx.load_texture(format!("asset {guid}"), pixels, egui::TextureOptions::NEAREST)
        });
        self.images.insert(guid, (version, image.clone()));
        image
    }

    /// The image that best stands for an asset: its own, or its sheet's,
    /// tileset's or font's texture.
    pub fn preview(
        &mut self,
        ctx: &egui::Context,
        db: &AssetDb,
        info: &AssetInfo,
    ) -> Option<TextureHandle> {
        let texture = match info.kind {
            AssetKind::Texture => Some(info.guid),
            AssetKind::SpriteSheet => {
                db.load(Handle::<SpriteSheet>::new(info.guid)).ok().map(|s| s.texture.guid)
            }
            AssetKind::Tileset => {
                db.load(Handle::<Tileset>::new(info.guid)).ok().map(|t| t.texture.guid)
            }
            AssetKind::Font => {
                db.load(Handle::<reign_assets::Font>::new(info.guid)).ok().map(|f| f.texture.guid)
            }
            AssetKind::AnimationClip => db
                .load(Handle::<reign_assets::AnimationClip>::new(info.guid))
                .ok()
                .and_then(|c| db.load(c.sheet).ok())
                .map(|s| s.texture.guid),
            _ => None,
        }?;
        self.texture(ctx, db, texture)
    }
}

/// What the panel asks the editor to open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Open {
    Tilemap(Guid),
    Slicer(Guid),
    Animation(Guid),
    /// Prefab Mode.
    Prefab(Guid),
}

/// What the Project panel and the asset Inspector keep between frames.
#[derive(Default)]
pub struct ProjectState {
    /// The folder shown in the grid, `""` for the root.
    pub folder: String,
    pub search: String,
    pub filter: Option<AssetKind>,
    pub selected: Option<Guid>,
    /// The selected folder, when a folder rather than an asset is selected.
    pub selected_folder: Option<String>,
    pub thumbs: Thumbs,
    /// When each asset last reloaded, for the flash.
    pub reloaded: BTreeMap<Guid, Instant>,
    /// A rename in progress: what, and the new name being typed.
    renaming: Option<(Target, String)>,
    /// A new folder's name being typed.
    new_folder: Option<String>,
    /// A delete waiting for confirmation, with what refers to it.
    confirm: Option<(Target, Vec<String>)>,
    /// The selected asset's import settings, as being edited.
    import: Option<(Guid, Meta)>,
    /// A request for the editor, taken each frame.
    pub open: Option<Open>,
    /// An object dragged from the Hierarchy onto a folder: make it a prefab
    /// there.
    pub make_prefab: Option<(reign_scene::ObjectId, String)>,
    /// What happened, for the toolbar's status.
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Asset(Guid),
    Folder(String),
}

impl ProjectState {
    /// Notes reloaded assets so they flash.
    pub fn changed(&mut self, changes: &[AssetChange]) {
        let now = Instant::now();
        for change in changes {
            if change.removed {
                self.reloaded.remove(&change.guid);
                if self.selected == Some(change.guid) {
                    self.selected = None;
                }
            } else {
                self.reloaded.insert(change.guid, now);
            }
            // A meta edited on disk replaces the settings being edited.
            if self.import.as_ref().is_some_and(|(g, _)| *g == change.guid) {
                self.import = None;
            }
        }
        self.reloaded.retain(|_, t| t.elapsed() < FLASH);
    }

    fn flash(&self, guid: Guid) -> Option<f32> {
        let t = self.reloaded.get(&guid)?.elapsed();
        (t < FLASH).then(|| 1.0 - t.as_secs_f32() / FLASH.as_secs_f32())
    }
}

/// Everything in the asset folder that mentions `guid`: other assets and
/// scenes holding a handle to it.
pub fn referrers(db: &AssetDb, guid: Guid) -> Vec<String> {
    let needle = guid.to_string();
    db.assets()
        .into_iter()
        .filter(|i| i.guid != guid && i.path.ends_with(".ron"))
        .filter(|i| {
            db.bytes(i.guid).is_ok_and(|b| b.windows(needle.len()).any(|w| w == needle.as_bytes()))
        })
        .map(|i| i.path)
        .collect()
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_owned() } else { format!("{dir}/{name}") }
}

fn kind_color(kind: AssetKind) -> Color32 {
    match kind {
        AssetKind::Texture => Color32::from_rgb(110, 170, 230),
        AssetKind::SpriteSheet => Color32::from_rgb(120, 200, 160),
        AssetKind::AnimationClip => Color32::from_rgb(230, 170, 90),
        AssetKind::Tileset => Color32::from_rgb(160, 200, 100),
        AssetKind::Tilemap => Color32::from_rgb(200, 220, 90),
        AssetKind::AudioClip | AssetKind::Music => Color32::from_rgb(200, 130, 220),
        AssetKind::Font => Color32::from_rgb(220, 220, 220),
        AssetKind::Scene => Color32::from_rgb(240, 120, 120),
        AssetKind::Prefab => Color32::from_rgb(90, 160, 240),
        AssetKind::MoveSet => Color32::from_rgb(240, 90, 90),
        AssetKind::Data => Color32::GRAY,
    }
}

/// The Project panel.
pub fn project(ui: &mut Ui, db: &AssetDb, state: &mut ProjectState) {
    if db.root().is_none() {
        ui.weak("Packed assets are read-only. Run the editor on the asset folder to edit them.");
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.search).hint_text("Search").desired_width(160.0),
        );
        egui::ComboBox::from_id_salt("asset kind filter")
            .selected_text(state.filter.map_or("All types", |k| k.name()))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.filter, None, "All types");
                for kind in AssetKind::ALL {
                    ui.selectable_value(&mut state.filter, Some(kind), kind.name());
                }
            });
        ui.separator();
        let crumbs = if state.folder.is_empty() {
            "assets".to_owned()
        } else {
            format!("assets/{}", state.folder)
        };
        ui.label(RichText::new(crumbs).monospace());
        if !state.folder.is_empty() && ui.small_button("⬆").on_hover_text("Up a folder").clicked()
        {
            state.folder = parent(&state.folder).to_owned();
        }
        if db.root().is_some() && ui.small_button("+ Folder").clicked() {
            state.new_folder = Some(String::new());
        }
    });
    if let Some(name) = &mut state.new_folder {
        let mut done = None;
        ui.horizontal(|ui| {
            ui.label("New folder in this one:");
            let r = ui.text_edit_singleline(name);
            r.request_focus();
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                done = Some(true);
            }
            if ui.button("Create").clicked() {
                done = Some(true);
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            }
        });
        if let Some(create) = done {
            let name = state.new_folder.take().unwrap_or_default();
            if create && !name.trim().is_empty() {
                let path = join(&state.folder, name.trim());
                state.status = Some(match db.create_folder(&path) {
                    Ok(()) => format!("Created folder {path}"),
                    Err(e) => format!("Couldn't create {path}: {e}"),
                });
            }
        }
    }
    ui.separator();
    let assets = db.assets();
    let folders = db.folders();
    egui::Panel::left("project folders").resizable(true).default_size(170.0).show(ui, |ui| {
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            folder_tree(ui, db, state, &folders, "");
        });
    });
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        grid(ui, db, state, &assets, &folders);
    });
    confirm_delete(ui, db, state);
}

fn folder_tree(ui: &mut Ui, db: &AssetDb, state: &mut ProjectState, folders: &[String], dir: &str) {
    let children: Vec<&String> = folders.iter().filter(|f| parent(f) == dir).collect();
    let name = if dir.is_empty() { "assets".to_owned() } else { file_label(dir) };
    let selected = state.folder == dir;
    let body = |ui: &mut Ui, state: &mut ProjectState| {
        for child in &children {
            folder_tree(ui, db, state, folders, child);
        }
    };
    let response = if children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.selectable_label(selected, name.clone())
        })
        .inner
    } else {
        let id = ui.make_persistent_id(("folder", dir));
        let header = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            id,
            dir.is_empty() || state.folder.starts_with(dir),
        );
        let (_, header, _) = header
            .show_header(ui, |ui| ui.selectable_label(selected, name.clone()))
            .body(|ui| body(ui, state));
        header.inner
    };
    if response.clicked() {
        state.folder = dir.to_owned();
        state.selected_folder = Some(dir.to_owned());
        state.selected = None;
    }
    drop_target(&response, db, state, dir);
}

/// Moves what is dropped on `response` into `dir`.
fn drop_target(response: &egui::Response, db: &AssetDb, state: &mut ProjectState, dir: &str) {
    if db.root().is_none() {
        return;
    }
    if response.dnd_hover_payload::<AssetDrag>().is_some()
        || response.dnd_hover_payload::<FolderDrag>().is_some()
        || response.dnd_hover_payload::<reign_scene::ObjectId>().is_some()
    {
        response.clone().highlight();
    }
    if let Some(object) = response.dnd_release_payload::<reign_scene::ObjectId>() {
        state.make_prefab = Some((*object, dir.to_owned()));
    }
    if let Some(drag) = response.dnd_release_payload::<AssetDrag>()
        && let Some(path) = db.path(drag.guid)
    {
        let to = join(dir, file_label(&path).as_str());
        if to != path {
            state.status = Some(match db.rename(drag.guid, &to) {
                Ok(()) => format!("Moved {path} to {to}"),
                Err(e) => format!("Couldn't move {path}: {e}"),
            });
        }
    }
    if let Some(drag) = response.dnd_release_payload::<FolderDrag>() {
        let from = &drag.0;
        let to = join(dir, &file_label(from));
        if to != *from && !dir.starts_with(&format!("{from}/")) && dir != from {
            state.status = Some(match db.rename_folder(from, &to) {
                Ok(()) => {
                    if state.folder.starts_with(from.as_str()) {
                        state.folder = to.clone();
                    }
                    format!("Moved {from} to {to}")
                }
                Err(e) => format!("Couldn't move {from}: {e}"),
            });
        }
    }
}

/// A folder being dragged in the grid.
#[derive(Clone, Debug)]
struct FolderDrag(String);

fn grid(
    ui: &mut Ui,
    db: &AssetDb,
    state: &mut ProjectState,
    assets: &[AssetInfo],
    folders: &[String],
) {
    let search = state.search.to_lowercase();
    let searching = !search.is_empty() || state.filter.is_some();
    // Searching looks through every folder; otherwise the grid shows the
    // current folder.
    let shown: Vec<&AssetInfo> = assets
        .iter()
        .filter(|a| {
            if searching {
                state.folder.is_empty() || a.path.starts_with(&format!("{}/", state.folder))
            } else {
                parent(&a.path) == state.folder
            }
        })
        .filter(|a| state.filter.is_none_or(|k| a.kind == k))
        .filter(|a| search.is_empty() || a.path.to_lowercase().contains(&search))
        .collect();
    let subfolders: Vec<&String> = if searching {
        Vec::new()
    } else {
        folders.iter().filter(|f| parent(f) == state.folder).collect()
    };
    let cell = Vec2::new(THUMB + 24.0, THUMB + 34.0);
    let gap = ui.spacing().item_spacing.x;
    let columns = (((ui.available_width() + gap) / (cell.x + gap)).floor() as usize).max(1);
    let ctx = ui.ctx().clone();
    let mut items: Vec<Item> = subfolders.into_iter().map(|f| Item::Folder(f.clone())).collect();
    items.extend(shown.into_iter().map(|a| Item::Asset(a.clone())));
    if items.is_empty() {
        ui.weak(if searching { "Nothing matches." } else { "This folder is empty." });
    }
    for row in items.chunks(columns) {
        ui.horizontal(|ui| {
            for item in row {
                let (rect, response) = ui.allocate_exact_size(cell, Sense::click_and_drag());
                match item {
                    Item::Folder(path) => folder_cell(ui, db, state, path, rect, &response),
                    Item::Asset(info) => asset_cell(ui, &ctx, db, state, info, rect, &response),
                }
            }
        });
    }
    // The space below takes objects dragged from the Hierarchy: each
    // becomes a prefab in this folder.
    let space = ui.available_size().max(Vec2::new(0.0, 48.0));
    let (rect, response) = ui.allocate_exact_size(space, Sense::hover());
    if response.dnd_hover_payload::<reign_scene::ObjectId>().is_some() {
        ui.painter().rect_stroke(
            rect.shrink(2.0),
            2.0,
            (1.0, Color32::LIGHT_BLUE),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to make a prefab",
            egui::FontId::proportional(13.0),
            Color32::LIGHT_BLUE,
        );
    }
    if let Some(object) = response.dnd_release_payload::<reign_scene::ObjectId>() {
        state.make_prefab = Some((*object, state.folder.clone()));
    }
}

enum Item {
    Folder(String),
    Asset(AssetInfo),
}

fn folder_cell(
    ui: &mut Ui,
    db: &AssetDb,
    state: &mut ProjectState,
    path: &str,
    rect: Rect,
    response: &egui::Response,
) {
    let painter = ui.painter();
    let selected = state.selected_folder.as_deref() == Some(path) && state.selected.is_none();
    if selected || response.hovered() {
        painter.rect_filled(rect, 4.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let icon = Rect::from_center_size(
        rect.center_top() + Vec2::new(0.0, 6.0 + THUMB / 2.0),
        Vec2::splat(THUMB * 0.7),
    );
    painter.rect_filled(icon, 6.0, Color32::from_rgb(190, 160, 90));
    name_label(ui, db, state, rect, Target::Folder(path.to_owned()), &file_label(path));
    if response.double_clicked() {
        state.folder = path.to_owned();
        state.selected_folder = None;
    } else if response.clicked() {
        state.selected_folder = Some(path.to_owned());
        state.selected = None;
    }
    response.dnd_set_drag_payload(FolderDrag(path.to_owned()));
    drop_target(response, db, state, path);
    folder_menu(response, db, state, path);
}

#[allow(clippy::too_many_arguments)]
fn asset_cell(
    ui: &mut Ui,
    ctx: &egui::Context,
    db: &AssetDb,
    state: &mut ProjectState,
    info: &AssetInfo,
    rect: Rect,
    response: &egui::Response,
) {
    let selected = state.selected == Some(info.guid);
    if selected || response.hovered() {
        let fill = if selected {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.hovered.weak_bg_fill
        };
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    let thumb = Rect::from_min_size(rect.min + Vec2::new(12.0, 4.0), Vec2::splat(THUMB));
    ui.painter().rect_filled(thumb, 2.0, Color32::from_gray(28));
    match state.thumbs.preview(ctx, db, info) {
        Some(texture) => {
            let size = texture.size_vec2();
            let fit = (THUMB / size.x).min(THUMB / size.y);
            let shown = Rect::from_center_size(thumb.center(), size * fit);
            ui.painter().image(
                texture.id(),
                shown,
                Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        None => {
            ui.painter().text(
                thumb.center(),
                egui::Align2::CENTER_CENTER,
                info.kind.name(),
                egui::FontId::proportional(11.0),
                kind_color(info.kind),
            );
        }
    }
    // The kind as a colour strip, and the reload flash.
    let strip =
        Rect::from_min_size(thumb.left_bottom() - Vec2::new(0.0, 3.0), Vec2::new(THUMB, 3.0));
    ui.painter().rect_filled(strip, 0.0, kind_color(info.kind));
    if let Some(flash) = state.flash(info.guid) {
        let color = Color32::from_rgba_unmultiplied(120, 230, 140, (200.0 * flash) as u8);
        ui.painter().rect_stroke(
            thumb.expand(2.0),
            3.0,
            Stroke::new(2.0, color),
            egui::StrokeKind::Outside,
        );
        ui.painter().text(
            thumb.right_top() + Vec2::new(-2.0, 2.0),
            egui::Align2::RIGHT_TOP,
            "↻",
            egui::FontId::proportional(14.0),
            color,
        );
    }
    name_label(ui, db, state, rect, Target::Asset(info.guid), &file_label(&info.path));
    let response = response.clone().on_hover_text(format!(
        "{}\n{}\n{}",
        info.path,
        info.kind.name(),
        info.guid
    ));
    if response.clicked() {
        state.selected = Some(info.guid);
        state.selected_folder = None;
    }
    if response.double_clicked() {
        state.open = match info.kind {
            AssetKind::Tilemap => Some(Open::Tilemap(info.guid)),
            AssetKind::SpriteSheet | AssetKind::Texture => Some(Open::Slicer(info.guid)),
            AssetKind::AnimationClip => Some(Open::Animation(info.guid)),
            AssetKind::Prefab => Some(Open::Prefab(info.guid)),
            _ => None,
        };
    }
    response.dnd_set_drag_payload(AssetDrag { guid: info.guid, kind: info.kind, sprite: None });
    response.context_menu(|ui| {
        match info.kind {
            AssetKind::Tilemap if ui.button("Open in Tilemap painter").clicked() => {
                state.open = Some(Open::Tilemap(info.guid));
                ui.close();
            }
            AssetKind::SpriteSheet | AssetKind::Texture
                if ui.button("Open in Sprite slicer").clicked() =>
            {
                state.open = Some(Open::Slicer(info.guid));
                ui.close();
            }
            AssetKind::AnimationClip if ui.button("Open in Animation editor").clicked() => {
                state.open = Some(Open::Animation(info.guid));
                ui.close();
            }
            AssetKind::Prefab if ui.button("Open in Prefab Mode").clicked() => {
                state.open = Some(Open::Prefab(info.guid));
                ui.close();
            }
            _ => {}
        }
        if ui.button("Copy GUID").clicked() {
            ui.ctx().copy_text(info.guid.to_string());
            ui.close();
        }
        if db.root().is_some() {
            if ui.button("Rename").clicked() {
                state.renaming = Some((Target::Asset(info.guid), file_label(&info.path)));
                ui.close();
            }
            if ui.button("Delete").clicked() {
                state.confirm = Some((Target::Asset(info.guid), referrers(db, info.guid)));
                ui.close();
            }
        }
    });
}

fn folder_menu(response: &egui::Response, db: &AssetDb, state: &mut ProjectState, path: &str) {
    if db.root().is_none() {
        return;
    }
    response.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            state.renaming = Some((Target::Folder(path.to_owned()), file_label(path)));
            ui.close();
        }
        if ui.button("Delete").clicked() {
            state.confirm = Some((Target::Folder(path.to_owned()), Vec::new()));
            ui.close();
        }
    });
}

/// The name under a grid cell, or the rename box while renaming it.
fn name_label(
    ui: &mut Ui,
    db: &AssetDb,
    state: &mut ProjectState,
    rect: Rect,
    target: Target,
    name: &str,
) {
    let area = Rect::from_min_max(rect.left_bottom() - Vec2::new(0.0, 28.0), rect.right_bottom());
    let renaming = state.renaming.as_ref().is_some_and(|(t, _)| *t == target);
    if !renaming {
        let galley = ui.painter().layout(
            name.to_owned(),
            egui::FontId::proportional(11.0),
            ui.visuals().text_color(),
            area.width() - 4.0,
        );
        ui.painter().galley(
            area.center_top() - Vec2::new(galley.size().x / 2.0, 0.0),
            galley,
            Color32::WHITE,
        );
        return;
    }
    let Some((_, text)) = &mut state.renaming else { return };
    let r = ui.put(area, egui::TextEdit::singleline(text).font(egui::TextStyle::Small));
    r.request_focus();
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escape {
        state.renaming = None;
    } else if enter || r.lost_focus() {
        let (target, new) = state.renaming.take().expect("renaming");
        let new = new.trim().to_owned();
        if new.is_empty() || new.contains('/') {
            return;
        }
        state.status = Some(match target {
            Target::Asset(guid) => {
                let old = db.path(guid).unwrap_or_default();
                let to = join(parent(&old), &new);
                match (to == old, db.rename(guid, &to)) {
                    (true, _) => return,
                    (false, Ok(())) => format!("Renamed {old} to {to}"),
                    (false, Err(e)) => format!("Couldn't rename {old}: {e}"),
                }
            }
            Target::Folder(old) => {
                let to = join(parent(&old), &new);
                match (to == old, db.rename_folder(&old, &to)) {
                    (true, _) => return,
                    (false, Ok(())) => format!("Renamed {old} to {to}"),
                    (false, Err(e)) => format!("Couldn't rename {old}: {e}"),
                }
            }
        });
    }
}

fn confirm_delete(ui: &mut Ui, db: &AssetDb, state: &mut ProjectState) {
    let Some((target, refs)) = state.confirm.clone() else { return };
    let what = match &target {
        Target::Asset(guid) => db.path(*guid).unwrap_or_default(),
        Target::Folder(path) => format!("{path}/"),
    };
    let mut close = false;
    egui::Window::new("Delete?")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ui.ctx(), |ui| {
            ui.label(format!("Delete {what} and its .meta?"));
            if let Target::Folder(_) = target {
                ui.weak("Only empty folders can be deleted.");
            }
            if !refs.is_empty() {
                ui.colored_label(
                    Color32::from_rgb(235, 190, 90),
                    format!(
                        "{} file(s) refer to it and will be left with a missing reference:",
                        refs.len()
                    ),
                );
                for r in refs.iter().take(12) {
                    ui.label(RichText::new(r).monospace());
                }
                if refs.len() > 12 {
                    ui.weak(format!("and {} more", refs.len() - 12));
                }
            }
            ui.horizontal(|ui| {
                let label = if refs.is_empty() { "Delete" } else { "Delete anyway" };
                if ui.button(label).clicked() {
                    state.status = Some(match &target {
                        Target::Asset(guid) => match db.delete(*guid) {
                            Ok(()) => format!("Deleted {what}"),
                            Err(e) => format!("Couldn't delete {what}: {e}"),
                        },
                        Target::Folder(path) => match db.delete_folder(path) {
                            Ok(()) => format!("Deleted {what}"),
                            Err(e) => format!("Couldn't delete {what}: {e}"),
                        },
                    });
                    if matches!(target, Target::Asset(g) if state.selected == Some(g)) {
                        state.selected = None;
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if close {
        state.confirm = None;
    }
}

/// The Inspector for the selected asset: what it is, a preview, its import
/// settings with Apply and Revert, and its sprites to drag out.
pub fn inspector(ui: &mut Ui, db: &AssetDb, state: &mut ProjectState, guid: Guid) {
    let Some(path) = db.path(guid) else {
        ui.weak("The selected asset is gone.");
        return;
    };
    let kind = db.kind(guid).unwrap_or(AssetKind::Data);
    ui.label(RichText::new(file_label(&path)).strong().size(15.0));
    ui.label(RichText::new(&path).monospace().weak());
    ui.horizontal(|ui| {
        ui.colored_label(kind_color(kind), kind.name());
        ui.weak(format!("· {guid}"));
        if kind.sim_affecting() {
            ui.weak("· simulation data").on_hover_text(
                "Included in the online content hash; hot reload is ignored during online play.",
            );
        }
    });
    let refs = referrers(db, guid);
    if !refs.is_empty() {
        ui.collapsing(format!("Used by {} file(s)", refs.len()), |ui| {
            for r in refs {
                ui.label(RichText::new(r).monospace());
            }
        });
    }
    ui.separator();

    // Import settings, edited on a copy until Apply.
    let saved = db.meta(guid);
    if state.import.as_ref().is_none_or(|(g, _)| *g != guid) {
        state.import = saved.clone().map(|m| (guid, m));
    }
    if let Some((_, meta)) = &mut state.import {
        ui.label(RichText::new("Import settings").strong());
        let pick = Pick {
            objects: Vec::new(),
            assets: Some(db),
            overridden: Vec::new(),
            revert: Default::default(),
        };
        match &mut meta.importer {
            Importer::Texture(settings) => fields(ui, settings, &pick, ("import", guid)),
            Importer::Audio(settings) => fields(ui, settings, &pick, ("import", guid)),
            other => {
                ui.weak(format!("{other:?}: nothing to set."));
            }
        }
        let changed = saved.as_ref() != Some(&*meta);
        ui.horizontal(|ui| {
            let writable = db.root().is_some();
            if ui.add_enabled(changed && writable, egui::Button::new("Apply")).clicked() {
                state.status = Some(match db.write_meta(guid, meta) {
                    Ok(()) => format!("Applied import settings to {path}"),
                    Err(e) => format!("Couldn't apply: {e}"),
                });
            }
            if ui.add_enabled(changed, egui::Button::new("Revert")).clicked()
                && let Some(saved) = &saved
            {
                *meta = saved.clone();
            }
            if changed {
                ui.colored_label(Color32::from_rgb(235, 190, 90), "unapplied");
            }
        });
        ui.separator();
    }

    match kind {
        AssetKind::Tilemap => {
            if ui.button("Open in Tilemap painter").clicked() {
                state.open = Some(Open::Tilemap(guid));
            }
        }
        AssetKind::Texture => {
            if ui.button("Slice into a sprite sheet").clicked() {
                state.open = Some(Open::Slicer(guid));
            }
        }
        AssetKind::SpriteSheet => {
            if ui.button("Open in Sprite slicer").clicked() {
                state.open = Some(Open::Slicer(guid));
            }
        }
        AssetKind::AnimationClip if ui.button("Open in Animation editor").clicked() => {
            state.open = Some(Open::Animation(guid));
        }
        _ => {}
    }
    let ctx = ui.ctx().clone();
    if let Some(info) = db.assets().into_iter().find(|i| i.guid == guid)
        && let Some(texture) = state.thumbs.preview(&ctx, db, &info)
    {
        let size = texture.size_vec2();
        let fit = (ui.available_width() / size.x).min(4.0).min(240.0 / size.y);
        ui.add(
            egui::Image::new((texture.id(), size * fit))
                .texture_options(egui::TextureOptions::NEAREST),
        );
        ui.weak(format!("{} x {}", size.x, size.y));
    }
    if kind == AssetKind::SpriteSheet
        && let Ok(sheet) = db.load(Handle::<SpriteSheet>::new(guid))
    {
        sprites(ui, db, state, guid, &sheet);
    }
}

/// A sheet's sprites, each draggable into the Game view.
fn sprites(
    ui: &mut Ui,
    db: &AssetDb,
    state: &mut ProjectState,
    guid: Guid,
    sheet: &Arc<SpriteSheet>,
) {
    if sheet.sprites.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    let Some(texture) = state.thumbs.texture(&ctx, db, sheet.texture.guid) else { return };
    let size = texture.size_vec2();
    ui.label(RichText::new(format!("Sprites ({})", sheet.sprites.len())).strong());
    ui.weak("Drag a sprite into the Game view to place it.");
    let per_row = ((ui.available_width() / 52.0).floor() as usize).max(1);
    for row in sheet.sprites.chunks(per_row) {
        ui.horizontal(|ui| {
            for sprite in row {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::splat(48.0), Sense::click_and_drag());
                ui.painter().rect_filled(rect, 2.0, Color32::from_gray(28));
                let [x, y, w, h] = sprite.rect.map(|v| v as f32);
                let uv = Rect::from_min_size(
                    egui::pos2(x / size.x, y / size.y),
                    Vec2::new(w / size.x, h / size.y),
                );
                let fit = (44.0 / w).min(44.0 / h);
                ui.painter().image(
                    texture.id(),
                    Rect::from_center_size(rect.center(), Vec2::new(w, h) * fit),
                    uv,
                    Color32::WHITE,
                );
                let response = response.on_hover_text(&sprite.name);
                response.dnd_set_drag_payload(AssetDrag {
                    guid,
                    kind: AssetKind::SpriteSheet,
                    sprite: Some(sprite.name.clone()),
                });
            }
        });
    }
}
