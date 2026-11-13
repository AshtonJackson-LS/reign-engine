//! Reign's assets: identities, import settings, typed handles, loading,
//! hot reload and packing.
//!
//! - Every asset file has a `.meta` sidecar ([`Meta`]) holding its stable
//!   [`Guid`] and its import settings ([`Importer`]). References (in scenes,
//!   in other assets) are GUIDs, wrapped in a typed [`Handle<T>`], so files
//!   can move and be renamed freely.
//! - [`AssetDb`] maps GUIDs to files and loads them as typed assets
//!   ([`Texture`], [`SpriteSheet`], [`AnimationClip`], [`Tileset`],
//!   [`Tilemap`], [`AudioClip`], [`Music`], [`Font`], [`SceneAsset`]).
//! - Development and the editor read loose files and can hot reload them
//!   ([`AssetDb::watch`] queues changes, [`AssetDb::apply_changes`] applies
//!   them at a frame boundary). Release builds read a packed archive
//!   ([`pack_dir`]) embedded in the executable.
//! - [`AssetDb::content_hash`] covers what can change the simulation, for
//!   online peers to compare.

mod db;
mod guid;
mod handle;
mod image;
mod meta;
mod pack;
pub mod prefab;
pub mod scan;
mod types;

use std::fmt;

pub use db::{AssetChange, AssetDb, AssetInfo};
pub use guid::Guid;
pub use handle::Handle;
pub use image::{PngError, RgbaImage, decode_png};
pub use meta::{AudioSettings, Importer, META_EXT, Meta, TextureSettings};
pub use pack::{PackEntry, pack_dir, read_pack};
pub use prefab::{InstantiateExt, Prefab, PrefabInstance, Prefabs, Template};
pub use reign_scene::Reflect;
pub use types::{
    AnimationClip, AudioClip, ClipFrame, DataAsset, Font, Grid, LoopMode, MoveSet, Music,
    SceneAsset, Sprite, SpriteSheet, Texture, TileLayer, Tilemap, Tileset, to_ron,
};

/// The kinds of asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetKind {
    Texture,
    SpriteSheet,
    AnimationClip,
    Tileset,
    Tilemap,
    AudioClip,
    Music,
    Font,
    Scene,
    Prefab,
    MoveSet,
    Data,
}

impl AssetKind {
    pub const ALL: [AssetKind; 12] = [
        AssetKind::Texture,
        AssetKind::SpriteSheet,
        AssetKind::AnimationClip,
        AssetKind::Tileset,
        AssetKind::Tilemap,
        AssetKind::AudioClip,
        AssetKind::Music,
        AssetKind::Font,
        AssetKind::Scene,
        AssetKind::Prefab,
        AssetKind::MoveSet,
        AssetKind::Data,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AssetKind::Texture => "Texture",
            AssetKind::SpriteSheet => "SpriteSheet",
            AssetKind::AnimationClip => "AnimationClip",
            AssetKind::Tileset => "Tileset",
            AssetKind::Tilemap => "Tilemap",
            AssetKind::AudioClip => "AudioClip",
            AssetKind::Music => "Music",
            AssetKind::Font => "Font",
            AssetKind::Scene => "Scene",
            AssetKind::Prefab => "Prefab",
            AssetKind::MoveSet => "MoveSet",
            AssetKind::Data => "Data",
        }
    }

    /// Whether this kind can change what the simulation does: collision and
    /// level markers (tilemaps), clip timing (animation clips drive attack
    /// frames), scenes, prefabs and move sets (combat frame data). Textures, sprite rects, sounds, fonts
    /// and plain data are presentation only.
    pub fn sim_affecting(self) -> bool {
        matches!(
            self,
            AssetKind::Tilemap
                | AssetKind::AnimationClip
                | AssetKind::Scene
                | AssetKind::Prefab
                | AssetKind::MoveSet
        )
    }
}

/// A type the database can load.
pub trait Asset: Send + Sync + Sized + 'static {
    const KIND: AssetKind;
    fn load(bytes: &[u8], meta: &Meta) -> Result<Self, AssetError>;
}

/// Why an asset couldn't be read, written or loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetError(pub String);

impl fmt::Display for AssetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AssetError {}
