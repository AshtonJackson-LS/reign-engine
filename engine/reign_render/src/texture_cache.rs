//! GPU textures for texture assets, uploaded once each and kept in sync
//! with hot reloads.

use std::collections::BTreeMap;

use reign_assets::{AssetDb, AssetError, Guid, Handle, Texture};

use crate::{Renderer, TextureId};

/// Texture assets on the GPU, by GUID. The same asset always gets the same
/// [`TextureId`], and a reload re-uploads into it, so everything drawing
/// with it shows the new pixels.
#[derive(Debug, Default)]
pub struct TextureCache {
    loaded: BTreeMap<Guid, TextureId>,
}

impl TextureCache {
    pub const fn new() -> Self {
        TextureCache { loaded: BTreeMap::new() }
    }

    /// The GPU texture for `handle`, uploading it the first time.
    pub fn get(
        &mut self,
        r: &mut Renderer,
        db: &AssetDb,
        handle: Handle<Texture>,
    ) -> Result<TextureId, AssetError> {
        if let Some(id) = self.loaded.get(&handle.guid) {
            return Ok(*id);
        }
        let texture = db.load(handle)?;
        let image = &texture.image;
        let id = r.load_texture(&image.rgba, image.width, image.height);
        self.loaded.insert(handle.guid, id);
        Ok(id)
    }

    /// Whether `guid` was uploaded.
    pub fn contains(&self, guid: Guid) -> bool {
        self.loaded.contains_key(&guid)
    }

    /// Re-uploads `guid` from the database, if it was uploaded. Returns
    /// whether it was.
    pub fn reload(
        &mut self,
        r: &mut Renderer,
        db: &AssetDb,
        guid: Guid,
    ) -> Result<bool, AssetError> {
        let Some(id) = self.loaded.get(&guid).copied() else { return Ok(false) };
        let texture = db.load(Handle::<Texture>::new(guid))?;
        let image = &texture.image;
        r.replace_texture(id, &image.rgba, image.width, image.height);
        Ok(true)
    }
}
