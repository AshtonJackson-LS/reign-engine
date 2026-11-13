//! The Tilemap painter: edits a [`Tilemap`] asset's collision, markers and
//! visual tile layers on its 16-pixel grid.
//!
//! Tools: brush (B), eraser (E), rectangle fill (R), flood fill (G) and
//! eyedropper (I); right-click picks too. The palette shows what the
//! selected layer holds: collision glyphs, marker glyphs, or the tileset's
//! tiles. Layers can be shown and hidden; visual layers can be added and
//! removed. Middle-drag or Space-drag pans, the wheel zooms. Ctrl+Z and
//! Ctrl+Y undo and redo while the painter has the pointer; Save writes the
//! asset, and hot reload carries it into the game.

use std::collections::VecDeque;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, Ui, Vec2};
use reign_assets::{AssetDb, Guid, Handle, TileLayer, Tilemap, Tileset, to_ron};

use crate::assets_ui::Thumbs;
use crate::undo::Edits;

/// Tile size in world pixels.
const TILE: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Brush,
    Eraser,
    Rect,
    Fill,
    Eyedropper,
}

/// The layer being painted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Collision,
    Markers,
    Visual(usize),
}

/// What a cell holds on one layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cell {
    Glyph(char),
    Tile(Option<u32>),
}

/// The painter tab's state.
pub struct TilemapPainter {
    open: Option<OpenMap>,
    tool: Tool,
    layer: Layer,
    /// The glyph or tile each kind of layer paints with.
    collision_glyph: char,
    marker_glyph: char,
    tile: u32,
    show_collision: bool,
    show_markers: bool,
    show_grid: bool,
    zoom: f32,
    pan: Vec2,
    /// A rectangle being dragged out, from this cell.
    rect_from: Option<(i32, i32)>,
    /// The cell the brush last painted, so a fast stroke leaves no gaps.
    last_paint: Option<(i32, i32)>,
    /// Fit the map to the canvas on the next frame.
    fit: bool,
    status: Option<String>,
}

struct OpenMap {
    guid: Guid,
    path: String,
    map: Tilemap,
    saved: Tilemap,
    edits: Edits<Tilemap>,
}

impl Default for TilemapPainter {
    fn default() -> Self {
        TilemapPainter {
            open: None,
            tool: Tool::Brush,
            layer: Layer::Collision,
            collision_glyph: '#',
            marker_glyph: 'E',
            tile: 0,
            show_collision: true,
            show_markers: true,
            show_grid: true,
            zoom: 1.0,
            pan: Vec2::ZERO,
            rect_from: None,
            last_paint: None,
            fit: false,
            status: None,
        }
    }
}

fn read(map: &Tilemap, layer: Layer, x: usize, y: usize) -> Cell {
    match layer {
        Layer::Collision => Cell::Glyph(Tilemap::glyph(&map.collision, x, y)),
        Layer::Markers => Cell::Glyph(Tilemap::glyph(&map.markers, x, y)),
        Layer::Visual(i) => Cell::Tile(map.layers.get(i).and_then(|l| l.get(x, y))),
    }
}

fn write(map: &mut Tilemap, layer: Layer, x: usize, y: usize, cell: Cell) {
    match (layer, cell) {
        (Layer::Collision, Cell::Glyph(c)) => Tilemap::set_glyph(&mut map.collision, x, y, c),
        (Layer::Markers, Cell::Glyph(c)) => Tilemap::set_glyph(&mut map.markers, x, y, c),
        (Layer::Visual(i), Cell::Tile(t)) => {
            if let Some(l) = map.layers.get_mut(i) {
                l.set(x, y, t);
            }
        }
        _ => {}
    }
}

/// Every cell connected to `(x, y)` holding the same thing, four-way.
fn flood(map: &Tilemap, layer: Layer, x: usize, y: usize) -> Vec<(usize, usize)> {
    let (w, h) = (map.width(), map.height());
    let target = read(map, layer, x, y);
    let mut seen = vec![false; w * h];
    let mut out = Vec::new();
    let mut queue = VecDeque::from([(x, y)]);
    while let Some((cx, cy)) = queue.pop_front() {
        if cx >= w || cy >= h || seen[cy * w + cx] || read(map, layer, cx, cy) != target {
            continue;
        }
        seen[cy * w + cx] = true;
        out.push((cx, cy));
        if cx > 0 {
            queue.push_back((cx - 1, cy));
        }
        if cy > 0 {
            queue.push_back((cx, cy - 1));
        }
        queue.push_back((cx + 1, cy));
        queue.push_back((cx, cy + 1));
    }
    out
}

impl TilemapPainter {
    pub fn open(&mut self, db: &AssetDb, guid: Guid) {
        let Some(path) = db.path(guid) else { return };
        match db.load(Handle::<Tilemap>::new(guid)) {
            Ok(map) => {
                let map = (*map).clone();
                self.open = Some(OpenMap {
                    guid,
                    path: path.clone(),
                    edits: Edits::new(&map),
                    saved: map.clone(),
                    map,
                });
                if matches!(self.layer, Layer::Visual(i) if i >= self.open.as_ref().map_or(0, |o| o.map.layers.len()))
                {
                    self.layer = Layer::Collision;
                }
                self.fit = true;
                self.status = Some(format!("Opened {path}"));
            }
            Err(e) => self.status = Some(format!("Couldn't open {path}: {e}")),
        }
    }

    /// The asset open, if any, and whether it has unsaved changes.
    pub fn editing(&self) -> Option<(Guid, bool)> {
        self.open.as_ref().map(|o| (o.guid, o.map != o.saved))
    }

    /// The asset changed on disk: take it if nothing is unsaved here.
    pub fn reloaded(&mut self, db: &AssetDb, guid: Guid) {
        if let Some(open) = &self.open
            && open.guid == guid
            && open.map == open.saved
            && db.load(Handle::<Tilemap>::new(guid)).is_ok_and(|m| *m != open.saved)
        {
            self.open(db, guid);
            self.status = Some("Reloaded from disk".into());
        }
    }

    pub fn undo(&mut self) -> bool {
        self.open.as_mut().is_some_and(|o| o.edits.undo(&mut o.map))
    }

    pub fn redo(&mut self) -> bool {
        self.open.as_mut().is_some_and(|o| o.edits.redo(&mut o.map))
    }

    pub fn ui(&mut self, ui: &mut Ui, db: &AssetDb, thumbs: &mut Thumbs) {
        let Some(open) = &self.open else {
            ui.weak("Open a tilemap: double-click one in the Project panel.");
            if let Some(status) = &self.status {
                ui.weak(status);
            }
            return;
        };
        let tileset = db.load(open.map.tileset).ok();
        let texture = tileset.as_ref().and_then(|t| thumbs.texture(ui.ctx(), db, t.texture.guid));
        self.toolbar(ui, db);
        egui::Panel::left("tilemap palette").resizable(true).default_size(190.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.layers(ui);
                ui.separator();
                self.palette(ui, tileset.as_deref(), texture.as_ref());
            });
        });
        self.canvas(ui, tileset.as_deref(), texture.as_ref());
        let holding = ui.input(|i| i.pointer.any_down());
        if let Some(open) = &mut self.open {
            open.edits.track(&open.map, holding);
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, db: &AssetDb) {
        let Some(open) = &mut self.open else { return };
        let dirty = open.map != open.saved;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}{}", open.path, if dirty { "*" } else { "" })).monospace(),
            );
            ui.weak(format!("{} x {} tiles", open.map.width(), open.map.height()));
            ui.separator();
            for (tool, label, key) in [
                (Tool::Brush, "Brush", "B"),
                (Tool::Eraser, "Eraser", "E"),
                (Tool::Rect, "Rect", "R"),
                (Tool::Fill, "Fill", "G"),
                (Tool::Eyedropper, "Pick", "I"),
            ] {
                ui.selectable_value(&mut self.tool, tool, label).on_hover_text(key);
            }
            ui.separator();
            ui.checkbox(&mut self.show_grid, "Grid");
            ui.add(egui::Slider::new(&mut self.zoom, 0.25..=4.0).logarithmic(true).text("zoom"));
            if ui.button("Fit").on_hover_text("Fit the map's height, from the left").clicked() {
                self.fit = true;
            }
            ui.separator();
            if ui.add_enabled(open.edits.can_undo(), egui::Button::new("Undo")).clicked() {
                open.edits.undo(&mut open.map);
            }
            if ui.add_enabled(open.edits.can_redo(), egui::Button::new("Redo")).clicked() {
                open.edits.redo(&mut open.map);
            }
            let writable = db.root().is_some();
            if ui.add_enabled(dirty && writable, egui::Button::new("Save")).clicked() {
                self.status = Some(match db.write(open.guid, to_ron(&open.map).as_bytes()) {
                    Ok(()) => {
                        open.saved = open.map.clone();
                        format!("Saved {}", open.path)
                    }
                    Err(e) => format!("Couldn't save: {e}"),
                });
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                open.map = open.saved.clone();
            }
            if let Some(status) = &self.status {
                ui.weak(status);
            }
        });
        // Tool keys while the pointer is over the painter.
        if ui.ui_contains_pointer() && !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                for (key, tool) in [
                    (egui::Key::B, Tool::Brush),
                    (egui::Key::E, Tool::Eraser),
                    (egui::Key::R, Tool::Rect),
                    (egui::Key::G, Tool::Fill),
                    (egui::Key::I, Tool::Eyedropper),
                ] {
                    if i.key_pressed(key) && i.modifiers.is_none() {
                        self.tool = tool;
                    }
                }
            });
        }
        ui.separator();
    }

    fn layers(&mut self, ui: &mut Ui) {
        let Some(open) = &mut self.open else { return };
        ui.label(RichText::new("Layers").strong());
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_collision, "").on_hover_text("Show");
            ui.selectable_value(&mut self.layer, Layer::Collision, "Collision");
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.show_markers, "").on_hover_text("Show");
            ui.selectable_value(&mut self.layer, Layer::Markers, "Markers");
        });
        let mut remove = None;
        for (i, layer) in open.map.layers.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut layer.visible, "").on_hover_text("Show (saved with the map)");
                let name =
                    if layer.name.is_empty() { format!("Layer {i}") } else { layer.name.clone() };
                ui.selectable_value(&mut self.layer, Layer::Visual(i), name);
                if ui.small_button("x").on_hover_text("Remove layer").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            open.map.layers.remove(i);
            self.layer = Layer::Collision;
        }
        if ui.small_button("+ Visual layer").clicked() {
            let (w, h) = (open.map.width(), open.map.height());
            let name = format!("Layer {}", open.map.layers.len());
            open.map.layers.push(TileLayer::empty(&name, w, h));
            self.layer = Layer::Visual(open.map.layers.len() - 1);
        }
        if let Layer::Visual(i) = self.layer
            && let Some(layer) = open.map.layers.get_mut(i)
        {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut layer.name);
            });
        }
    }

    fn palette(&mut self, ui: &mut Ui, tileset: Option<&Tileset>, texture: Option<&TextureHandle>) {
        ui.label(RichText::new("Palette").strong());
        match self.layer {
            Layer::Collision => {
                for (glyph, name) in
                    [('#', "Solid"), ('-', "One-way"), ('/', "Slope up"), ('\\', "Slope down")]
                {
                    ui.selectable_value(
                        &mut self.collision_glyph,
                        glyph,
                        format!("{glyph}  {name}"),
                    );
                }
            }
            Layer::Markers => {
                let mut used: Vec<char> = self
                    .open
                    .as_ref()
                    .map(|o| {
                        o.map.markers.iter().flat_map(|r| r.chars()).filter(|&c| c != '.').collect()
                    })
                    .unwrap_or_default();
                used.sort_unstable();
                used.dedup();
                ui.weak("Glyphs in this map:");
                ui.horizontal_wrapped(|ui| {
                    for c in used {
                        ui.selectable_value(
                            &mut self.marker_glyph,
                            c,
                            RichText::new(c.to_string()).monospace().size(15.0),
                        );
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Glyph");
                    let mut text = self.marker_glyph.to_string();
                    if ui.add(egui::TextEdit::singleline(&mut text).desired_width(24.0)).changed()
                        && let Some(c) =
                            text.chars().rev().find(|c| !c.is_whitespace() && *c != '.')
                    {
                        self.marker_glyph = c;
                    }
                });
            }
            Layer::Visual(_) => {
                let (Some(tileset), Some(texture)) = (tileset, texture) else {
                    ui.weak("The map has no tileset with a texture.");
                    return;
                };
                let size = texture.size_vec2();
                let per = tileset.tile_size as f32;
                let cell = 30.0;
                let columns = tileset.columns.max(1);
                for row in 0..tileset.count.div_ceil(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::splat(2.0);
                        for col in 0..columns {
                            let index = row * columns + col;
                            if index >= tileset.count {
                                break;
                            }
                            let (rect, response) =
                                ui.allocate_exact_size(Vec2::splat(cell), Sense::click());
                            let uv = Rect::from_min_size(
                                Pos2::new(col as f32 * per / size.x, row as f32 * per / size.y),
                                Vec2::new(per / size.x, per / size.y),
                            );
                            ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
                            if self.tile == index {
                                ui.painter().rect_stroke(
                                    rect,
                                    0.0,
                                    Stroke::new(2.0, Color32::YELLOW),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if response.on_hover_text(format!("Tile {index}")).clicked() {
                                self.tile = index;
                            }
                        }
                    });
                }
            }
        }
    }

    /// What the current tool puts down.
    fn paint_cell(&self) -> Cell {
        let erase = self.tool == Tool::Eraser;
        match self.layer {
            Layer::Collision => Cell::Glyph(if erase { '.' } else { self.collision_glyph }),
            Layer::Markers => Cell::Glyph(if erase { '.' } else { self.marker_glyph }),
            Layer::Visual(_) => Cell::Tile(if erase { None } else { Some(self.tile) }),
        }
    }

    fn pick(&mut self, cell: Cell) {
        match (self.layer, cell) {
            (Layer::Collision, Cell::Glyph(c)) if c != '.' => self.collision_glyph = c,
            (Layer::Markers, Cell::Glyph(c)) if c != '.' => self.marker_glyph = c,
            (Layer::Visual(_), Cell::Tile(Some(t))) => self.tile = t,
            _ => {}
        }
    }

    fn canvas(&mut self, ui: &mut Ui, tileset: Option<&Tileset>, texture: Option<&TextureHandle>) {
        let space = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(space, Sense::click_and_drag());
        let painter = ui.painter_at(space);
        painter.rect_filled(space, 0.0, Color32::from_rgb(24, 22, 30));
        let Some(open) = &self.open else { return };
        let (w, h) = (open.map.width(), open.map.height());
        if std::mem::take(&mut self.fit) && h > 0 {
            self.zoom = (space.height() / (h as f32 * TILE)).clamp(0.25, 4.0);
            self.pan = Vec2::new(8.0, (space.height() - h as f32 * TILE * self.zoom) / 2.0);
        }

        // Pan and zoom.
        let panning = response.dragged_by(egui::PointerButton::Middle)
            || (response.dragged_by(egui::PointerButton::Primary)
                && ui.input(|i| i.key_down(egui::Key::Space)));
        if panning {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let old = self.zoom;
                self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.25, 4.0);
                if let Some(p) = response.hover_pos() {
                    let origin = space.min + self.pan;
                    self.pan += (p - origin) * (1.0 - self.zoom / old);
                }
            }
        }
        let cell = TILE * self.zoom;
        let origin = space.min + self.pan;
        let to_cell = |p: Pos2| -> (i32, i32) {
            let local = (p - origin) / cell;
            (local.x.floor() as i32, local.y.floor() as i32)
        };
        let inside = |(x, y): (i32, i32)| x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h;

        // Only the visible cells.
        let first = to_cell(space.min);
        let last = to_cell(space.max);
        let xs = first.0.max(0) as usize..((last.0 + 1).max(0) as usize).min(w);
        let ys = first.1.max(0) as usize..((last.1 + 1).max(0) as usize).min(h);
        let cell_rect = |x: usize, y: usize| {
            Rect::from_min_size(origin + Vec2::new(x as f32, y as f32) * cell, Vec2::splat(cell))
        };
        painter.rect_filled(
            Rect::from_min_size(origin, Vec2::new(w as f32, h as f32) * cell),
            0.0,
            Color32::from_rgb(40, 36, 50),
        );
        if let (Some(tileset), Some(texture)) = (tileset, texture) {
            let size = texture.size_vec2();
            let per = tileset.tile_size as f32;
            let columns = tileset.columns.max(1);
            for layer in open.map.layers.iter().filter(|l| l.visible) {
                for y in ys.clone() {
                    for x in xs.clone() {
                        let Some(t) = layer.get(x, y) else { continue };
                        let (col, row) = (t % columns, t / columns);
                        let uv = Rect::from_min_size(
                            Pos2::new(col as f32 * per / size.x, row as f32 * per / size.y),
                            Vec2::new(per / size.x, per / size.y),
                        );
                        painter.image(texture.id(), cell_rect(x, y), uv, Color32::WHITE);
                    }
                }
            }
        }
        if self.show_collision {
            let fill = Color32::from_rgba_unmultiplied(90, 170, 230, 110);
            let edge = Color32::from_rgb(120, 200, 255);
            for y in ys.clone() {
                for x in xs.clone() {
                    let r = cell_rect(x, y);
                    match Tilemap::glyph(&open.map.collision, x, y) {
                        '#' => {
                            painter.rect_filled(r, 0.0, fill);
                        }
                        '-' => {
                            let top = Vec2::new(r.width(), r.height() * 0.25);
                            painter.rect_filled(Rect::from_min_size(r.min, top), 0.0, edge);
                        }
                        '/' => {
                            painter.add(egui::Shape::convex_polygon(
                                vec![r.left_bottom(), r.right_top(), r.right_bottom()],
                                fill,
                                Stroke::NONE,
                            ));
                        }
                        '\\' => {
                            painter.add(egui::Shape::convex_polygon(
                                vec![r.left_top(), r.right_bottom(), r.left_bottom()],
                                fill,
                                Stroke::NONE,
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
        if self.show_markers {
            let font = egui::FontId::monospace((cell * 0.8).max(6.0));
            for y in ys.clone() {
                for x in xs.clone() {
                    let c = Tilemap::glyph(&open.map.markers, x, y);
                    if c != '.' {
                        let r = cell_rect(x, y);
                        painter.circle_filled(
                            r.center(),
                            cell * 0.45,
                            Color32::from_rgba_unmultiplied(200, 60, 90, 170),
                        );
                        painter.text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            c,
                            font.clone(),
                            Color32::WHITE,
                        );
                    }
                }
            }
        }
        if self.show_grid && cell >= 6.0 {
            let line = Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 22));
            for x in xs.start..=xs.end {
                let px = origin.x + x as f32 * cell;
                painter.line_segment(
                    [
                        Pos2::new(px, cell_rect(0, ys.start).top()),
                        Pos2::new(px, cell_rect(0, ys.end).top()),
                    ],
                    line,
                );
            }
            for y in ys.start..=ys.end {
                let py = origin.y + y as f32 * cell;
                painter.line_segment(
                    [
                        Pos2::new(cell_rect(xs.start, 0).left(), py),
                        Pos2::new(cell_rect(xs.end, 0).left(), py),
                    ],
                    line,
                );
            }
        }

        // The cell under the pointer, and the tools.
        let hover = response.hover_pos().map(to_cell).filter(|&c| inside(c));
        if let Some((x, y)) = hover {
            painter.rect_stroke(
                cell_rect(x as usize, y as usize),
                0.0,
                Stroke::new(1.5, Color32::YELLOW),
                egui::StrokeKind::Inside,
            );
            painter.text(
                space.left_bottom() + Vec2::new(6.0, -6.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{x}, {y}  ({} px, {} px)", x * 16, y * 16),
                egui::FontId::monospace(12.0),
                Color32::LIGHT_GRAY,
            );
        }
        if panning {
            return;
        }
        let pointer = response.interact_pointer_pos().map(to_cell);
        let secondary = response.clicked_by(egui::PointerButton::Secondary);
        let open = self.open.as_mut().expect("open");
        if secondary && let Some(c) = pointer.filter(|&c| inside(c)) {
            let cell = read(&open.map, self.layer, c.0 as usize, c.1 as usize);
            self.pick(cell);
            return;
        }
        let primary_down =
            response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        let paint = self.paint_cell();
        let open = self.open.as_mut().expect("open");
        match self.tool {
            Tool::Brush | Tool::Eraser => match pointer.filter(|_| primary_down) {
                Some(to) => {
                    for (x, y) in line(self.last_paint.unwrap_or(to), to) {
                        if inside((x, y)) {
                            write(&mut open.map, self.layer, x as usize, y as usize, paint);
                        }
                    }
                    self.last_paint = Some(to);
                }
                None => self.last_paint = None,
            },
            Tool::Fill => {
                if response.clicked()
                    && let Some((x, y)) = pointer.filter(|&c| inside(c))
                {
                    for (cx, cy) in flood(&open.map, self.layer, x as usize, y as usize) {
                        write(&mut open.map, self.layer, cx, cy, paint);
                    }
                }
            }
            Tool::Eyedropper => {
                if response.clicked()
                    && let Some((x, y)) = pointer.filter(|&c| inside(c))
                {
                    let cell = read(&open.map, self.layer, x as usize, y as usize);
                    self.pick(cell);
                    self.tool = Tool::Brush;
                }
            }
            Tool::Rect => {
                if response.drag_started_by(egui::PointerButton::Primary) {
                    self.rect_from = pointer;
                }
                if let (Some(from), Some(to)) = (self.rect_from, pointer.or(hover)) {
                    let (x0, x1) = (from.0.min(to.0).max(0), from.0.max(to.0).min(w as i32 - 1));
                    let (y0, y1) = (from.1.min(to.1).max(0), from.1.max(to.1).min(h as i32 - 1));
                    let r = cell_rect(x0 as usize, y0 as usize)
                        .union(cell_rect(x1.max(x0) as usize, y1.max(y0) as usize));
                    painter.rect_stroke(
                        r,
                        0.0,
                        Stroke::new(2.0, Color32::YELLOW),
                        egui::StrokeKind::Inside,
                    );
                    if response.drag_stopped() {
                        for y in y0..=y1 {
                            for x in x0..=x1 {
                                write(&mut open.map, self.layer, x as usize, y as usize, paint);
                            }
                        }
                        self.rect_from = None;
                    }
                }
                if response.clicked()
                    && let Some((x, y)) = pointer.filter(|&c| inside(c))
                {
                    write(&mut open.map, self.layer, x as usize, y as usize, paint);
                }
            }
        }
    }
}

/// The cells on a line between two cells, both included.
fn line((x0, y0): (i32, i32), (x1, y1): (i32, i32)) -> Vec<(i32, i32)> {
    let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
    let (sx, sy) = ((x1 - x0).signum(), (y1 - y0).signum());
    let (mut x, mut y, mut err) = (x0, y0, dx + dy);
    let mut out = vec![(x, y)];
    while (x, y) != (x1, y1) {
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        out.push((x, y));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Tilemap {
        Tilemap::from_ascii("....\n.##.\n....\n####\n")
    }

    #[test]
    fn brush_strokes_have_no_gaps() {
        assert_eq!(line((0, 0), (3, 1)), [(0, 0), (1, 0), (2, 1), (3, 1)]);
        assert_eq!(line((2, 2), (2, 2)), [(2, 2)]);
        assert_eq!(line((0, 3), (0, 0)).len(), 4);
    }

    #[test]
    fn flood_fill_stays_in_the_region() {
        let mut m = map();
        for (x, y) in flood(&m, Layer::Collision, 0, 0) {
            write(&mut m, Layer::Collision, x, y, Cell::Glyph('-'));
        }
        assert_eq!(m.collision, ["----", "-##-", "----", "####"]);
    }

    #[test]
    fn visual_layers_paint_tiles() {
        let mut m = map();
        m.layers.push(TileLayer::empty("ground", 4, 4));
        write(&mut m, Layer::Visual(0), 2, 3, Cell::Tile(Some(40)));
        assert_eq!(read(&m, Layer::Visual(0), 2, 3), Cell::Tile(Some(40)));
        assert_eq!(read(&m, Layer::Visual(0), 1, 3), Cell::Tile(None));
        let filled = flood(&m, Layer::Visual(0), 0, 0).len();
        assert_eq!(filled, 15);
    }
}
