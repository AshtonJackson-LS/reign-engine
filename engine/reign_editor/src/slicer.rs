//! The Sprite slicer: cuts a texture into a [`SpriteSheet`].
//!
//! Grid slicing takes a cell size, an offset and the padding between
//! cells, and names the cells `prefix_0`, `prefix_1` and so on (empty cells
//! are skipped). Sprites can also be dragged out by hand on the image.
//! Each sprite has a name, a rect and a pivot; the sheet has a density.
//! Save writes the sheet next to its texture (`name.sheet.ron`), or over
//! the sheet that was opened.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use reign_assets::{AssetDb, AssetKind, Grid, Guid, Handle, Sprite, SpriteSheet, Texture, to_ron};

use crate::assets_ui::Thumbs;
use crate::undo::Edits;

pub struct Slicer {
    open: Option<OpenSheet>,
    cell: [u32; 2],
    offset: [u32; 2],
    padding: [u32; 2],
    /// Pivot for sliced cells, from the cell's top-left.
    pivot: [i32; 2],
    prefix: String,
    /// Keep the grid in the sheet too (for animation rows).
    keep_grid: bool,
    zoom: f32,
    selected: Option<usize>,
    /// A sprite being dragged out by hand, from this texel.
    drag_from: Option<[u32; 2]>,
    status: Option<String>,
}

struct OpenSheet {
    /// The sheet's asset, if it exists yet.
    guid: Option<Guid>,
    /// Where it saves.
    path: String,
    sheet: SpriteSheet,
    saved: SpriteSheet,
    edits: Edits<SpriteSheet>,
}

impl Default for Slicer {
    fn default() -> Self {
        Slicer {
            open: None,
            cell: [16, 16],
            offset: [0, 0],
            padding: [0, 0],
            pivot: [8, 16],
            prefix: "sprite".into(),
            keep_grid: false,
            zoom: 2.0,
            selected: None,
            drag_from: None,
            status: None,
        }
    }
}

/// The cells of `grid` across an image, skipping fully transparent ones.
pub fn slice(
    image: &reign_assets::RgbaImage,
    grid: Grid,
    prefix: &str,
    pivot: [i32; 2],
) -> Vec<Sprite> {
    let mut out = Vec::new();
    for row in 0..grid.rows {
        for column in 0..grid.columns {
            let rect = grid.cell_rect(column, row);
            if empty(image, rect) {
                continue;
            }
            let name = format!("{prefix}_{}", out.len());
            out.push(Sprite { name, rect, pivot });
        }
    }
    out
}

/// How many cells of `cell` fit across `size`, with the offset and padding.
pub fn fit(size: u32, cell: u32, offset: u32, padding: u32) -> u32 {
    if cell == 0 || size < offset + cell {
        return 0;
    }
    (size - offset + padding) / (cell + padding)
}

fn empty(image: &reign_assets::RgbaImage, [x, y, w, h]: [u32; 4]) -> bool {
    for py in y..(y + h).min(image.height) {
        for px in x..(x + w).min(image.width) {
            if image.rgba[((py * image.width + px) * 4 + 3) as usize] != 0 {
                return false;
            }
        }
    }
    true
}

impl Slicer {
    /// Opens a sheet, or a texture's sheet (a new one if it has none).
    pub fn open(&mut self, db: &AssetDb, guid: Guid) {
        let Some(path) = db.path(guid) else { return };
        let open = match db.kind(guid) {
            Some(AssetKind::SpriteSheet) => match db.load(Handle::<SpriteSheet>::new(guid)) {
                Ok(sheet) => Some((Some(guid), path.clone(), (*sheet).clone())),
                Err(e) => {
                    self.status = Some(format!("Couldn't open {path}: {e}"));
                    None
                }
            },
            Some(AssetKind::Texture) => {
                // The sheet already cut from this texture, if any.
                let existing =
                    db.assets().into_iter().filter(|i| i.kind == AssetKind::SpriteSheet).find(
                        |i| {
                            db.load(Handle::<SpriteSheet>::new(i.guid))
                                .is_ok_and(|s| s.texture.guid == guid)
                        },
                    );
                match existing {
                    Some(info) => {
                        let sheet =
                            db.load(Handle::<SpriteSheet>::new(info.guid)).expect("checked");
                        Some((Some(info.guid), info.path, (*sheet).clone()))
                    }
                    None => {
                        let density = match db.meta(guid).map(|m| m.importer) {
                            Some(reign_assets::Importer::Texture(t)) => t.density.max(1),
                            _ => 1,
                        };
                        let sheet = SpriteSheet {
                            texture: Handle::new(guid),
                            density,
                            ..SpriteSheet::default()
                        };
                        let base = path.strip_suffix(".png").unwrap_or(&path);
                        Some((None, format!("{base}.sheet.ron"), sheet))
                    }
                }
            }
            _ => None,
        };
        if let Some((guid, path, sheet)) = open {
            self.status =
                Some(format!("Opened {path}{}", if guid.is_none() { " (new)" } else { "" }));
            if let Some(grid) = sheet.grid {
                (self.cell, self.offset, self.padding) = (grid.cell, grid.offset, grid.padding);
                self.pivot = sheet.cell_pivot;
                self.keep_grid = true;
            }
            self.prefix = path
                .rsplit('/')
                .next()
                .unwrap_or("sprite")
                .split('.')
                .next()
                .unwrap_or("sprite")
                .to_owned();
            self.selected = None;
            self.open = Some(OpenSheet {
                guid,
                path,
                edits: Edits::new(&sheet),
                saved: sheet.clone(),
                sheet,
            });
        }
    }

    pub fn editing(&self) -> Option<(Option<Guid>, bool)> {
        self.open.as_ref().map(|o| (o.guid, o.sheet != o.saved || o.guid.is_none()))
    }

    pub fn undo(&mut self) -> bool {
        self.open.as_mut().is_some_and(|o| o.edits.undo(&mut o.sheet))
    }

    pub fn redo(&mut self) -> bool {
        self.open.as_mut().is_some_and(|o| o.edits.redo(&mut o.sheet))
    }

    pub fn ui(&mut self, ui: &mut Ui, db: &AssetDb, thumbs: &mut Thumbs) {
        let Some(open) = &self.open else {
            ui.weak("Open a texture or sprite sheet: double-click one in the Project panel.");
            if let Some(status) = &self.status {
                ui.weak(status);
            }
            return;
        };
        let texture_guid = open.sheet.texture.guid;
        let image = db.load(Handle::<Texture>::new(texture_guid)).ok();
        let handle = thumbs.texture(ui.ctx(), db, texture_guid);
        self.toolbar(ui, db);
        egui::Panel::left("slicer settings").resizable(true).default_size(230.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| self.settings(ui, image.as_deref()));
        });
        if let (Some(image), Some(handle)) = (image, handle) {
            self.canvas(ui, &image, &handle);
        } else {
            ui.weak("The sheet's texture doesn't load.");
        }
        let holding = ui.input(|i| i.pointer.any_down());
        if let Some(open) = &mut self.open {
            open.edits.track(&open.sheet, holding);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, db: &AssetDb) {
        let Some(open) = &mut self.open else { return };
        let dirty = open.sheet != open.saved || open.guid.is_none();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}{}", open.path, if dirty { "*" } else { "" })).monospace(),
            );
            ui.weak(format!("{} sprites", open.sheet.sprites.len()));
            ui.separator();
            ui.add(egui::Slider::new(&mut self.zoom, 0.5..=8.0).logarithmic(true).text("zoom"));
            if ui.add_enabled(open.edits.can_undo(), egui::Button::new("Undo")).clicked() {
                open.edits.undo(&mut open.sheet);
            }
            if ui.add_enabled(open.edits.can_redo(), egui::Button::new("Redo")).clicked() {
                open.edits.redo(&mut open.sheet);
            }
            let writable = db.root().is_some();
            if ui.add_enabled(dirty && writable, egui::Button::new("Save")).clicked() {
                let text = to_ron(&open.sheet);
                let result = match open.guid {
                    Some(guid) => db.write(guid, text.as_bytes()).map(|()| guid),
                    None => db.create(&open.path, text.as_bytes()),
                };
                self.status = Some(match result {
                    Ok(guid) => {
                        open.guid = Some(guid);
                        open.saved = open.sheet.clone();
                        format!("Saved {}", open.path)
                    }
                    Err(e) => format!("Couldn't save: {e}"),
                });
            }
            if let Some(status) = &self.status {
                ui.weak(status);
            }
        });
        ui.separator();
    }

    fn settings(&mut self, ui: &mut Ui, image: Option<&Texture>) {
        let Some(open) = &mut self.open else { return };
        ui.label(RichText::new("Sheet").strong());
        egui::Grid::new("sheet settings").num_columns(2).show(ui, |ui| {
            ui.label("Density");
            ui.add(egui::DragValue::new(&mut open.sheet.density).range(1..=4))
                .on_hover_text("Texels per world pixel");
            ui.end_row();
        });
        ui.separator();
        ui.label(RichText::new("Grid slicing").strong());
        let pair = |ui: &mut Ui, label: &str, v: &mut [u32; 2], lo: u32| {
            ui.label(label);
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut v[0]).range(lo..=4096).prefix("x "));
                ui.add(egui::DragValue::new(&mut v[1]).range(lo..=4096).prefix("y "));
            });
            ui.end_row();
        };
        egui::Grid::new("grid settings").num_columns(2).show(ui, |ui| {
            pair(ui, "Cell", &mut self.cell, 1);
            pair(ui, "Offset", &mut self.offset, 0);
            pair(ui, "Padding", &mut self.padding, 0);
            ui.label("Pivot");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.pivot[0]).prefix("x "));
                ui.add(egui::DragValue::new(&mut self.pivot[1]).prefix("y "));
            });
            ui.end_row();
            ui.label("Names");
            ui.text_edit_singleline(&mut self.prefix);
            ui.end_row();
        });
        ui.checkbox(&mut self.keep_grid, "Keep as the sheet's grid")
            .on_hover_text("Animation clips address grid cells by row and column");
        if let Some(texture) = image {
            let (w, h) = (texture.image.width, texture.image.height);
            let columns = fit(w, self.cell[0], self.offset[0], self.padding[0]);
            let rows = fit(h, self.cell[1], self.offset[1], self.padding[1]);
            ui.weak(format!("{columns} x {rows} cells in {w} x {h}"));
            if ui
                .button("Slice")
                .on_hover_text("Replaces the sprites; empty cells are skipped")
                .clicked()
            {
                let grid = Grid {
                    cell: self.cell,
                    offset: self.offset,
                    padding: self.padding,
                    columns,
                    rows,
                };
                open.sheet.sprites = slice(&texture.image, grid, &self.prefix, self.pivot);
                open.sheet.grid = self.keep_grid.then_some(grid);
                open.sheet.cell_pivot = self.pivot;
                self.selected = None;
            }
        }
        ui.separator();
        ui.label(RichText::new("Sprites").strong());
        ui.weak("Drag on the image to add one by hand.");
        let mut remove = None;
        egui::ScrollArea::vertical().max_height(220.0).id_salt("sprite list").show(ui, |ui| {
            for (i, sprite) in open.sheet.sprites.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.selected == Some(i), &sprite.name).clicked() {
                        self.selected = Some(i);
                    }
                    if ui.small_button("x").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = remove {
            open.sheet.sprites.remove(i);
            self.selected = None;
        }
        if let Some(sprite) = self.selected.and_then(|i| open.sheet.sprites.get_mut(i)) {
            ui.separator();
            egui::Grid::new("sprite fields").num_columns(2).show(ui, |ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut sprite.name);
                ui.end_row();
                ui.label("Rect");
                ui.horizontal(|ui| {
                    for (v, p) in sprite.rect.iter_mut().zip(["x ", "y ", "w ", "h "]) {
                        ui.add(egui::DragValue::new(v).prefix(p));
                    }
                });
                ui.end_row();
                ui.label("Pivot");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut sprite.pivot[0]).prefix("x "));
                    ui.add(egui::DragValue::new(&mut sprite.pivot[1]).prefix("y "));
                });
                ui.end_row();
            });
        }
    }

    fn canvas(&mut self, ui: &mut Ui, texture: &Texture, handle: &egui::TextureHandle) {
        let Some(open) = &mut self.open else { return };
        let size = Vec2::new(texture.image.width as f32, texture.image.height as f32) * self.zoom;
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
            let painter = ui.painter_at(rect);
            // A checkerboard shows transparency.
            let check = 8.0 * self.zoom.max(1.0);
            let clip = ui.clip_rect().intersect(rect);
            let (cx0, cy0) = (
                ((clip.min.x - rect.min.x) / check) as i32,
                ((clip.min.y - rect.min.y) / check) as i32,
            );
            let (cx1, cy1) = (
                ((clip.max.x - rect.min.x) / check) as i32 + 1,
                ((clip.max.y - rect.min.y) / check) as i32 + 1,
            );
            for cy in cy0..cy1 {
                for cx in cx0..cx1 {
                    let shade = if (cx + cy) % 2 == 0 { 50 } else { 64 };
                    let r = Rect::from_min_size(
                        rect.min + Vec2::new(cx as f32, cy as f32) * check,
                        Vec2::splat(check),
                    );
                    painter.rect_filled(r, 0.0, Color32::from_gray(shade));
                }
            }
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            painter.image(handle.id(), rect, uv, Color32::WHITE);
            let to_screen = |r: [u32; 4]| {
                Rect::from_min_size(
                    rect.min + Vec2::new(r[0] as f32, r[1] as f32) * self.zoom,
                    Vec2::new(r[2] as f32, r[3] as f32) * self.zoom,
                )
            };
            for (i, sprite) in open.sheet.sprites.iter().enumerate() {
                let r = to_screen(sprite.rect);
                let selected = self.selected == Some(i);
                let color = if selected {
                    Color32::YELLOW
                } else {
                    Color32::from_rgba_unmultiplied(120, 220, 160, 180)
                };
                painter.rect_stroke(
                    r,
                    0.0,
                    Stroke::new(if selected { 2.0 } else { 1.0 }, color),
                    egui::StrokeKind::Inside,
                );
                let pivot =
                    r.min + Vec2::new(sprite.pivot[0] as f32, sprite.pivot[1] as f32) * self.zoom;
                painter.circle_stroke(
                    pivot,
                    3.0,
                    Stroke::new(1.0, Color32::from_rgb(255, 120, 200)),
                );
                if selected || self.zoom >= 2.0 {
                    painter.text(
                        r.left_top() + Vec2::new(2.0, 1.0),
                        egui::Align2::LEFT_TOP,
                        &sprite.name,
                        egui::FontId::proportional(10.0),
                        color,
                    );
                }
            }
            let to_texel = |p: Pos2| {
                let local = (p - rect.min) / self.zoom;
                [
                    (local.x.max(0.0) as u32).min(texture.image.width),
                    (local.y.max(0.0) as u32).min(texture.image.height),
                ]
            };
            if response.clicked()
                && let Some(p) = response.interact_pointer_pos()
            {
                let [x, y] = to_texel(p);
                self.selected = open.sheet.sprites.iter().position(|s| {
                    let [sx, sy, sw, sh] = s.rect;
                    (sx..sx + sw).contains(&x) && (sy..sy + sh).contains(&y)
                });
            }
            if response.drag_started_by(egui::PointerButton::Primary) {
                self.drag_from = response.interact_pointer_pos().map(to_texel);
            }
            if let (Some(from), Some(p)) = (self.drag_from, response.interact_pointer_pos()) {
                let to = to_texel(p);
                let r = [
                    from[0].min(to[0]),
                    from[1].min(to[1]),
                    from[0].abs_diff(to[0]),
                    from[1].abs_diff(to[1]),
                ];
                painter.rect_stroke(
                    to_screen(r),
                    0.0,
                    Stroke::new(2.0, Color32::YELLOW),
                    egui::StrokeKind::Inside,
                );
                if response.drag_stopped() {
                    self.drag_from = None;
                    if r[2] > 0 && r[3] > 0 {
                        let name = format!("{}_{}", self.prefix, open.sheet.sprites.len());
                        let pivot = [r[2] as i32 / 2, r[3] as i32];
                        open.sheet.sprites.push(Sprite { name, rect: r, pivot });
                        self.selected = Some(open.sheet.sprites.len() - 1);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_slicing_skips_empty_cells() {
        // 4 x 2 cells of 2 x 2 texels with one texel of padding; the second
        // cell of each row is empty.
        let (w, h) = (11, 5);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let grid = Grid {
            cell: [2, 2],
            offset: [0, 0],
            padding: [1, 1],
            columns: fit(w, 2, 0, 1),
            rows: fit(h, 2, 0, 1),
        };
        assert_eq!((grid.columns, grid.rows), (4, 2));
        for (column, row) in [(0, 0), (2, 0), (3, 0), (0, 1), (2, 1), (3, 1)] {
            let [x, y, _, _] = grid.cell_rect(column, row);
            rgba[((y * w + x) * 4 + 3) as usize] = 255;
        }
        let image = reign_assets::RgbaImage { rgba, width: w, height: h };
        let sprites = slice(&image, grid, "bat", [1, 2]);
        assert_eq!(sprites.len(), 6);
        assert_eq!(sprites[1].rect, [6, 0, 2, 2]);
        assert_eq!(sprites[1].name, "bat_1");
        assert_eq!(sprites[3].rect, [0, 3, 2, 2]);
        assert_eq!(sprites[0].pivot, [1, 2]);
    }
}
