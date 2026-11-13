//! Drawing a scene: sprites, images and tilemaps back to front, from the
//! first camera. The editor's Play and edit views and the exported game all draw
//! through here, so a game looks the same everywhere.

use std::collections::HashMap;

use reign::assets::{AssetDb, Guid, Handle, SpriteSheet, Texture};
use reign::render::{Color, Renderer, TextureId, VIEW_HEIGHT, VIEW_WIDTH};
use reign::scene::{ObjectId, Scene, Transform};

use crate::components::{Animator, Camera, Image, SpriteRenderer, Text, TilemapRenderer};
use crate::font::glyph_rect;
use crate::physics::TILE;

/// The colour behind everything.
pub const CLEAR: Color = Color::rgb8(24, 24, 32);

/// The world pixel at the view's top-left, from the first camera.
pub fn camera_origin(scene: &Scene) -> (f32, f32) {
    scene
        .iter::<Camera>()
        .min_by_key(|(id, camera)| (std::cmp::Reverse(camera.priority), *id))
        .map(|(id, _)| {
            let p = scene.world_position(id);
            (p.x.to_f32() - VIEW_WIDTH as f32 / 2.0, p.y.to_f32() - VIEW_HEIGHT as f32 / 2.0)
        })
        .unwrap_or((0.0, 0.0))
}

/// What a [`SpriteRenderer`] draws: the sheet, the rectangle of its texture
/// (x, y, w, h in texels) and the pivot in that rectangle.
pub struct Frame {
    pub sheet: std::sync::Arc<SpriteSheet>,
    pub rect: [u32; 4],
    pub pivot: [i32; 2],
}

/// The sprite an object shows right now: the frame its [`Animator`] is on,
/// else the sprite its renderer names. An animation's frames come from
/// the clip's own sheet: cell `n` of the clip's row in a sheet with a grid,
/// else the sheet's `n`th sprite.
pub fn current_frame(
    db: &AssetDb,
    sr: &SpriteRenderer,
    animator: Option<&Animator>,
) -> Option<Frame> {
    if let Some(animator) = animator
        && let Ok(clip) = db.load(animator.clip)
        && let Some(frame) = clip.frames.get(animator.frame(&clip) as usize)
    {
        let handle = if clip.sheet.is_none() { sr.sheet } else { clip.sheet };
        if let Ok(sheet) = db.load(handle) {
            let found = match sheet.grid {
                Some(grid) if frame.cell < grid.columns && clip.row < grid.rows => {
                    Some((grid.cell_rect(frame.cell, clip.row), sheet.cell_pivot))
                }
                Some(_) => None,
                None => sheet.sprites.get(frame.cell as usize).map(|s| (s.rect, s.pivot)),
            };
            if let Some((rect, pivot)) = found {
                return Some(Frame { sheet, rect, pivot });
            }
        }
    }
    let sheet = db.load(sr.sheet).ok()?;
    let sprite =
        if sr.sprite.is_empty() { sheet.sprites.first() } else { sheet.sprite(&sr.sprite) }?;
    let (rect, pivot) = (sprite.rect, sprite.pivot);
    Some(Frame { sheet, rect, pivot })
}

/// Textures uploaded to the renderer, kept across frames and replaced when
/// their asset reloads.
#[derive(Default)]
pub struct Sprites {
    /// By asset, with the version that was uploaded.
    textures: HashMap<Guid, (u32, TextureId)>,
}

impl Sprites {
    fn texture(
        &mut self,
        r: &mut Renderer,
        db: &AssetDb,
        handle: Handle<Texture>,
    ) -> Option<(TextureId, u8)> {
        let guid = handle.guid;
        let version = db.version(guid);
        let loaded = match db.load(handle) {
            Ok(t) => t,
            Err(e) => {
                log::warn!("texture {guid}: {e}");
                return None;
            }
        };
        let img = &loaded.image;
        let id = match self.textures.get(&guid) {
            Some(&(v, id)) if v == version => id,
            Some(&(_, id)) => {
                r.replace_texture(id, &img.rgba, img.width, img.height);
                self.textures.insert(guid, (version, id));
                id
            }
            None => {
                let id = r.load_texture(&img.rgba, img.width, img.height);
                self.textures.insert(guid, (version, id));
                id
            }
        };
        Some((id, loaded.density.max(1)))
    }

    /// Queues a tilemap's visible layers, first layer at the back: only
    /// the cells inside `view` (x, y, w, h in world pixels).
    fn draw_tilemap(
        &mut self,
        r: &mut Renderer,
        db: &AssetDb,
        tm: &TilemapRenderer,
        (x, y): (i32, i32),
        view: [f32; 4],
    ) {
        let Ok(map) = db.load(tm.tilemap) else { return };
        let Ok(tileset) = db.load(map.tileset) else { return };
        let Some((tex, _)) = self.texture(r, db, tileset.texture) else { return };
        let per = tileset.tile_size.max(1);
        let columns = tileset.columns.max(1);
        let tile = TILE as f32;
        // The cells in view, clamped to the map below.
        let first = |v: f32, o: i32| (((v - o as f32) / tile).floor() as i64).max(0) as usize;
        let last = |v: f32, o: i32| ((v - o as f32) / tile).ceil().max(0.0) as usize;
        let (tx0, ty0) = (first(view[0], x), first(view[1], y));
        let (tx1, ty1) = (last(view[0] + view[2], x), last(view[1] + view[3], y));
        let c = tm.color;
        let tint = Color::rgba8(c.r, c.g, c.b, c.a);
        for layer in map.layers.iter().filter(|l| l.visible) {
            for ty in ty0..ty1.min(layer.rows.len()) {
                for tx in tx0..tx1 {
                    let Some(t) = layer.get(tx, ty) else { continue };
                    if tileset.count > 0 && t >= tileset.count {
                        continue;
                    }
                    let src =
                        [(t % columns) * per, (t / columns) * per, per, per].map(|v| v as f32);
                    let (wx, wy) = (x + tx as i32 * TILE, y + ty as i32 * TILE);
                    r.sprite(tex, src, wx as f32, wy as f32, tile, tile, false, tint);
                }
            }
        }
    }

    /// Queues a [`Text`], one glyph at a time, from `(x, y)`: the top of its
    /// first line, at the side its alignment says.
    fn draw_text(&mut self, r: &mut Renderer, db: &AssetDb, text: &Text, (x, y): (f32, f32)) {
        let Ok(font) = db.load(text.font) else { return };
        let Some((tex, _)) = self.texture(r, db, font.texture) else { return };
        let scale = f32::from(text.scale.max(1));
        let (w, h) = (font.cell[0] as f32 * scale, font.cell[1] as f32 * scale);
        let c = text.color;
        let tint = Color::rgba8(c.r, c.g, c.b, c.a);
        for (row, line) in text.text.split('\n').enumerate() {
            let width = line.chars().count() as i32 * w as i32;
            let left = x - text.align_offset(width) as f32;
            for (column, ch) in line.chars().enumerate() {
                let Some(src) = glyph_rect(&font, ch) else { continue };
                if ch == ' ' {
                    continue;
                }
                let (gx, gy) = (left + column as f32 * w, y + row as f32 * h);
                r.sprite(tex, src.map(|v| v as f32), gx, gy, w, h, false, tint);
            }
        }
    }

    /// Queues every sprite, image, tilemap and text, back to front. `view` is
    /// the world rectangle on screen (x, y, w, h): tilemaps draw only the
    /// cells inside it.
    pub fn draw_objects(&mut self, r: &mut Renderer, scene: &Scene, db: &AssetDb, view: [f32; 4]) {
        let mut items: Vec<(i32, ObjectId)> = Vec::new();
        for (id, _) in scene.iter::<TilemapRenderer>() {
            items.push((scene.transform(id).map_or(0, |t| t.z), id));
        }
        for (id, _) in scene.iter::<SpriteRenderer>() {
            items.push((scene.transform(id).map_or(0, |t| t.z), id));
        }
        for (id, _) in scene.iter::<Image>() {
            items.push((scene.transform(id).map_or(0, |t| t.z), id));
        }
        for (id, _) in scene.iter::<Text>() {
            items.push((scene.transform(id).map_or(0, |t| t.z), id));
        }
        items.sort();
        items.dedup();
        for (_, id) in items {
            let pos = scene.world_position(id);
            if let Some(tm) = scene.get::<TilemapRenderer>(id) {
                let at = (pos.x.round_px(), pos.y.round_px());
                self.draw_tilemap(r, db, tm, at, view);
            }
            let (x, y) = (pos.x.to_f32(), pos.y.to_f32());
            let t: Transform = scene.transform(id).copied().unwrap_or_default();
            if let Some(image) = scene.get::<Image>(id)
                && let Some((tex, d)) = self.texture(r, db, image.texture)
            {
                let (w, h) = r.texture_size(tex).unwrap_or((0, 0));
                let (w, h) = (w as f32, h as f32);
                let d = d as f32;
                r.sprite(tex, [0.0, 0.0, w, h], x, y, w / d, h / d, t.flip_x, Color::WHITE);
            }
            if let Some(sr) = scene.get::<SpriteRenderer>(id)
                && let Some(frame) = current_frame(db, sr, scene.get::<Animator>(id))
                && let Some((tex, _)) = self.texture(r, db, frame.sheet.texture)
                && frame.rect[2] > 0
                && frame.rect[3] > 0
            {
                let d = frame.sheet.density.max(1) as f32;
                let [sx, sy, sw, sh] = frame.rect.map(|v| v as f32);
                // Drawn size in world pixels, and how much that stretches it.
                let w = if sr.size[0] > 0 { sr.size[0] as f32 } else { sw / d };
                let h = if sr.size[1] > 0 { sr.size[1] as f32 } else { sh / d };
                let (kx, ky) = (w / sw, h / sh);
                let (px, py) = (frame.pivot[0] as f32 * kx, frame.pivot[1] as f32 * ky);
                let left = if t.flip_x { x - (w - px) } else { x - px };
                let c = sr.color;
                let tint = Color::rgba8(c.r, c.g, c.b, c.a);
                r.sprite(tex, [sx, sy, sw, sh], left, y - py, w, h, t.flip_x, tint);
            }
            if let Some(text) = scene.get::<Text>(id) {
                // Screen text counts from the view's corner, wherever the
                // camera is.
                let at = if text.screen { (view[0] + x, view[1] + y) } else { (x, y) };
                self.draw_text(r, db, text, (at.0.round(), at.1.round()));
            }
        }
    }

    /// The game's frame, as Play shows it: everything from the first
    /// camera, presented. Returns the view's origin in world pixels.
    pub fn draw_game(
        &mut self,
        r: &mut Renderer,
        scene: &Scene,
        db: Option<&AssetDb>,
    ) -> (f32, f32) {
        let origin = camera_origin(scene);
        r.set_offset(-origin.0, -origin.1);
        if let Some(db) = db {
            let view = [origin.0, origin.1, VIEW_WIDTH as f32, VIEW_HEIGHT as f32];
            self.draw_objects(r, scene, db, view);
        }
        r.present(CLEAR);
        origin
    }
}
