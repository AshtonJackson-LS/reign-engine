//! The built-in components every project has.

use reign::assets::{AnimationClip, AudioClip, Font, Handle, SpriteSheet, Texture, Tilemap};
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

/// The built-in components, before a project's own.
pub fn register_builtins(r: &mut ComponentRegistry) {
    r.register::<Camera>()
        .register::<SpriteRenderer>()
        .register::<Image>()
        .register::<TilemapRenderer>()
        .register::<Animator>()
        .register::<Text>()
        .register::<AudioSource>()
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
