//! Lit scenes drawn on the GPU through the same path as the editor and the
//! game (`view.rs` into the renderer), read back pixel by pixel. Skipped
//! where there is no GPU adapter.

use std::path::Path;

use reign::assets::{AssetDb, Guid, Handle, Importer, Meta, Sprite, SpriteSheet, TextureSettings};
use reign::core::{Fx, FxVec2};
use reign::render::wgpu::TextureFormat;
use reign::render::{Color, Renderer, VIEW_HEIGHT, VIEW_WIDTH};
use reign::scene::{Rgba8, Scene};

use crate::components::{AmbientLight, Light2D, SpriteRenderer};
use crate::view::Sprites;

const WALL_TEXTURE: Guid = Guid(0x7465_7374_0000_0000_0000_0000_0000_0001);
const WALL_NORMAL: Guid = Guid(0x7465_7374_0000_0000_0000_0000_0000_0002);
const WALL_MASK: Guid = Guid(0x7465_7374_0000_0000_0000_0000_0000_0003);
const WALL_SHEET: Guid = Guid(0x7465_7374_0000_0000_0000_0000_0000_0004);
const PLAIN_SHEET: Guid = Guid(0x7465_7374_0000_0000_0000_0000_0000_0005);
/// The wall's colour, in sRGB.
const GREY: u8 = 128;

fn write_png(path: &Path, size: u32, pixel: [u8; 4]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let rgba: Vec<u8> = (0..size * size).flat_map(|_| pixel).collect();
    encoder.write_header().unwrap().write_image_data(&rgba).unwrap();
}

fn write_meta(path: &Path, guid: Guid, importer: Importer) {
    let mut name = path.as_os_str().to_owned();
    name.push(".meta");
    std::fs::write(name, Meta { guid, importer }.to_ron()).unwrap();
}

/// A grey 32x32 wall texture with a flat normal map and a full light mask,
/// one sheet that uses the maps and one that doesn't.
fn assets(name: &str) -> (std::path::PathBuf, AssetDb) {
    let dir = std::env::temp_dir().join(format!("reign-lighting-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let texture = Importer::Texture(TextureSettings::default());
    for (file, guid, pixel) in [
        ("wall.png", WALL_TEXTURE, [GREY, GREY, GREY, 255]),
        ("wall_n.png", WALL_NORMAL, [128, 128, 255, 255]),
        ("wall_mask.png", WALL_MASK, [255, 255, 255, 255]),
    ] {
        write_png(&dir.join(file), 32, pixel);
        write_meta(&dir.join(file), guid, texture.clone());
    }
    let plain = SpriteSheet {
        texture: Handle::new(WALL_TEXTURE),
        density: 1,
        sprites: vec![Sprite { name: "wall".into(), rect: [0, 0, 32, 32], pivot: [0, 0] }],
        ..SpriteSheet::default()
    };
    let mapped = SpriteSheet {
        normal: Handle::new(WALL_NORMAL),
        light_mask: Handle::new(WALL_MASK),
        ..plain.clone()
    };
    for (file, guid, sheet) in
        [("wall.sheet.ron", WALL_SHEET, mapped), ("plain.sheet.ron", PLAIN_SHEET, plain)]
    {
        let text = ron::ser::to_string_pretty(&sheet, Default::default()).unwrap();
        std::fs::write(dir.join(file), text).unwrap();
        write_meta(&dir.join(file), guid, Importer::SpriteSheet);
    }
    let db = AssetDb::loose(&dir).expect("assets load");
    (dir, db)
}

/// The view filled by one wall sprite from `sheet`, with `ambient` if any.
fn scene(sheet: Guid, ambient: Option<AmbientLight>, lamp: Option<Light2D>) -> Scene {
    let mut scene = Scene::new();
    let wall = scene.spawn("Wall");
    scene.insert(
        wall,
        SpriteRenderer {
            sheet: Handle::new(sheet),
            sprite: "wall".into(),
            size: [VIEW_WIDTH as u16, VIEW_HEIGHT as u16],
            ..SpriteRenderer::default()
        },
    );
    if let Some(ambient) = ambient {
        let sky = scene.spawn("Ambient");
        scene.insert(sky, ambient);
    }
    if let Some(lamp) = lamp {
        let id = scene.spawn("Lamp");
        scene.transform_mut(id).unwrap().position = FxVec2::from_px(160, 180);
        scene.insert(id, lamp);
    }
    scene
}

struct Gpu {
    renderer: Renderer,
    sprites: Sprites,
}

impl Gpu {
    fn new() -> Option<Self> {
        let size = (VIEW_WIDTH, VIEW_HEIGHT);
        match pollster::block_on(Renderer::headless(size, TextureFormat::Rgba8UnormSrgb)) {
            Ok(renderer) => Some(Gpu { renderer, sprites: Sprites::default() }),
            Err(e) => {
                eprintln!("No GPU adapter available ({e}): skipping a lighting test");
                None
            }
        }
    }

    /// Draws `scene` as Play does, and returns the frame's sRGB pixels.
    fn frame(&mut self, scene: &Scene, db: &AssetDb) -> Vec<u8> {
        self.sprites.draw_game(&mut self.renderer, scene, Some(db), Color::rgb8(24, 24, 32));
        self.renderer.read_offscreen().expect("an offscreen frame")
    }
}

/// The red channel at `(x, y)` of a frame the size of the view.
fn red(frame: &[u8], x: u32, y: u32) -> u8 {
    frame[((y * VIEW_WIDTH + x) * 4) as usize]
}

/// sRGB byte to linear, and back.
fn linear(c: u8) -> f32 {
    Color::rgb8(c, 0, 0).to_linear()[0]
}

fn srgb(l: f32) -> f32 {
    let c = if l <= 0.003_130_8 { l * 12.92 } else { 1.055 * l.powf(1.0 / 2.4) - 0.055 };
    c * 255.0
}

/// Away from the lamp and inside the vignette's clear middle.
const FAR: (u32, u32) = (480, 180);

fn assert_near(got: u8, want: f32, what: &str) {
    assert!((f32::from(got) - want).abs() <= 2.0, "{what}: got {got}, want about {want:.0}");
}

#[test]
fn a_wall_without_an_ambient_light_shows_as_drawn() {
    let Some(mut gpu) = Gpu::new() else { return };
    let (dir, db) = assets("default");
    for sheet in [PLAIN_SHEET, WALL_SHEET] {
        let frame = gpu.frame(&scene(sheet, None, None), &db);
        assert_near(red(&frame, FAR.0, FAR.1), f32::from(GREY), "unlit wall at default light");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_dim_ambient_light_scales_the_wall_in_linear_light() {
    let Some(mut gpu) = Gpu::new() else { return };
    let (dir, db) = assets("ambient");
    let ambient = AmbientLight { color: Rgba8::WHITE, intensity: Fx::from_subpx(0, 128) };
    let want = srgb(linear(GREY) * 0.5);
    for sheet in [PLAIN_SHEET, WALL_SHEET] {
        let frame = gpu.frame(&scene(sheet, Some(ambient.clone()), None), &db);
        assert_near(red(&frame, FAR.0, FAR.1), want, "wall at half ambient");
    }
    // A tinted, dim ambient: each channel scaled by its own linear value.
    let tint = AmbientLight {
        color: Rgba8 { r: 120, g: 135, b: 210, a: 255 },
        intensity: Fx::from_subpx(0, 51),
    };
    let k = Fx::from_subpx(0, 51).to_f32();
    let frame = gpu.frame(&scene(WALL_SHEET, Some(tint), None), &db);
    let i = ((FAR.1 * VIEW_WIDTH + FAR.0) * 4) as usize;
    for (c, ch) in [(120, 0), (135, 1), (210, 2)] {
        assert_near(frame[i + ch], srgb(linear(GREY) * linear(c) * k), "tinted ambient");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_lamp_lights_a_mapped_wall_in_the_dark() {
    let Some(mut gpu) = Gpu::new() else { return };
    let (dir, db) = assets("lamp");
    let dark = AmbientLight { color: Rgba8::WHITE, intensity: Fx::ZERO };
    let lamp = Light2D { radius: Fx::from_px(120), ..Light2D::default() };
    for sheet in [PLAIN_SHEET, WALL_SHEET] {
        let frame = gpu.frame(&scene(sheet, Some(dark.clone()), Some(lamp.clone())), &db);
        assert!(red(&frame, FAR.0, FAR.1) <= 1, "nothing reaches far from the lamp");
        // Beside the lamp a flat, fully unmasked surface is lit.
        let near = red(&frame, 180, 180);
        assert!(near > 60, "the wall beside the lamp is lit: {near} (sheet {sheet:?})");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn frames_stay_lit_from_one_to_the_next() {
    // Textures and materials are kept across frames: the second and later
    // frames must look like the first.
    let Some(mut gpu) = Gpu::new() else { return };
    let (dir, db) = assets("frames");
    let lamp = Light2D::default();
    let scene = scene(WALL_SHEET, None, Some(lamp));
    let first = gpu.frame(&scene, &db);
    for _ in 0..3 {
        let next = gpu.frame(&scene, &db);
        assert_eq!(red(&next, FAR.0, FAR.1), red(&first, FAR.0, FAR.1));
        assert_eq!(red(&next, 170, 180), red(&first, 170, 180));
    }
    let _ = std::fs::remove_dir_all(dir);
}
