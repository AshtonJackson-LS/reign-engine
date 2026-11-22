//! Reactive water: bodies of water whose surface ripples, splashes and
//! settles, drawn reflective and refractive by the renderer.
//!
//! A [`Water`] is a rectangle of water in world pixels: its resting surface
//! at `y`, from `x` to `x + w`, `depth` deep. Its surface is a row of
//! columns joined by springs (a 1D wave equation), so a disturbance travels
//! out both ways, bounces off the ends and dies away. Disturb it with
//! [`Water::splash`] (a hit, with droplets) or [`Water::push`] (a nudge),
//! or let it follow the things in it: call [`Water::touch`] each frame with
//! each body's rectangle and the water splashes as they fall in or jump
//! out, and ripples ahead of them as they wade. Step it once a frame with
//! [`Water::step`] and draw it with [`Water::draw`]; how it looks is its
//! [`WaterStyle`].
//!
//! Draw water after what it should reflect and refract, and before what
//! should stand in front of it: the renderer copies the frame drawn so far
//! to see through it.
//!
//! Water is presentation only. It steps in real time, never feeds back
//! into the simulation, and uses its own random numbers, so it can't change
//! what a replay or an online game does.

use reign_render::{Blend, Color, Renderer};
pub use reign_render::{WaterStyle, WaterSurface};

/// How a surface moves. The defaults suit water a few tiles across, for
/// characters a couple of tiles tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ripples {
    /// How fast ripples travel along the surface, in pixels a second.
    pub speed: f32,
    /// How hard each column is pulled back to rest; higher bobs faster.
    pub stiffness: f32,
    /// How quickly motion dies away, per second.
    pub damping: f32,
    /// The width of a surface column, in pixels. Narrower is finer and
    /// costs more.
    pub column: f32,
    /// How far the surface can be pushed from rest, in pixels.
    pub limit: f32,
}

impl Default for Ripples {
    fn default() -> Self {
        Ripples { speed: 150.0, stiffness: 14.0, damping: 1.4, column: 2.0, limit: 14.0 }
    }
}

/// How bodies moving through the water disturb it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reaction {
    /// How much of a body's speed into the water becomes the surface's,
    /// on entry.
    pub entry: f32,
    /// The same, for a body leaving: it drags the surface up after it.
    pub exit: f32,
    /// How hard a body wading through pushes up a ripple ahead of it.
    pub wake: f32,
    /// Droplets thrown up per 100 pixels a second of entry speed.
    pub spray: f32,
    /// Bodies wading faster than this (pixels a second) kick up droplets.
    pub kick: f32,
}

impl Default for Reaction {
    fn default() -> Self {
        Reaction { entry: 0.45, exit: 0.25, wake: 2.0, spray: 5.0, kick: 150.0 }
    }
}

/// Droplets fall this fast, in pixels a second squared.
const GRAVITY: f32 = 520.0;
/// The most droplets in the air at once, per body of water.
const MAX_DROPLETS: usize = 160;
/// The slowest entry (pixels a second) that splashes rather than slips in.
const SPLASH_SPEED: f32 = 40.0;

/// A body of water. See the [crate docs](crate).
#[derive(Clone, Debug)]
pub struct Water {
    /// The resting surface's left end, in world pixels.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    /// From the resting surface to the bottom.
    pub depth: f32,
    pub style: WaterStyle,
    pub ripples: Ripples,
    pub reaction: Reaction,
    /// Displacement of each column from rest, down positive, and its speed.
    heights: Vec<f32>,
    speeds: Vec<f32>,
    droplets: Vec<Droplet>,
    visitors: Vec<Visitor>,
    /// Seconds stepped, for the shader's swell and ripples.
    time: f32,
    /// Time not yet simulated, less than one sub-step.
    pending: f32,
    rng: u32,
}

/// A drop of water in flight.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Droplet {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// 1 or 2 pixels.
    size: f32,
}

/// Something [`Water::touch`] follows, by its key.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Visitor {
    key: u64,
    /// Where it was at the last step, and where it is now.
    was: Rect,
    now: Rect,
    /// Touched since the last step; forgotten if not.
    seen: bool,
    /// Seconds until wading next kicks up a droplet.
    kick: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Water {
    /// A calm body of water with the default look and motion.
    pub fn new(x: f32, y: f32, w: f32, depth: f32) -> Self {
        let ripples = Ripples::default();
        let n = columns(w, ripples.column);
        Water {
            x,
            y,
            w: w.max(0.0),
            depth: depth.max(0.0),
            style: WaterStyle::default(),
            ripples,
            reaction: Reaction::default(),
            heights: vec![0.0; n],
            speeds: vec![0.0; n],
            droplets: Vec::new(),
            visitors: Vec::new(),
            time: 0.0,
            pending: 0.0,
            // Any odd seed; two bodies differ by where they are.
            rng: (x.to_bits() ^ y.to_bits().rotate_left(13) ^ 0x9e37_79b9) | 1,
        }
    }

    pub fn with_style(mut self, style: WaterStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_ripples(mut self, ripples: Ripples) -> Self {
        self.ripples = ripples;
        let n = columns(self.w, ripples.column);
        self.heights = vec![0.0; n];
        self.speeds = vec![0.0; n];
        self
    }

    pub fn with_reaction(mut self, reaction: Reaction) -> Self {
        self.reaction = reaction;
        self
    }

    /// The surface's world y above `x`, as simulated (the shader's swell
    /// adds a pixel or so on top). The resting surface outside the water.
    pub fn surface(&self, x: f32) -> f32 {
        self.y + self.height(x)
    }

    /// Whether the world point is in the water, under its surface.
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.surface(x) && y <= self.y + self.depth
    }

    /// The water's bottom edge, in world y.
    pub fn bottom(&self) -> f32 {
        self.y + self.depth
    }

    /// Hits the surface at `x`: `speed` pixels a second down (negative
    /// pulls it up) over `radius` pixels either side, and throws up
    /// droplets in proportion.
    pub fn splash(&mut self, x: f32, speed: f32, radius: f32) {
        self.push(x, speed, radius);
        let count = (speed.abs() / 100.0 * self.reaction.spray).round() as usize;
        let surface = self.surface(x);
        for _ in 0..count {
            let across = (self.random() * 2.0 - 1.0) * radius;
            let up = speed.abs() * (0.25 + 0.5 * self.random()) + 40.0;
            let out = across / radius.max(1.0) * (40.0 + speed.abs() * 0.25)
                + (self.random() - 0.5) * 30.0;
            self.throw(x + across, surface - 1.0, out, -up.min(380.0));
        }
    }

    /// Nudges the surface at `x`, `speed` pixels a second down (negative is
    /// up) at the centre, easing to nothing `radius` pixels either side. No
    /// droplets.
    pub fn push(&mut self, x: f32, speed: f32, radius: f32) {
        let radius = radius.max(self.ripples.column);
        let column = self.ripples.column;
        let first = ((x - radius - self.x) / column).floor().max(0.0) as usize;
        let last = ((x + radius - self.x) / column).ceil();
        if last < 0.0 {
            return;
        }
        let last = (last as usize).min(self.speeds.len().saturating_sub(1));
        for i in first..=last {
            let d = (self.x + i as f32 * column - x) / radius;
            if d.abs() < 1.0 {
                // A smooth bump: cosine down to zero at the radius.
                let k = 0.5 + 0.5 * (d * std::f32::consts::PI).cos();
                self.speeds[i] += speed * k;
            }
        }
    }

    /// Tells the water where a body is this frame, by a key that names it
    /// from frame to frame (an object id, an index). Call it for every body
    /// that might be in the water, before [`Water::step`]; the water works
    /// out from where it was last time whether it fell in, jumped out or is
    /// wading, and reacts. Bodies not touched before a step are forgotten.
    pub fn touch(&mut self, key: u64, x: f32, y: f32, w: f32, h: f32) {
        let now = Rect { x, y, w, h };
        match self.visitors.iter_mut().find(|v| v.key == key) {
            Some(v) => {
                v.now = now;
                v.seen = true;
            }
            None => self.visitors.push(Visitor { key, was: now, now, seen: true, kick: 0.0 }),
        }
    }

    /// Moves the water on by `dt` seconds: bodies' splashes and wakes, the
    /// ripples, and the droplets.
    pub fn step(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.1);
        if dt <= 0.0 {
            return;
        }
        self.time += dt;
        self.react(dt);
        self.ripple(dt);
        self.fall(dt);
    }

    /// Whether nothing is moving: the surface is flat to within a tenth of
    /// a pixel and no droplets are in the air.
    pub fn calm(&self) -> bool {
        self.droplets.is_empty()
            && self.heights.iter().all(|h| h.abs() < 0.1)
            && self.speeds.iter().all(|v| v.abs() < 1.0)
    }

    /// Queues the water and its droplets. Draw it after everything it
    /// should show through and mirror.
    pub fn draw(&self, r: &mut Renderer) {
        r.water(&WaterSurface {
            x: self.x,
            y: self.y,
            w: self.w,
            depth: self.depth,
            heights: &self.heights,
            time: self.time,
            style: self.style,
        });
        self.draw_droplets(r);
    }

    /// Queues just the droplets, as when the water itself is drawn
    /// elsewhere in the frame.
    pub fn draw_droplets(&self, r: &mut Renderer) {
        let c = self.style.highlight;
        r.set_blend(Blend::Alpha);
        for d in &self.droplets {
            // Stretched along their fall, a pixel or two long.
            let stretch = (d.vy.abs() / 160.0).clamp(0.0, 1.5);
            let tint = Color { a: c.a * 0.9, ..c };
            r.rect(d.x - d.size / 2.0, d.y - d.size / 2.0, d.size, d.size + stretch, tint);
        }
    }

    /// How many droplets are in the air.
    pub fn droplet_count(&self) -> usize {
        self.droplets.len()
    }

    fn height(&self, x: f32) -> f32 {
        if self.heights.is_empty() || x < self.x || x > self.x + self.w {
            return 0.0;
        }
        let f = (x - self.x) / self.ripples.column;
        let i = (f.floor() as usize).min(self.heights.len() - 1);
        let j = (i + 1).min(self.heights.len() - 1);
        let t = f - f.floor();
        self.heights[i] + (self.heights[j] - self.heights[i]) * t
    }

    /// Splashes, wakes and kicks from the bodies touched since last step.
    fn react(&mut self, dt: f32) {
        let mut visitors = std::mem::take(&mut self.visitors);
        visitors.retain(|v| v.seen);
        for v in &mut visitors {
            let (was, now) = (v.was, v.now);
            v.was = now;
            v.seen = false;
            let cx = now.x + now.w / 2.0;
            if cx < self.x || cx > self.x + self.w {
                continue;
            }
            let surface = self.surface(cx);
            let (feet_was, feet) = (was.y + was.h, now.y + now.h);
            let vy = (feet - feet_was) / dt;
            let vx = (now.x - was.x) / dt;
            let radius = now.w * 0.6 + 4.0;
            let r = self.reaction;
            if feet_was <= surface && feet > surface && vy > SPLASH_SPEED {
                // In it goes.
                self.splash(cx, vy * r.entry, radius);
            } else if feet_was > surface && feet <= surface && vy < -SPLASH_SPEED {
                // Out again, pulling the surface up after it.
                self.splash(cx, vy * r.exit, radius * 0.8);
            } else if feet > surface && now.y < surface && vx.abs() > 8.0 {
                // Wading: the front pushes a ripple up ahead, the back
                // leaves a trough.
                let dir = vx.signum();
                let push = vx.abs() * r.wake * dt;
                self.push(cx + dir * now.w * 0.5, -push, now.w * 0.5 + 3.0);
                self.push(cx - dir * now.w * 0.5, push * 0.5, now.w * 0.5 + 3.0);
                v.kick -= dt;
                if vx.abs() > r.kick && v.kick <= 0.0 {
                    v.kick = 0.05;
                    let front = cx + dir * now.w * 0.5;
                    let up = 60.0 + vx.abs() * 0.4 * (0.5 + self.random());
                    let out = dir * vx.abs() * (0.2 + 0.3 * self.random());
                    self.throw(front, surface - 1.0, out, -up.min(300.0));
                }
            }
        }
        self.visitors = visitors;
    }

    /// The wave equation over the columns, in sub-steps short enough to
    /// stay stable at any frame time.
    fn ripple(&mut self, dt: f32) {
        let p = self.ripples;
        let n = self.heights.len();
        if n == 0 {
            return;
        }
        let column = p.column.max(0.25);
        let longest = 0.5 * column / p.speed.max(1.0);
        let step = longest.min(1.0 / 240.0);
        self.pending += dt;
        let steps = (self.pending / step).floor() as usize;
        self.pending -= steps as f32 * step;
        let c2 = (p.speed / column).powi(2);
        for _ in 0..steps.min(2400) {
            for i in 0..n {
                let left = self.heights[i.saturating_sub(1)];
                let right = self.heights[(i + 1).min(n - 1)];
                let h = self.heights[i];
                let pull = c2 * (left + right - 2.0 * h) - p.stiffness * h;
                let v = self.speeds[i];
                self.speeds[i] = v + (pull - p.damping * v) * step;
            }
            for (h, v) in self.heights.iter_mut().zip(&self.speeds) {
                *h = (*h + v * step).clamp(-p.limit, p.limit);
            }
        }
    }

    /// Droplets fly, and fall back in with a tiny splash of their own.
    fn fall(&mut self, dt: f32) {
        let mut droplets = std::mem::take(&mut self.droplets);
        droplets.retain_mut(|d| {
            d.vy += GRAVITY * dt;
            d.x += d.vx * dt;
            d.y += d.vy * dt;
            let inside = d.x >= self.x && d.x <= self.x + self.w;
            if inside && d.vy > 0.0 && d.y >= self.surface(d.x) {
                self.push(d.x, d.vy * 0.05, 3.0);
                return false;
            }
            // Past the ends it falls until it's well below the water.
            d.y < self.y + self.depth + 64.0
        });
        self.droplets = droplets;
    }

    fn throw(&mut self, x: f32, y: f32, vx: f32, vy: f32) {
        if self.droplets.len() >= MAX_DROPLETS {
            return;
        }
        let size = if self.random() < 0.3 { 2.0 } else { 1.0 };
        self.droplets.push(Droplet { x, y, vx, vy, size });
    }

    /// 0..1, from a xorshift of the water's own.
    fn random(&mut self) -> f32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// Columns spanning `w` pixels, `column` apart, both ends included.
fn columns(w: f32, column: f32) -> usize {
    (w.max(0.0) / column.max(0.25)).ceil() as usize + 1
}

#[cfg(test)]
mod tests;
