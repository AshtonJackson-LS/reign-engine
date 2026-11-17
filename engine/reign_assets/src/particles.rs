//! Particle effects as data: what a `*.particles.ron` file holds.
//!
//! Modelled on Unity's Particle System. The [`Main`] module says how long
//! one cycle lasts, whether it loops and what each particle starts with
//! (lifetime, speed, size, colour). The other modules each switch on with
//! their `enabled` flag:
//!
//! - [`Emission`]: particles per second and timed bursts.
//! - [`Shape`]: where particles are born and which way they head.
//! - [`Forces`]: gravity, drag and wind (always on; zero does nothing).
//! - [`VelocityOverLife`], [`ColorOverLife`], [`SizeOverLife`]: curves
//!   over each particle's life, from 0 (born) to 1 (gone).
//! - [`Collision`]: a floor (and the level, where the game gives one) that
//!   particles bounce off, stick to or die on.
//! - [`SubEmitter`]s: other effects started where a particle is born, hits
//!   something or dies.
//! - [`Lights`]: lights that follow some of the particles.
//! - [`Render`]: squares, streaks, soft glows or sprites, blended normally
//!   or added as light.
//!
//! Units are world pixels, seconds and degrees (0 points right, -90 up,
//! the way screen y grows down). `reign_particles` runs and draws effects.
//! Particles are presentation only: the simulation never sees them, so an
//! effect can't change what a replay or an online game does.

use serde::{Deserialize, Serialize};

use crate::{Asset, AssetError, AssetKind, Handle, Meta, SpriteSheet};

/// A particle effect: a `*.particles.ron` file. Every field has a default,
/// so a file only needs what it changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParticleEffect {
    pub main: Main,
    pub emission: Emission,
    pub shape: Shape,
    pub forces: Forces,
    pub velocity: VelocityOverLife,
    pub color: ColorOverLife,
    pub size: SizeOverLife,
    pub collision: Collision,
    pub sub_emitters: Vec<SubEmitter>,
    pub lights: Lights,
    pub render: Render,
}

/// A random value between two ends, picked once per particle. Equal ends
/// give a constant.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Range(pub f32, pub f32);

impl Range {
    /// The value a fraction `t` (0 to 1) of the way from the first end to
    /// the second.
    pub fn lerp(self, t: f32) -> f32 {
        self.0 + (self.1 - self.0) * t
    }
}

/// A random whole number from the first to the second, inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CountRange(pub u32, pub u32);

/// A colour as red, green, blue and alpha, 0 to 255 (sRGB, as picked).
pub type Rgba = [u8; 4];

/// A random colour between two, picked once per particle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorRange(pub Rgba, pub Rgba);

/// What every particle starts with, and how one cycle of the effect runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Main {
    /// Seconds in one cycle: emission and bursts run over this long.
    pub duration: f32,
    /// Starts another cycle when one ends, until stopped.
    pub looping: bool,
    /// Seconds before anything is emitted.
    pub start_delay: f32,
    /// How long each particle lives, in seconds.
    pub lifetime: Range,
    /// How fast each particle starts, in pixels a second.
    pub speed: Range,
    /// How big each particle starts, in pixels (for sprites, a scale: 1 is
    /// the sprite's own size).
    pub size: Range,
    /// Each particle's colour, somewhere between the two.
    pub color: ColorRange,
    /// The most particles alive at once; more aren't emitted.
    pub max_particles: u32,
    /// Whether particles move with the emitter after they're born.
    pub space: Space,
}

impl Default for Main {
    fn default() -> Self {
        Main {
            duration: 1.0,
            looping: true,
            start_delay: 0.0,
            lifetime: Range(0.8, 1.2),
            speed: Range(30.0, 50.0),
            size: Range(1.0, 2.0),
            color: ColorRange([255, 255, 255, 255], [255, 255, 255, 255]),
            max_particles: 500,
            space: Space::World,
        }
    }
}

/// Where particles live once they're born.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Space {
    /// They stay where they were born when the emitter moves: smoke left
    /// behind, blood sprayed from a moving body.
    #[default]
    World,
    /// They move with the emitter: an aura around a character.
    Local,
}

/// How many particles are born, and when.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Emission {
    pub enabled: bool,
    /// Particles a second, spread evenly over each cycle.
    pub rate: f32,
    /// Particles born all at once at set times.
    pub bursts: Vec<Burst>,
}

impl Default for Emission {
    fn default() -> Self {
        Emission { enabled: true, rate: 20.0, bursts: Vec::new() }
    }
}

/// A group of particles born together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Burst {
    /// Seconds into the cycle.
    pub time: f32,
    /// How many, picked each time it fires.
    pub count: CountRange,
    /// How many times it fires in a cycle, `interval` seconds apart.
    pub cycles: u32,
    pub interval: f32,
    /// The chance (0 to 1) that it fires at all, each time.
    pub probability: f32,
}

impl Default for Burst {
    fn default() -> Self {
        Burst { time: 0.0, count: CountRange(10, 10), cycles: 1, interval: 0.1, probability: 1.0 }
    }
}

/// Where particles are born, around the emitter, and which way they go.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shape {
    pub kind: ShapeKind,
    /// The direction particles head, in degrees: 0 right, 90 down, -90 up.
    /// The emitter can turn the whole effect further when it's played.
    pub angle: f32,
    /// How far either side of `angle` they can head, in degrees, all told:
    /// 0 is a straight line, 360 every way.
    pub spread: f32,
    /// The size of a cone's mouth (across), or a circle's radius, in pixels.
    pub radius: f32,
    /// A box's width and height, or a line's length (width), in pixels.
    pub size: [f32; 2],
    /// Circles: born on the edge only, not anywhere inside.
    pub edge: bool,
    /// Squashes the shape and the directions up and down: 1 keeps them,
    /// 0.3 flattens a circle into a ring on the ground.
    pub squash: f32,
    /// Mixes each direction with a random one: 0 not at all, 1 fully.
    pub randomize: f32,
    /// Moves the whole shape from the emitter, in pixels: x, y (turned and
    /// mirrored with the effect).
    pub offset: [f32; 2],
}

impl Default for Shape {
    fn default() -> Self {
        Shape {
            kind: ShapeKind::Cone,
            angle: -90.0,
            spread: 30.0,
            radius: 0.0,
            size: [16.0, 16.0],
            edge: false,
            squash: 1.0,
            randomize: 0.0,
            offset: [0.0, 0.0],
        }
    }
}

/// The shape particles are born in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// All at the emitter, heading within the spread.
    Point,
    /// Across a mouth `radius` wide, square to the angle, heading within
    /// the spread.
    #[default]
    Cone,
    /// Inside (or on the edge of) a circle, heading outward from its
    /// middle; the spread limits which part of the circle.
    Circle,
    /// Anywhere in a box, heading within the spread.
    Box,
    /// Along a line `size[0]` long, square to the angle, heading within the
    /// spread.
    Line,
}

/// Pushes on every particle all its life.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Forces {
    /// Pixels a second, each second, downward (negative floats up).
    pub gravity: f32,
    /// The fraction of its speed a particle loses each second, 0 to 1.
    pub drag: f32,
    /// A steady push, in pixels a second each second: x, y.
    pub wind: [f32; 2],
}

/// Speeds particles up or slows them down over their life.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VelocityOverLife {
    pub enabled: bool,
    /// How much of a particle's own speed it moves at, over its life.
    pub speed: Curve,
    /// Added on top, in pixels a second: x, y.
    pub linear: [f32; 2],
}

impl Default for VelocityOverLife {
    fn default() -> Self {
        VelocityOverLife { enabled: false, speed: Curve::constant(1.0), linear: [0.0, 0.0] }
    }
}

/// Tints particles over their life (multiplied with their own colour).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorOverLife {
    pub enabled: bool,
    pub gradient: Gradient,
}

impl Default for ColorOverLife {
    fn default() -> Self {
        ColorOverLife {
            enabled: false,
            gradient: Gradient(vec![
                ColorKey { t: 0.0, color: [255, 255, 255, 255] },
                ColorKey { t: 1.0, color: [255, 255, 255, 0] },
            ]),
        }
    }
}

/// Grows or shrinks particles over their life (multiplied with their own
/// size).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SizeOverLife {
    pub enabled: bool,
    pub curve: Curve,
}

impl Default for SizeOverLife {
    fn default() -> Self {
        SizeOverLife { enabled: false, curve: Curve(vec![(0.0, 1.0), (1.0, 0.0)]) }
    }
}

/// Values over a particle's life: (life from 0 to 1, value) keys, joined
/// by straight lines. Before the first key and after the last, their
/// values hold. No keys is 1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Curve(pub Vec<(f32, f32)>);

impl Default for Curve {
    fn default() -> Self {
        Curve::constant(1.0)
    }
}

impl Curve {
    pub fn constant(value: f32) -> Self {
        Curve(vec![(0.0, value)])
    }

    /// The value at `t` (0 to 1).
    pub fn at(&self, t: f32) -> f32 {
        let keys = &self.0;
        let Some(&(first_t, first)) = keys.first() else { return 1.0 };
        if t <= first_t {
            return first;
        }
        for pair in keys.windows(2) {
            let ((t0, v0), (t1, v1)) = (pair[0], pair[1]);
            if t <= t1 {
                let span = t1 - t0;
                return if span <= 0.0 { v1 } else { v0 + (v1 - v0) * (t - t0) / span };
            }
        }
        keys.last().map_or(1.0, |k| k.1)
    }
}

/// One colour of a [`Gradient`], at life `t` (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorKey {
    pub t: f32,
    pub color: Rgba,
}

/// Colours over a particle's life, blended between keys. No keys is white.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Gradient(pub Vec<ColorKey>);

impl Gradient {
    /// The colour at `t` (0 to 1), as red, green, blue and alpha from 0 to
    /// 1.
    pub fn at(&self, t: f32) -> [f32; 4] {
        let unit = |c: Rgba| c.map(|v| v as f32 / 255.0);
        let keys = &self.0;
        let Some(first) = keys.first() else { return [1.0; 4] };
        if t <= first.t {
            return unit(first.color);
        }
        for pair in keys.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t <= b.t {
                let span = b.t - a.t;
                let k = if span <= 0.0 { 1.0 } else { (t - a.t) / span };
                let (a, b) = (unit(a.color), unit(b.color));
                return [0, 1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * k);
            }
        }
        keys.last().map_or([1.0; 4], |k| unit(k.color))
    }
}

/// Lets particles hit a floor, and the level where the game gives one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Collision {
    pub enabled: bool,
    pub mode: CollisionMode,
    /// A floor this many pixels below the emitter, unless the game playing
    /// the effect says where the floor is.
    pub floor: f32,
    /// Also hit the level's solid ground and walls, where the game playing
    /// the effect gives them.
    pub world: bool,
    /// How much speed a bounce keeps, square to what was hit: 0 to 1.
    pub bounce: f32,
    /// How much speed along what was hit each bounce loses: 0 to 1.
    pub friction: f32,
    /// How much of its remaining life a particle loses each hit: 0 to 1.
    pub lifetime_loss: f32,
}

impl Default for Collision {
    fn default() -> Self {
        Collision {
            enabled: false,
            mode: CollisionMode::Bounce,
            floor: 16.0,
            world: true,
            bounce: 0.3,
            friction: 0.3,
            lifetime_loss: 0.0,
        }
    }
}

/// What a particle does when it hits something.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollisionMode {
    #[default]
    Bounce,
    /// Stops where it hit and stays for the rest of its life: splats,
    /// stains.
    Stick,
    /// Disappears.
    Die,
}

/// Another effect started by each particle, where the particle is. It's
/// mirrored and scaled like its parent but not turned, so an effect started
/// where a drop lands stays upright on the ground.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubEmitter {
    pub trigger: SubTrigger,
    pub effect: Handle<ParticleEffect>,
    /// The chance (0 to 1) that a particle starts it.
    pub probability: f32,
    /// How much of the particle's velocity the new effect's particles get:
    /// 0 to 1.
    pub inherit_velocity: f32,
    /// The new effect's particles are tinted with the particle's colour.
    pub inherit_color: bool,
}

impl Default for SubEmitter {
    fn default() -> Self {
        SubEmitter {
            trigger: SubTrigger::Death,
            effect: Handle::NONE,
            probability: 1.0,
            inherit_velocity: 0.0,
            inherit_color: false,
        }
    }
}

/// When a sub-emitter starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubTrigger {
    /// When the particle is born; it follows the particle (a trail) until
    /// the particle dies.
    Birth,
    /// Each time the particle hits something.
    Collision,
    /// When the particle's life ends.
    #[default]
    Death,
}

/// Lights that follow particles: glowing embers, sparks lighting a wall.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lights {
    pub enabled: bool,
    /// The fraction of particles (0 to 1) that carry a light.
    pub ratio: f32,
    /// In pixels.
    pub radius: f32,
    pub intensity: f32,
    /// Lit in each particle's own colour, rather than `color`.
    pub use_particle_color: bool,
    pub color: Rgba,
}

impl Default for Lights {
    fn default() -> Self {
        Lights {
            enabled: false,
            ratio: 0.2,
            radius: 32.0,
            intensity: 1.0,
            use_particle_color: true,
            color: [255, 255, 255, 255],
        }
    }
}

/// How particles are drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Render {
    pub mode: RenderMode,
    pub blend: BlendMode,
    /// Squares: width over height (3 draws a flat splat three times as wide
    /// as it's tall).
    pub aspect: f32,
    /// Streaks: how many seconds of travel each one shows.
    pub streak: f32,
    /// Streaks: the longest one, in pixels.
    pub max_streak: f32,
    /// Sprites: the sheet they come from.
    pub sheet: Handle<SpriteSheet>,
    /// Sprites: the sprites to use, by name, in order. Empty uses them all.
    pub sprites: Vec<String>,
    /// Sprites: how a particle picks which one it shows.
    pub frames: FrameMode,
    /// Sprites with [`FrameMode::Fps`]: sprites a second.
    pub fps: f32,
    /// Draws on whole pixels, so particles stay as crisp as the art.
    pub pixel_snap: bool,
    /// Which effects draw first, like Unity's Order in Layer: lower first.
    /// A game can draw a range of orders at a time, such as stains on the
    /// ground (below 0) behind its characters and the rest in front.
    pub order: i32,
}

impl Default for Render {
    fn default() -> Self {
        Render {
            mode: RenderMode::Square,
            blend: BlendMode::Alpha,
            aspect: 1.0,
            streak: 0.012,
            max_streak: 6.0,
            sheet: Handle::NONE,
            sprites: Vec::new(),
            frames: FrameMode::OverLife,
            fps: 12.0,
            pixel_snap: true,
            order: 0,
        }
    }
}

/// What each particle looks like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderMode {
    /// A solid square (or rectangle, with `aspect`).
    #[default]
    Square,
    /// A line along the way it's moving, longer the faster it goes.
    Streak,
    /// A soft round glow.
    Glow,
    /// A sprite from a sheet.
    Sprite,
}

/// How particles combine with what's under them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlendMode {
    /// Covers what's under it by its alpha.
    #[default]
    Alpha,
    /// Adds its colour, like light: sparks, fire, magic.
    Add,
}

/// How a sprite particle picks its sprite.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameMode {
    /// Plays through the sprites once over its life.
    #[default]
    OverLife,
    /// One sprite, picked at random when it's born.
    Random,
    /// Plays through them at `fps`, round and round.
    Fps,
}

impl Asset for ParticleEffect {
    const KIND: AssetKind = AssetKind::ParticleEffect;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        let text = std::str::from_utf8(bytes).map_err(|e| AssetError(e.to_string()))?;
        let effect: ParticleEffect = ron::from_str(text).map_err(|e| AssetError(e.to_string()))?;
        match effect.problems().into_iter().next() {
            Some(problem) => Err(AssetError(problem)),
            None => Ok(effect),
        }
    }
}

impl ParticleEffect {
    /// The file as RON, ready to save.
    pub fn to_ron(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new()
            .indentor("    ")
            .struct_names(false)
            .depth_limit(3)
            .separate_tuple_members(false);
        let mut text = ron::ser::to_string_pretty(self, pretty).expect("effects serialize");
        text.push('\n');
        text
    }

    /// What's wrong with the effect, in words for the person who made it:
    /// empty when it can be played. Numbers that would make it misbehave
    /// (negative lifetimes, a zero-length looping cycle, not-a-number) are
    /// refused rather than quietly fixed.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut finite = |what: &str, values: &[f32]| {
            if values.iter().any(|v| !v.is_finite()) {
                out.push(format!("{what} must be a number."));
            }
        };
        let m = &self.main;
        finite("The duration", &[m.duration, m.start_delay]);
        finite("The lifetime", &[m.lifetime.0, m.lifetime.1]);
        finite("The start speed", &[m.speed.0, m.speed.1]);
        finite("The start size", &[m.size.0, m.size.1]);
        finite("The emission rate", &[self.emission.rate]);
        let s = &self.shape;
        finite(
            "The shape",
            &[
                s.angle,
                s.spread,
                s.radius,
                s.size[0],
                s.size[1],
                s.squash,
                s.offset[0],
                s.offset[1],
            ],
        );
        let f = &self.forces;
        finite("The forces", &[f.gravity, f.drag, f.wind[0], f.wind[1]]);
        if m.duration <= 0.0 {
            out.push("The duration must be more than 0 seconds.".into());
        }
        if m.start_delay < 0.0 {
            out.push("The start delay can't be negative.".into());
        }
        if m.lifetime.0 <= 0.0 || m.lifetime.1 <= 0.0 {
            out.push("A particle's lifetime must be more than 0 seconds.".into());
        }
        if m.size.0 < 0.0 || m.size.1 < 0.0 {
            out.push("A particle's size can't be negative.".into());
        }
        if self.emission.rate < 0.0 {
            out.push("The emission rate can't be negative.".into());
        }
        for b in &self.emission.bursts {
            if !b.time.is_finite() || b.time < 0.0 || !b.interval.is_finite() || b.interval < 0.0 {
                out.push("A burst's time and interval can't be negative.".into());
            }
            if b.count.0 > b.count.1 {
                out.push(format!(
                    "A burst's count goes from {} down to {}: put the smaller first.",
                    b.count.0, b.count.1
                ));
            }
        }
        if !(0.0..=1.0).contains(&f.drag) {
            out.push("Drag must be from 0 to 1.".into());
        }
        if self.render.mode == RenderMode::Sprite && self.render.sheet.is_none() {
            out.push("Sprite particles need a sprite sheet.".into());
        }
        if !self.render.aspect.is_finite() || self.render.aspect <= 0.0 {
            out.push("The aspect must be more than 0.".into());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_default_effect() {
        let effect: ParticleEffect = ron::from_str("()").unwrap();
        assert_eq!(effect, ParticleEffect::default());
        assert!(effect.problems().is_empty(), "{:?}", effect.problems());
    }

    #[test]
    fn effects_save_and_load_back() {
        let effect = ParticleEffect {
            main: Main { looping: false, lifetime: Range(0.2, 0.4), ..Main::default() },
            emission: Emission {
                rate: 0.0,
                bursts: vec![Burst { count: CountRange(20, 30), ..Burst::default() }],
                ..Emission::default()
            },
            collision: Collision {
                enabled: true,
                mode: CollisionMode::Stick,
                ..Collision::default()
            },
            sub_emitters: vec![SubEmitter {
                trigger: SubTrigger::Collision,
                effect: Handle::new(crate::Guid(42)),
                ..SubEmitter::default()
            }],
            ..ParticleEffect::default()
        };
        let text = effect.to_ron();
        let back = ParticleEffect::load(text.as_bytes(), &Meta::new_for("a.particles.ron"));
        assert_eq!(back, Ok(effect), "{text}");
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let text = "(main: (looping: false), forces: (gravity: 300.0))";
        let effect: ParticleEffect = ron::from_str(text).unwrap();
        assert!(!effect.main.looping);
        assert_eq!(effect.main.duration, 1.0);
        assert_eq!(effect.forces.gravity, 300.0);
        assert_eq!(effect.forces.drag, 0.0);
        assert_eq!(effect.render, Render::default());
    }

    #[test]
    fn bad_effects_are_refused_and_say_why() {
        let meta = Meta::new_for("a.particles.ron");
        let cases = [
            ("(main: (duration: 0.0))", "duration"),
            ("(main: (lifetime: (0.0, 1.0)))", "lifetime"),
            ("(main: (start_delay: -1.0))", "delay"),
            ("(emission: (rate: -5.0))", "rate"),
            ("(emission: (bursts: [(count: (9, 3))]))", "smaller first"),
            ("(forces: (drag: 2.0))", "Drag"),
            ("(render: (mode: Sprite))", "sprite sheet"),
            ("(render: (aspect: 0.0))", "aspect"),
            ("(main: (duration: inf))", "number"),
            ("(main: (lifetime: oops))", ""),
        ];
        for (text, words) in cases {
            let error = ParticleEffect::load(text.as_bytes(), &meta).unwrap_err();
            assert!(error.0.contains(words), "{text}: {error}");
        }
    }

    #[test]
    fn curves_join_their_keys_with_straight_lines() {
        let c = Curve(vec![(0.0, 0.0), (0.5, 1.0), (1.0, 0.5)]);
        assert_eq!(c.at(-1.0), 0.0);
        assert_eq!(c.at(0.25), 0.5);
        assert_eq!(c.at(0.5), 1.0);
        assert_eq!(c.at(0.75), 0.75);
        assert_eq!(c.at(2.0), 0.5);
        assert_eq!(Curve(vec![]).at(0.3), 1.0);
        assert_eq!(Curve::constant(2.0).at(0.9), 2.0);
        // Two keys at the same time: a step, not a division by zero.
        assert_eq!(Curve(vec![(0.0, 0.0), (0.5, 0.0), (0.5, 1.0)]).at(0.5), 0.0);
        assert_eq!(Curve(vec![(0.0, 0.0), (0.5, 0.0), (0.5, 1.0)]).at(0.6), 1.0);
    }

    #[test]
    fn gradients_blend_between_keys() {
        let g = Gradient(vec![
            ColorKey { t: 0.0, color: [255, 0, 0, 255] },
            ColorKey { t: 1.0, color: [0, 0, 255, 0] },
        ]);
        assert_eq!(g.at(0.0), [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(g.at(0.5), [0.5, 0.0, 0.5, 0.5]);
        assert_eq!(g.at(1.5), [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(Gradient::default().at(0.5), [1.0; 4]);
    }
}
