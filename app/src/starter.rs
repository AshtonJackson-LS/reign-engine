//! Assets every project starts with, written into `assets/Reign/` when a
//! project opens without them (never over files that are already there).
//!
//! - `input.ron` (at the assets root): the default input actions.
//! - `Shapes.png` / `Shapes.sheet.ron`: white shape sprites (Square, Circle,
//!   Triangle, Diamond, Hexagon, Capsule, Rounded Square) with centred
//!   pivots, drawn with hard pixel edges so they tint and stretch cleanly.
//!
//! The GUIDs are fixed, so scenes and prefabs that use the shapes work in
//! any project.

use std::path::Path;

use reign::assets::{Guid, Handle, Importer, Meta, Sprite, SpriteSheet, TextureSettings, to_ron};

pub const FOLDER: &str = "Reign";
pub const SHAPES_TEXTURE: Guid = Guid(0x5245_4947_4e00_5348_4150_4553_0000_0001);
pub const SHAPES_SHEET: Guid = Guid(0x5245_4947_4e00_5348_4150_4553_0000_0002);

/// A built-in shape: its menu name, sprite name and size in pixels.
pub struct Shape {
    pub name: &'static str,
    pub sprite: &'static str,
    pub size: [u32; 2],
    inside: fn(f32, f32) -> bool,
}

pub const SHAPES: &[Shape] = &[
    Shape { name: "Square", sprite: "square", size: [32, 32], inside: |_, _| true },
    Shape {
        name: "Circle",
        sprite: "circle",
        size: [32, 32],
        inside: |x, y| {
            let (dx, dy) = (x - 16.0, y - 16.0);
            dx * dx + dy * dy <= 16.0 * 16.0
        },
    },
    Shape {
        name: "Triangle",
        sprite: "triangle",
        size: [32, 32],
        inside: |x, y| (x - 16.0).abs() <= y / 2.0,
    },
    Shape {
        name: "Diamond",
        sprite: "diamond",
        size: [32, 16],
        inside: |x, y| (x - 16.0).abs() / 16.0 + (y - 8.0).abs() / 8.0 <= 1.0,
    },
    Shape {
        name: "Hexagon",
        sprite: "hexagon",
        size: [32, 28],
        inside: |x, y| {
            let (dx, dy) = ((x - 16.0).abs(), (y - 14.0).abs());
            dy <= 14.0 && dx <= 16.0 - dy * 8.0 / 14.0
        },
    },
    Shape {
        name: "Capsule",
        sprite: "capsule",
        size: [16, 32],
        inside: |x, y| {
            let cy = y.clamp(8.0, 24.0);
            let (dx, dy) = (x - 8.0, y - cy);
            dx * dx + dy * dy <= 8.0 * 8.0
        },
    },
    Shape {
        name: "Rounded Square",
        sprite: "rounded_square",
        size: [32, 32],
        inside: |x, y| {
            let r = 7.0;
            let cx = x.clamp(r, 32.0 - r);
            let cy = y.clamp(r, 32.0 - r);
            let (dx, dy) = (x - cx, y - cy);
            dx * dx + dy * dy <= r * r
        },
    },
];

pub fn shapes_sheet() -> Handle<SpriteSheet> {
    Handle::new(SHAPES_SHEET)
}

pub fn shape(name: &str) -> Option<&'static Shape> {
    SHAPES.iter().find(|s| s.name == name)
}

/// Writes whatever starter assets `assets` is missing.
pub fn install(assets: &Path) -> std::io::Result<()> {
    let input = assets.join(crate::input::INPUT_FILE);
    if !input.exists() {
        std::fs::create_dir_all(assets)?;
        std::fs::write(&input, crate::input::InputMap::default().to_text())?;
    }
    let dir = assets.join(FOLDER);
    std::fs::create_dir_all(&dir)?;
    let png = dir.join("Shapes.png");
    let sheet = dir.join("Shapes.sheet.ron");
    if png.exists() || sheet.exists() {
        return Ok(());
    }

    // One row of shapes, left to right, each in its own rect, with a gap
    // so stretched sprites never pick up their neighbours' edges.
    const GAP: u32 = 2;
    let width: u32 = SHAPES.iter().map(|s| s.size[0] + GAP).sum::<u32>() - GAP;
    let height: u32 = SHAPES.iter().map(|s| s.size[1]).max().unwrap_or(1);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    let mut sprites = Vec::new();
    let mut x0 = 0;
    for shape in SHAPES {
        let [w, h] = shape.size;
        for y in 0..h {
            for x in 0..w {
                if (shape.inside)(x as f32 + 0.5, y as f32 + 0.5) {
                    let i = (((y * width) + x0 + x) * 4) as usize;
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        sprites.push(Sprite {
            name: shape.sprite.to_owned(),
            rect: [x0, 0, w, h],
            pivot: [w as i32 / 2, h as i32 / 2],
        });
        x0 += w + GAP;
    }

    write_png(&png, width, height, &rgba)?;
    let meta = |guid, importer| Meta { guid, importer }.to_ron();
    std::fs::write(
        dir.join("Shapes.png.meta"),
        meta(SHAPES_TEXTURE, Importer::Texture(TextureSettings::default())),
    )?;
    let sheet_asset = SpriteSheet {
        texture: Handle::new(SHAPES_TEXTURE),
        density: 1,
        sprites,
        ..SpriteSheet::default()
    };
    std::fs::write(&sheet, to_ron(&sheet_asset))?;
    std::fs::write(dir.join("Shapes.sheet.ron.meta"), meta(SHAPES_SHEET, Importer::SpriteSheet))?;
    log::info!("added the built-in shapes to {}", dir.display());
    Ok(())
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer.write_image_data(rgba).map_err(std::io::Error::other)
}
