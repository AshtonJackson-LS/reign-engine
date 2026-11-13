//! Playing what a tick asks for: sounds and music, through the system's
//! audio output. The output opens the first time something plays, so a
//! game with no sound (and every test) never touches an audio device.

use std::collections::HashMap;

use reign::assets::{AssetDb, Guid};
use reign::audio::{Bus, Clip, Output};

use crate::runtime::Event;

/// How long music takes to fade in over what was playing, in seconds.
const MUSIC_FADE: f32 = 0.5;

#[derive(Default)]
pub struct Audio {
    output: Option<Output>,
    /// The output couldn't open (no device): said once, then silent.
    unavailable: bool,
    /// Decoded sounds, with the asset version they were decoded from.
    clips: HashMap<Guid, (u32, Clip)>,
}

impl Audio {
    fn output(&mut self) -> Option<&Output> {
        if self.output.is_none() && !self.unavailable {
            match Output::open() {
                Ok(output) => self.output = Some(output),
                Err(e) => {
                    log::warn!("no sound: {e}");
                    self.unavailable = true;
                }
            }
        }
        self.output.as_ref()
    }

    fn clip(&mut self, db: &AssetDb, guid: Guid) -> Option<Clip> {
        let version = db.version(guid);
        if let Some((v, clip)) = self.clips.get(&guid)
            && *v == version
        {
            return Some(clip.clone());
        }
        let name = || db.path(guid).unwrap_or_else(|| guid.to_string());
        let bytes = match db.bytes(guid) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("{}: {e}", name());
                return None;
            }
        };
        match Clip::wav(&bytes) {
            Ok(clip) => {
                self.clips.insert(guid, (version, clip.clone()));
                Some(clip)
            }
            Err(e) => {
                log::warn!("{}: {e} (sounds are 16-bit PCM .wav files)", name());
                None
            }
        }
    }

    /// Plays a tick's events.
    pub fn play(&mut self, events: Vec<Event>, db: Option<&AssetDb>) {
        let Some(db) = db else { return };
        for event in events {
            match event {
                Event::Sound { clip, volume } => {
                    let Some(clip) = self.clip(db, clip) else { continue };
                    if let Some(output) = self.output() {
                        let gain = f32::from(volume) / 100.0;
                        output.play(&clip, Bus::Effects, gain, 0.0, 1.0, false);
                    }
                }
                Event::Music { clip, volume } => {
                    // The mixer loops music over its length: none is no music.
                    let Some(clip) = self.clip(db, clip).filter(|c| c.seconds() > 0.0) else {
                        continue;
                    };
                    if let Some(output) = self.output() {
                        output.theme(&clip, f32::from(volume) / 100.0, MUSIC_FADE);
                        output.theme_playing(true);
                    }
                }
                Event::StopMusic => {
                    if let Some(output) = &self.output {
                        output.theme_playing(false);
                    }
                }
            }
        }
    }

    /// Silence: when Play stops in the editor.
    pub fn stop(&mut self) {
        if let Some(output) = &self.output {
            output.stop_all();
        }
    }
}
