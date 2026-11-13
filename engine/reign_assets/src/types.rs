//! The asset types Reign knows how to load.

use std::sync::Arc;

use reign_core::animation::Clip;
use reign_physics::TileMap;
use serde::{Deserialize, Serialize};

use crate::{Asset, AssetError, AssetKind, Handle, Importer, Meta, Reflect, RgbaImage, decode_png};

/// A decoded image and how dense its pixels are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Texture {
    pub image: RgbaImage,
    /// Texels per world pixel.
    pub density: u8,
}

impl Asset for Texture {
    const KIND: AssetKind = AssetKind::Texture;
    fn load(bytes: &[u8], meta: &Meta) -> Result<Self, AssetError> {
        let image = decode_png(bytes).map_err(|e| AssetError(e.to_string()))?;
        let density = match &meta.importer {
            Importer::Texture(t) => t.density.max(1),
            _ => 1,
        };
        Ok(Texture { image, density })
    }
}

/// A named rectangle in a sprite sheet's texture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Sprite {
    pub name: String,
    /// x, y, width, height in texels.
    pub rect: [u32; 4],
    /// The point that sits on the object's position, from the rect's
    /// top-left, in texels.
    pub pivot: [i32; 2],
}

/// A regular grid of cells, row by row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Grid {
    /// Cell width and height in texels.
    pub cell: [u32; 2],
    /// Where the first cell starts.
    pub offset: [u32; 2],
    /// Gap between cells.
    pub padding: [u32; 2],
    pub columns: u32,
    pub rows: u32,
}

impl Grid {
    /// The rect of the cell at `column`, `row`.
    pub fn cell_rect(&self, column: u32, row: u32) -> [u32; 4] {
        let [w, h] = self.cell;
        let x = self.offset[0] + column * (w + self.padding[0]);
        let y = self.offset[1] + row * (h + self.padding[1]);
        [x, y, w, h]
    }
}

/// Sprites cut from a texture: named rects with pivots, a grid of cells,
/// or both.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct SpriteSheet {
    pub texture: Handle<Texture>,
    /// The same layout drawn facing the other way, for sheets that come in
    /// left and right versions.
    pub mirrored: Handle<Texture>,
    /// Texels per world pixel.
    #[reflect(range(1, 4))]
    pub density: u8,
    /// Cells for animation rows, if the sheet is a grid.
    pub grid: Option<Grid>,
    /// The pivot every grid cell uses, from the cell's top-left.
    pub cell_pivot: [i32; 2],
    pub sprites: Vec<Sprite>,
}

impl SpriteSheet {
    pub fn sprite(&self, name: &str) -> Option<&Sprite> {
        self.sprites.iter().find(|s| s.name == name)
    }
}

impl Asset for SpriteSheet {
    const KIND: AssetKind = AssetKind::SpriteSheet;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        ron_asset(bytes)
    }
}

/// How a clip ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum LoopMode {
    /// Holds the last frame.
    #[default]
    Once,
    /// Starts over.
    Loop,
}

/// One frame of a clip.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipFrame {
    /// The cell in the clip's row, or the sprite index without a grid.
    pub cell: u32,
    /// How long the frame shows, in milliseconds. Sampled on the 120 Hz
    /// tick: the frame changes on the first tick at or after its start.
    pub ms: u16,
    /// A gameplay hit is live on this frame.
    #[serde(skip_serializing_if = "is_default")]
    pub hit: bool,
    /// A named event fired when the frame starts (`"gas"`, `"step"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    /// Presentation only: a camera shake in world pixels, and a flash and
    /// dim in 0..1. Never fed into the simulation.
    #[serde(skip_serializing_if = "is_default")]
    pub shake: (i32, i32),
    #[serde(skip_serializing_if = "is_default")]
    pub flash: f32,
    #[serde(skip_serializing_if = "is_default")]
    pub dim: f32,
}

impl std::hash::Hash for ClipFrame {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (self.cell, self.ms, self.hit, &self.event, self.shake).hash(state);
        (self.flash.to_bits(), self.dim.to_bits()).hash(state);
    }
}

/// A sequence of frames from a sprite sheet, with their timing.
#[derive(Clone, Debug, Default, PartialEq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct AnimationClip {
    pub sheet: Handle<SpriteSheet>,
    /// The sheet row the frames are in.
    pub row: u32,
    pub mode: LoopMode,
    pub frames: Vec<ClipFrame>,
    /// Frames from which the move can be cancelled into another, if any.
    pub cancel_from: Option<u32>,
}

impl AnimationClip {
    pub fn looping(&self) -> bool {
        self.mode == LoopMode::Loop
    }

    pub fn durations(&self) -> Vec<u16> {
        self.frames.iter().map(|f| f.ms).collect()
    }

    /// The frame shown `tick` ticks in, exactly as [`Clip::frame`] samples.
    pub fn frame_at_tick(&self, tick: u32) -> u32 {
        if self.frames.is_empty() {
            return 0;
        }
        self.to_clip_with(&self.durations()).frame(tick)
    }

    /// Ticks until the clip ends.
    pub fn ticks(&self) -> u16 {
        self.to_clip_with(&self.durations()).ticks()
    }

    /// The engine's timing view of this clip. The frame durations are
    /// leaked to get the `'static` slice `Clip` holds, so build it once per
    /// load, not per tick.
    pub fn to_clip(&self) -> Clip {
        let ms: &'static [u16] = Box::leak(self.durations().into_boxed_slice());
        Clip { row: self.row, ms, looping: self.looping() }
    }

    fn to_clip_with<'a>(&self, ms: &'a [u16]) -> ClipRef<'a> {
        ClipRef { ms, looping: self.looping() }
    }
}

/// [`Clip`]'s sampling, over a borrowed slice.
struct ClipRef<'a> {
    ms: &'a [u16],
    looping: bool,
}

impl ClipRef<'_> {
    fn frame(&self, tick: u32) -> u32 {
        let total: u64 = self.ms.iter().map(|&ms| ms as u64 * reign_core::TICK_HZ as u64).sum();
        let mut time = tick as u64 * 1000;
        if self.looping && total > 0 {
            time %= total;
        }
        let mut end = 0;
        for (i, &ms) in self.ms.iter().enumerate() {
            end += ms as u64 * reign_core::TICK_HZ as u64;
            if time < end {
                return i as u32;
            }
        }
        self.ms.len() as u32 - 1
    }

    fn ticks(&self) -> u16 {
        let ms: u32 = self.ms.iter().map(|&m| m as u32).sum();
        (ms * reign_core::TICK_HZ).div_ceil(1000) as u16
    }
}

impl Asset for AnimationClip {
    const KIND: AssetKind = AssetKind::AnimationClip;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        ron_asset(bytes)
    }
}

/// Square tiles cut from a texture, numbered row by row from 0.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Tileset {
    pub texture: Handle<Texture>,
    /// Tile width and height in texels.
    pub tile_size: u32,
    pub columns: u32,
    pub count: u32,
}

impl Asset for Tileset {
    const KIND: AssetKind = AssetKind::Tileset;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        ron_asset(bytes)
    }
}

/// A layer of tiles from the tileset: one row of text per map row, two
/// characters per cell (`..` for empty, otherwise the tile index in base
/// 36, `00` to `zz`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct TileLayer {
    pub name: String,
    pub visible: bool,
    pub rows: Vec<String>,
}

impl TileLayer {
    pub fn empty(name: &str, width: usize, height: usize) -> Self {
        TileLayer { name: name.into(), visible: true, rows: vec![".".repeat(width * 2); height] }
    }

    pub fn get(&self, x: usize, y: usize) -> Option<u32> {
        let cell = self.rows.get(y)?.get(x * 2..x * 2 + 2)?;
        if cell == ".." { None } else { u32::from_str_radix(cell, 36).ok() }
    }

    pub fn set(&mut self, x: usize, y: usize, tile: Option<u32>) {
        let Some(row) = self.rows.get_mut(y) else { return };
        if x * 2 + 2 > row.len() {
            return;
        }
        let text = match tile {
            Some(t) => format!("{:0>2}", to_base36(t.min(36 * 36 - 1))),
            None => "..".into(),
        };
        row.replace_range(x * 2..x * 2 + 2, &text);
    }
}

fn to_base36(mut n: u32) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// A level map: collision, entity markers and visual tile layers, all on
/// the same 16-pixel grid.
///
/// `collision` uses the physics glyphs, one character per cell: `#`
/// solid, `-` one-way, `/` and `\` slopes, `.` empty. `markers` places
/// things the game spawns from the map (one character per cell, `.` for
/// none). Both are simulation data; the visual layers are not.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Tilemap {
    pub tileset: Handle<Tileset>,
    pub collision: Vec<String>,
    pub markers: Vec<String>,
    pub layers: Vec<TileLayer>,
}

impl Tilemap {
    /// From an ASCII level: tiles stay in `collision`, every other
    /// non-space glyph goes to `markers`.
    pub fn from_ascii(text: &str) -> Tilemap {
        let tile = |c: char| matches!(c, '#' | '-' | '/' | '\\');
        let rows: Vec<&str> = text.lines().collect();
        let collision = rows
            .iter()
            .map(|r| r.chars().map(|c| if tile(c) { c } else { '.' }).collect())
            .collect();
        let markers = rows
            .iter()
            .map(|r| r.chars().map(|c| if tile(c) || c == ' ' { '.' } else { c }).collect())
            .collect();
        Tilemap { tileset: Handle::NONE, collision, markers, layers: Vec::new() }
    }

    pub fn width(&self) -> usize {
        self.collision.iter().map(|r| r.chars().count()).max().unwrap_or(0)
    }

    pub fn height(&self) -> usize {
        self.collision.len()
    }

    /// The physics map for the collision layer.
    pub fn collision_map(&self) -> TileMap {
        let rows: Vec<&str> = self.collision.iter().map(String::as_str).collect();
        TileMap::from_ascii(&rows)
    }

    /// The collision and markers merged back into one ASCII map, markers
    /// over empty cells: the form [`Tilemap::from_ascii`] reads.
    pub fn to_ascii(&self) -> String {
        let mut out = String::new();
        for (y, row) in self.collision.iter().enumerate() {
            let marks: Vec<char> = self.markers.get(y).map_or(Vec::new(), |m| m.chars().collect());
            for (x, c) in row.chars().enumerate() {
                let m = marks.get(x).copied().unwrap_or('.');
                out.push(if c == '.' && m != '.' { m } else { c });
            }
            out.push('\n');
        }
        out
    }

    /// The glyph at a cell of a character layer.
    pub fn glyph(rows: &[String], x: usize, y: usize) -> char {
        rows.get(y).and_then(|r| r.chars().nth(x)).unwrap_or('.')
    }

    /// Sets a cell of a character layer, growing nothing.
    pub fn set_glyph(rows: &mut [String], x: usize, y: usize, c: char) {
        let Some(row) = rows.get_mut(y) else { return };
        let mut chars: Vec<char> = row.chars().collect();
        if let Some(cell) = chars.get_mut(x) {
            *cell = c;
            *row = chars.into_iter().collect();
        }
    }
}

impl Asset for Tilemap {
    const KIND: AssetKind = AssetKind::Tilemap;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        ron_asset(bytes)
    }
}

/// A sound: PCM16 WAV bytes, as the mixer reads them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioClip {
    pub wav: Arc<[u8]>,
    pub looping: bool,
}

impl Asset for AudioClip {
    const KIND: AssetKind = AssetKind::AudioClip;
    fn load(bytes: &[u8], meta: &Meta) -> Result<Self, AssetError> {
        let looping = matches!(&meta.importer, Importer::Audio(a) if a.looping);
        Ok(AudioClip { wav: bytes.into(), looping })
    }
}

/// Long music, streamed. The same bytes as an [`AudioClip`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Music {
    pub wav: Arc<[u8]>,
    pub looping: bool,
}

impl Asset for Music {
    const KIND: AssetKind = AssetKind::Music;
    fn load(bytes: &[u8], meta: &Meta) -> Result<Self, AssetError> {
        let looping = matches!(&meta.importer, Importer::Audio(a) if a.looping);
        Ok(Music { wav: bytes.into(), looping })
    }
}

/// A bitmap font: fixed cells of ASCII in a texture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct Font {
    pub texture: Handle<Texture>,
    /// Cell width and height in texels.
    pub cell: [u32; 2],
    /// Cells per atlas row.
    pub columns: u32,
    /// The first and last characters in the atlas, in order.
    pub first: u8,
    pub last: u8,
    /// Advance for a space, in texels.
    pub space: u32,
}

impl Asset for Font {
    const KIND: AssetKind = AssetKind::Font;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        ron_asset(bytes)
    }
}

/// A scene file's text. Loading it into a scene needs the game's component
/// registry: `reign_scene::Scene::from_ron(&asset.text, &registry)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SceneAsset {
    pub text: String,
}

impl Asset for SceneAsset {
    const KIND: AssetKind = AssetKind::Scene;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        let text = std::str::from_utf8(bytes).map_err(|e| AssetError(e.to_string()))?;
        Ok(SceneAsset { text: text.to_owned() })
    }
}

/// Combat frame data: named moves with timing, hitboxes per frame,
/// damage, knockback, hitstop and shake (`*.moves.ron`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct MoveSet {
    pub file: reign_combat::MoveSetFile,
}

impl Asset for MoveSet {
    const KIND: AssetKind = AssetKind::MoveSet;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        Ok(MoveSet { file: ron_asset(bytes)? })
    }
}

impl MoveSet {
    /// The file as RON, ready to save.
    pub fn to_ron(&self) -> String {
        let pretty =
            ron::ser::PrettyConfig::new().indentor("    ").struct_names(false).depth_limit(3);
        ron::ser::to_string_pretty(&self.file, pretty).expect("move sets serialize")
    }
}

/// Any other file, as bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DataAsset {
    pub bytes: Vec<u8>,
}

impl Asset for DataAsset {
    const KIND: AssetKind = AssetKind::Data;
    fn load(bytes: &[u8], _: &Meta) -> Result<Self, AssetError> {
        Ok(DataAsset { bytes: bytes.to_vec() })
    }
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

fn ron_asset<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, AssetError> {
    let text = std::str::from_utf8(bytes).map_err(|e| AssetError(e.to_string()))?;
    ron::from_str(text).map_err(|e| AssetError(e.to_string()))
}

/// The RON text a data asset saves as.
pub fn to_ron<T: Serialize>(value: &T) -> String {
    let pretty = ron::ser::PrettyConfig::new()
        .indentor("    ")
        .depth_limit(2)
        .struct_names(false)
        .separate_tuple_members(false);
    let mut text = ron::ser::to_string_pretty(value, pretty).expect("assets serialize");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_sampling_matches_the_engine_clip() {
        let ms = [170, 130, 60, 80, 160, 120, 120];
        for looping in [false, true] {
            let asset = AnimationClip {
                mode: if looping { LoopMode::Loop } else { LoopMode::Once },
                frames: ms.iter().map(|&ms| ClipFrame { ms, ..ClipFrame::default() }).collect(),
                ..AnimationClip::default()
            };
            let clip = asset.to_clip();
            assert_eq!(asset.ticks(), clip.ticks());
            for tick in 0..400 {
                assert_eq!(asset.frame_at_tick(tick), clip.frame(tick), "tick {tick}");
            }
        }
    }

    #[test]
    fn tile_layers_store_base36_cells() {
        let mut layer = TileLayer::empty("decor", 4, 2);
        layer.set(1, 0, Some(37));
        layer.set(3, 1, Some(5));
        assert_eq!(layer.rows, ["..11....", "......05"]);
        assert_eq!(layer.get(1, 0), Some(37));
        assert_eq!(layer.get(0, 0), None);
        layer.set(1, 0, None);
        assert_eq!(layer.get(1, 0), None);
    }

    #[test]
    fn ascii_maps_split_into_collision_and_markers_and_back() {
        let text = "..C..\n#-/\\E\n#####\n";
        let map = Tilemap::from_ascii(text);
        assert_eq!(map.collision, [".....", "#-/\\.", "#####"]);
        assert_eq!(map.markers, ["..C..", "....E", "....."]);
        assert_eq!(map.to_ascii(), text);
        let rows: Vec<&str> = text.lines().collect();
        assert_eq!(map.collision_map(), TileMap::from_ascii(&rows));
    }
}
