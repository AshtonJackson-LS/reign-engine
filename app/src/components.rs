//! The built-in components every project has.

use reign::assets::{Handle, SpriteSheet, Texture, Tilemap};
use reign::scene::{Bounds, Component, ComponentRegistry, ObjectId, Rgba8, Scene};
use serde::{Deserialize, Serialize};

use crate::{DB, physics};

/// Where the Game view looks: the first active camera's position is the
/// centre of the view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
pub struct Camera {}

/// Draws one sprite of a sprite sheet at the object's position (on the
/// sprite's pivot), tinted by `color`. An empty `sprite` draws the sheet's
/// first sprite. `size` stretches it to that many pixels; 0 keeps the
/// sprite's own width or height.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
#[component(bounds = "SpriteRenderer::bounds_hint")]
pub struct SpriteRenderer {
    pub sheet: Handle<SpriteSheet>,
    pub sprite: String,
    pub color: Rgba8,
    pub size: [u16; 2],
}

impl Default for SpriteRenderer {
    fn default() -> Self {
        SpriteRenderer {
            sheet: Handle::NONE,
            sprite: String::new(),
            color: Rgba8::WHITE,
            size: [0, 0],
        }
    }
}

impl SpriteRenderer {
    fn bounds_hint(&self) -> Option<Bounds> {
        let [w, h] = self.size.map(i32::from);
        (w > 0 && h > 0).then(|| Bounds::new(-w / 2, -h / 2, w, h))
    }
}

/// Draws a whole texture with its top-left at the object's position.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
pub struct Image {
    pub texture: Handle<Texture>,
}

/// Draws a tilemap asset's visible tile layers, first layer at the back,
/// with the map's top-left corner at the object's position: one tileset
/// tile per 16-pixel cell, tinted by `color`. Add a
/// [`TilemapCollider`](physics::TilemapCollider) to make its collision
/// layer solid.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
#[component(bounds = "TilemapRenderer::bounds_hint")]
pub struct TilemapRenderer {
    /// The `.tilemap.ron` to draw. Empty draws nothing: pick one, or make
    /// one with GameObject > 2D Object > Tilemap. Paint it in the Tilemap
    /// tab (double-click it in the Project panel).
    pub tilemap: Handle<Tilemap>,
    pub color: Rgba8,
}

impl Default for TilemapRenderer {
    fn default() -> Self {
        TilemapRenderer { tilemap: Handle::NONE, color: Rgba8::WHITE }
    }
}

impl TilemapRenderer {
    /// The whole map, from the object's position.
    fn bounds_hint(&self) -> Option<Bounds> {
        let map = DB.get()?.load(self.tilemap).ok()?;
        let (w, h) = (map.width() as i32 * physics::TILE, map.height() as i32 * physics::TILE);
        (w > 0 && h > 0).then(|| Bounds::new(0, 0, w, h))
    }
}

/// The built-in components, before a project's own.
pub fn register_builtins(r: &mut ComponentRegistry) {
    r.register::<Camera>()
        .register::<SpriteRenderer>()
        .register::<Image>()
        .register::<TilemapRenderer>()
        .register::<physics::Rigidbody>()
        .register::<physics::BoxCollider>()
        .register::<physics::TilemapCollider>();
}

/// How big an object's sprite draws, in world pixels: what an unsized
/// collider fits.
pub fn sprite_size(scene: &Scene, id: ObjectId) -> Option<[i32; 2]> {
    let sr = scene.get::<SpriteRenderer>(id)?;
    let [w, h] = sr.size.map(i32::from);
    if w > 0 && h > 0 {
        return Some([w, h]);
    }
    let sheet = DB.get()?.load(sr.sheet).ok()?;
    let sprite =
        if sr.sprite.is_empty() { sheet.sprites.first() } else { sheet.sprite(&sr.sprite) }?;
    let d = sheet.density.max(1) as i32;
    let natural = [sprite.rect[2] as i32 / d, sprite.rect[3] as i32 / d];
    Some([if w > 0 { w } else { natural[0] }, if h > 0 { h } else { natural[1] }])
}
