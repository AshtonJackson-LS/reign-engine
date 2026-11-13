//! New tilemaps for GameObject > 2D Object > Tilemap.
//!
//! The Tilemap painter edits a map but can't make one or change its size,
//! so the menu item writes a small starter map into `assets/tilemaps/`: a
//! floor, two low walls and a one-way plank, in collision and in a visual
//! "Ground" layer, ready to paint over. Its tiles come from a built-in
//! tileset, `assets/Reign/Tiles.tileset.ron` (written with its texture the
//! first time a tilemap is made, then shared), so the painter's palette has
//! tiles from the start.

use reign::assets::{AssetDb, AssetError, Guid, Handle, TileLayer, Tilemap, Tileset, to_ron};

pub const TILES_TEXTURE: &str = "Reign/Tiles.png";
pub const TILES_TILESET: &str = "Reign/Tiles.tileset.ron";
pub const FOLDER: &str = "tilemaps";

/// The built-in tiles, left to right.
pub const GRASS: u32 = 0;
pub const DIRT: u32 = 1;
pub const BRICK: u32 = 2;
pub const STONE: u32 = 3;
pub const PLANK: u32 = 4;
pub const BACK_WALL: u32 = 5;
const TILE_COUNT: u32 = 6;
const TEXELS: u32 = 16;

/// The starter map's size in cells: 640 x 192 pixels.
pub const STARTER_SIZE: (usize, usize) = (40, 12);

/// Writes a new starter tilemap (and the built-in tileset, if the project
/// doesn't have it yet) and returns it.
pub fn new_tilemap(db: &AssetDb) -> Result<Handle<Tilemap>, AssetError> {
    let tileset = builtin_tileset(db)?;
    let map = starter_map(Handle::new(tileset));
    let path = (1..1000)
        .map(|i| match i {
            1 => format!("{FOLDER}/Tilemap.tilemap.ron"),
            _ => format!("{FOLDER}/Tilemap {i}.tilemap.ron"),
        })
        .find(|p| db.guid(p).is_none() && db.root().is_none_or(|r| !r.join(p).exists()))
        .ok_or_else(|| AssetError("no free tilemap name".into()))?;
    db.create(&path, to_ron(&map).as_bytes()).map(Handle::new)
}

/// The built-in tileset's GUID, writing it and its texture if missing.
fn builtin_tileset(db: &AssetDb) -> Result<Guid, AssetError> {
    if let Some(guid) = db.guid(TILES_TILESET) {
        return Ok(guid);
    }
    let texture = match db.guid(TILES_TEXTURE) {
        Some(guid) => guid,
        None => db.create(TILES_TEXTURE, &tiles_png()?)?,
    };
    let tileset = Tileset {
        texture: Handle::new(texture),
        tile_size: TEXELS,
        columns: TILE_COUNT,
        count: TILE_COUNT,
    };
    db.create(TILES_TILESET, to_ron(&tileset).as_bytes())
}

/// A floor along the bottom, a low wall at each end and a one-way plank
/// in the middle, the same in collision and in the "Ground" layer.
pub fn starter_map(tileset: Handle<Tileset>) -> Tilemap {
    let (w, h) = STARTER_SIZE;
    let mut collision = vec![".".repeat(w); h];
    let mut ground = TileLayer::empty("Ground", w, h);
    let mut put = |x: usize, y: usize, glyph: char, tile: u32| {
        Tilemap::set_glyph(&mut collision, x, y, glyph);
        ground.set(x, y, Some(tile));
    };
    for x in 0..w {
        put(x, h - 1, '#', GRASS);
    }
    for y in h - 3..h - 1 {
        put(0, y, '#', BRICK);
        put(w - 1, y, '#', BRICK);
    }
    for x in w / 2 - 5..w / 2 + 5 {
        put(x, h - 5, '-', PLANK);
    }
    Tilemap { tileset, collision, markers: vec![".".repeat(w); h], layers: vec![ground] }
}

/// The built-in tiles' texture: one row of 16x16 tiles.
fn tiles_png() -> Result<Vec<u8>, AssetError> {
    let (width, height) = (TEXELS * TILE_COUNT, TEXELS);
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for tile in 0..TILE_COUNT {
        for y in 0..TEXELS {
            for x in 0..TEXELS {
                let i = ((y * width + tile * TEXELS + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&texel(tile, x, y));
            }
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let err = |e: png::EncodingError| AssetError(e.to_string());
    encoder.write_header().map_err(err)?.write_image_data(&rgba).map_err(err)?;
    Ok(out)
}

/// A little fixed noise, so the tiles aren't flat.
fn noise(tile: u32, x: u32, y: u32) -> u32 {
    let mut h =
        tile.wrapping_mul(0x9e37_79b9) ^ x.wrapping_mul(0x85eb_ca6b) ^ y.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 12)
}

fn shade([r, g, b]: [u8; 3], by: i32) -> [u8; 4] {
    let s = |c: u8| (c as i32 + by).clamp(0, 255) as u8;
    [s(r), s(g), s(b), 255]
}

fn texel(tile: u32, x: u32, y: u32) -> [u8; 4] {
    let n = noise(tile, x, y);
    let speck = if n.is_multiple_of(7) {
        -18
    } else if n.is_multiple_of(11) {
        14
    } else {
        0
    };
    let dirt = [120, 82, 52];
    // Bricks: two rows of 8x8 bricks a row, offset by half on odd rows.
    let brick = |base: [u8; 3]| {
        let row = y / 8;
        let bx = (x + row * 4) % 8;
        if y % 8 == 7 || bx == 7 { shade(base, -55) } else { shade(base, speck) }
    };
    match tile {
        GRASS => {
            let edge = 4 + (noise(tile, x, 0) % 3);
            if y < edge {
                shade([86, 168, 72], if y == 0 { 30 } else { speck })
            } else {
                shade(dirt, speck)
            }
        }
        DIRT => shade(dirt, speck),
        BRICK => brick([150, 72, 58]),
        STONE => {
            let rim = x == 0 || y == 0 || x == TEXELS - 1 || y == TEXELS - 1;
            let light = x <= 1 || y <= 1;
            shade(
                [128, 128, 140],
                if rim && !light {
                    -40
                } else if light {
                    28
                } else {
                    speck
                },
            )
        }
        PLANK => match y {
            0 => shade([196, 146, 90], 0),
            1..=4 if x % 8 == 7 => shade([150, 102, 60], -30),
            1..=4 => shade([170, 120, 72], speck),
            5 => shade([110, 74, 44], 0),
            _ => [0, 0, 0, 0],
        },
        BACK_WALL => brick([70, 62, 84]),
        _ => [0, 0, 0, 0],
    }
}
