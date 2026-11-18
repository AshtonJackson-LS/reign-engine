//! The built-in components every project has.

use reign::assets::{
    AnimationClip, AudioClip, Font, Handle, ParticleEffect, SpriteSheet, Texture, Tilemap,
};
use reign::scene::{Bounds, Component, ComponentRegistry, ObjectId, Reflect, Rgba8, Scene};
use serde::{Deserialize, Serialize};

use crate::{DB, physics};

/// Where the Game view looks: the highest-priority active camera's position is the
/// centre of the view.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct Camera {
    /// Higher priorities win. Equal priorities use the lowest object ID.
    pub priority: i32,
    /// Name of the active object to track. Empty disables follow.
    pub follow: String,
    /// Close one part of the gap each tick. Zero or one snaps immediately.
    pub smoothing: u16,
    /// Limit the centre to the rectangle between min and max.
    pub bounded: bool,
    pub min: reign::core::FxVec2,
    pub max: reign::core::FxVec2,
}

pub(crate) fn update_cameras(scene: &mut Scene) {
    use reign::core::{Fx, FxVec2};
    let moves: Vec<_> = scene
        .iter::<Camera>()
        .filter(|(_, c)| c.bounded || !c.follow.is_empty())
        .map(|(id, c)| {
            let current = scene.world_position(id);
            let target = if c.follow.is_empty() {
                current
            } else {
                scene
                    .ids()
                    .find(|&other| {
                        scene.is_active(other) && scene.name(other) == Some(c.follow.as_str())
                    })
                    .map_or(current, |other| scene.world_position(other))
            };
            let advance = |from: Fx, to: Fx, min: Fx, max: Fx| {
                let delta = i64::from(to.raw()) - i64::from(from.raw());
                let divisor = i64::from(c.smoothing.max(1));
                let step = delta.signum() * ((delta.abs() + divisor - 1) / divisor);
                let position = Fx::from_raw((i64::from(from.raw()) + step) as i32);
                if c.bounded { position.clamp(min.min(max), min.max(max)) } else { position }
            };
            (
                id,
                FxVec2::new(
                    advance(current.x, target.x, c.min.x, c.max.x),
                    advance(current.y, target.y, c.min.y, c.max.y),
                ),
            )
        })
        .collect();
    for (id, pos) in moves {
        scene.set_world_position(id, pos);
    }
}

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

/// A clip an [`Animator`] can switch to, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct NamedClip {
    /// What scripts call it: `ctx.play_animation(me, "run")`.
    pub name: String,
    pub clip: Handle<AnimationClip>,
}

/// Plays an animation clip on the object's [`SpriteRenderer`]: each frame
/// of the clip replaces the sprite that's drawn. Make clips in the
/// Animation tab (or write `*.anim.ron` by hand).
///
/// A frame's `event` arrives in `on_message` as a message with that name
/// (`msg.body.is("step")`) when the frame starts, and a clip that doesn't
/// loop sends `"AnimationFinished"` when it ends.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct Animator {
    /// The clip that's playing: the one the object starts on.
    pub clip: Handle<AnimationClip>,
    /// Clips scripts switch between by name with `ctx.play_animation`.
    pub clips: Vec<NamedClip>,
    /// Untick to hold the current frame.
    pub playing: bool,
    /// Percent of the clip's own speed: 200 plays twice as fast.
    #[reflect(range(0, 400))]
    pub speed: u16,
    /// How far into the clip it is, in hundredths of a tick. State, not a
    /// setting.
    #[serde(skip)]
    #[reflect(read_only)]
    pub time: u32,
}

impl Default for Animator {
    fn default() -> Self {
        Animator { clip: Handle::NONE, clips: Vec::new(), playing: true, speed: 100, time: 0 }
    }
}

impl Animator {
    /// Ticks into the clip.
    pub fn tick(&self) -> u32 {
        self.time / 100
    }

    /// The frame of `clip` it's showing.
    pub fn frame(&self, clip: &AnimationClip) -> u32 {
        clip.frame_at_tick(self.tick())
    }

    /// Whether `clip` has played to its end (never, for a looping clip).
    pub fn finished(&self, clip: &AnimationClip) -> bool {
        !clip.looping() && self.tick() >= u32::from(clip.ticks())
    }

    /// The clip called `name` in [`Animator::clips`].
    pub fn named(&self, name: &str) -> Option<Handle<AnimationClip>> {
        self.clips.iter().find(|c| c.name == name).map(|c| c.clip)
    }

    /// Starts `clip` from its first frame, unless it's already the one
    /// playing. Returns whether it switched.
    pub fn play(&mut self, clip: Handle<AnimationClip>) -> bool {
        let switched = self.clip != clip;
        if switched {
            self.clip = clip;
            self.time = 0;
        }
        self.playing = true;
        switched
    }
}

/// Which side of the object's position a [`Text`] starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum TextAlign {
    /// The text starts at the object's position.
    #[default]
    Left,
    /// Each line is centred on it.
    Center,
    /// Each line ends at it.
    Right,
}

/// Draws text with a bitmap font, its top at the object's position. A new
/// Text uses the built-in pixel font. Characters the font doesn't have are
/// skipped; `\n` starts a new line. Scripts change it with
/// `ctx.set_text(me, format!("Score: {score}"))`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
#[component(bounds = "Text::bounds_hint")]
pub struct Text {
    pub text: String,
    pub font: Handle<Font>,
    pub color: Rgba8,
    /// How many pixels each font pixel covers.
    #[reflect(range(1, 16))]
    pub scale: u8,
    pub align: TextAlign,
    /// Stays put on screen (a score, a health bar): the position counts
    /// from the view's top-left corner instead of the world's origin.
    pub screen: bool,
}

impl Default for Text {
    fn default() -> Self {
        Text {
            text: "Text".to_owned(),
            font: crate::font::font(),
            color: Rgba8::WHITE,
            scale: 1,
            align: TextAlign::Left,
            screen: false,
        }
    }
}

impl Text {
    /// The size it draws at, in pixels, with a font of `cell`-sized glyphs.
    pub fn size(&self, cell: [u32; 2]) -> [i32; 2] {
        let lines = self.text.split('\n').count() as u32;
        let longest = self.text.split('\n').map(|l| l.chars().count() as u32).max().unwrap_or(0);
        let scale = u32::from(self.scale.max(1));
        [(longest * cell[0] * scale) as i32, (lines * cell[1] * scale) as i32]
    }

    /// How far left of the object's position a line `width` pixels wide
    /// starts.
    pub fn align_offset(&self, width: i32) -> i32 {
        match self.align {
            TextAlign::Left => 0,
            TextAlign::Center => width / 2,
            TextAlign::Right => width,
        }
    }

    fn bounds_hint(&self) -> Option<Bounds> {
        let cell = DB
            .get()
            .and_then(|db| db.load(self.font).ok())
            .map_or(crate::font::CELL, |font| font.cell);
        let [w, h] = self.size(cell);
        (w > 0 && h > 0).then(|| Bounds::new(-self.align_offset(w), 0, w, h))
    }
}

/// A sound the object makes. With `play_on_start` it plays when the object
/// appears (a spawned explosion, a pickup); otherwise a script plays it
/// with `ctx.play(me)`. For a sound with no object, scripts call
/// `ctx.play_sound("sounds/jump.wav")`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct AudioSource {
    /// A `.wav` (16-bit PCM).
    pub clip: Handle<AudioClip>,
    /// Percent: 100 is the file's own loudness.
    #[reflect(range(0, 200))]
    pub volume: u8,
    pub play_on_start: bool,
}

impl Default for AudioSource {
    fn default() -> Self {
        AudioSource { clip: Handle::NONE, volume: 100, play_on_start: true }
    }
}

/// Plays a particle effect (`*.particles.ron`: make one with + Create >
/// Particle Effect in the Project panel and edit it in the Particles tab)
/// at the object. It follows the object as it moves and mirrors with it
/// (`flip_x`). With `play_on_start` it starts when the object appears;
/// scripts start it again with `ctx.play_particles(me)` and stop it with
/// `ctx.stop_particles(me)`. Particles are only drawn, over the scene: the
/// game itself never sees them.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct ParticleSystem {
    pub effect: Handle<ParticleEffect>,
    pub play_on_start: bool,
    /// Untick to stop emitting; the particles already out finish their
    /// lives. `ctx.play_particles` starts it again.
    pub emitting: bool,
    /// How many times scripts have started it. State, not a setting.
    #[serde(skip)]
    #[reflect(read_only)]
    pub plays: u32,
}

impl Default for ParticleSystem {
    fn default() -> Self {
        ParticleSystem { effect: Handle::NONE, play_on_start: true, emitting: true, plays: 0 }
    }
}

/// A point light at the object: it brightens the scene around it, most at the
/// middle. On a sprite with a normal map (made in the Lighting tab) it shades
/// by which way each pixel faces, as if the light hung `height` pixels in
/// front of it, and by the sprite's light mask; everything else gets the
/// plain soft disc of light. Lights add to the scene's [`AmbientLight`], so
/// they show most when that is dim. Lights are only drawn, never part of the
/// game's simulation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct Light2D {
    /// Untick to switch it off.
    pub on: bool,
    pub color: Rgba8,
    /// Above 1 overexposes into bloom.
    #[reflect(range(0, 8))]
    pub intensity: reign::core::Fx,
    /// How far the light reaches, in pixels.
    #[reflect(range(1, 1000))]
    pub radius: reign::core::Fx,
    /// How far in front of the sprites it hangs, in pixels: lower is more
    /// grazing, higher lights flat surfaces more evenly. Only matters for
    /// sprites with normal maps.
    #[reflect(range(0, 200))]
    pub height: reign::core::Fx,
}

impl Default for Light2D {
    fn default() -> Self {
        Light2D {
            on: true,
            color: Rgba8::WHITE,
            intensity: reign::core::Fx::ONE,
            radius: reign::core::Fx::from_px(96),
            height: reign::core::Fx::from_px(24),
        }
    }
}

/// How bright the scene is where no light reaches. Add one to any object
/// (the first one counts); without one the scene is fully lit, as every scene
/// always was. Lower it to darken the scene and let [`Light2D`]s show.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct AmbientLight {
    pub color: Rgba8,
    /// 1 is full brightness, 0 is black.
    #[reflect(range(0, 4))]
    pub intensity: reign::core::Fx,
}

impl Default for AmbientLight {
    fn default() -> Self {
        AmbientLight { color: Rgba8::WHITE, intensity: reign::core::Fx::from_subpx(0, 64) }
    }
}

/// One point of a [`TrailRenderer`]'s path: where the object was, and how
/// many ticks ago.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TrailPoint {
    pub at: reign::core::FxVec2,
    pub age: u32,
}

/// Draws a ribbon behind a moving object, like Unity's Trail Renderer. Every
/// tick the object's position is added to the trail; points older than
/// `time` fade away. The ribbon runs from `start_width` and `start_color`
/// at the object to `end_width` and `end_color` at the oldest point, and is
/// drawn with the object's `z`, so sprites in front of it hide it.
/// `ctx.clear_trail(me)` empties it, for a teleport.
///
/// The points are part of the game's state (whole subpixels, replayed
/// exactly), so a trail looks the same in every run and after a rewind.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct TrailRenderer {
    /// Untick to stop adding points; the trail already drawn fades out.
    pub emitting: bool,
    /// How long a point stays, in seconds.
    #[reflect(range(0, 10))]
    pub time: reign::core::Fx,
    /// How far the object moves, in pixels, before the trail gets a new point.
    #[reflect(range(0, 64))]
    pub min_vertex_distance: reign::core::Fx,
    /// Width in pixels at the object.
    #[reflect(range(0, 256))]
    pub start_width: reign::core::Fx,
    /// Width in pixels at the oldest point.
    #[reflect(range(0, 256))]
    pub end_width: reign::core::Fx,
    /// Colour at the object. Its alpha fades like the colour does.
    pub start_color: Rgba8,
    /// Colour at the oldest point.
    pub end_color: Rgba8,
    /// Adds its colour to what's under it instead of covering it: glows.
    pub additive: bool,
    /// Most points kept: the oldest go first.
    #[reflect(range(2, 512))]
    pub max_points: u16,
    /// Where the object has been, newest last. State, not a setting.
    #[serde(skip)]
    #[reflect(skip)]
    pub points: Vec<TrailPoint>,
}

impl Default for TrailRenderer {
    fn default() -> Self {
        TrailRenderer {
            emitting: true,
            time: reign::core::Fx::from_subpx(0, 128),
            min_vertex_distance: reign::core::Fx::from_px(2),
            start_width: reign::core::Fx::from_px(6),
            end_width: reign::core::Fx::ZERO,
            start_color: Rgba8::WHITE,
            end_color: Rgba8::rgba(255, 255, 255, 0),
            additive: false,
            max_points: 128,
            points: Vec::new(),
        }
    }
}

impl TrailRenderer {
    /// How many ticks a point lives.
    pub fn lifetime_ticks(&self) -> u32 {
        let ticks = i64::from(self.time.raw().max(0)) * i64::from(reign::core::TICK_HZ)
            / i64::from(reign::core::Fx::ONE.raw());
        ticks.clamp(0, i64::from(u32::MAX)) as u32
    }

    /// One tick: ages and drops points, then adds the object's position at
    /// `at` if it has moved far enough.
    pub fn step(&mut self, at: reign::core::FxVec2) {
        let lifetime = self.lifetime_ticks();
        for p in &mut self.points {
            p.age = p.age.saturating_add(1);
        }
        self.points.retain(|p| p.age <= lifetime);
        if self.emitting {
            let far_enough = self.points.last().is_none_or(|last| {
                let (dx, dy) = (
                    i64::from(at.x.raw()) - i64::from(last.at.x.raw()),
                    i64::from(at.y.raw()) - i64::from(last.at.y.raw()),
                );
                let min = i64::from(self.min_vertex_distance.raw().max(1));
                dx * dx + dy * dy >= min * min
            });
            if far_enough {
                self.points.push(TrailPoint { at, age: 0 });
            }
        }
        let cap = usize::from(self.max_points.max(2));
        if self.points.len() > cap {
            self.points.drain(..self.points.len() - cap);
        }
    }
}

/// The built-in components, before a project's own.
pub fn register_builtins(r: &mut ComponentRegistry) {
    r.register::<Camera>()
        .register::<SpriteRenderer>()
        .register::<Image>()
        .register::<TilemapRenderer>()
        .register::<Animator>()
        .register::<Text>()
        .register::<AudioSource>()
        .register::<ParticleSystem>()
        .register::<TrailRenderer>()
        .register::<Light2D>()
        .register::<AmbientLight>()
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
