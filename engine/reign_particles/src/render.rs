//! Drawing particles with the game's [`Renderer`].

use reign_assets::particles::BlendMode;
use reign_assets::{AssetDb, Guid};
use reign_render::{Blend, Color, Light, Renderer, TextureCache, TextureId};

use crate::{ParticleWorld, QuadImage};

/// The side of the glow texture, in texels.
pub const GLOW_SIZE: u32 = 32;

/// The soft round glow [`QuadImage::Glow`] draws: white, opaque in the
/// middle and fading smoothly to nothing at the edge, as RGBA8.
pub fn glow_image() -> Vec<u8> {
    let mut rgba = Vec::with_capacity((GLOW_SIZE * GLOW_SIZE * 4) as usize);
    let half = GLOW_SIZE as f32 / 2.0;
    for y in 0..GLOW_SIZE {
        for x in 0..GLOW_SIZE {
            let d = ((x as f32 + 0.5 - half).hypot(y as f32 + 0.5 - half) / half).min(1.0);
            let a = (1.0 - d) * (1.0 - d);
            rgba.extend_from_slice(&[255, 255, 255, (a * 255.0).round() as u8]);
        }
    }
    rgba
}

/// Draws a [`ParticleWorld`]: keeps the textures sprite particles use, and
/// the glow.
#[derive(Debug, Default)]
pub struct ParticleRenderer {
    textures: TextureCache,
    glow: Option<TextureId>,
}

impl ParticleRenderer {
    pub fn new() -> Self {
        ParticleRenderer::default()
    }

    /// Queues every particle, with the blend each effect asks for. Leaves
    /// the renderer blending normally ([`Blend::Alpha`]).
    pub fn draw(&mut self, r: &mut Renderer, db: &AssetDb, world: &ParticleWorld) {
        self.draw_in(r, db, world, i32::MIN..=i32::MAX);
    }

    /// [`ParticleRenderer::draw`] for effects whose render `order` is in
    /// `orders`.
    pub fn draw_in(
        &mut self,
        r: &mut Renderer,
        db: &AssetDb,
        world: &ParticleWorld,
        orders: std::ops::RangeInclusive<i32>,
    ) {
        let quads = world.quads_in(orders, &mut |h| db.load(h).ok());
        for q in quads {
            r.set_blend(match q.blend {
                BlendMode::Alpha => Blend::Alpha,
                BlendMode::Add => Blend::Add,
            });
            let [cr, cg, cb, ca] = q.color;
            let color = Color { r: cr, g: cg, b: cb, a: ca };
            match q.image {
                QuadImage::Solid => r.rect(q.x, q.y, q.w, q.h, color),
                QuadImage::Glow => {
                    let glow = *self
                        .glow
                        .get_or_insert_with(|| r.load_texture(&glow_image(), GLOW_SIZE, GLOW_SIZE));
                    let full = [0.0, 0.0, GLOW_SIZE as f32, GLOW_SIZE as f32];
                    r.sprite(glow, full, q.x, q.y, q.w, q.h, false, color);
                }
                QuadImage::Sprite { texture, src } => match self.textures.get(r, db, texture) {
                    Ok(id) => r.sprite(id, src, q.x, q.y, q.w, q.h, q.flip, color),
                    Err(e) => log::warn!("particle texture {}: {e}", texture.guid),
                },
            }
        }
        r.set_blend(Blend::Alpha);
    }

    /// Queues the lights particles carry.
    pub fn lights(&self, r: &mut Renderer, world: &ParticleWorld) {
        for l in world.lights() {
            let [cr, cg, cb] = l.color;
            r.light(Light {
                x: l.x,
                y: l.y,
                radius: l.radius,
                color: Color { r: cr, g: cg, b: cb, a: 1.0 },
                intensity: l.intensity,
            });
        }
    }

    /// A texture asset reloaded: re-uploads it if particles use it.
    pub fn reload(&mut self, r: &mut Renderer, db: &AssetDb, guid: Guid) {
        if let Err(e) = self.textures.reload(r, db, guid) {
            log::warn!("particle texture {guid}: {e}");
        }
    }
}
