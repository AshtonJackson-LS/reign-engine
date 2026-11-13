//! The Animation editor: an [`AnimationClip`]'s frames on a timeline, with
//! a live preview sampled on the 120 Hz tick exactly as the game samples
//! it ([`AnimationClip::frame_at_tick`]).
//!
//! Frames can be added, reordered and removed; each has a cell, a duration
//! (in milliseconds, shown with the ticks it covers), a hit flag and an
//! optional event. The clip has a sheet, a row and a loop mode. The
//! preview plays, pauses, steps one tick and scrubs.
//!
//! The preview also shows the hitboxes and hurtboxes of the moves (in
//! `MoveSet` assets) that play the clip, and edits them per frame (see
//! `hitboxes`). Their undo history is the tool's too: Ctrl+Z undoes the
//! last edit, frames or boxes.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use reign_assets::{
    AnimationClip, AssetDb, ClipFrame, Guid, Handle, LoopMode, SpriteSheet, to_ron,
};

use crate::assets_ui::{AssetDrag, Thumbs};
use crate::undo::Edits;

/// Ticks per second, as the game runs.
const TICK_HZ: u32 = reign_core::TICK_HZ;

pub struct AnimEditor {
    open: Option<OpenClip>,
    playing: bool,
    /// The preview's tick, and time left over toward the next.
    tick: u32,
    carry: f32,
    selected: Option<usize>,
    zoom: f32,
    status: Option<String>,
}

struct OpenClip {
    guid: Guid,
    path: String,
    clip: AnimationClip,
    saved: AnimationClip,
    edits: Edits<AnimationClip>,
    boxes: crate::hitboxes::Boxes,
    /// Which history each undo step is in: true for boxes.
    undo_parts: Vec<bool>,
    redo_parts: Vec<bool>,
}

impl Default for AnimEditor {
    fn default() -> Self {
        AnimEditor {
            open: None,
            playing: false,
            tick: 0,
            carry: 0.0,
            selected: None,
            zoom: 3.0,
            status: None,
        }
    }
}

/// The texel rect of a clip frame in its sheet: the grid cell in the
/// clip's row, or the sprite with that index without a grid.
pub fn frame_rect(
    sheet: &SpriteSheet,
    clip: &AnimationClip,
    frame: &ClipFrame,
) -> Option<([u32; 4], [i32; 2])> {
    match sheet.grid {
        Some(grid) => Some((grid.cell_rect(frame.cell, clip.row), sheet.cell_pivot)),
        None => sheet.sprites.get(frame.cell as usize).map(|s| (s.rect, s.pivot)),
    }
}

/// Ticks a frame of `ms` milliseconds covers, rounded the way the clip
/// samples (the frame starts on the first tick at or after its time).
fn ticks_label(start_ms: u32, ms: u16) -> u32 {
    let first = (start_ms * TICK_HZ).div_ceil(1000);
    let next = ((start_ms + ms as u32) * TICK_HZ).div_ceil(1000);
    next - first
}

impl AnimEditor {
    pub fn open(&mut self, db: &AssetDb, guid: Guid) {
        let Some(path) = db.path(guid) else { return };
        match db.load(Handle::<AnimationClip>::new(guid)) {
            Ok(clip) => {
                let clip = (*clip).clone();
                self.open = Some(OpenClip {
                    guid,
                    path: path.clone(),
                    edits: Edits::new(&clip),
                    saved: clip.clone(),
                    clip,
                    boxes: crate::hitboxes::Boxes::find(db, guid),
                    undo_parts: Vec::new(),
                    redo_parts: Vec::new(),
                });
                self.tick = 0;
                self.playing = true;
                self.selected = None;
                self.status = Some(format!("Opened {path}"));
            }
            Err(e) => self.status = Some(format!("Couldn't open {path}: {e}")),
        }
    }

    pub fn editing(&self) -> Option<(Guid, bool)> {
        self.open.as_ref().map(|o| (o.guid, o.clip != o.saved))
    }

    pub fn reloaded(&mut self, db: &AssetDb, guid: Guid) {
        if let Some(open) = &self.open
            && open.guid == guid
            && open.clip == open.saved
            && db.load(Handle::<AnimationClip>::new(guid)).is_ok_and(|c| *c != open.saved)
        {
            let tick = self.tick;
            self.open(db, guid);
            self.tick = tick;
            self.status = Some("Reloaded from disk".into());
        }
    }

    pub fn undo(&mut self) -> bool {
        let Some(o) = self.open.as_mut() else { return false };
        let boxes = o.undo_parts.pop().unwrap_or(false);
        let done =
            if boxes { o.boxes.edits.undo(&mut o.boxes.sets) } else { o.edits.undo(&mut o.clip) };
        if done {
            o.redo_parts.push(boxes);
        }
        done
    }

    pub fn redo(&mut self) -> bool {
        let Some(o) = self.open.as_mut() else { return false };
        let boxes = o.redo_parts.pop().unwrap_or(false);
        let done =
            if boxes { o.boxes.edits.redo(&mut o.boxes.sets) } else { o.edits.redo(&mut o.clip) };
        if done {
            o.undo_parts.push(boxes);
        }
        done
    }

    pub fn ui(&mut self, ui: &mut Ui, db: &AssetDb, thumbs: &mut Thumbs) {
        let Some(open) = &self.open else {
            ui.weak("Open an animation clip: double-click one in the Project panel.");
            if let Some(status) = &self.status {
                ui.weak(status);
            }
            return;
        };
        let sheet = db.load(open.clip.sheet).ok();
        let texture = sheet.as_ref().and_then(|s| thumbs.texture(ui.ctx(), db, s.texture.guid));
        self.advance(ui);
        self.toolbar(ui, db);
        egui::Panel::bottom("anim timeline").resizable(true).default_size(170.0).show(ui, |ui| {
            self.timeline(ui, sheet.as_deref(), texture.as_ref());
        });
        egui::Panel::left("anim clip settings").resizable(true).default_size(220.0).show(
            ui,
            |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.settings(ui, db, sheet.as_deref());
                    if let Some(open) = &mut self.open {
                        ui.separator();
                        let frame = open.clip.frame_at_tick(self.tick) as u16;
                        open.boxes.panel(ui, db, frame, self.tick as u16);
                    }
                });
            },
        );
        self.preview(ui, sheet.as_deref(), texture.as_ref());
        let holding = ui.input(|i| i.pointer.any_down());
        if let Some(open) = &mut self.open {
            let (clip_steps, box_steps) = (open.edits.steps(), open.boxes.edits.steps());
            open.edits.track(&open.clip, holding);
            open.boxes.edits.track(&open.boxes.sets, holding);
            if open.edits.steps() > clip_steps {
                open.undo_parts.push(false);
                open.redo_parts.clear();
            }
            if open.boxes.edits.steps() > box_steps {
                open.undo_parts.push(true);
                open.redo_parts.clear();
            }
        }
    }

    /// Moves the preview on by real time, in whole ticks.
    fn advance(&mut self, ui: &Ui) {
        let Some(open) = &self.open else { return };
        if !self.playing || open.clip.frames.is_empty() {
            self.carry = 0.0;
            return;
        }
        self.carry += ui.input(|i| i.stable_dt).min(0.1) * TICK_HZ as f32;
        let whole = self.carry.floor();
        self.carry -= whole;
        self.tick += whole as u32;
        let end = open.clip.ticks() as u32;
        if open.clip.mode == LoopMode::Once && self.tick >= end + TICK_HZ / 2 {
            // Hold the last frame briefly, then show it again from the start.
            self.tick = 0;
        }
        ui.ctx().request_repaint();
    }

    fn toolbar(&mut self, ui: &mut Ui, db: &AssetDb) {
        let Some(open) = &mut self.open else { return };
        let dirty = open.clip != open.saved;
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}{}", open.path, if dirty { "*" } else { "" })).monospace(),
            );
            ui.separator();
            if ui.button(if self.playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
                self.playing = !self.playing;
            }
            if ui.button("⏭ Step").on_hover_text("One 120 Hz tick").clicked() {
                self.playing = false;
                self.tick += 1;
            }
            if ui.button("⏮").on_hover_text("Back to the start").clicked() {
                self.tick = 0;
            }
            let end = (open.clip.ticks() as u32).max(1);
            let mut shown = self.tick.min(end);
            if ui.add(egui::Slider::new(&mut shown, 0..=end).text("tick")).changed() {
                self.playing = false;
                self.tick = shown;
            }
            let frame = open.clip.frame_at_tick(self.tick);
            ui.weak(format!(
                "frame {frame} · {:.0} ms",
                self.tick as f32 * 1000.0 / TICK_HZ as f32
            ));
            ui.separator();
            if ui.add_enabled(open.edits.can_undo(), egui::Button::new("Undo")).clicked() {
                open.edits.undo(&mut open.clip);
            }
            if ui.add_enabled(open.edits.can_redo(), egui::Button::new("Redo")).clicked() {
                open.edits.redo(&mut open.clip);
            }
            if ui.add_enabled(dirty && db.root().is_some(), egui::Button::new("Save")).clicked() {
                self.status = Some(match db.write(open.guid, to_ron(&open.clip).as_bytes()) {
                    Ok(()) => {
                        open.saved = open.clip.clone();
                        format!("Saved {}", open.path)
                    }
                    Err(e) => format!("Couldn't save: {e}"),
                });
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                open.clip = open.saved.clone();
            }
            if let Some(status) = &self.status {
                ui.weak(status);
            }
        });
        ui.separator();
    }

    fn settings(&mut self, ui: &mut Ui, db: &AssetDb, sheet: Option<&SpriteSheet>) {
        let Some(open) = &mut self.open else { return };
        let clip = &mut open.clip;
        ui.label(RichText::new("Clip").strong());
        egui::Grid::new("clip settings").num_columns(2).show(ui, |ui| {
            ui.label("Sheet");
            let name = db.path(clip.sheet.guid).unwrap_or_else(|| "None".into());
            let r = ui.label(RichText::new(name.rsplit('/').next().unwrap_or(&name)).monospace());
            if let Some(drag) = r.dnd_release_payload::<AssetDrag>()
                && drag.kind == reign_assets::AssetKind::SpriteSheet
            {
                clip.sheet = Handle::new(drag.guid);
            }
            r.on_hover_text("Drop a sprite sheet here to change it");
            ui.end_row();
            ui.label("Row");
            ui.add(egui::DragValue::new(&mut clip.row).range(0..=255));
            ui.end_row();
            ui.label("Loop");
            egui::ComboBox::from_id_salt("loop mode")
                .selected_text(format!("{:?}", clip.mode))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut clip.mode, LoopMode::Once, "Once");
                    ui.selectable_value(&mut clip.mode, LoopMode::Loop, "Loop");
                });
            ui.end_row();
            ui.label("Cancel from");
            ui.horizontal(|ui| {
                let mut some = clip.cancel_from.is_some();
                ui.checkbox(&mut some, "");
                match (some, &mut clip.cancel_from) {
                    (true, Some(f)) => {
                        ui.add(
                            egui::DragValue::new(f).range(0..=clip.frames.len().max(1) as u32 - 1),
                        );
                    }
                    (true, c @ None) => *c = Some(0),
                    (false, c) => *c = None,
                }
            });
            ui.end_row();
        });
        ui.weak(format!(
            "{} frames · {} ticks · {} ms",
            clip.frames.len(),
            clip.ticks(),
            clip.frames.iter().map(|f| f.ms as u32).sum::<u32>()
        ));
        if sheet.is_some_and(|s| s.grid.is_none()) {
            ui.weak("The sheet has no grid: cells are sprite indices.");
        }
        ui.separator();
        let Some(i) = self.selected.filter(|&i| i < clip.frames.len()) else {
            ui.weak("Select a frame on the timeline to edit it.");
            return;
        };
        ui.label(RichText::new(format!("Frame {i}")).strong());
        let start: u32 = clip.frames[..i].iter().map(|f| f.ms as u32).sum();
        let frame = &mut clip.frames[i];
        egui::Grid::new("frame settings").num_columns(2).show(ui, |ui| {
            ui.label("Cell");
            ui.add(egui::DragValue::new(&mut frame.cell).range(0..=1023));
            ui.end_row();
            ui.label("Duration");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut frame.ms).range(1..=10_000).suffix(" ms"));
                ui.weak(format!("= {} ticks", ticks_label(start, frame.ms)));
            });
            ui.end_row();
            ui.label("Hit");
            ui.checkbox(&mut frame.hit, "").on_hover_text("A gameplay hit is live on this frame");
            ui.end_row();
            ui.label("Event");
            let mut text = frame.event.clone().unwrap_or_default();
            if ui.text_edit_singleline(&mut text).changed() {
                frame.event = (!text.is_empty()).then_some(text);
            }
            ui.end_row();
            ui.label("Shake");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut frame.shake.0).prefix("x "));
                ui.add(egui::DragValue::new(&mut frame.shake.1).prefix("y "));
            });
            ui.end_row();
            ui.label("Flash");
            ui.add(egui::Slider::new(&mut frame.flash, 0.0..=1.0));
            ui.end_row();
            ui.label("Dim");
            ui.add(egui::Slider::new(&mut frame.dim, 0.0..=1.0));
            ui.end_row();
        });
    }

    fn timeline(
        &mut self,
        ui: &mut Ui,
        sheet: Option<&SpriteSheet>,
        texture: Option<&egui::TextureHandle>,
    ) {
        let Some(open) = &mut self.open else { return };
        let current = open.clip.frame_at_tick(self.tick) as usize;
        let mut action: Option<(usize, i32)> = None; // (index, -1 left, +1 right, 0 remove)
        let mut insert = None;
        egui::ScrollArea::horizontal().auto_shrink(false).show(ui, |ui| {
            ui.horizontal(|ui| {
                let mut start = 0u32;
                for (i, frame) in open.clip.frames.iter().enumerate() {
                    let width = (40.0 + frame.ms as f32 * 0.6).clamp(64.0, 220.0);
                    ui.vertical(|ui| {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(width, 96.0), Sense::click());
                        let selected = self.selected == Some(i);
                        let fill = if i == current {
                            Color32::from_rgb(60, 80, 110)
                        } else if selected {
                            ui.visuals().selection.bg_fill
                        } else {
                            Color32::from_gray(36)
                        };
                        ui.painter().rect_filled(rect, 3.0, fill);
                        if selected {
                            ui.painter().rect_stroke(
                                rect,
                                3.0,
                                Stroke::new(1.5, Color32::YELLOW),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if let (Some(sheet), Some(texture)) = (sheet, texture)
                            && let Some((r, _)) = frame_rect(sheet, &open.clip, frame)
                        {
                            let size = texture.size_vec2();
                            let [x, y, w, h] = r.map(|v| v as f32);
                            let uv = Rect::from_min_size(
                                Pos2::new(x / size.x, y / size.y),
                                Vec2::new(w / size.x, h / size.y),
                            );
                            let fit = (60.0 / w).min(60.0 / h);
                            let image = Rect::from_center_size(
                                rect.center_top() + Vec2::new(0.0, 34.0),
                                Vec2::new(w, h) * fit,
                            );
                            ui.painter().image(texture.id(), image, uv, Color32::WHITE);
                        }
                        let ticks = ticks_label(start, frame.ms);
                        let mut label = format!("{i} · {} ms · {ticks}t", frame.ms);
                        if frame.hit {
                            label.push_str(" · HIT");
                        }
                        ui.painter().text(
                            rect.left_bottom() + Vec2::new(4.0, -4.0),
                            egui::Align2::LEFT_BOTTOM,
                            label,
                            egui::FontId::proportional(10.0),
                            Color32::LIGHT_GRAY,
                        );
                        if let Some(event) = &frame.event {
                            ui.painter().text(
                                rect.left_top() + Vec2::new(4.0, 3.0),
                                egui::Align2::LEFT_TOP,
                                format!("> {event}"),
                                egui::FontId::proportional(10.0),
                                Color32::from_rgb(240, 200, 100),
                            );
                        }
                        if response.clicked() {
                            self.selected = Some(i);
                            self.playing = false;
                            self.tick = (start * TICK_HZ).div_ceil(1000);
                        }
                        ui.horizontal(|ui| {
                            if ui.small_button("◀").on_hover_text("Move earlier").clicked() {
                                action = Some((i, -1));
                            }
                            if ui.small_button("▶").on_hover_text("Move later").clicked() {
                                action = Some((i, 1));
                            }
                            if ui.small_button("x").on_hover_text("Remove").clicked() {
                                action = Some((i, 0));
                            }
                            if ui.small_button("+").on_hover_text("Insert a copy after").clicked() {
                                insert = Some(i + 1);
                            }
                        });
                    });
                    start += frame.ms as u32;
                }
                if ui.button("+ Frame").clicked() {
                    insert = Some(open.clip.frames.len());
                }
            });
        });
        let frames = &mut open.clip.frames;
        match action {
            Some((i, 0)) => {
                frames.remove(i);
                self.selected = None;
            }
            Some((i, -1)) if i > 0 => {
                frames.swap(i, i - 1);
                self.selected = Some(i - 1);
            }
            Some((i, 1)) if i + 1 < frames.len() => {
                frames.swap(i, i + 1);
                self.selected = Some(i + 1);
            }
            _ => {}
        }
        if let Some(at) = insert {
            let frame = match at.checked_sub(1).and_then(|i| frames.get(i)) {
                Some(prev) => {
                    ClipFrame { cell: prev.cell + 1, ms: prev.ms, ..ClipFrame::default() }
                }
                None => ClipFrame { cell: 0, ms: 100, ..ClipFrame::default() },
            };
            frames.insert(at, frame);
            self.selected = Some(at);
        }
    }

    fn preview(
        &mut self,
        ui: &mut Ui,
        sheet: Option<&SpriteSheet>,
        texture: Option<&egui::TextureHandle>,
    ) {
        let space = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(space, Sense::hover());
        let painter = ui.painter_at(space);
        painter.rect_filled(space, 0.0, Color32::from_rgb(28, 26, 36));
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.5, 12.0);
        }
        let Some(open) = &self.open else { return };
        let (Some(sheet), Some(texture)) = (sheet, texture) else {
            painter.text(
                space.center(),
                egui::Align2::CENTER_CENTER,
                "The clip's sheet doesn't load",
                egui::FontId::proportional(14.0),
                Color32::GRAY,
            );
            return;
        };
        let Some(frame) = open.clip.frames.get(open.clip.frame_at_tick(self.tick) as usize) else {
            return;
        };
        let Some((rect, pivot)) = frame_rect(sheet, &open.clip, frame) else { return };
        let size = texture.size_vec2();
        let [x, y, w, h] = rect.map(|v| v as f32);
        let scale = self.zoom / sheet.density.max(1) as f32;
        // The pivot sits on a ground line in the middle of the view.
        let ground = Pos2::new(space.center().x, space.top() + space.height() * 0.7);
        let min = ground - Vec2::new(pivot[0] as f32, pivot[1] as f32) * scale;
        let shown = Rect::from_min_size(min, Vec2::new(w, h) * scale);
        painter.line_segment(
            [Pos2::new(space.left(), ground.y), Pos2::new(space.right(), ground.y)],
            Stroke::new(1.0, Color32::from_gray(70)),
        );
        painter.rect_stroke(
            shown,
            0.0,
            Stroke::new(1.0, Color32::from_gray(60)),
            egui::StrokeKind::Outside,
        );
        let uv = Rect::from_min_size(
            Pos2::new(x / size.x, y / size.y),
            Vec2::new(w / size.x, h / size.y),
        );
        painter.image(texture.id(), shown, uv, Color32::WHITE);
        painter.circle_stroke(ground, 3.0, Stroke::new(1.0, Color32::from_rgb(255, 120, 200)));
        let frame_index = open.clip.frame_at_tick(self.tick) as u16;
        let hit_flag = frame.hit;
        let (zoom, tick) = (self.zoom, self.tick as u16);
        if let Some(open) = &mut self.open {
            open.boxes.overlay(ui, ground, zoom, frame_index, tick);
        }
        let Some(open) = &self.open else { return };
        if hit_flag {
            painter.text(
                space.left_top() + Vec2::new(8.0, 8.0),
                egui::Align2::LEFT_TOP,
                "HIT",
                egui::FontId::proportional(16.0),
                Color32::from_rgb(255, 90, 90),
            );
        }
        painter.text(
            space.right_top() + Vec2::new(-8.0, 8.0),
            egui::Align2::RIGHT_TOP,
            format!("tick {} / {}", self.tick, open.clip.ticks()),
            egui::FontId::monospace(12.0),
            Color32::LIGHT_GRAY,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_labels_add_up_to_the_clip() {
        let clip = AnimationClip {
            frames: [70u16, 70, 95, 120, 33]
                .into_iter()
                .map(|ms| ClipFrame { ms, ..ClipFrame::default() })
                .collect(),
            ..AnimationClip::default()
        };
        let mut start = 0;
        let mut total = 0;
        for (i, f) in clip.frames.iter().enumerate() {
            let ticks = ticks_label(start, f.ms);
            // The frame shows for exactly that many ticks in the preview.
            let first = (start * TICK_HZ).div_ceil(1000);
            for t in first..first + ticks {
                assert_eq!(clip.frame_at_tick(t) as usize, i);
            }
            start += f.ms as u32;
            total += ticks;
        }
        assert_eq!(total, clip.ticks() as u32);
    }
}
