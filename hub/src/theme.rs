//! The hub's look: fonts, colours, and a few drawing helpers.
//!
//! Bebas Neue for display type and labels, Inter with tight tracking for
//! everything else, a near-black ground with hairline rules, and one red.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, ColorImage, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Mesh,
    Painter, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions, Vec2,
    text::{LayoutJob, TextFormat},
};

// ------------------------------------------------------------------- colours

pub const INK: Color32 = Color32::from_rgb(11, 9, 10);
pub const BG: Color32 = Color32::from_rgb(15, 13, 14);
pub const SURFACE: Color32 = Color32::from_rgb(23, 20, 21);
pub const SURFACE_2: Color32 = Color32::from_rgb(32, 28, 29);
pub const HAIRLINE: Color32 = Color32::from_rgb(43, 38, 39);
pub const TEXT: Color32 = Color32::from_rgb(244, 240, 240);
pub const MUTED: Color32 = Color32::from_rgb(146, 137, 139);
pub const FAINT: Color32 = Color32::from_rgb(96, 89, 91);
/// The primary colour.
pub const RED: Color32 = Color32::from_rgb(228, 46, 52);
pub const RED_HOT: Color32 = Color32::from_rgb(247, 74, 76);
pub const RED_DEEP: Color32 = Color32::from_rgb(92, 14, 22);
pub const GOOD: Color32 = Color32::from_rgb(120, 214, 150);

// --------------------------------------------------------------------- fonts

#[derive(Clone, Copy)]
pub enum Face {
    Display,
    Regular,
    Medium,
    SemiBold,
    Bold,
}

impl Face {
    fn family(self) -> FontFamily {
        match self {
            Face::Regular => FontFamily::Proportional,
            Face::Display => FontFamily::Name("bebas".into()),
            Face::Medium => FontFamily::Name("inter-medium".into()),
            Face::SemiBold => FontFamily::Name("inter-semibold".into()),
            Face::Bold => FontFamily::Name("inter-bold".into()),
        }
    }

    pub fn id(self, size: f32) -> FontId {
        FontId::new(size, self.family())
    }
}

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    };
    add("bebas", include_bytes!("../assets/fonts/BebasNeue-Regular.ttf"));
    add("inter", include_bytes!("../assets/fonts/Inter-Regular.ttf"));
    add("inter-medium", include_bytes!("../assets/fonts/Inter-Medium.ttf"));
    add("inter-semibold", include_bytes!("../assets/fonts/Inter-SemiBold.ttf"));
    add("inter-bold", include_bytes!("../assets/fonts/Inter-Bold.ttf"));
    // egui's own fonts stay behind Inter for symbols Inter lacks.
    let fallback = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let family = |first: &str| {
        let mut list = vec![first.to_owned()];
        list.extend(fallback.iter().cloned());
        list
    };
    fonts.families.insert(FontFamily::Proportional, family("inter"));
    for name in ["bebas", "inter-medium", "inter-semibold", "inter-bold"] {
        fonts.families.insert(FontFamily::Name(name.into()), family(name));
    }
    ctx.set_fonts(fonts);

    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(14.0, 7.0);
        s.spacing.interact_size.y = 30.0;
        s.text_styles.insert(egui::TextStyle::Body, Face::Regular.id(14.0));
        s.text_styles.insert(egui::TextStyle::Button, Face::SemiBold.id(13.0));
        s.text_styles.insert(egui::TextStyle::Small, Face::Regular.id(11.5));
        s.text_styles.insert(egui::TextStyle::Heading, Face::Display.id(32.0));

        let v = &mut s.visuals;
        v.panel_fill = BG;
        v.window_fill = SURFACE;
        v.window_stroke = Stroke::new(1.0, HAIRLINE);
        v.window_corner_radius = CornerRadius::same(16);
        v.menu_corner_radius = CornerRadius::same(10);
        v.extreme_bg_color = INK;
        v.faint_bg_color = SURFACE;
        v.hyperlink_color = RED_HOT;
        v.selection.bg_fill = RED.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, TEXT);
        v.text_cursor.stroke = Stroke::new(2.0, RED);
        v.override_text_color = None;
        let radius = CornerRadius::same(8);
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, HAIRLINE);
        w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
        w.noninteractive.corner_radius = radius;
        w.inactive.weak_bg_fill = SURFACE_2;
        w.inactive.bg_fill = SURFACE_2;
        w.inactive.bg_stroke = Stroke::new(1.0, HAIRLINE);
        w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
        w.inactive.corner_radius = radius;
        w.hovered.weak_bg_fill = Color32::from_rgb(44, 38, 40);
        w.hovered.bg_fill = Color32::from_rgb(44, 38, 40);
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(78, 68, 70));
        w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
        w.hovered.corner_radius = radius;
        w.active.weak_bg_fill = Color32::from_rgb(56, 48, 50);
        w.active.bg_fill = Color32::from_rgb(56, 48, 50);
        w.active.bg_stroke = Stroke::new(1.0, RED);
        w.active.fg_stroke = Stroke::new(1.0, TEXT);
        w.active.corner_radius = radius;
        w.open = w.active;
    });
}

/// Text in one face, with tracking (negative tightens, as on big Inter).
pub fn text(
    s: impl Into<String>,
    face: Face,
    size: f32,
    color: Color32,
    tracking: f32,
) -> LayoutJob {
    LayoutJob::single_section(
        s.into(),
        TextFormat {
            font_id: face.id(size),
            color,
            extra_letter_spacing: tracking,
            ..Default::default()
        },
    )
}

/// A small uppercase label, e.g. `↳ RECENT`.
pub fn eyebrow(s: &str, color: Color32) -> LayoutJob {
    text(s.to_uppercase(), Face::SemiBold, 10.5, color, 1.2)
}

// ------------------------------------------------------------------ buttons

/// A filled red pill.
pub fn primary(label: &str) -> egui::Button<'static> {
    egui::Button::new(text(label, Face::SemiBold, 13.0, Color32::WHITE, -0.1))
        .fill(RED)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(200))
        .min_size(Vec2::new(0.0, 34.0))
}

/// An outlined pill.
pub fn secondary(label: &str) -> egui::Button<'static> {
    egui::Button::new(text(label, Face::SemiBold, 13.0, TEXT, -0.1))
        .fill(Color32::TRANSPARENT)
        .stroke(Stroke::new(1.0, Color32::from_rgb(70, 62, 64)))
        .corner_radius(CornerRadius::same(200))
        .min_size(Vec2::new(0.0, 34.0))
}

// ------------------------------------------------------------------ drawing

/// A rectangle with a colour at each corner: top-left, top-right,
/// bottom-right, bottom-left.
pub fn gradient(painter: &Painter, rect: Rect, [tl, tr, br, bl]: [Color32; 4]) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), tl);
    mesh.colored_vertex(rect.right_top(), tr);
    mesh.colored_vertex(rect.right_bottom(), br);
    mesh.colored_vertex(rect.left_bottom(), bl);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

/// Film grain, tiled over `rect`.
pub fn grain(painter: &Painter, rect: Rect, texture: &TextureHandle, strength: f32) {
    let size = 160.0;
    let uv = Rect::from_min_max(
        Pos2::new(rect.min.x / size, rect.min.y / size),
        Pos2::new(rect.max.x / size, rect.max.y / size),
    );
    painter.image(texture.id(), rect, uv, Color32::WHITE.gamma_multiply(strength));
}

/// A tile of soft monochrome noise for [`grain`].
pub fn grain_texture(ctx: &egui::Context) -> TextureHandle {
    let n = 160usize;
    let mut seed: u32 = 0x9E37_79B9;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    let mut rgba = Vec::with_capacity(n * n * 4);
    for _ in 0..n * n {
        let r = next();
        let light = r & 1 == 0;
        let a = ((r >> 8) % 40) as u8;
        let v = if light { 255 } else { 0 };
        rgba.extend_from_slice(&[v, v, v, a]);
    }
    let image = ColorImage::from_rgba_unmultiplied([n, n], &rgba);
    ctx.load_texture("grain", image, TextureOptions::NEAREST_REPEAT)
}

pub fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// How far `t` is through `[start, end]`, in 0..1.
pub fn span(t: f32, start: f32, end: f32) -> f32 {
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}
