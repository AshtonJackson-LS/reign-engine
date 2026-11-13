//! `.meta` sidecar files: an asset's GUID and its import settings.
//!
//! Every asset file `foo.png` has a `foo.png.meta` next to it, in RON:
//!
//! ```ron
//! (guid: "6b1f0c...", importer: Texture((density: 2)))
//! ```
//!
//! The GUID is what scenes and other assets reference, so the file can be
//! moved or renamed freely as long as its `.meta` goes with it. Missing
//! `.meta` files are created when a loose asset folder is opened; commit
//! them.

use serde::{Deserialize, Serialize};

use crate::{AssetKind, Guid, Reflect};

/// The file extension of sidecar files.
pub const META_EXT: &str = "meta";

/// How a texture is imported.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct TextureSettings {
    /// Texels per world pixel: 1 for chunky tiles, 2 or 3 for finer heroes
    /// and showpiece props on the same 640x360 view.
    #[reflect(range(1, 4))]
    pub density: u8,
}

impl Default for TextureSettings {
    fn default() -> Self {
        TextureSettings { density: 1 }
    }
}

/// How an audio file is imported.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
#[serde(default)]
pub struct AudioSettings {
    /// Plays again from the start when it ends.
    pub looping: bool,
    /// Long music: streamed rather than kept as one sound effect. Makes the
    /// asset a [`AssetKind::Music`].
    pub stream: bool,
}

/// What kind of asset a file is, and how to import it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Importer {
    Texture(TextureSettings),
    Audio(AudioSettings),
    SpriteSheet,
    AnimationClip,
    Tileset,
    Tilemap,
    Font,
    Scene,
    Prefab,
    MoveSet,
    /// Anything else: kept and packed as plain bytes.
    Data,
}

impl Importer {
    /// The default importer for a file, from its name.
    pub fn for_path(path: &str) -> Importer {
        let lower = path.to_ascii_lowercase();
        let ends = |s: &str| lower.ends_with(s);
        if ends(".png") {
            Importer::Texture(TextureSettings::default())
        } else if ends(".wav") {
            let stream = lower.contains("music/");
            Importer::Audio(AudioSettings { looping: stream, stream })
        } else if ends(".sheet.ron") {
            Importer::SpriteSheet
        } else if ends(".anim.ron") {
            Importer::AnimationClip
        } else if ends(".tileset.ron") {
            Importer::Tileset
        } else if ends(".tilemap.ron") {
            Importer::Tilemap
        } else if ends(".font.ron") {
            Importer::Font
        } else if ends(".scene.ron") {
            Importer::Scene
        } else if ends(".prefab.ron") {
            Importer::Prefab
        } else if ends(".moves.ron") {
            Importer::MoveSet
        } else {
            Importer::Data
        }
    }

    pub fn kind(&self) -> AssetKind {
        match self {
            Importer::Texture(_) => AssetKind::Texture,
            Importer::Audio(a) if a.stream => AssetKind::Music,
            Importer::Audio(_) => AssetKind::AudioClip,
            Importer::SpriteSheet => AssetKind::SpriteSheet,
            Importer::AnimationClip => AssetKind::AnimationClip,
            Importer::Tileset => AssetKind::Tileset,
            Importer::Tilemap => AssetKind::Tilemap,
            Importer::Font => AssetKind::Font,
            Importer::Scene => AssetKind::Scene,
            Importer::Prefab => AssetKind::Prefab,
            Importer::MoveSet => AssetKind::MoveSet,
            Importer::Data => AssetKind::Data,
        }
    }
}

/// The contents of a `.meta` file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Meta {
    pub guid: Guid,
    pub importer: Importer,
}

impl Meta {
    /// A new meta for the file at `path`, with a fresh GUID.
    pub fn new_for(path: &str) -> Meta {
        Meta { guid: Guid::new(), importer: Importer::for_path(path) }
    }

    pub fn to_ron(&self) -> String {
        let pretty = ron::ser::PrettyConfig::new().depth_limit(1).struct_names(false);
        let mut text = ron::ser::to_string_pretty(self, pretty).expect("metas serialize");
        text.push('\n');
        text
    }

    pub fn from_ron(text: &str) -> Result<Meta, String> {
        ron::from_str(text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metas_round_trip_and_pick_importers_by_name() {
        for path in [
            "a/hero.png",
            "audio/hit.wav",
            "audio/music/boss.wav",
            "x.sheet.ron",
            "x.anim.ron",
            "x.tileset.ron",
            "x.tilemap.ron",
            "ui/font.font.ron",
            "scenes/a.scene.ron",
            "notes.json",
        ] {
            let meta = Meta::new_for(path);
            let back = Meta::from_ron(&meta.to_ron()).unwrap();
            assert_eq!(back, meta, "{path}");
        }
        assert_eq!(Importer::for_path("audio/music/boss.wav").kind(), AssetKind::Music);
        assert_eq!(Importer::for_path("audio/hit.wav").kind(), AssetKind::AudioClip);
        assert_eq!(Importer::for_path("a/HERO.PNG").kind(), AssetKind::Texture);
        let text =
            Meta { guid: Guid(5), importer: Importer::Texture(TextureSettings { density: 2 }) }
                .to_ron();
        assert!(text.contains("density: 2"), "{text}");
    }
}
