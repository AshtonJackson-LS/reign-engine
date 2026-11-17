//! The Particle editor: a [`ParticleEffect`]'s modules on the left, Unity
//! style, and a live preview that plays the effect as it's edited.
//!
//! Every change shows at once: the preview swaps the edited effect into
//! the running system without restarting it (Restart starts it over). An
//! effect that has a problem (a zero duration, a sprite mode with no sheet)
//! says what's wrong and keeps previewing the last good version. A one-shot
//! effect plays again a moment after it ends. Sub-emitters play from their
//! saved files.

use std::sync::Arc;

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use reign_assets::particles::*;
use reign_assets::{AssetDb, AssetKind, Guid, Handle};
use reign_particles::{ParticleWorld, Quad, QuadImage, Spawn, SystemId, no_ground};

use crate::assets_ui::{AssetDrag, Thumbs};
use crate::undo::Edits;

/// Seconds a finished one-shot effect waits before the preview plays it
/// again.
const REPLAY_PAUSE: f32 = 0.6;

pub struct ParticleEditor {
    open: Option<OpenEffect>,
    world: ParticleWorld,
    system: Option<SystemId>,
    /// The version the preview plays: the last one without problems.
    shown: Option<ParticleEffect>,
    playing: bool,
    /// Seconds since the effect finished, toward playing it again.
    idle: f32,
    /// Seconds the preview has run, for the moving emitter.
    time: f32,
    zoom: f32,
    flip: bool,
    /// Swings the emitter side to side, to see trails and world space.
    moving: bool,
    speed: f32,
    status: Option<String>,
    glow: Option<egui::TextureHandle>,
}

struct OpenEffect {
    guid: Guid,
    path: String,
    effect: ParticleEffect,
    saved: ParticleEffect,
    edits: Edits<ParticleEffect>,
}

impl Default for ParticleEditor {
    fn default() -> Self {
        ParticleEditor {
            open: None,
            world: ParticleWorld::new(),
            system: None,
            shown: None,
            playing: true,
            idle: 0.0,
            time: 0.0,
            zoom: 3.0,
            flip: false,
            moving: false,
            speed: 1.0,
            status: None,
            glow: None,
        }
    }
}

impl ParticleEditor {
    pub fn open(&mut self, db: &AssetDb, guid: Guid) {
        let Some(path) = db.path(guid) else { return };
        match db.load(Handle::<ParticleEffect>::new(guid)) {
            Ok(effect) => {
                let effect = (*effect).clone();
                self.open = Some(OpenEffect {
                    guid,
                    path: path.clone(),
                    edits: Edits::new(&effect),
                    saved: effect.clone(),
                    effect,
                });
                self.shown = None;
                self.world.clear();
                self.system = None;
                self.playing = true;
                self.status = Some(format!("Opened {path}"));
            }
            Err(e) => self.status = Some(format!("Couldn't open {path}: {e}")),
        }
    }

    pub fn editing(&self) -> Option<(Guid, bool)> {
        self.open.as_ref().map(|o| (o.guid, o.effect != o.saved))
    }

    /// The file changed on disk: take it up, unless there are unsaved edits.
    pub fn reloaded(&mut self, db: &AssetDb, guid: Guid) {
        if let Some(open) = &self.open
            && open.guid == guid
            && open.effect == open.saved
            && db.load(Handle::<ParticleEffect>::new(guid)).is_ok_and(|e| *e != open.saved)
        {
            self.open(db, guid);
            self.status = Some("Reloaded from disk".into());
        }
    }

    pub fn undo(&mut self) -> bool {
        self.open.as_mut().is_some_and(|o| o.edits.undo(&mut o.effect))
    }

    pub fn redo(&mut self) -> bool {
        self.open.as_mut().is_some_and(|o| o.edits.redo(&mut o.effect))
    }

    /// Particles alive in the preview.
    #[cfg(test)]
    fn particle_count(&self) -> usize {
        self.world.particle_count()
    }

    pub fn ui(&mut self, ui: &mut Ui, db: &AssetDb, thumbs: &mut Thumbs) {
        if self.open.is_none() {
            ui.weak(
                "Open a particle effect: double-click one in the Project panel, or make one \
                 with + Create > Particle Effect.",
            );
            if let Some(status) = &self.status {
                ui.weak(status);
            }
            return;
        }
        self.toolbar(ui, db);
        egui::Panel::left("particle modules").resizable(true).default_size(300.0).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some(open) = &mut self.open {
                    modules(ui, db, &mut open.effect);
                }
            });
        });
        let dt = ui.input(|i| i.stable_dt).min(0.1);
        self.advance(dt, db);
        if self.playing {
            ui.ctx().request_repaint();
        }
        self.preview(ui, db, thumbs);
        let holding = ui.input(|i| i.pointer.any_down());
        if let Some(open) = &mut self.open {
            open.edits.track(&open.effect, holding);
        }
    }

    /// Runs the preview on by `dt` seconds, swapping in edits.
    fn advance(&mut self, dt: f32, db: &AssetDb) {
        let Some(open) = &self.open else { return };
        if open.effect.problems().is_empty() && self.shown.as_ref() != Some(&open.effect) {
            self.shown = Some(open.effect.clone());
            match self.system {
                Some(id) if self.world.is_alive(id) => {
                    self.world.set_effect(id, Arc::new(open.effect.clone()))
                }
                _ => self.restart(),
            }
        }
        if !self.playing {
            return;
        }
        let dt = dt * self.speed;
        self.time += dt;
        if let Some(id) = self.system {
            let x = if self.moving { (self.time * 2.0).sin() * 40.0 } else { 0.0 };
            self.world.move_to(id, x, 0.0);
            if let Some(spawn) = self.world.spawn_mut(id) {
                spawn.flip_x = self.flip;
            }
        }
        self.world.step(dt, db, &no_ground);
        if self.system.is_none_or(|id| !self.world.is_alive(id)) {
            self.idle += dt;
            if self.idle >= REPLAY_PAUSE {
                self.restart();
            }
        }
    }

    /// Plays the effect from the start.
    pub fn restart(&mut self) {
        self.world.clear();
        self.idle = 0.0;
        self.system = self.shown.as_ref().map(|effect| {
            self.world.play_effect(Arc::new(effect.clone()), Spawn::at(0.0, 0.0).flip_x(self.flip))
        });
    }

    fn toolbar(&mut self, ui: &mut Ui, db: &AssetDb) {
        // Taken out while the toolbar uses the rest of the editor, and put
        // back after.
        let Some(mut taken) = self.open.take() else { return };
        let open = &mut taken;
        let dirty = open.effect != open.saved;
        let problems = open.effect.problems();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(format!("{}{}", open.path, if dirty { "*" } else { "" })).monospace(),
            );
            ui.separator();
            if ui.button(if self.playing { "⏸ Pause" } else { "▶ Play" }).clicked() {
                self.playing = !self.playing;
            }
            let mut restart =
                ui.button("⏮ Restart").on_hover_text("Play it from the start").clicked();
            if ui.button("⏹ Stop").on_hover_text("Stop emitting; particles finish").clicked()
                && let Some(id) = self.system
            {
                self.world.stop(id);
            }
            restart |= ui
                .checkbox(&mut self.flip, "Mirror")
                .on_hover_text("Play it facing left")
                .changed();
            ui.checkbox(&mut self.moving, "Move emitter")
                .on_hover_text("Swing the emitter side to side");
            ui.add(egui::Slider::new(&mut self.speed, 0.1..=2.0).text("speed"));
            ui.weak(format!("{} particles", self.world.particle_count()));
            ui.separator();
            if ui.add_enabled(open.edits.can_undo(), egui::Button::new("Undo")).clicked() {
                open.edits.undo(&mut open.effect);
            }
            if ui.add_enabled(open.edits.can_redo(), egui::Button::new("Redo")).clicked() {
                open.edits.redo(&mut open.effect);
            }
            let can_save = dirty && db.root().is_some() && problems.is_empty();
            let save = ui.add_enabled(can_save, egui::Button::new("Save"));
            let save = if problems.is_empty() {
                save
            } else {
                save.on_disabled_hover_text("Fix the problems first")
            };
            if save.clicked() {
                self.status = Some(match db.write(open.guid, open.effect.to_ron().as_bytes()) {
                    Ok(()) => {
                        open.saved = open.effect.clone();
                        format!("Saved {}", open.path)
                    }
                    Err(e) => format!("Couldn't save: {e}"),
                });
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                open.effect = open.saved.clone();
            }
            if let Some(status) = &self.status {
                ui.weak(status);
            }
            if restart {
                self.restart();
            }
        });
        for problem in &problems {
            ui.colored_label(Color32::from_rgb(255, 120, 110), problem);
        }
        ui.separator();
        self.open = Some(taken);
    }

    fn preview(&mut self, ui: &mut Ui, db: &AssetDb, thumbs: &mut Thumbs) {
        let space = ui.available_rect_before_wrap();
        let response = ui.allocate_rect(space, Sense::hover());
        let painter = ui.painter_at(space);
        painter.rect_filled(space, 0.0, Color32::from_rgb(22, 20, 28));
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.5, 16.0);
        }
        // The emitter sits in the middle, a little below centre.
        let origin = Pos2::new(space.center().x, space.top() + space.height() * 0.6);
        let zoom = self.zoom;
        let to_screen = |x: f32, y: f32| origin + Vec2::new(x, y) * zoom;
        let faint = Stroke::new(1.0, Color32::from_gray(48));
        painter.line_segment(
            [Pos2::new(space.left(), origin.y), Pos2::new(space.right(), origin.y)],
            faint,
        );
        painter.line_segment(
            [Pos2::new(origin.x, space.top()), Pos2::new(origin.x, space.bottom())],
            faint,
        );
        if let Some(effect) = &self.shown
            && effect.collision.enabled
        {
            let y = to_screen(0.0, effect.collision.floor).y;
            painter.line_segment(
                [Pos2::new(space.left(), y), Pos2::new(space.right(), y)],
                Stroke::new(1.0, Color32::from_rgb(120, 90, 60)),
            );
            painter.text(
                Pos2::new(space.left() + 6.0, y - 2.0),
                egui::Align2::LEFT_BOTTOM,
                "floor",
                egui::FontId::proportional(10.0),
                Color32::from_rgb(150, 120, 90),
            );
        }
        let glow = self
            .glow
            .get_or_insert_with(|| {
                let size = reign_particles::GLOW_SIZE as usize;
                let image = egui::ColorImage::from_rgba_unmultiplied(
                    [size, size],
                    &reign_particles::glow_image(),
                );
                ui.ctx().load_texture("particle glow", image, egui::TextureOptions::LINEAR)
            })
            .clone();
        let quads = self.world.quads(&mut |h| db.load(h).ok());
        for q in &quads {
            draw_quad(&painter, q, &to_screen, zoom, &glow, &mut |guid| {
                thumbs.texture(ui.ctx(), db, guid)
            });
        }
        if let Some(id) = self.system
            && let Some(spawn) = self.world.spawn_mut(id)
        {
            let at = to_screen(spawn.x, spawn.y);
            painter.circle_stroke(at, 4.0, Stroke::new(1.0, Color32::from_rgb(255, 120, 200)));
        }
        painter.text(
            space.right_top() + Vec2::new(-8.0, 8.0),
            egui::Align2::RIGHT_TOP,
            format!("{:.0}%  scroll to zoom", zoom * 100.0 / 3.0),
            egui::FontId::monospace(11.0),
            Color32::GRAY,
        );
    }
}

/// Paints one particle quad in the preview. Added light uses premultiplied
/// colour with no alpha, which egui adds onto what's under it.
fn draw_quad(
    painter: &egui::Painter,
    q: &Quad,
    to_screen: &dyn Fn(f32, f32) -> Pos2,
    zoom: f32,
    glow: &egui::TextureHandle,
    texture: &mut dyn FnMut(Guid) -> Option<egui::TextureHandle>,
) {
    let rect = Rect::from_min_size(to_screen(q.x, q.y), Vec2::new(q.w, q.h) * zoom);
    let [r, g, b, a] = q.color.map(|c| c.clamp(0.0, 1.0));
    let color = match q.blend {
        BlendMode::Alpha => Color32::from_rgba_unmultiplied(
            (r * 255.0) as u8,
            (g * 255.0) as u8,
            (b * 255.0) as u8,
            (a * 255.0) as u8,
        ),
        BlendMode::Add => Color32::from_rgba_premultiplied(
            (r * a * 255.0) as u8,
            (g * a * 255.0) as u8,
            (b * a * 255.0) as u8,
            0,
        ),
    };
    let full = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
    match q.image {
        QuadImage::Solid => {
            painter.rect_filled(rect, 0.0, color);
        }
        QuadImage::Glow => {
            painter.image(glow.id(), rect, full, color);
        }
        QuadImage::Sprite { texture: t, src } => {
            let Some(tex) = texture(t.guid) else { return };
            let size = tex.size_vec2();
            let (mut u0, mut u1) = (src[0] / size.x, (src[0] + src[2]) / size.x);
            if q.flip {
                std::mem::swap(&mut u0, &mut u1);
            }
            let uv = Rect::from_min_max(
                Pos2::new(u0, src[1] / size.y),
                Pos2::new(u1, (src[1] + src[3]) / size.y),
            );
            painter.image(tex.id(), rect, uv, color);
        }
    }
}

/// Every module's settings, Unity style: one foldout each.
fn modules(ui: &mut Ui, db: &AssetDb, e: &mut ParticleEffect) {
    module(ui, "Main", None, true, |ui| main_module(ui, &mut e.main));
    module(ui, "Emission", Some(e.emission.enabled), true, |ui| emission(ui, &mut e.emission));
    module(ui, "Shape", None, true, |ui| shape(ui, &mut e.shape));
    module(ui, "Forces", None, false, |ui| forces(ui, &mut e.forces));
    module(ui, "Velocity over Lifetime", Some(e.velocity.enabled), false, |ui| {
        let v = &mut e.velocity;
        ui.checkbox(&mut v.enabled, "On");
        ui.add_enabled_ui(v.enabled, |ui| {
            ui.label("Speed (times its own)").on_hover_text("Over the particle's life, 0 to 1");
            curve(ui, "velocity curve", &mut v.speed, 0.0..=4.0);
            ui.horizontal(|ui| {
                ui.label("Linear");
                ui.add(egui::DragValue::new(&mut v.linear[0]).prefix("x ").speed(1.0));
                ui.add(egui::DragValue::new(&mut v.linear[1]).prefix("y ").speed(1.0));
            });
        });
    });
    module(ui, "Color over Lifetime", Some(e.color.enabled), false, |ui| {
        let c = &mut e.color;
        ui.checkbox(&mut c.enabled, "On");
        ui.add_enabled_ui(c.enabled, |ui| gradient(ui, "color gradient", &mut c.gradient));
    });
    module(ui, "Size over Lifetime", Some(e.size.enabled), false, |ui| {
        let s = &mut e.size;
        ui.checkbox(&mut s.enabled, "On");
        ui.add_enabled_ui(s.enabled, |ui| curve(ui, "size curve", &mut s.curve, 0.0..=4.0));
    });
    module(ui, "Collision", Some(e.collision.enabled), false, |ui| collision(ui, &mut e.collision));
    let subs = if e.sub_emitters.is_empty() { Some(false) } else { None };
    module(ui, "Sub Emitters", subs, false, |ui| sub_emitters(ui, db, &mut e.sub_emitters));
    module(ui, "Lights", Some(e.lights.enabled), false, |ui| lights(ui, &mut e.lights));
    module(ui, "Renderer", None, true, |ui| render(ui, db, &mut e.render));
}

/// One module's foldout. `on` marks it switched off in the title.
fn module(ui: &mut Ui, title: &str, on: Option<bool>, open: bool, body: impl FnOnce(&mut Ui)) {
    let label = match on {
        Some(false) => RichText::new(format!("{title}  (off)")).strong().weak(),
        _ => RichText::new(title).strong(),
    };
    // Tests open them all, so every module's settings are drawn.
    let open = open || cfg!(test);
    egui::CollapsingHeader::new(label).id_salt(title).default_open(open).show(ui, body);
}

fn row(ui: &mut Ui, label: &str, tip: &str, add: impl FnOnce(&mut Ui)) {
    let r = ui.label(label);
    if !tip.is_empty() {
        r.on_hover_text(tip);
    }
    add(ui);
    ui.end_row();
}

fn grid(ui: &mut Ui, id: &str, add: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id).num_columns(2).spacing([8.0, 4.0]).show(ui, add);
}

fn number(
    value: &mut f32,
    speed: f64,
    range: std::ops::RangeInclusive<f32>,
) -> egui::DragValue<'_> {
    egui::DragValue::new(value).speed(speed).range(range).max_decimals(3)
}

fn range(ui: &mut Ui, r: &mut Range, speed: f64, min: f32, suffix: &str) {
    ui.horizontal(|ui| {
        ui.add(number(&mut r.0, speed, min..=f32::MAX).suffix(suffix));
        ui.label("to");
        ui.add(number(&mut r.1, speed, min..=f32::MAX).suffix(suffix));
    });
}

fn combo<T: Copy + PartialEq + std::fmt::Debug>(
    ui: &mut Ui,
    id: &str,
    value: &mut T,
    options: &[T],
) {
    egui::ComboBox::from_id_salt(id).selected_text(format!("{value:?}")).show_ui(ui, |ui| {
        for &o in options {
            ui.selectable_value(value, o, format!("{o:?}"));
        }
    });
}

fn main_module(ui: &mut Ui, m: &mut Main) {
    grid(ui, "main", |ui| {
        row(ui, "Duration", "Seconds in one cycle", |ui| {
            ui.add(number(&mut m.duration, 0.01, 0.01..=600.0).suffix(" s"));
        });
        row(ui, "Looping", "Starts another cycle when one ends", |ui| {
            ui.checkbox(&mut m.looping, "");
        });
        row(ui, "Start delay", "", |ui| {
            ui.add(number(&mut m.start_delay, 0.01, 0.0..=600.0).suffix(" s"));
        });
        row(ui, "Lifetime", "Seconds each particle lives", |ui| {
            range(ui, &mut m.lifetime, 0.01, 0.001, " s")
        });
        row(ui, "Speed", "Pixels a second", |ui| range(ui, &mut m.speed, 1.0, f32::MIN, ""));
        row(ui, "Size", "Pixels (sprites: a scale)", |ui| range(ui, &mut m.size, 0.1, 0.0, ""));
        row(ui, "Color", "Each particle is somewhere between the two", |ui| {
            ui.horizontal(|ui| {
                ui.color_edit_button_srgba_unmultiplied(&mut m.color.0);
                ui.label("to");
                ui.color_edit_button_srgba_unmultiplied(&mut m.color.1);
            });
        });
        row(ui, "Max particles", "", |ui| {
            ui.add(egui::DragValue::new(&mut m.max_particles).range(1..=20_000));
        });
        row(
            ui,
            "Space",
            "World: born particles stay put. Local: they move with the emitter.",
            |ui| {
                combo(ui, "space", &mut m.space, &[Space::World, Space::Local]);
            },
        );
    });
}

fn emission(ui: &mut Ui, e: &mut Emission) {
    ui.checkbox(&mut e.enabled, "On");
    ui.add_enabled_ui(e.enabled, |ui| {
        grid(ui, "emission", |ui| {
            row(ui, "Rate", "Particles a second", |ui| {
                ui.add(number(&mut e.rate, 1.0, 0.0..=100_000.0).suffix(" /s"));
            });
        });
        ui.label("Bursts");
        let mut remove = None;
        for (i, b) in e.bursts.iter_mut().enumerate() {
            ui.horizontal_wrapped(|ui| {
                ui.add(number(&mut b.time, 0.01, 0.0..=600.0).prefix("at ").suffix(" s"));
                ui.add(egui::DragValue::new(&mut b.count.0).range(0..=10_000).prefix("count "));
                ui.add(egui::DragValue::new(&mut b.count.1).range(0..=10_000).prefix("to "));
                ui.add(egui::DragValue::new(&mut b.cycles).range(1..=1000).prefix("×"));
                if b.cycles > 1 {
                    ui.add(number(&mut b.interval, 0.01, 0.0..=60.0).prefix("every ").suffix(" s"));
                }
                ui.add(number(&mut b.probability, 0.01, 0.0..=1.0).prefix("chance "));
                if ui.small_button("x").on_hover_text("Remove this burst").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            e.bursts.remove(i);
        }
        if ui.button("+ Burst").clicked() {
            e.bursts.push(Burst::default());
        }
    });
}

fn shape(ui: &mut Ui, s: &mut Shape) {
    grid(ui, "shape", |ui| {
        row(ui, "Shape", "", |ui| {
            combo(
                ui,
                "shape kind",
                &mut s.kind,
                &[
                    ShapeKind::Point,
                    ShapeKind::Cone,
                    ShapeKind::Circle,
                    ShapeKind::Box,
                    ShapeKind::Line,
                ],
            );
        });
        row(ui, "Angle", "The way they head: 0 right, 90 down, -90 up", |ui| {
            ui.add(number(&mut s.angle, 1.0, -360.0..=360.0).suffix("°"));
        });
        row(ui, "Spread", "How wide, all told: 0 a line, 360 every way", |ui| {
            ui.add(number(&mut s.spread, 1.0, 0.0..=360.0).suffix("°"));
        });
        match s.kind {
            ShapeKind::Cone | ShapeKind::Circle => row(ui, "Radius", "", |ui| {
                ui.add(number(&mut s.radius, 0.5, 0.0..=10_000.0));
            }),
            ShapeKind::Box => row(ui, "Size", "", |ui| {
                ui.add(number(&mut s.size[0], 0.5, 0.0..=10_000.0).prefix("w "));
                ui.add(number(&mut s.size[1], 0.5, 0.0..=10_000.0).prefix("h "));
            }),
            ShapeKind::Line => row(ui, "Length", "", |ui| {
                ui.add(number(&mut s.size[0], 0.5, 0.0..=10_000.0));
            }),
            ShapeKind::Point => {}
        }
        if s.kind == ShapeKind::Circle {
            row(ui, "Edge only", "Born on the edge, not inside", |ui| {
                ui.checkbox(&mut s.edge, "");
            });
        }
        row(ui, "Squash", "1 keeps it round; 0.3 flattens it into a ring on the ground", |ui| {
            ui.add(number(&mut s.squash, 0.01, 0.0..=4.0));
        });
        row(ui, "Randomize", "Mixes each direction with a random one", |ui| {
            ui.add(egui::Slider::new(&mut s.randomize, 0.0..=1.0));
        });
    });
}

fn forces(ui: &mut Ui, f: &mut Forces) {
    grid(ui, "forces", |ui| {
        row(ui, "Gravity", "Pixels a second each second, downward", |ui| {
            ui.add(number(&mut f.gravity, 1.0, -10_000.0..=10_000.0));
        });
        row(ui, "Drag", "Fraction of its speed lost each second", |ui| {
            ui.add(egui::Slider::new(&mut f.drag, 0.0..=1.0));
        });
        row(ui, "Wind", "", |ui| {
            ui.add(number(&mut f.wind[0], 1.0, -10_000.0..=10_000.0).prefix("x "));
            ui.add(number(&mut f.wind[1], 1.0, -10_000.0..=10_000.0).prefix("y "));
        });
    });
}

fn collision(ui: &mut Ui, c: &mut Collision) {
    ui.checkbox(&mut c.enabled, "On");
    ui.add_enabled_ui(c.enabled, |ui| {
        grid(ui, "collision", |ui| {
            row(ui, "On hit", "", |ui| {
                combo(
                    ui,
                    "collision mode",
                    &mut c.mode,
                    &[CollisionMode::Bounce, CollisionMode::Stick, CollisionMode::Die],
                );
            });
            row(ui, "Floor", "Pixels below the emitter, unless the game gives the floor", |ui| {
                ui.add(number(&mut c.floor, 0.5, -10_000.0..=10_000.0));
            });
            row(ui, "Hit the level", "Also the game's solid ground and walls", |ui| {
                ui.checkbox(&mut c.world, "");
            });
            if c.mode == CollisionMode::Bounce {
                row(ui, "Bounce", "Speed kept off the surface", |ui| {
                    ui.add(egui::Slider::new(&mut c.bounce, 0.0..=1.0));
                });
                row(ui, "Friction", "Speed along the surface lost", |ui| {
                    ui.add(egui::Slider::new(&mut c.friction, 0.0..=1.0));
                });
            }
            row(ui, "Lifetime loss", "Remaining life lost each hit", |ui| {
                ui.add(egui::Slider::new(&mut c.lifetime_loss, 0.0..=1.0));
            });
        });
    });
}

fn sub_emitters(ui: &mut Ui, db: &AssetDb, subs: &mut Vec<SubEmitter>) {
    let mut remove = None;
    for (i, s) in subs.iter_mut().enumerate() {
        ui.push_id(i, |ui| {
            ui.horizontal(|ui| {
                combo(
                    ui,
                    "trigger",
                    &mut s.trigger,
                    &[SubTrigger::Birth, SubTrigger::Collision, SubTrigger::Death],
                );
                asset_slot(ui, db, &mut s.effect.guid, AssetKind::ParticleEffect);
                if ui.small_button("x").on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            });
            ui.horizontal(|ui| {
                ui.add(number(&mut s.probability, 0.01, 0.0..=1.0).prefix("chance "));
                ui.add(number(&mut s.inherit_velocity, 0.01, 0.0..=1.0).prefix("velocity "))
                    .on_hover_text("How much of the particle's velocity it inherits");
                ui.checkbox(&mut s.inherit_color, "color");
            });
            ui.separator();
        });
    }
    if let Some(i) = remove {
        subs.remove(i);
    }
    if ui.button("+ Sub-emitter").clicked() {
        subs.push(SubEmitter::default());
    }
    ui.weak("Drop a particle effect from the Project panel on a slot.");
}

fn lights(ui: &mut Ui, l: &mut Lights) {
    ui.checkbox(&mut l.enabled, "On");
    ui.add_enabled_ui(l.enabled, |ui| {
        grid(ui, "lights", |ui| {
            row(ui, "Ratio", "Fraction of particles with a light", |ui| {
                ui.add(egui::Slider::new(&mut l.ratio, 0.0..=1.0));
            });
            row(ui, "Radius", "", |ui| {
                ui.add(number(&mut l.radius, 0.5, 0.0..=2000.0));
            });
            row(ui, "Intensity", "", |ui| {
                ui.add(number(&mut l.intensity, 0.05, 0.0..=20.0));
            });
            row(ui, "Particle color", "Lit in each particle's own colour", |ui| {
                ui.checkbox(&mut l.use_particle_color, "");
            });
            if !l.use_particle_color {
                row(ui, "Color", "", |ui| {
                    ui.color_edit_button_srgba_unmultiplied(&mut l.color);
                });
            }
        });
        ui.weak("The preview doesn't show lights; the game does.");
    });
}

fn render(ui: &mut Ui, db: &AssetDb, r: &mut Render) {
    grid(ui, "render", |ui| {
        row(ui, "Mode", "", |ui| {
            combo(
                ui,
                "render mode",
                &mut r.mode,
                &[RenderMode::Square, RenderMode::Streak, RenderMode::Glow, RenderMode::Sprite],
            );
        });
        row(ui, "Blend", "Add makes them glow like light", |ui| {
            combo(ui, "blend", &mut r.blend, &[BlendMode::Alpha, BlendMode::Add]);
        });
        match r.mode {
            RenderMode::Square => {
                row(ui, "Aspect", "Width over height: 3 draws a flat splat", |ui| {
                    ui.add(number(&mut r.aspect, 0.05, 0.05..=32.0));
                })
            }
            RenderMode::Streak => {
                row(ui, "Length", "Seconds of travel each streak shows", |ui| {
                    ui.add(number(&mut r.streak, 0.001, 0.0..=1.0).suffix(" s"));
                });
                row(ui, "Longest", "Pixels", |ui| {
                    ui.add(number(&mut r.max_streak, 0.5, 1.0..=256.0));
                });
            }
            RenderMode::Sprite => {
                row(ui, "Sheet", "Drop a sprite sheet here", |ui| {
                    asset_slot(ui, db, &mut r.sheet.guid, AssetKind::SpriteSheet);
                });
                row(ui, "Sprites", "Names, comma separated; empty uses them all", |ui| {
                    let mut text = r.sprites.join(", ");
                    if ui.text_edit_singleline(&mut text).changed() {
                        r.sprites = text
                            .split(',')
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                });
                row(ui, "Frames", "", |ui| {
                    combo(
                        ui,
                        "frames",
                        &mut r.frames,
                        &[FrameMode::OverLife, FrameMode::Random, FrameMode::Fps],
                    );
                });
                if r.frames == FrameMode::Fps {
                    row(ui, "FPS", "", |ui| {
                        ui.add(number(&mut r.fps, 0.5, 0.0..=240.0));
                    });
                }
            }
            RenderMode::Glow => {}
        }
        row(ui, "Pixel snap", "Draw on whole pixels, as crisp as the art", |ui| {
            ui.checkbox(&mut r.pixel_snap, "");
        });
    });
}

/// A slot showing an asset by name; drop one of `kind` from the Project
/// panel on it, or clear it.
fn asset_slot(ui: &mut Ui, db: &AssetDb, guid: &mut Guid, kind: AssetKind) {
    let name = if guid.is_nil() {
        "None".to_owned()
    } else {
        db.path(*guid).map_or_else(
            || format!("missing {guid}"),
            |p| p.rsplit('/').next().unwrap_or(&p).to_owned(),
        )
    };
    let r = ui.add(egui::Label::new(RichText::new(name).monospace()).sense(Sense::hover()));
    if let Some(drag) = r.dnd_release_payload::<AssetDrag>()
        && drag.kind == kind
    {
        *guid = drag.guid;
    }
    r.on_hover_text(format!("Drop a {} here", kind.name()));
    if !guid.is_nil() && ui.small_button("∅").on_hover_text("Clear").clicked() {
        *guid = Guid::NIL;
    }
}

/// A curve over life: a small plot and its keys.
fn curve(ui: &mut Ui, id: &str, c: &mut Curve, values: std::ops::RangeInclusive<f32>) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width().min(240.0), 48.0), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_gray(30));
    let top = c.0.iter().map(|k| k.1).fold(1.0f32, f32::max);
    let point = |t: f32, v: f32| {
        Pos2::new(
            rect.left() + t * rect.width(),
            rect.bottom() - (v / top).clamp(0.0, 1.0) * rect.height(),
        )
    };
    let line: Vec<Pos2> = (0..=32).map(|i| i as f32 / 32.0).map(|t| point(t, c.at(t))).collect();
    painter.add(egui::Shape::line(line, Stroke::new(1.5, Color32::from_rgb(120, 200, 255))));
    ui.push_id(id, |ui| {
        let mut remove = None;
        for (i, key) in c.0.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(number(&mut key.0, 0.01, 0.0..=1.0).prefix("life "));
                ui.add(number(&mut key.1, 0.01, values.clone()).prefix("value "));
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            c.0.remove(i);
        }
        if ui.small_button("+ Key").clicked() {
            let last = c.0.last().map_or((0.0, 1.0), |k| *k);
            c.0.push(((last.0 + 0.25).min(1.0), last.1));
        }
        c.0.sort_by(|a, b| a.0.total_cmp(&b.0));
    });
}

/// A gradient over life: a strip and its colour keys.
fn gradient(ui: &mut Ui, id: &str, g: &mut Gradient) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width().min(240.0), 18.0), Sense::hover());
    let painter = ui.painter_at(rect);
    // A checkerboard under it, so transparency shows.
    for i in 0..24 {
        let x = rect.left() + i as f32 * rect.width() / 24.0;
        let shade = if i % 2 == 0 { 70 } else { 110 };
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(x, rect.top()),
                Vec2::new(rect.width() / 24.0, rect.height()),
            ),
            0.0,
            Color32::from_gray(shade),
        );
    }
    for i in 0..48 {
        let t = (i as f32 + 0.5) / 48.0;
        let [r, gr, b, a] = g.at(t).map(|v| (v * 255.0) as u8);
        let x = rect.left() + i as f32 * rect.width() / 48.0;
        painter.rect_filled(
            Rect::from_min_size(
                Pos2::new(x, rect.top()),
                Vec2::new(rect.width() / 48.0 + 0.5, rect.height()),
            ),
            0.0,
            Color32::from_rgba_unmultiplied(r, gr, b, a),
        );
    }
    ui.push_id(id, |ui| {
        let mut remove = None;
        for (i, key) in g.0.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(number(&mut key.t, 0.01, 0.0..=1.0).prefix("life "));
                ui.color_edit_button_srgba_unmultiplied(&mut key.color);
                if ui.small_button("x").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            g.0.remove(i);
        }
        if ui.small_button("+ Key").clicked() {
            let last = g.0.last().map_or(ColorKey { t: 0.0, color: [255; 4] }, |k| *k);
            g.0.push(ColorKey { t: (last.t + 0.25).min(1.0), ..last });
        }
        g.0.sort_by(|a, b| a.t.total_cmp(&b.t));
    });
}

/// The name a new effect gets in `folder`: "New Particle Effect", then
/// "New Particle Effect 2" and so on, past the ones that exist.
pub fn new_effect_path(folder: &str, exists: impl Fn(&str) -> bool) -> String {
    let at = |name: String| {
        if folder.is_empty() {
            format!("{name}.particles.ron")
        } else {
            format!("{folder}/{name}.particles.ron")
        }
    };
    let mut n = 1;
    loop {
        let name = if n == 1 {
            "New Particle Effect".to_owned()
        } else {
            format!("New Particle Effect {n}")
        };
        let path = at(name);
        if !exists(&path) {
            return path;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every module switched on, with a burst, a sub-emitter and keys.
    fn everything() -> ParticleEffect {
        ParticleEffect {
            emission: Emission { bursts: vec![Burst::default()], ..Emission::default() },
            velocity: VelocityOverLife { enabled: true, ..VelocityOverLife::default() },
            color: ColorOverLife { enabled: true, ..ColorOverLife::default() },
            size: SizeOverLife { enabled: true, ..SizeOverLife::default() },
            collision: Collision { enabled: true, ..Collision::default() },
            sub_emitters: vec![SubEmitter {
                effect: Handle::new(Guid(1234)),
                ..SubEmitter::default()
            }],
            lights: Lights { enabled: true, use_particle_color: false, ..Lights::default() },
            ..ParticleEffect::default()
        }
    }

    #[test]
    fn every_module_and_render_mode_draws() {
        let dir = std::env::temp_dir().join(format!("reign-particle-ui-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("all.particles.ron"), everything().to_ron()).unwrap();
        let db = AssetDb::loose(&dir).unwrap();
        let guid = db.guid("all.particles.ron").unwrap();
        let mut editor = ParticleEditor::default();
        editor.open(&db, guid);
        let mut thumbs = Thumbs::default();
        let ctx = egui::Context::default();
        let shapes =
            [ShapeKind::Point, ShapeKind::Cone, ShapeKind::Circle, ShapeKind::Box, ShapeKind::Line];
        let modes = [RenderMode::Square, RenderMode::Streak, RenderMode::Glow, RenderMode::Sprite];
        for (i, mode) in modes.into_iter().enumerate() {
            let open = editor.open.as_mut().unwrap();
            open.effect.render.mode = mode;
            open.effect.shape.kind = shapes[i];
            open.effect.render.frames = FrameMode::Fps;
            for _ in 0..20 {
                let input = egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 800.0))),
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| editor.ui(ui, &db, &mut thumbs));
                output.textures_delta.clear();
            }
        }
        // Sprites with no sheet is a problem: the last good version (glow)
        // kept playing, and nothing was saved.
        assert_eq!(editor.shown.as_ref().unwrap().render.mode, RenderMode::Glow);
        assert_eq!(editor.editing(), Some((guid, true)));
        assert!(editor.world.particle_count() > 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn new_effects_get_names_that_arent_taken() {
        assert_eq!(new_effect_path("", |_| false), "New Particle Effect.particles.ron");
        let taken =
            ["fx/New Particle Effect.particles.ron", "fx/New Particle Effect 2.particles.ron"];
        assert_eq!(
            new_effect_path("fx", |p| taken.contains(&p)),
            "fx/New Particle Effect 3.particles.ron"
        );
    }

    #[test]
    fn the_preview_plays_edits_at_once_and_replays_one_shots() {
        let dir =
            std::env::temp_dir().join(format!("reign-particle-editor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let effect = ParticleEffect {
            main: Main {
                looping: false,
                duration: 0.1,
                lifetime: Range(0.2, 0.2),
                ..Main::default()
            },
            emission: Emission {
                rate: 0.0,
                bursts: vec![Burst { count: CountRange(5, 5), ..Burst::default() }],
                ..Emission::default()
            },
            ..ParticleEffect::default()
        };
        std::fs::write(dir.join("puff.particles.ron"), effect.to_ron()).unwrap();
        let db = AssetDb::loose(&dir).unwrap();
        let guid = db.guid("puff.particles.ron").unwrap();
        let mut editor = ParticleEditor::default();
        editor.open(&db, guid);
        editor.advance(1.0 / 60.0, &db);
        assert_eq!(editor.particle_count(), 5);
        assert_eq!(editor.editing(), Some((guid, false)));

        // An edit shows on the running effect without a restart.
        editor.open.as_mut().unwrap().effect.emission.bursts[0].count = CountRange(9, 9);
        assert_eq!(editor.editing(), Some((guid, true)));
        // The first play's particles die at frame 13 (0.2 s); the preview
        // waits 0.6 s and replays at frame 48, with the edit.
        for _ in 0..46 {
            editor.advance(1.0 / 60.0, &db);
        }
        assert_eq!(editor.particle_count(), 0, "over, waiting to replay");
        for _ in 0..6 {
            editor.advance(1.0 / 60.0, &db);
        }
        assert_eq!(editor.particle_count(), 9, "played again, with the edit");

        // A broken edit keeps the last good version playing.
        editor.open.as_mut().unwrap().effect.main.duration = 0.0;
        for _ in 0..60 {
            editor.advance(1.0 / 60.0, &db);
        }
        assert_eq!(editor.shown.as_ref().unwrap().main.duration, 0.1);
        let _ = std::fs::remove_dir_all(dir);
    }
}
