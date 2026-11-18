//! The Lighting tab's Modify dialog: edit a normal map or a light mask by
//! hand. Pick pixels with the wand (W: a colour cluster), the pencil (B) or
//! the marquee (M); Shift adds to the selection and Alt takes away. Then
//! point the selected pixels somewhere (normal map) or change their
//! brightness (light mask). Edits show in the lit preview as you make them;
//! Apply keeps them, Cancel throws them away, and Undo steps back through
//! them (selections included).

use egui::{Color32, Pos2, Rect, Stroke, Ui, Vec2};
use reign_assets::RgbaImage;
use reign_assets::lighting::{
    BrightnessMode, Direction, SelectMode, Selection, adjust_brightness, apply_normal,
    average_normals, direction_normal, scale_normal_strength, sphere_facet,
};

use crate::lighting_editor::{Map, MapKind, Versioned};
use crate::undo::Edits;

/// Where the image is on the canvas.
pub(crate) struct CanvasMap {
    pub image_rect: Rect,
    pub zoom: f32,
    pub width: u32,
    pub height: u32,
}

impl CanvasMap {
    /// The image pixel under a screen point, if it is over the image.
    pub fn pixel(&self, p: Pos2) -> Option<(u32, u32)> {
        let v = (p - self.image_rect.min) / self.zoom;
        if v.x < 0.0 || v.y < 0.0 {
            return None;
        }
        let (x, y) = (v.x as u32, v.y as u32);
        (x < self.width && y < self.height).then_some((x, y))
    }

    /// The nearest image pixel to a screen point, even off the image.
    pub fn pixel_clamped(&self, p: Pos2) -> (u32, u32) {
        let v = (p - self.image_rect.min) / self.zoom;
        let x = (v.x.floor().max(0.0) as u32).min(self.width.saturating_sub(1));
        let y = (v.y.floor().max(0.0) as u32).min(self.height.saturating_sub(1));
        (x, y)
    }

    fn screen(&self, x: u32, y: u32) -> Pos2 {
        self.image_rect.min + Vec2::new(x as f32, y as f32) * self.zoom
    }
}

/// The images the wand can read colours from.
pub(crate) struct WandSources<'a> {
    pub albedo: &'a RgbaImage,
    pub normal: Option<&'a RgbaImage>,
    pub mask: Option<&'a RgbaImage>,
}

/// What the dialog did this frame.
pub(crate) enum Outcome {
    Open,
    Apply,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Wand,
    Pencil,
    Marquee,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WandSource {
    Albedo,
    NormalMap,
    LightMask,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApplyMode {
    Average,
    Face,
}

/// What undo steps back through.
#[derive(Clone, PartialEq)]
struct State {
    image: Map,
    sel: Versioned<Selection>,
    /// Normal Strength in percent: scales the whole normal map.
    strength: u32,
}

enum Act {
    ClearSelection,
    FillHoles,
    MergeOutlines,
    Direction(Direction),
    Normal([f32; 3], &'static str),
    Average,
    Brightness,
}

/// Gaps up to this many pixels are filled by Fill Holes.
const FILL_HOLES: usize = 16;

pub(crate) struct ModifyDialog {
    pub kind: MapKind,
    /// Show the map itself instead of the lit result.
    pub show_texture: bool,
    state: State,
    edits: Edits<State>,
    tool: Tool,
    wand_source: WandSource,
    contiguous: bool,
    mode: ApplyMode,
    /// Degrees, for the direction buttons.
    angle: f32,
    brightness: f32,
    brightness_mode: BrightnessMode,
    selection_color: Color32,
    hint_color: Color32,
    readout: String,
    /// The last pixel a drag reached.
    last: Option<(u32, u32)>,
    /// The marquee being dragged: where it started and where it is.
    marquee: Option<((u32, u32), (u32, u32))>,
    scaled: Option<((u64, u32), Map)>,
}

impl ModifyDialog {
    pub fn new(kind: MapKind, map: Map, width: u32, height: u32) -> Self {
        let state =
            State { image: map, sel: Versioned::new(Selection::new(width, height)), strength: 100 };
        ModifyDialog {
            kind,
            show_texture: false,
            edits: Edits::new(&state),
            state,
            tool: Tool::Wand,
            wand_source: WandSource::Albedo,
            contiguous: true,
            mode: ApplyMode::Face,
            angle: 45.0,
            brightness: 100.0,
            brightness_mode: BrightnessMode::Relative,
            selection_color: Color32::from_rgba_unmultiplied(80, 160, 255, 70),
            hint_color: Color32::from_rgb(255, 255, 255),
            readout: "No angle applied".into(),
            last: None,
            marquee: None,
            scaled: None,
        }
    }

    /// How many pixels are selected.
    #[cfg(test)]
    pub fn selected(&self) -> usize {
        self.state.sel.value.count()
    }

    pub fn undo(&mut self) -> bool {
        self.edits.undo(&mut self.state)
    }

    pub fn redo(&mut self) -> bool {
        self.edits.redo(&mut self.state)
    }

    /// The map as edited so far (a normal map with Normal Strength applied).
    pub fn shown(&mut self) -> Map {
        if self.kind != MapKind::Normal || self.state.strength == 100 {
            return self.state.image.clone();
        }
        let key = (self.state.image.rev, self.state.strength);
        if self.scaled.as_ref().is_none_or(|(k, _)| *k != key) {
            let mut image = (*self.state.image.value).clone();
            scale_normal_strength(&mut image, self.state.strength as f32 / 100.0);
            self.scaled = Some((key, Map::new(image)));
        }
        self.scaled.as_ref().map(|(_, m)| m.clone()).unwrap_or_else(|| self.state.image.clone())
    }

    fn do_act(&mut self, act: Act, sources: &WandSources) {
        let sel = self.state.sel.value.clone();
        match act {
            Act::ClearSelection => self.state.sel.edit(Selection::clear),
            Act::FillHoles => self.state.sel.edit(|s| s.fill_holes(FILL_HOLES)),
            Act::MergeOutlines => self.state.sel.edit(|s| s.merge_outlines(sources.albedo)),
            Act::Direction(d) => {
                let n = direction_normal(d, self.angle);
                self.state.image.edit(|i| apply_normal(i, &sel, n));
                self.readout = d.label().to_owned();
            }
            Act::Normal(n, name) => {
                self.state.image.edit(|i| apply_normal(i, &sel, n));
                self.readout = name.to_owned();
            }
            Act::Average => {
                let done = self.state.image.edit(|i| average_normals(i, &sel));
                self.readout =
                    if done { "Existing directions" } else { "Select pixels first" }.into();
            }
            Act::Brightness => {
                let (percent, mode) = (self.brightness, self.brightness_mode);
                self.state.image.edit(|i| adjust_brightness(i, &sel, percent, mode));
            }
        }
    }

    /// The dialog's window. It reads and edits only itself; the tab applies or drops the result.
    pub fn window(&mut self, ctx: &egui::Context, sources: &WandSources) -> Outcome {
        let mut outcome = Outcome::Open;
        let mut act = None;
        let title = format!("Modify {}", self.kind.label().to_lowercase());
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::LEFT_TOP, Vec2::new(12.0, 80.0))
            .show(ctx, |ui| {
                ui.set_width(290.0);
                ui.horizontal(|ui| {
                    for (tool, label, tip) in [
                        (Tool::Wand, "Wand (W)", "Click a colour cluster. Drag to keep selecting"),
                        (Tool::Pencil, "Pencil (B)", "Select pixels one by one"),
                        (Tool::Marquee, "Marquee (M)", "Drag a rectangle"),
                    ] {
                        ui.selectable_value(&mut self.tool, tool, label).on_hover_text(tip);
                    }
                });
                ui.weak("Shift+click adds to the selection; Alt+click takes away.");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_label("Wand Source")
                        .selected_text(match self.wand_source {
                            WandSource::Albedo => "Albedo",
                            WandSource::NormalMap => "Normal Map",
                            WandSource::LightMask => "Light Mask",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.wand_source,
                                WandSource::Albedo,
                                "Albedo",
                            );
                            ui.selectable_value(
                                &mut self.wand_source,
                                WandSource::NormalMap,
                                "Normal Map",
                            );
                            ui.selectable_value(
                                &mut self.wand_source,
                                WandSource::LightMask,
                                "Light Mask",
                            );
                        });
                });
                ui.checkbox(&mut self.contiguous, "Contiguous")
                    .on_hover_text("On: only touching pixels. Off: every pixel of that colour");
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Clear Selection").clicked() {
                        act = Some(Act::ClearSelection);
                    }
                    if ui
                        .button("Fill Holes")
                        .on_hover_text("Fill small gaps the selection encloses")
                        .clicked()
                    {
                        act = Some(Act::FillHoles);
                    }
                    if ui
                        .button("Merge Outlines")
                        .on_hover_text("Add the outlines that touch the selection")
                        .clicked()
                    {
                        act = Some(Act::MergeOutlines);
                    }
                });
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgba(&mut self.selection_color);
                    ui.label("Selection color");
                    ui.color_edit_button_srgba(&mut self.hint_color);
                    ui.label("Hint color");
                });
                ui.weak(format!("{} pixels selected", self.state.sel.value.count()));
                ui.checkbox(&mut self.show_texture, "Show Texture")
                    .on_hover_text("Show the map itself instead of the lit result");
                ui.separator();

                match self.kind {
                    MapKind::Normal => self.normal_controls(ui, &mut act),
                    MapKind::LightMask => {
                        crate::lighting_editor::reset_slider(
                            ui,
                            "Brightness",
                            &mut self.brightness,
                            0.0..=200.0,
                            100.0,
                            "%",
                        );
                        ui.horizontal(|ui| {
                            ui.radio_value(
                                &mut self.brightness_mode,
                                BrightnessMode::Relative,
                                "Relative",
                            )
                            .on_hover_text("Multiplies what is there");
                            ui.radio_value(
                                &mut self.brightness_mode,
                                BrightnessMode::Fixed,
                                "Fixed",
                            )
                            .on_hover_text("Sets every selected pixel to this");
                        });
                        if ui.button("Adjust selection").clicked() {
                            act = Some(Act::Brightness);
                        }
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.edits.can_undo(), egui::Button::new("Undo")).clicked() {
                        self.edits.undo(&mut self.state);
                    }
                    if ui.add_enabled(self.edits.can_redo(), egui::Button::new("Redo")).clicked() {
                        self.edits.redo(&mut self.state);
                    }
                    if ui.button("Apply").clicked() {
                        outcome = Outcome::Apply;
                    }
                    if ui.button("Cancel").clicked() {
                        outcome = Outcome::Cancel;
                    }
                });
            });
        if let Some(act) = act {
            self.do_act(act, sources);
        }
        let holding = ctx.input(|i| i.pointer.any_down());
        self.edits.track(&self.state, holding);
        outcome
    }

    fn normal_controls(&mut self, ui: &mut Ui, act: &mut Option<Act>) {
        ui.label(egui::RichText::new("Apply Direction").strong());
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.mode, ApplyMode::Average, "Average Angle")
                .on_hover_text("Averages the existing normals across the selected pixels");
            ui.radio_value(&mut self.mode, ApplyMode::Face, "Face Angle")
                .on_hover_text("Click a facet of the sphere, or a direction");
        });
        match self.mode {
            ApplyMode::Average => {
                if ui.button("Average selection").clicked() {
                    *act = Some(Act::Average);
                }
            }
            ApplyMode::Face => {
                if let Some(n) = sphere_widget(ui) {
                    *act = Some(Act::Normal(n, "Sphere facet"));
                }
                crate::lighting_editor::reset_slider(
                    ui,
                    "Angle Strength",
                    &mut self.angle,
                    0.0..=89.0,
                    45.0,
                    "°",
                );
                egui::Grid::new("directions").show(ui, |ui| {
                    for (i, d) in Direction::ALL.into_iter().enumerate() {
                        if ui.button(d.label()).clicked() {
                            *act = Some(Act::Direction(d));
                        }
                        if i % 3 == 2 {
                            ui.end_row();
                        }
                    }
                });
            }
        }
        ui.weak(&self.readout);
        let mut strength = self.state.strength as f32;
        crate::lighting_editor::reset_slider(
            ui,
            "Normal Strength",
            &mut strength,
            0.0..=150.0,
            100.0,
            "%",
        );
        self.state.strength = strength.round() as u32;
        ui.weak("Normal Strength adjusts the entire map.");
    }

    /// Reads the pointer on the canvas: the tools, and W, B and M to pick one.
    pub fn canvas(
        &mut self,
        ui: &mut Ui,
        response: &egui::Response,
        map: &CanvasMap,
        sources: &WandSources,
        space_held: bool,
    ) {
        if !ui.ctx().egui_wants_keyboard_input() {
            ui.input(|i| {
                for (key, tool) in [
                    (egui::Key::W, Tool::Wand),
                    (egui::Key::B, Tool::Pencil),
                    (egui::Key::M, Tool::Marquee),
                ] {
                    if i.key_pressed(key) && !i.modifiers.command {
                        self.tool = tool;
                    }
                }
            });
        }
        if space_held || response.dragged_by(egui::PointerButton::Middle) {
            return;
        }
        let modifiers = ui.input(|i| i.modifiers);
        let held = if modifiers.alt {
            SelectMode::Subtract
        } else if modifiers.shift {
            SelectMode::Add
        } else {
            SelectMode::Replace
        };
        let primary = egui::PointerButton::Primary;
        let at = response.interact_pointer_pos().or(response.hover_pos());
        let here = at.and_then(|p| map.pixel(p));
        let source = match self.wand_source {
            WandSource::NormalMap => sources.normal.unwrap_or(sources.albedo),
            WandSource::LightMask => sources.mask.unwrap_or(sources.albedo),
            WandSource::Albedo => sources.albedo,
        };
        let (tool, contiguous) = (self.tool, self.contiguous);
        if response.clicked_by(primary)
            && let Some((x, y)) = here
        {
            match tool {
                Tool::Wand => self.state.sel.edit(|s| s.wand(source, x, y, contiguous, held)),
                Tool::Pencil | Tool::Marquee => self.state.sel.edit(|s| s.select_pixel(x, y, held)),
            }
        }
        if response.drag_started_by(primary) {
            self.last = None;
            if tool == Tool::Marquee
                && let Some(p) = at
            {
                let start = map.pixel_clamped(p);
                self.marquee = Some((start, start));
            }
        }
        if response.dragged_by(primary)
            && let Some(p) = at
        {
            let first = self.last.is_none();
            let mode = if first { held } else { SelectMode::Add };
            let mode =
                if !first && held == SelectMode::Subtract { SelectMode::Subtract } else { mode };
            match tool {
                Tool::Marquee => {
                    if let Some((start, _)) = self.marquee {
                        self.marquee = Some((start, map.pixel_clamped(p)));
                    }
                }
                Tool::Wand | Tool::Pencil => {
                    if let Some((x, y)) = map.pixel(p)
                        && self.last != Some((x, y))
                    {
                        match tool {
                            Tool::Wand => {
                                self.state.sel.edit(|s| s.wand(source, x, y, contiguous, mode))
                            }
                            _ => {
                                // Fill the gap a fast stroke leaves.
                                let from = self.last.unwrap_or((x, y));
                                for (px, py) in line(from, (x, y)) {
                                    self.state.sel.edit(|s| s.select_pixel(px, py, mode));
                                }
                            }
                        }
                        self.last = Some((x, y));
                    }
                }
            }
        }
        if response.drag_stopped_by(primary) {
            if let Some((a, b)) = self.marquee.take() {
                self.state.sel.edit(|s| s.select_rect(a, b, held));
            }
            self.last = None;
        }
    }

    /// The selection over the canvas: a tint, and marching ants round its edge.
    pub fn overlay(&self, painter: &egui::Painter, map: &CanvasMap, time: f64) {
        let sel = &*self.state.sel.value;
        let clip = painter.clip_rect();
        let v0 = (clip.min - map.image_rect.min) / map.zoom;
        let v1 = (clip.max - map.image_rect.min) / map.zoom;
        let (x0, y0) = (v0.x.floor().max(0.0) as u32, v0.y.floor().max(0.0) as u32);
        let (x1, y1) = (
            (v1.x.ceil().max(0.0) as u32).min(map.width),
            (v1.y.ceil().max(0.0) as u32).min(map.height),
        );
        let phase = (time * 6.0) as i64;
        for y in y0..y1 {
            let mut run: Option<u32> = None;
            for x in x0..=x1 {
                let on = x < x1 && sel.contains(x, y);
                match (on, run) {
                    (true, None) => run = Some(x),
                    (false, Some(start)) => {
                        let r = Rect::from_min_max(map.screen(start, y), map.screen(x, y + 1));
                        painter.rect_filled(r, 0.0, self.selection_color);
                        run = None;
                    }
                    _ => {}
                }
                if !on {
                    continue;
                }
                let edges = [
                    (y == 0 || !sel.contains(x, y - 1), (x, y), (x + 1, y)),
                    (!sel.contains(x, y + 1), (x, y + 1), (x + 1, y + 1)),
                    (x == 0 || !sel.contains(x - 1, y), (x, y), (x, y + 1)),
                    (!sel.contains(x + 1, y), (x + 1, y), (x + 1, y + 1)),
                ];
                for (edge, a, b) in edges {
                    if edge {
                        let on_phase = ((i64::from(x) + i64::from(y) + phase) / 2) % 2 == 0;
                        let color = if on_phase { self.hint_color } else { Color32::BLACK };
                        painter.line_segment(
                            [map.screen(a.0, a.1), map.screen(b.0, b.1)],
                            Stroke::new(1.5, color),
                        );
                    }
                }
            }
        }
        if let Some((a, b)) = self.marquee {
            let (min, max) = ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0) + 1, a.1.max(b.1) + 1));
            let r = Rect::from_min_max(map.screen(min.0, min.1), map.screen(max.0, max.1));
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.5, self.hint_color),
                egui::StrokeKind::Outside,
            );
        }
    }
}

/// The pixels on a straight line between two, both included.
fn line(a: (u32, u32), b: (u32, u32)) -> Vec<(u32, u32)> {
    let (mut x, mut y) = (i64::from(a.0), i64::from(a.1));
    let (x1, y1) = (i64::from(b.0), i64::from(b.1));
    let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
    let (sx, sy) = (if x < x1 { 1 } else { -1 }, if y < y1 { 1 } else { -1 });
    let mut err = dx + dy;
    let mut out = Vec::new();
    loop {
        out.push((x as u32, y as u32));
        if (x, y) == (x1, y1) {
            return out;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// The direction sphere: a flat middle and three rings of eight facets.
/// Returns the facet's normal when one is clicked.
fn sphere_widget(ui: &mut Ui) -> Option<[f32; 3]> {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(120.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let (centre, radius) = (rect.center(), rect.width() / 2.0 - 2.0);
    painter.circle_filled(centre, radius, Color32::from_gray(32));
    let line = Stroke::new(1.0, Color32::from_gray(110));
    for ring in [0.25, 0.5, 0.75, 1.0] {
        painter.circle_stroke(centre, radius * ring, line);
    }
    for i in 0..8 {
        let a = (i as f32 * 45.0 + 22.5).to_radians();
        let dir = Vec2::new(a.cos(), -a.sin());
        painter.line_segment([centre + dir * radius * 0.25, centre + dir * radius], line);
    }
    let to_unit = |p: Pos2| ((p.x - centre.x) / radius, -(p.y - centre.y) / radius);
    if let Some(p) = response.hover_pos() {
        let (x, y) = to_unit(p);
        if let Some(n) = sphere_facet(x, y) {
            let at = centre
                + Vec2::new(n[0], -n[1])
                    * radius
                    * (n[0].hypot(n[1]).asin() / std::f32::consts::FRAC_PI_2).min(1.0);
            painter.circle_filled(at, 4.0, Color32::from_rgb(255, 200, 80));
        }
    }
    if response.clicked()
        && let Some(p) = response.interact_pointer_pos()
    {
        let (x, y) = to_unit(p);
        return sphere_facet(x, y);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_map(w: u32, h: u32) -> Map {
        Map::new(RgbaImage {
            rgba: [128, 128, 255, 255].repeat((w * h) as usize),
            width: w,
            height: h,
        })
    }

    fn sources(albedo: &RgbaImage) -> WandSources<'_> {
        WandSources { albedo, normal: None, mask: None }
    }

    fn select(d: &mut ModifyDialog, a: (u32, u32), b: (u32, u32)) {
        d.state.sel.edit(|s| s.select_rect(a, b, SelectMode::Replace));
    }

    #[test]
    fn a_direction_points_only_the_selected_pixels_and_undo_steps_back() {
        let albedo = RgbaImage { rgba: [9, 9, 9, 255].repeat(16), width: 4, height: 4 };
        let mut d = ModifyDialog::new(MapKind::Normal, flat_map(4, 4), 4, 4);
        select(&mut d, (0, 0), (1, 1));
        d.edits.track(&d.state, false);
        d.do_act(Act::Direction(Direction::Right), &sources(&albedo));
        d.edits.track(&d.state, false);
        let at = |d: &ModifyDialog, x: usize, y: usize| d.state.image.value.rgba[(y * 4 + x) * 4];
        assert!(at(&d, 0, 0) > 200, "right-facing: red is up");
        assert_eq!(at(&d, 3, 3), 128, "unselected pixels stay flat");
        assert_eq!(d.readout, "Right");
        assert!(d.undo());
        assert_eq!(at(&d, 0, 0), 128);
        assert!(d.redo());
        assert!(at(&d, 0, 0) > 200);
    }

    #[test]
    fn angle_strength_sets_how_far_a_direction_tilts() {
        let albedo = RgbaImage { rgba: [9, 9, 9, 255].repeat(4), width: 2, height: 2 };
        let tilt = |angle: f32| {
            let mut d = ModifyDialog::new(MapKind::Normal, flat_map(2, 2), 2, 2);
            d.angle = angle;
            select(&mut d, (0, 0), (0, 0));
            d.do_act(Act::Direction(Direction::Up), &sources(&albedo));
            d.state.image.value.rgba[1]
        };
        assert!(tilt(60.0) > tilt(20.0), "green is up");
        assert_eq!(tilt(0.0), 128, "zero degrees is flat");
    }

    #[test]
    fn normal_strength_scales_the_whole_map_when_shown() {
        let albedo = RgbaImage { rgba: [9, 9, 9, 255].repeat(4), width: 2, height: 2 };
        let mut d = ModifyDialog::new(MapKind::Normal, flat_map(2, 2), 2, 2);
        select(&mut d, (0, 0), (1, 1));
        d.do_act(Act::Direction(Direction::Right), &sources(&albedo));
        let full = d.shown().value.rgba[0];
        d.state.strength = 50;
        let half = d.shown().value.rgba[0];
        assert!(half < full && half > 128, "{half} between flat and {full}");
        d.state.strength = 0;
        assert_eq!(d.shown().value.rgba[0], 128);
        d.state.strength = 100;
        assert_eq!(d.shown().value.rgba[0], full);
    }

    #[test]
    fn brightness_adjusts_the_selection_relative_or_fixed() {
        let albedo = RgbaImage { rgba: [9, 9, 9, 255].repeat(4), width: 2, height: 2 };
        let mask =
            Map::new(RgbaImage { rgba: [200, 200, 200, 255].repeat(4), width: 2, height: 2 });
        let mut d = ModifyDialog::new(MapKind::LightMask, mask, 2, 2);
        select(&mut d, (0, 0), (0, 0));
        d.brightness = 50.0;
        d.do_act(Act::Brightness, &sources(&albedo));
        assert_eq!(d.state.image.value.rgba[0], 100);
        assert_eq!(d.state.image.value.rgba[4], 200, "unselected");
        d.brightness_mode = BrightnessMode::Fixed;
        d.brightness = 25.0;
        d.do_act(Act::Brightness, &sources(&albedo));
        assert_eq!(d.state.image.value.rgba[0], 64);
        // Normal Strength means nothing for a mask.
        d.state.strength = 10;
        assert_eq!(d.shown().value.rgba[0], 64);
    }

    #[test]
    fn selection_buttons_clear_and_fill() {
        let albedo = RgbaImage { rgba: [9, 9, 9, 255].repeat(25), width: 5, height: 5 };
        let mut d = ModifyDialog::new(MapKind::Normal, flat_map(5, 5), 5, 5);
        select(&mut d, (0, 0), (4, 4));
        d.state.sel.edit(|s| s.select_pixel(2, 2, SelectMode::Subtract));
        assert_eq!(d.selected(), 24);
        d.do_act(Act::FillHoles, &sources(&albedo));
        assert_eq!(d.selected(), 25);
        d.do_act(Act::ClearSelection, &sources(&albedo));
        assert_eq!(d.selected(), 0);
    }

    #[test]
    fn a_line_includes_both_ends_and_leaves_no_gaps() {
        let l = line((0, 0), (5, 2));
        assert_eq!(l.first(), Some(&(0, 0)));
        assert_eq!(l.last(), Some(&(5, 2)));
        for pair in l.windows(2) {
            let (dx, dy) = (pair[1].0.abs_diff(pair[0].0), pair[1].1.abs_diff(pair[0].1));
            assert!(dx <= 1 && dy <= 1 && (dx, dy) != (0, 0), "{pair:?}");
        }
        assert_eq!(line((3, 3), (3, 3)), [(3, 3)]);
        assert_eq!(line((4, 0), (1, 0)), [(4, 0), (3, 0), (2, 0), (1, 0)]);
    }

    #[test]
    fn the_canvas_maps_screen_points_to_pixels() {
        let map = CanvasMap {
            image_rect: Rect::from_min_size(Pos2::new(100.0, 50.0), Vec2::new(40.0, 20.0)),
            zoom: 2.0,
            width: 20,
            height: 10,
        };
        assert_eq!(map.pixel(Pos2::new(101.0, 51.0)), Some((0, 0)));
        assert_eq!(map.pixel(Pos2::new(139.0, 69.0)), Some((19, 9)));
        assert_eq!(map.pixel(Pos2::new(99.0, 60.0)), None);
        assert_eq!(map.pixel(Pos2::new(140.5, 60.0)), None);
        assert_eq!(map.pixel_clamped(Pos2::new(0.0, 0.0)), (0, 0));
        assert_eq!(map.pixel_clamped(Pos2::new(900.0, 900.0)), (19, 9));
    }
}
