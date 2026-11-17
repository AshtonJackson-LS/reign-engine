//! What particles look like this frame: [`Quad`]s and lights, worked out
//! from each particle's life, ready for any renderer (the game's, or the
//! editor's preview).

use std::sync::Arc;

use reign_assets::particles::{BlendMode, FrameMode, RenderMode};
use reign_assets::{Handle, SpriteSheet, Texture};

use crate::{Particle, ParticleWorld, System};

/// One rectangle to draw, in world pixels, top-left corner first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Red, green, blue, alpha, 0 to 1 (sRGB, not premultiplied).
    pub color: [f32; 4],
    pub blend: BlendMode,
    pub image: QuadImage,
    /// Mirrored left to right (sprites).
    pub flip: bool,
}

/// What fills a [`Quad`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum QuadImage {
    /// The colour, solid.
    Solid,
    /// The soft round glow ([`crate::glow_image`]), tinted.
    Glow,
    /// The texel rectangle `src` (x, y, w, h) of a texture, tinted.
    Sprite { texture: Handle<Texture>, src: [f32; 4] },
}

/// A light following a particle, in world pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleLight {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    /// Red, green, blue, 0 to 1 (sRGB).
    pub color: [f32; 3],
    pub intensity: f32,
}

/// Colours fainter than this aren't drawn.
const INVISIBLE: f32 = 1.0 / 255.0;

impl ParticleWorld {
    /// Every particle as quads, in the order they're drawn: effects in the
    /// order they were played, each one's particles oldest first. `sheets`
    /// finds sprite sheets for sprite particles.
    pub fn quads(
        &self,
        sheets: &mut dyn FnMut(Handle<SpriteSheet>) -> Option<Arc<SpriteSheet>>,
    ) -> Vec<Quad> {
        let mut out = Vec::new();
        for s in &self.systems {
            let sheet = (s.effect.render.mode == RenderMode::Sprite)
                .then(|| sheets(s.effect.render.sheet))
                .flatten();
            for p in &s.particles {
                s.particle_quads(p, sheet.as_deref(), &mut out);
            }
        }
        out
    }

    /// The lights particles carry this frame.
    pub fn lights(&self) -> Vec<ParticleLight> {
        let mut out = Vec::new();
        for s in &self.systems {
            let l = &s.effect.lights;
            if !l.enabled {
                continue;
            }
            for p in s.particles.iter().filter(|p| p.light) {
                let color = s.color(p);
                let fade = (1.0 - p.age / p.life).clamp(0.0, 1.0) * color[3];
                if fade <= INVISIBLE {
                    continue;
                }
                let rgb = if l.use_particle_color {
                    [color[0], color[1], color[2]]
                } else {
                    [0, 1, 2].map(|i| l.color[i] as f32 / 255.0)
                };
                let (x, y) = s.world_pos(p);
                out.push(ParticleLight {
                    x,
                    y,
                    radius: l.radius * s.spawn.scale,
                    color: rgb,
                    intensity: l.intensity * fade,
                });
            }
        }
        out
    }
}

impl System {
    /// A particle's colour now: its own, through the colour-over-life.
    fn color(&self, p: &Particle) -> [f32; 4] {
        let c = &self.effect.color;
        if !c.enabled {
            return p.color;
        }
        let g = c.gradient.at(p.age / p.life);
        [0, 1, 2, 3].map(|i| p.color[i] * g[i])
    }

    fn size(&self, p: &Particle) -> f32 {
        let s = &self.effect.size;
        if s.enabled { p.size * s.curve.at(p.age / p.life).max(0.0) } else { p.size }
    }

    fn particle_quads(&self, p: &Particle, sheet: Option<&SpriteSheet>, out: &mut Vec<Quad>) {
        let r = &self.effect.render;
        let color = self.color(p);
        if color[3] <= INVISIBLE {
            return;
        }
        let size = self.size(p);
        let (x, y) = self.world_pos(p);
        let snap = |v: f32| if r.pixel_snap { v.round() } else { v };
        let quad = |x: f32, y: f32, w: f32, h: f32, image: QuadImage, flip: bool| Quad {
            x,
            y,
            w,
            h,
            color,
            blend: r.blend,
            image,
            flip,
        };
        match r.mode {
            RenderMode::Square => {
                let (mut w, mut h) = (size * r.aspect, size);
                if r.pixel_snap {
                    if size < 0.5 {
                        return;
                    }
                    (w, h) = (w.round().max(1.0), h.round().max(1.0));
                } else if w <= 0.0 || h <= 0.0 {
                    return;
                }
                out.push(quad(snap(x - w / 2.0), snap(y - h / 2.0), w, h, QuadImage::Solid, false));
            }
            RenderMode::Streak => {
                let thick = if r.pixel_snap { size.round().max(1.0) } else { size.max(0.25) };
                let speed = p.vx.hypot(p.vy);
                let len = (speed * r.streak).clamp(1.0, r.max_streak.max(1.0));
                let (dx, dy) = if speed > 0.0 { (p.vx / speed, p.vy / speed) } else { (0.0, 0.0) };
                // A pixel line back along the way it came, head first.
                let mut last = None;
                for i in 0..len.round().max(1.0) as u32 {
                    let at = (snap(x - dx * i as f32), snap(y - dy * i as f32));
                    if last == Some(at) {
                        continue;
                    }
                    last = Some(at);
                    let half = thick / 2.0;
                    let (qx, qy) = if r.pixel_snap { at } else { (at.0 - half, at.1 - half) };
                    out.push(quad(qx, qy, thick, thick, QuadImage::Solid, false));
                }
            }
            RenderMode::Glow => {
                if size <= 0.0 {
                    return;
                }
                out.push(quad(
                    snap(x - size / 2.0),
                    snap(y - size / 2.0),
                    size,
                    size,
                    QuadImage::Glow,
                    false,
                ));
            }
            RenderMode::Sprite => {
                let Some(sheet) = sheet else { return };
                let Some((rect, pivot)) = self.sprite(p, sheet) else { return };
                let d = sheet.density.max(1) as f32;
                let (w, h) = (rect[2] as f32 / d * size, rect[3] as f32 / d * size);
                if w <= 0.0 || h <= 0.0 {
                    return;
                }
                let (px, py) = (pivot[0] as f32 / d * size, pivot[1] as f32 / d * size);
                let flip = self.spawn.flip_x;
                let left = if flip { x - (w - px) } else { x - px };
                let src = rect.map(|v| v as f32);
                let image = QuadImage::Sprite { texture: sheet.texture, src };
                out.push(quad(snap(left), snap(y - py), w, h, image, flip));
            }
        }
    }

    /// The sprite a particle shows now: its texel rectangle and pivot.
    fn sprite(&self, p: &Particle, sheet: &SpriteSheet) -> Option<([u32; 4], [i32; 2])> {
        let r = &self.effect.render;
        // The sprites to choose from: the named ones, else every sprite,
        // else every cell of the grid.
        let named: Vec<([u32; 4], [i32; 2])> = if !r.sprites.is_empty() {
            r.sprites.iter().filter_map(|n| sheet.sprite(n)).map(|s| (s.rect, s.pivot)).collect()
        } else if !sheet.sprites.is_empty() {
            sheet.sprites.iter().map(|s| (s.rect, s.pivot)).collect()
        } else if let Some(grid) = sheet.grid {
            (0..grid.rows)
                .flat_map(|row| (0..grid.columns).map(move |col| (col, row)))
                .map(|(col, row)| (grid.cell_rect(col, row), sheet.cell_pivot))
                .collect()
        } else {
            Vec::new()
        };
        let n = named.len();
        if n == 0 {
            return None;
        }
        let i = match r.frames {
            FrameMode::OverLife => ((p.age / p.life) * n as f32) as usize,
            FrameMode::Random => p.seed as usize % n,
            FrameMode::Fps => (p.age * r.fps.max(0.0)) as usize % n,
        };
        named.get(i.min(n - 1)).copied()
    }
}
