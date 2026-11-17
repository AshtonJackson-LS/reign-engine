//! Particle systems: plays [`ParticleEffect`] assets (`*.particles.ron`)
//! and draws them, in the spirit of Unity's Particle System.
//!
//! A [`ParticleWorld`] holds every effect that's playing. Start one with
//! [`ParticleWorld::play`] at a [`Spawn`] (where, which way, mirrored or
//! not), move it with [`ParticleWorld::move_to`], stop it with
//! [`ParticleWorld::stop`] (its particles live out their lives), and step
//! the world once a frame with [`ParticleWorld::step`]. Draw it with a
//! [`ParticleRenderer`], or turn it into [`Quad`]s and lights yourself (the
//! editor's preview does).
//!
//! Effects playing from the asset database pick up changes to their file
//! as soon as it reloads. A game tells particles where its level is solid
//! through [`Ground`], and where to find sub-emitters' effects through
//! [`Effects`] (an [`AssetDb`] is one).
//!
//! Particles are presentation only. They step in real time, never feed
//! back into the simulation, and use their own random numbers, so they
//! can't change what a replay or an online game does.

mod draw;
mod render;

use std::sync::Arc;

use reign_assets::particles::{CollisionMode, ShapeKind, Space, SubTrigger};
use reign_assets::{AssetDb, Guid, Handle};

pub use draw::{ParticleLight, Quad, QuadImage};
pub use reign_assets::ParticleEffect;
pub use reign_assets::particles;
pub use render::{GLOW_SIZE, ParticleRenderer, glow_image};

/// The longest step particles take at once, in seconds: a long frame is
/// split into steps this long, so fast particles still hit the floor.
const MAX_STEP: f32 = 1.0 / 60.0;
/// The most frame time stepped at once: after a long stall, effects jump
/// ahead rather than replaying it all.
const MAX_FRAME: f32 = 0.25;
/// How deep sub-emitters can start sub-emitters.
const MAX_DEPTH: u8 = 4;
/// Impacts slower than this (pixels a second, into the surface) count as
/// resting, not hitting: no sub-emitters, no lifetime loss.
const RESTING: f32 = 12.0;
/// Seconds of rounding allowed in timings: frame times don't add up
/// exactly (sixty steps of 1/60 isn't quite 1), and a burst due at 0.75 s
/// shouldn't wait a frame for 0.7499999.
const SLACK: f32 = 1e-4;

/// Where a game's level is solid, for particles that collide with it.
pub trait Ground {
    /// Whether the world pixel at (`x`, `y`) is inside something solid.
    fn solid(&self, x: f32, y: f32) -> bool;
}

impl<F: Fn(f32, f32) -> bool> Ground for F {
    fn solid(&self, x: f32, y: f32) -> bool {
        self(x, y)
    }
}

/// A level with nothing solid in it.
pub fn no_ground(_: f32, _: f32) -> bool {
    false
}

/// Where effects come from, by handle: for effects played by handle (which
/// follow their file as it changes) and for sub-emitters.
pub trait Effects {
    fn effect(&self, handle: Handle<ParticleEffect>) -> Option<Arc<ParticleEffect>>;
}

impl Effects for AssetDb {
    fn effect(&self, handle: Handle<ParticleEffect>) -> Option<Arc<ParticleEffect>> {
        self.load(handle).ok()
    }
}

/// No effects at all: for playing an effect with no sub-emitters.
pub struct NoEffects;

impl Effects for NoEffects {
    fn effect(&self, _: Handle<ParticleEffect>) -> Option<Arc<ParticleEffect>> {
        None
    }
}

/// Where and how an effect plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spawn {
    /// The emitter, in world pixels.
    pub x: f32,
    pub y: f32,
    /// Turns the whole effect, in degrees (clockwise, as screen y grows
    /// down): an effect that sprays up (-90) sprays right when turned 90.
    pub angle: f32,
    /// Mirrors the effect left to right: for a character facing left.
    pub flip_x: bool,
    /// Scales positions, speeds and sizes.
    pub scale: f32,
    /// Where the floor is, in world pixels, for an effect with collision.
    /// None uses the effect's own `floor` below the emitter.
    pub floor: Option<f32>,
    /// Added to every particle's starting velocity, in pixels a second.
    pub velocity: [f32; 2],
    /// Multiplies every particle's colour: red, green, blue, alpha, 0 to 1.
    pub tint: [f32; 4],
    /// The random numbers to start from, for an effect that should look
    /// the same each time. None picks new ones.
    pub seed: Option<u32>,
}

impl Default for Spawn {
    fn default() -> Self {
        Spawn {
            x: 0.0,
            y: 0.0,
            angle: 0.0,
            flip_x: false,
            scale: 1.0,
            floor: None,
            velocity: [0.0, 0.0],
            tint: [1.0; 4],
            seed: None,
        }
    }
}

impl Spawn {
    /// Plays at (`x`, `y`), unturned and unmirrored.
    pub fn at(x: f32, y: f32) -> Self {
        Spawn { x, y, ..Spawn::default() }
    }

    pub fn angle(self, degrees: f32) -> Self {
        Spawn { angle: degrees, ..self }
    }

    pub fn flip_x(self, flip: bool) -> Self {
        Spawn { flip_x: flip, ..self }
    }

    pub fn scale(self, scale: f32) -> Self {
        Spawn { scale, ..self }
    }

    pub fn floor(self, y: f32) -> Self {
        Spawn { floor: Some(y), ..self }
    }

    pub fn tint(self, tint: [f32; 4]) -> Self {
        Spawn { tint, ..self }
    }

    pub fn seed(self, seed: u32) -> Self {
        Spawn { seed: Some(seed), ..self }
    }
}

/// One playing effect, as [`ParticleWorld::play`] returns it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SystemId(u32);

/// Where a system's effect comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// The asset: it follows the file as it reloads.
    Asset(Handle<ParticleEffect>),
    /// Handed over directly (an editor's unsaved edits).
    Inline,
}

#[derive(Clone, Debug)]
struct Particle {
    id: u32,
    /// In world pixels, or from the emitter for [`Space::Local`].
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    age: f32,
    life: f32,
    size: f32,
    /// Red, green, blue, alpha, 0 to 1, with the emitter's tint.
    color: [f32; 4],
    /// Picks its sprite, for random sprites.
    seed: u32,
    light: bool,
    stuck: bool,
}

/// A sub-emitter to start once this step's updates are done.
struct Pending {
    handle: Handle<ParticleEffect>,
    spawn: Spawn,
    depth: u8,
    follow: Option<(SystemId, u32)>,
}

#[derive(Clone, Debug)]
struct System {
    id: SystemId,
    source: Source,
    effect: Arc<ParticleEffect>,
    spawn: Spawn,
    /// Seconds since it was played.
    time: f32,
    /// Particles owed by the emission rate, carried between steps.
    carry: f32,
    /// Which cycle the bursts were last fired in, and how many times each
    /// has fired in it.
    cycle: u32,
    fired: Vec<u32>,
    emitting: bool,
    particles: Vec<Particle>,
    rng: Rng,
    next_particle: u32,
    /// A birth sub-emitter's parent particle: the system and particle it
    /// follows until that particle dies.
    follow: Option<(SystemId, u32)>,
    depth: u8,
}

/// Every effect that's playing.
#[derive(Clone, Debug, Default)]
pub struct ParticleWorld {
    systems: Vec<System>,
    next_id: u32,
    /// Seeds effects played without one.
    seed: u32,
    /// Effects that wouldn't load, warned about once each.
    warned: Vec<Guid>,
}

impl ParticleWorld {
    pub fn new() -> Self {
        ParticleWorld::default()
    }

    /// Plays the effect `handle` names at `spawn`. None if it doesn't load
    /// (logged once).
    pub fn play(
        &mut self,
        effects: &dyn Effects,
        handle: Handle<ParticleEffect>,
        spawn: Spawn,
    ) -> Option<SystemId> {
        match effects.effect(handle) {
            Some(effect) => Some(self.add(Source::Asset(handle), effect, spawn, 0)),
            None => {
                self.warn(handle.guid);
                None
            }
        }
    }

    /// Plays an effect handed over directly, not from a file: it doesn't
    /// follow any file, but [`ParticleWorld::set_effect`] can change it.
    pub fn play_effect(&mut self, effect: Arc<ParticleEffect>, spawn: Spawn) -> SystemId {
        self.add(Source::Inline, effect, spawn, 0)
    }

    fn add(
        &mut self,
        source: Source,
        effect: Arc<ParticleEffect>,
        spawn: Spawn,
        depth: u8,
    ) -> SystemId {
        self.next_id = self.next_id.wrapping_add(1);
        let id = SystemId(self.next_id);
        let seed = spawn.seed.unwrap_or_else(|| {
            self.seed = self.seed.wrapping_add(0x9e37_79b9);
            self.seed ^ self.next_id.wrapping_mul(0x85eb_ca6b)
        });
        self.systems.push(System {
            id,
            source,
            effect,
            spawn,
            time: 0.0,
            carry: 0.0,
            cycle: 0,
            fired: Vec::new(),
            emitting: true,
            particles: Vec::new(),
            rng: Rng::new(seed),
            next_particle: 0,
            follow: None,
            depth,
        });
        id
    }

    fn warn(&mut self, guid: Guid) {
        if !self.warned.contains(&guid) {
            log::warn!("particle effect {guid} doesn't load");
            self.warned.push(guid);
        }
    }

    fn system_mut(&mut self, id: SystemId) -> Option<&mut System> {
        self.systems.iter_mut().find(|s| s.id == id)
    }

    /// Moves an effect's emitter. Particles already born stay put, unless
    /// the effect simulates in [`Space::Local`].
    pub fn move_to(&mut self, id: SystemId, x: f32, y: f32) {
        if let Some(s) = self.system_mut(id) {
            s.spawn.x = x;
            s.spawn.y = y;
        }
    }

    /// The effect's spawn settings, to change any of them while it plays.
    pub fn spawn_mut(&mut self, id: SystemId) -> Option<&mut Spawn> {
        self.system_mut(id).map(|s| &mut s.spawn)
    }

    /// Replaces the effect a system plays (an editor showing edits as
    /// they're made). Its particles keep going.
    pub fn set_effect(&mut self, id: SystemId, effect: Arc<ParticleEffect>) {
        if let Some(s) = self.system_mut(id) {
            s.effect = effect;
            s.source = Source::Inline;
        }
    }

    /// Stops emitting; the particles already born live out their lives.
    pub fn stop(&mut self, id: SystemId) {
        if let Some(s) = self.system_mut(id) {
            s.emitting = false;
        }
    }

    /// Removes an effect and its particles at once.
    pub fn remove(&mut self, id: SystemId) {
        self.systems.retain(|s| s.id != id);
    }

    /// Removes everything.
    pub fn clear(&mut self) {
        self.systems.clear();
    }

    /// Whether the effect is still emitting or has particles alive.
    pub fn is_alive(&self, id: SystemId) -> bool {
        self.systems.iter().any(|s| s.id == id)
    }

    /// Whether the effect is still emitting.
    pub fn is_emitting(&self, id: SystemId) -> bool {
        self.systems.iter().any(|s| s.id == id && s.emitting)
    }

    /// Particles alive, in every effect.
    pub fn particle_count(&self) -> usize {
        self.systems.iter().map(|s| s.particles.len()).sum()
    }

    /// Effects playing, sub-emitters included.
    pub fn system_count(&self) -> usize {
        self.systems.len()
    }

    /// Advances every effect by `dt` seconds of real time.
    pub fn step(&mut self, dt: f32, effects: &dyn Effects, ground: &dyn Ground) {
        if dt.is_nan() || dt <= 0.0 {
            return;
        }
        let dt = dt.min(MAX_FRAME);
        let steps = (dt / MAX_STEP).ceil().max(1.0);
        let h = dt / steps;
        for _ in 0..steps as u32 {
            self.substep(h, effects, ground);
        }
    }

    fn substep(&mut self, dt: f32, effects: &dyn Effects, ground: &dyn Ground) {
        for s in &mut self.systems {
            if let Source::Asset(handle) = s.source
                && let Some(effect) = effects.effect(handle)
                && !Arc::ptr_eq(&effect, &s.effect)
            {
                s.effect = effect;
            }
        }
        self.follow_parents();
        let mut pending = Vec::new();
        for s in &mut self.systems {
            s.update(dt, ground, &mut pending);
        }
        self.systems.retain(System::alive);
        for p in pending {
            if p.depth > MAX_DEPTH {
                continue;
            }
            let Some(effect) = effects.effect(p.handle) else {
                self.warn(p.handle.guid);
                continue;
            };
            let id = self.add(Source::Asset(p.handle), effect, p.spawn, p.depth);
            if let Some(s) = self.system_mut(id) {
                s.follow = p.follow;
            }
        }
    }

    /// Moves birth sub-emitters to their particles, and stops the ones
    /// whose particle has died.
    fn follow_parents(&mut self) {
        let mut moves = Vec::new();
        for (i, s) in self.systems.iter().enumerate() {
            let Some((parent, particle)) = s.follow else { continue };
            let at = self.systems.iter().find(|p| p.id == parent).and_then(|p| {
                p.particles.iter().find(|q| q.id == particle).map(|q| p.world_pos(q))
            });
            moves.push((i, at));
        }
        for (i, at) in moves {
            let s = &mut self.systems[i];
            match at {
                Some((x, y)) => (s.spawn.x, s.spawn.y) = (x, y),
                None => {
                    s.emitting = false;
                    s.follow = None;
                }
            }
        }
    }
}

impl System {
    fn alive(&self) -> bool {
        self.emitting || !self.particles.is_empty()
    }

    /// Where the emitter puts a particle's position in the world.
    fn offset(&self) -> (f32, f32) {
        match self.effect.main.space {
            Space::World => (0.0, 0.0),
            Space::Local => (self.spawn.x, self.spawn.y),
        }
    }

    fn world_pos(&self, p: &Particle) -> (f32, f32) {
        let (ox, oy) = self.offset();
        (p.x + ox, p.y + oy)
    }

    fn update(&mut self, dt: f32, ground: &dyn Ground, pending: &mut Vec<Pending>) {
        let effect = self.effect.clone();
        self.time += dt;
        self.emit_due(&effect, dt, pending);
        self.move_particles(&effect, dt, ground, pending);
    }

    /// Emits what the rate and the bursts call for by now.
    fn emit_due(&mut self, e: &ParticleEffect, dt: f32, pending: &mut Vec<Pending>) {
        let t = self.time - e.main.start_delay;
        if !self.emitting || t < 0.0 {
            return;
        }
        let duration = e.main.duration.max(0.001);
        let cycle = ((t + SLACK) / duration).floor() as u32;
        if !e.main.looping && cycle >= 1 {
            // The one cycle is over: whatever was still due fires, then
            // emission ends.
            self.fire_bursts(e, duration, pending);
            self.emitting = false;
            return;
        }
        if cycle != self.cycle {
            self.cycle = cycle;
            self.fired.iter_mut().for_each(|f| *f = 0);
        }
        self.fire_bursts(e, (t - cycle as f32 * duration).max(0.0), pending);
        if e.emission.enabled && e.emission.rate > 0.0 {
            // Only the part of this step after the start delay counts.
            self.carry += e.emission.rate * dt.min(t);
            let whole = self.carry.floor();
            self.carry -= whole;
            self.emit(e, whole as u32, pending);
        }
    }

    fn fire_bursts(&mut self, e: &ParticleEffect, cycle_time: f32, pending: &mut Vec<Pending>) {
        if !e.emission.enabled {
            return;
        }
        self.fired.resize(e.emission.bursts.len(), 0);
        for (i, b) in e.emission.bursts.iter().enumerate() {
            while self.fired[i] < b.cycles.max(1)
                && b.time + self.fired[i] as f32 * b.interval <= cycle_time + SLACK
            {
                self.fired[i] += 1;
                if self.rng.unit() < b.probability || b.probability >= 1.0 {
                    let (lo, hi) = (b.count.0, b.count.1.max(b.count.0));
                    let count = lo + self.rng.below(hi - lo + 1);
                    self.emit(e, count, pending);
                }
            }
        }
    }

    /// Gives birth to `count` particles (fewer if that would pass the
    /// effect's maximum).
    fn emit(&mut self, e: &ParticleEffect, count: u32, pending: &mut Vec<Pending>) {
        let room = (e.main.max_particles as usize).saturating_sub(self.particles.len());
        for _ in 0..(count as usize).min(room) {
            let p = self.new_particle(e);
            for sub in e.sub_emitters.iter().filter(|s| s.trigger == SubTrigger::Birth) {
                if self.rng.unit() < sub.probability || sub.probability >= 1.0 {
                    let at = self.world_pos(&p);
                    let mut queued = self.sub_spawn(sub, &p, at);
                    queued.follow = Some((self.id, p.id));
                    pending.push(queued);
                }
            }
            self.particles.push(p);
        }
    }

    fn new_particle(&mut self, e: &ParticleEffect) -> Particle {
        let r = &mut self.rng;
        let sh = &e.shape;
        let base = sh.angle.to_radians();
        let heading = base + (r.unit() - 0.5) * sh.spread.to_radians();
        let across = |r: &mut Rng, width: f32| {
            let off = (r.unit() - 0.5) * width;
            let square = base + std::f32::consts::FRAC_PI_2;
            (square.cos() * off, square.sin() * off)
        };
        let (mut px, mut py) = match sh.kind {
            ShapeKind::Point => (0.0, 0.0),
            ShapeKind::Cone => across(r, sh.radius),
            ShapeKind::Line => across(r, sh.size[0]),
            ShapeKind::Circle => {
                let out = if sh.edge { sh.radius } else { sh.radius * r.unit().sqrt() };
                (heading.cos() * out, heading.sin() * out)
            }
            ShapeKind::Box => ((r.unit() - 0.5) * sh.size[0], (r.unit() - 0.5) * sh.size[1]),
        };
        let (mut dx, mut dy) = (heading.cos(), heading.sin());
        if sh.randomize > 0.0 {
            let k = sh.randomize.min(1.0);
            let a = r.unit() * std::f32::consts::TAU;
            (dx, dy) = (dx * (1.0 - k) + a.cos() * k, dy * (1.0 - k) + a.sin() * k);
            let len = dx.hypot(dy);
            if len > 1e-4 {
                (dx, dy) = (dx / len, dy / len);
            }
        }
        py *= sh.squash;
        dy *= sh.squash;
        // The emitter's turn, mirror and scale.
        let s = self.spawn;
        let (sin, cos) = s.angle.to_radians().sin_cos();
        (px, py) = (px * cos - py * sin, px * sin + py * cos);
        (dx, dy) = (dx * cos - dy * sin, dx * sin + dy * cos);
        if s.flip_x {
            px = -px;
            dx = -dx;
        }
        let m = &e.main;
        let speed = m.speed.lerp(r.unit()) * s.scale;
        let life = m.lifetime.lerp(r.unit()).max(0.001);
        let size = m.size.lerp(r.unit()).max(0.0) * s.scale;
        let k = r.unit();
        let unit = |c: [u8; 4]| c.map(|v| v as f32 / 255.0);
        let (a, b) = (unit(m.color.0), unit(m.color.1));
        let color = [0, 1, 2, 3].map(|i| (a[i] + (b[i] - a[i]) * k) * s.tint[i]);
        let light = e.lights.enabled && r.unit() < e.lights.ratio;
        let seed = r.next();
        let (x, y) = match m.space {
            Space::World => (s.x + px * s.scale, s.y + py * s.scale),
            Space::Local => (px * s.scale, py * s.scale),
        };
        self.next_particle = self.next_particle.wrapping_add(1);
        Particle {
            id: self.next_particle,
            x,
            y,
            vx: dx * speed + s.velocity[0],
            vy: dy * speed + s.velocity[1],
            age: 0.0,
            life,
            size,
            color,
            seed,
            light,
            stuck: false,
        }
    }

    /// Where a sub-emitter starts from a particle at world (`x`, `y`).
    fn sub_spawn(
        &self,
        sub: &reign_assets::particles::SubEmitter,
        p: &Particle,
        (x, y): (f32, f32),
    ) -> Pending {
        let k = sub.inherit_velocity;
        Pending {
            handle: sub.effect,
            spawn: Spawn {
                x,
                y,
                velocity: [p.vx * k, p.vy * k],
                tint: if sub.inherit_color { p.color } else { [1.0; 4] },
                seed: None,
                ..self.spawn
            },
            depth: self.depth + 1,
            follow: None,
        }
    }

    fn move_particles(
        &mut self,
        e: &ParticleEffect,
        dt: f32,
        ground: &dyn Ground,
        pending: &mut Vec<Pending>,
    ) {
        let f = &e.forces;
        let keep = (1.0 - f.drag).clamp(0.0, 1.0).powf(dt);
        let (ox, oy) = self.offset();
        let c = &e.collision;
        let floor = self.spawn.floor.unwrap_or(self.spawn.y + c.floor * self.spawn.scale);
        let mut dead = Vec::new();
        let mut hits = Vec::new();
        for (i, p) in self.particles.iter_mut().enumerate() {
            p.age += dt;
            if p.age >= p.life {
                dead.push(i);
                continue;
            }
            if p.stuck {
                continue;
            }
            p.vx = p.vx * keep + f.wind[0] * dt;
            p.vy = p.vy * keep + (f.gravity + f.wind[1]) * dt;
            let (mult, linear) = if e.velocity.enabled {
                (e.velocity.speed.at(p.age / p.life), e.velocity.linear)
            } else {
                (1.0, [0.0, 0.0])
            };
            let (mvx, mvy) = (p.vx * mult + linear[0], p.vy * mult + linear[1]);
            let (nx, ny) = (p.x + mvx * dt, p.y + mvy * dt);
            if !c.enabled {
                (p.x, p.y) = (nx, ny);
                continue;
            }
            let (wx0, wy0, wx1, wy1) = (p.x + ox, p.y + oy, nx + ox, ny + oy);
            // A surface's normal, and where it was touched.
            let hit = if mvy > 0.0 && wy0 <= floor && wy1 >= floor {
                let k = if wy1 > wy0 { (floor - wy0) / (wy1 - wy0) } else { 0.0 };
                Some((wx0 + (wx1 - wx0) * k, floor, (0.0, -1.0)))
            } else if c.world && ground.solid(wx1, wy1) && !ground.solid(wx0, wy0) {
                // Find the surface: halve the move until the last clear
                // point and the first solid one are a hair apart.
                let at = |k: f32| (wx0 + (wx1 - wx0) * k, wy0 + (wy1 - wy0) * k);
                let (mut clear, mut solid) = (0.0, 1.0);
                for _ in 0..10 {
                    let mid = (clear + solid) / 2.0;
                    let (mx, my) = at(mid);
                    if ground.solid(mx, my) { solid = mid } else { clear = mid }
                }
                let ((cx, cy), (sx, sy)) = (at(clear), at(solid));
                // A wall if moving across alone runs into it; else a floor
                // or ceiling.
                let normal = if ground.solid(sx, cy) && !ground.solid(cx, sy) {
                    (-mvx.signum(), 0.0)
                } else {
                    (0.0, -mvy.signum())
                };
                Some((cx, cy, normal))
            } else {
                None
            };
            let Some((hx, hy, (nxn, nyn))) = hit else {
                (p.x, p.y) = (nx, ny);
                continue;
            };
            (p.x, p.y) = (hx - ox, hy - oy);
            let into = -(p.vx * nxn + p.vy * nyn);
            let impact = into > RESTING;
            if impact {
                p.life -= (p.life - p.age) * c.lifetime_loss.clamp(0.0, 1.0);
                hits.push((i, (hx, hy)));
            }
            match c.mode {
                CollisionMode::Bounce => {
                    // Split into along the surface and into it.
                    let (tx, ty) = (p.vx + into * nxn, p.vy + into * nyn);
                    let keep_t = 1.0 - c.friction.clamp(0.0, 1.0);
                    let back = if impact { into * c.bounce.max(0.0) } else { 0.0 };
                    p.vx = tx * keep_t + nxn * back;
                    p.vy = ty * keep_t + nyn * back;
                }
                CollisionMode::Stick => {
                    (p.vx, p.vy) = (0.0, 0.0);
                    p.stuck = true;
                }
                CollisionMode::Die => dead.push(i),
            }
        }
        for (i, at) in hits {
            let p = self.particles[i].clone();
            for sub in e.sub_emitters.iter().filter(|s| s.trigger == SubTrigger::Collision) {
                if self.rng.unit() < sub.probability || sub.probability >= 1.0 {
                    pending.push(self.sub_spawn(sub, &p, at));
                }
            }
        }
        dead.sort_unstable();
        dead.dedup();
        for &i in dead.iter().rev() {
            let p = self.particles.remove(i);
            let at = self.world_pos(&p);
            for sub in e.sub_emitters.iter().filter(|s| s.trigger == SubTrigger::Death) {
                if self.rng.unit() < sub.probability || sub.probability >= 1.0 {
                    pending.push(self.sub_spawn(sub, &p, at));
                }
            }
        }
    }
}

/// Fast random numbers for particles (xorshift): the same seed gives the
/// same effect.
#[derive(Clone, Debug)]
struct Rng(u32);

impl Rng {
    fn new(seed: u32) -> Self {
        Rng(seed | 1)
    }

    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// From 0 up to (not including) 1.
    fn unit(&mut self) -> f32 {
        (self.next() >> 8) as f32 / (1u32 << 24) as f32
    }

    /// From 0 up to (not including) `n`; 0 for 0.
    fn below(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { self.next() % n }
    }
}

#[cfg(test)]
mod tests;
