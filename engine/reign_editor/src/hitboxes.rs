//! Hitboxes and hurtboxes on the Animation editor's preview: the moves (in
//! `MoveSet` assets) that play the open clip, their boxes drawn on the
//! current frame and edited there, with their own undo history.
//!
//! A move's own hitbox shows while the move is active; a frame's hitboxes
//! replace it on that frame (the simulation uses them on the ticks the
//! frame covers). Hurtboxes are stored and drawn; the simulation still
//! takes hurtboxes from bodies. Boxes are in world pixels, authored facing
//! right: `x` from the body's centre, `y` from its top, so the preview
//! needs the body's height (set beside the boxes).

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use reign_assets::{AssetDb, AssetKind, Guid, Handle, MoveSet};
use reign_combat::{FrameBoxes, HitShape, MoveSetFile, MoveSpec};

use crate::undo::Edits;

/// The move sets that have moves playing one clip, being edited.
pub(crate) struct Boxes {
    /// (set, move name) for every move that plays the clip.
    pub moves: Vec<(Guid, String)>,
    pub sets: Vec<(Guid, MoveSetFile)>,
    saved: Vec<(Guid, MoveSetFile)>,
    pub edits: Edits<Vec<(Guid, MoveSetFile)>>,
    pub chosen: usize,
    /// The body the boxes hang from, in world pixels.
    pub body_h: i32,
    /// The box being dragged on the preview, and pixels left over.
    drag: Option<(bool, usize)>,
    carry: Vec2,
    pub status: Option<String>,
}

impl Boxes {
    /// Every move in `db` that plays `clip`.
    pub fn find(db: &AssetDb, clip: Guid) -> Self {
        let key = clip.to_string();
        let mut moves = Vec::new();
        let mut sets = Vec::new();
        for info in db.assets().into_iter().filter(|a| a.kind == AssetKind::MoveSet) {
            let Ok(set) = db.load(Handle::<MoveSet>::new(info.guid)) else { continue };
            let names: Vec<String> = set
                .file
                .moves
                .iter()
                .filter(|(_, m)| m.clip.as_deref() == Some(key.as_str()))
                .map(|(n, _)| n.clone())
                .collect();
            if !names.is_empty() {
                moves.extend(names.into_iter().map(|n| (info.guid, n)));
                sets.push((info.guid, set.file.clone()));
            }
        }
        Boxes {
            moves,
            edits: Edits::new(&sets),
            saved: sets.clone(),
            sets,
            chosen: 0,
            body_h: 30,
            drag: None,
            carry: Vec2::ZERO,
            status: None,
        }
    }

    pub fn dirty(&self) -> bool {
        self.sets != self.saved
    }

    fn spec_mut(&mut self) -> Option<&mut MoveSpec> {
        let (set, name) = self.moves.get(self.chosen)?.clone();
        self.sets.iter_mut().find(|(g, _)| *g == set)?.1.moves.get_mut(&name)
    }

    fn spec(&self) -> Option<&MoveSpec> {
        let (set, name) = self.moves.get(self.chosen)?;
        self.sets.iter().find(|(g, _)| g == set)?.1.moves.get(name)
    }

    /// Writes every changed move set.
    pub fn save(&mut self, db: &AssetDb) {
        let mut wrote = 0;
        for (guid, file) in &self.sets {
            if self.saved.iter().any(|(g, f)| g == guid && f == file) {
                continue;
            }
            let text = format!(
                "// Combat frame data: each move's ticks, hitboxes, damage, knockback, hitstop,\n\
                 // shake, cancel and chain. Edit here or in the editor's Animation tool.\n{}\n",
                MoveSet { file: file.clone() }.to_ron()
            );
            match db.write(*guid, text.as_bytes()) {
                Ok(()) => wrote += 1,
                Err(e) => {
                    self.status = Some(format!("Couldn't save: {e}"));
                    return;
                }
            }
        }
        self.saved = self.sets.clone();
        self.status = Some(format!("Saved {wrote} move set(s)"));
    }

    /// The boxes panel: which move, the body, and this frame's boxes.
    pub fn panel(&mut self, ui: &mut Ui, db: &AssetDb, frame: u16, tick: u16) {
        ui.label(RichText::new("Hitboxes").strong());
        if self.moves.is_empty() {
            ui.weak("No move set move plays this clip.");
            return;
        }
        let names: Vec<String> = self
            .moves
            .iter()
            .map(|(g, n)| {
                format!(
                    "{n} ({})",
                    db.path(*g).unwrap_or_default().rsplit('/').next().unwrap_or("")
                )
            })
            .collect();
        egui::ComboBox::from_id_salt("hitbox move")
            .selected_text(names.get(self.chosen).cloned().unwrap_or_default())
            .show_ui(ui, |ui| {
                for (i, n) in names.iter().enumerate() {
                    ui.selectable_value(&mut self.chosen, i, n);
                }
            });
        ui.horizontal(|ui| {
            ui.label("Body height");
            ui.add(egui::DragValue::new(&mut self.body_h).range(4..=128));
        });
        let Some(spec) = self.spec_mut() else { return };
        let active = (spec.startup..spec.startup + spec.active).contains(&tick);
        ui.weak(format!(
            "frame {frame}, tick {tick}: {}",
            if active { "active" } else { "not active" }
        ));
        ui.collapsing("Move hitbox", |ui| shape_ui(ui, &mut spec.hitbox, "move"));
        let at = spec.frames.iter().position(|f| f.frame == frame);
        ui.horizontal(|ui| {
            if ui.small_button("+ Hitbox").on_hover_text("A hitbox on this frame").clicked() {
                let own = spec.hitbox;
                frame_mut(spec, frame).hitboxes.push(own);
            }
            if ui.small_button("+ Hurtbox").clicked() {
                frame_mut(spec, frame).hurtboxes.push(HitShape::new(-6, 0, 12, 24));
            }
        });
        if let Some(i) = at {
            let mut gone = None;
            let f = &mut spec.frames[i];
            for (k, b) in f.hitboxes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(255, 90, 90), format!("hit {k}"));
                    shape_ui(ui, b, &format!("hit{k}"));
                    if ui.small_button("✕").clicked() {
                        gone = Some((true, k));
                    }
                });
            }
            for (k, b) in f.hurtboxes.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(90, 220, 120), format!("hurt {k}"));
                    shape_ui(ui, b, &format!("hurt{k}"));
                    if ui.small_button("✕").clicked() {
                        gone = Some((false, k));
                    }
                });
            }
            match gone {
                Some((true, k)) => {
                    f.hitboxes.remove(k);
                }
                Some((false, k)) => {
                    f.hurtboxes.remove(k);
                }
                None => {}
            }
            if f.hitboxes.is_empty() && f.hurtboxes.is_empty() {
                spec.frames.remove(i);
            }
        }
        let dirty = self.dirty();
        if ui
            .add_enabled(dirty, egui::Button::new(if dirty { "Save boxes*" } else { "Save boxes" }))
            .clicked()
        {
            self.save(db);
        }
        if let Some(s) = &self.status {
            ui.weak(s);
        }
    }

    /// Draws the chosen move's boxes for `frame` around the pivot at
    /// `ground` (`zoom` points per world pixel), and lets frame boxes be
    /// dragged.
    pub fn overlay(&mut self, ui: &mut Ui, ground: Pos2, zoom: f32, frame: u16, tick: u16) {
        let body_h = self.body_h;
        let Some(spec) = self.spec() else { return };
        let top = ground.y - body_h as f32 * zoom;
        let place = |s: &HitShape| {
            Rect::from_min_size(
                Pos2::new(ground.x + s.x as f32 * zoom, top + s.y as f32 * zoom),
                Vec2::new(s.w as f32, s.h as f32) * zoom,
            )
        };
        let painter = ui.painter().clone();
        let red = Color32::from_rgb(255, 90, 90);
        let green = Color32::from_rgb(90, 220, 120);
        let boxes = spec.frames.iter().find(|f| f.frame == frame).cloned().unwrap_or_default();
        let active = (spec.startup..spec.startup + spec.active).contains(&tick);
        if boxes.hitboxes.is_empty() && active {
            painter.rect_stroke(
                place(&spec.hitbox),
                0.0,
                Stroke::new(1.5, red),
                egui::StrokeKind::Inside,
            );
        }
        let mut grab = None;
        for (hit, list, colour) in [(true, &boxes.hitboxes, red), (false, &boxes.hurtboxes, green)]
        {
            for (k, b) in list.iter().enumerate() {
                let r = place(b);
                painter.rect_filled(r, 0.0, colour.gamma_multiply(0.25));
                painter.rect_stroke(r, 0.0, Stroke::new(1.5, colour), egui::StrokeKind::Inside);
                let response = ui.interact(r, egui::Id::new(("hitbox", hit, k)), Sense::drag());
                if response.drag_started() {
                    grab = Some((hit, k));
                }
            }
        }
        if let Some(g) = grab {
            self.drag = Some(g);
            self.carry = Vec2::ZERO;
        }
        if let Some((hit, k)) = self.drag {
            if ui.input(|i| !i.pointer.any_down()) {
                self.drag = None;
                return;
            }
            self.carry += ui.input(|i| i.pointer.delta()) / zoom;
            let (dx, dy) = (self.carry.x.trunc(), self.carry.y.trunc());
            self.carry -= Vec2::new(dx, dy);
            if let Some(f) =
                self.spec_mut().and_then(|s| s.frames.iter_mut().find(|f| f.frame == frame))
            {
                let list = if hit { &mut f.hitboxes } else { &mut f.hurtboxes };
                if let Some(b) = list.get_mut(k) {
                    b.x += dx as i32;
                    b.y += dy as i32;
                }
            }
        }
    }
}

fn frame_mut(spec: &mut MoveSpec, frame: u16) -> &mut FrameBoxes {
    let i = match spec.frames.iter().position(|f| f.frame == frame) {
        Some(i) => i,
        None => {
            spec.frames.push(FrameBoxes { frame, ..FrameBoxes::default() });
            spec.frames.sort_by_key(|f| f.frame);
            spec.frames.iter().position(|f| f.frame == frame).expect("just added")
        }
    };
    &mut spec.frames[i]
}

fn shape_ui(ui: &mut Ui, s: &mut HitShape, salt: &str) {
    ui.push_id(salt, |ui| {
        ui.horizontal(|ui| {
            for (label, v) in [("x", &mut s.x), ("y", &mut s.y), ("w", &mut s.w), ("h", &mut s.h)] {
                ui.label(label);
                ui.add(egui::DragValue::new(v).speed(0.25));
            }
        });
    });
}
