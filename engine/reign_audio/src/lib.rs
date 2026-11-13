//! Device output and a bounded stereo mixer. Audio lives outside rollback state.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub enum Bus {
    Music,
    Effects,
}

#[derive(Clone)]
pub struct Clip {
    frames: Arc<[[f32; 2]]>,
    rate: u32,
}

impl Clip {
    pub fn seconds(&self) -> f32 {
        self.frames.len() as f32 / self.rate as f32
    }

    /// RIFF/WAVE PCM16, mono or stereo. Ignores ancillary RIFF chunks.
    pub fn wav(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
            return Err("not a WAVE file");
        }
        let mut format = None;
        let mut data = None;
        let mut pos = 12usize;
        while pos + 8 <= bytes.len() {
            let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
            let start = pos + 8;
            let end = start
                .checked_add(len)
                .filter(|&end| end <= bytes.len())
                .ok_or("truncated WAVE chunk")?;
            match &bytes[pos..pos + 4] {
                b"fmt " if len >= 16 => {
                    let u16_at = |i| u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap());
                    let rate = u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap());
                    format = Some((
                        u16_at(start),
                        u16_at(start + 2),
                        rate,
                        u16_at(start + 12),
                        u16_at(start + 14),
                    ));
                }
                b"data" => data = Some(&bytes[start..end]),
                _ => {}
            }
            pos = end.checked_add(len % 2).ok_or("WAVE chunk overflow")?;
        }
        let (kind, channels, rate, align, bits) = format.ok_or("missing WAVE format")?;
        if kind != 1
            || !(1..=2).contains(&channels)
            || bits != 16
            || rate == 0
            || align != channels * 2
        {
            return Err("expected mono/stereo PCM16");
        }
        let data = data.ok_or("missing WAVE data")?;
        if data.is_empty() || data.len() % align as usize != 0 {
            return Err("incomplete PCM frames");
        }
        let sample =
            |bytes: &[u8]| i16::from_le_bytes(bytes[..2].try_into().unwrap()) as f32 / 32768.0;
        let frames: Vec<_> = data
            .chunks_exact(align as usize)
            .map(|frame| {
                let left = sample(frame);
                [left, if channels == 2 { sample(&frame[2..]) } else { left }]
            })
            .collect();
        Ok(Self { frames: frames.into(), rate })
    }
}

struct Voice {
    clip: Clip,
    position: f64,
    step: f64,
    gain: [f32; 2],
    bus: Bus,
    looping: bool,
}

/// Music runs on the device clock, so switches stay aligned even when game
/// frames stall, simulation rolls back, or effects fill the voice pool.
struct AdaptiveMusic {
    clips: Vec<Clip>,
    current: usize,
    requested: usize,
    fading_from: Option<(usize, u32)>,
    clock: u64,
    bar_frames: u64,
    fade_frames: u32,
    gain: f32,
}

impl AdaptiveMusic {
    fn sample(&self, index: usize, rate: u32) -> [f32; 2] {
        let clip = &self.clips[index];
        let position = self.clock as f64 * clip.rate as f64 / rate as f64;
        let position = position % clip.frames.len() as f64;
        let a = position as usize;
        let b = (a + 1) % clip.frames.len();
        let fraction = (position - a as f64) as f32;
        std::array::from_fn(|ch| {
            clip.frames[a][ch] * (1.0 - fraction) + clip.frames[b][ch] * fraction
        })
    }

    fn next_frame(&mut self, rate: u32) -> [f32; 2] {
        if self.clock.is_multiple_of(self.bar_frames)
            && self.current != self.requested
            && self.fading_from.is_none()
        {
            self.fading_from = Some((self.current, 0));
            self.current = self.requested;
        }
        let mut out = self.sample(self.current, rate);
        if let Some((previous, elapsed)) = self.fading_from {
            let t = (elapsed as f32 / self.fade_frames as f32).min(1.0);
            let blend = t * t * (3.0 - 2.0 * t);
            let old = self.sample(previous, rate);
            for ch in 0..2 {
                out[ch] = old[ch] * (1.0 - blend) + out[ch] * blend;
            }
            self.fading_from = (elapsed + 1 < self.fade_frames).then_some((previous, elapsed + 1));
        }
        let entrance = (self.clock as f32 / self.fade_frames as f32).min(1.0);
        self.clock += 1;
        out.map(|sample| sample * self.gain * entrance)
    }
}

struct Theme {
    clip: Clip,
    position: f64,
    step: f64,
    gain: f32,
    level: f32,
    on: bool,
    fade_frames: f32,
}

pub struct Mixer {
    rate: u32,
    voices: Vec<Voice>,
    master: f32,
    music: f32,
    effects: f32,
    duck_gain: f32,
    duck_target: f32,
    duck_frames: u32,
    adaptive: Option<AdaptiveMusic>,
    theme: Option<Theme>,
}

impl Mixer {
    pub fn new(rate: u32) -> Self {
        assert!(rate > 0);
        Self {
            rate,
            voices: Vec::with_capacity(32),
            master: 1.0,
            music: 1.0,
            effects: 1.0,
            duck_gain: 1.0,
            duck_target: 1.0,
            duck_frames: 0,
            adaptive: None,
            theme: None,
        }
    }

    pub fn volumes(&mut self, master: f32, music: f32, effects: f32) {
        self.master = master.clamp(0.0, 1.0);
        self.music = music.clamp(0.0, 1.0);
        self.effects = effects.clamp(0.0, 1.0);
    }

    /// Install equal-duration musical sections sharing a beat grid. The first
    /// section plays initially; intensity requests switch on the next bar.
    pub fn adaptive_music(
        &mut self,
        clips: &[Clip],
        gain: f32,
        bar_seconds: f32,
        fade_seconds: f32,
    ) {
        assert!(!clips.is_empty());
        self.adaptive = Some(AdaptiveMusic {
            clips: clips.to_vec(),
            current: 0,
            requested: 0,
            fading_from: None,
            clock: 0,
            bar_frames: (bar_seconds.clamp(0.1, 30.0) * self.rate as f32).round().max(1.0) as u64,
            fade_frames: (fade_seconds.clamp(0.001, bar_seconds.max(0.001)) * self.rate as f32)
                .round()
                .max(1.0) as u32,
            gain: gain.clamp(0.0, 2.0),
        });
    }

    pub fn music_intensity(&mut self, section: usize) {
        if let Some(music) = &mut self.adaptive {
            music.requested = section.min(music.clips.len() - 1);
        }
    }

    /// A full title theme that crossfades with the adaptive score. Once it
    /// has faded out completely it resets, ready for the next title visit.
    pub fn theme(&mut self, clip: &Clip, gain: f32, fade_seconds: f32) {
        self.theme = Some(Theme {
            clip: clip.clone(),
            position: 0.0,
            step: clip.rate as f64 / self.rate as f64,
            gain: gain.clamp(0.0, 2.0),
            level: 0.0,
            on: false,
            fade_frames: (fade_seconds.clamp(0.001, 30.0) * self.rate as f32).max(1.0),
        });
    }

    pub fn theme_playing(&mut self, on: bool) {
        if let Some(theme) = &mut self.theme {
            theme.on = on;
        }
    }

    pub fn play(&mut self, clip: &Clip, bus: Bus, gain: f32, pan: f32, pitch: f32, looping: bool) {
        if self.voices.len() >= 32 {
            if let Some(i) = self.voices.iter().position(|v| !v.looping) {
                self.voices.swap_remove(i);
            } else {
                return;
            }
        }
        let pan = pan.clamp(-1.0, 1.0);
        let gain = gain.clamp(0.0, 2.0);
        self.voices.push(Voice {
            clip: clip.clone(),
            position: 0.0,
            step: clip.rate as f64 / self.rate as f64 * pitch.clamp(0.25, 4.0) as f64,
            gain: [gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0)],
            bus,
            looping,
        });
    }

    /// Briefly make room for an impact without changing the user's bus levels.
    pub fn duck_music(&mut self, seconds: f32, gain: f32) {
        let frames = (seconds.clamp(0.0, 2.0) * self.rate as f32) as u32;
        if frames > 0 {
            self.duck_frames = self.duck_frames.max(frames);
            self.duck_target = self.duck_target.min(gain.clamp(0.0, 1.0));
        }
    }

    pub fn next_frame(&mut self) -> [f32; 2] {
        if self.duck_frames == 0 {
            self.duck_target = 1.0;
        }
        self.duck_frames = self.duck_frames.saturating_sub(1);
        let speed = if self.duck_target < self.duck_gain { 250.0 } else { 8.0 };
        self.duck_gain += (self.duck_target - self.duck_gain) * (speed / self.rate as f32).min(1.0);
        let mut out = [0.0; 2];
        let mut score_gain = 1.0;
        if let Some(theme) = &mut self.theme {
            let delta = 1.0 / theme.fade_frames;
            theme.level = if theme.on {
                (theme.level + delta).min(1.0)
            } else {
                (theme.level - delta).max(0.0)
            };
            let blend = theme.level * theme.level * (3.0 - 2.0 * theme.level);
            score_gain = 1.0 - blend;
            if theme.level == 0.0 {
                theme.position = 0.0;
            } else {
                let len = theme.clip.frames.len();
                let a = theme.position as usize % len;
                let b = (a + 1) % len;
                let fraction = (theme.position - theme.position.floor()) as f32;
                for (ch, sample) in out.iter_mut().enumerate() {
                    *sample = (theme.clip.frames[a][ch] * (1.0 - fraction)
                        + theme.clip.frames[b][ch] * fraction)
                        * theme.gain
                        * blend
                        * self.music
                        * self.duck_gain;
                }
                theme.position = (theme.position + theme.step) % len as f64;
            }
        }
        if let Some(music) = &mut self.adaptive {
            let score = music.next_frame(self.rate);
            for (ch, sample) in out.iter_mut().enumerate() {
                *sample += score[ch] * self.music * self.duck_gain * score_gain;
            }
        }
        for voice in &mut self.voices {
            let len = voice.clip.frames.len();
            if voice.position >= len as f64 {
                if voice.looping {
                    voice.position %= len as f64;
                } else {
                    continue;
                }
            }
            let a = voice.position as usize;
            let b = if a + 1 < len {
                a + 1
            } else if voice.looping {
                0
            } else {
                a
            };
            let fraction = (voice.position - a as f64) as f32;
            let bus = match voice.bus {
                Bus::Music => self.music * self.duck_gain,
                Bus::Effects => self.effects,
            };
            for (channel, out) in out.iter_mut().enumerate() {
                let x = voice.clip.frames[a][channel];
                let y = voice.clip.frames[b][channel];
                *out += (x + (y - x) * fraction) * voice.gain[channel] * bus;
            }
            voice.position += voice.step;
        }
        self.voices.retain(|v| v.looping || v.position < v.clip.frames.len() as f64);
        out.map(|sample| (sample * self.master).tanh())
    }
}

pub struct Output {
    _stream: cpal::Stream,
    mixer: Arc<Mutex<Mixer>>,
}

impl Output {
    pub fn open() -> Result<Self, String> {
        let device =
            cpal::default_host().default_output_device().ok_or("no output audio device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let mixer = Arc::new(Mutex::new(Mixer::new(config.sample_rate.0)));
        let stream = match format {
            cpal::SampleFormat::F32 => stream::<f32>(&device, &config, mixer.clone()),
            cpal::SampleFormat::F64 => stream::<f64>(&device, &config, mixer.clone()),
            cpal::SampleFormat::I16 => stream::<i16>(&device, &config, mixer.clone()),
            cpal::SampleFormat::U16 => stream::<u16>(&device, &config, mixer.clone()),
            cpal::SampleFormat::I32 => stream::<i32>(&device, &config, mixer.clone()),
            _ => return Err(format!("unsupported output sample format: {format}")),
        }
        .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        log::info!("Audio output: {} Hz, {} channels", config.sample_rate.0, config.channels);
        Ok(Self { _stream: stream, mixer })
    }

    pub fn volumes(&self, master: f32, music: f32, effects: f32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.volumes(master, music, effects);
        }
    }

    pub fn adaptive_music(&self, clips: &[Clip], gain: f32, bar_seconds: f32, fade_seconds: f32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.adaptive_music(clips, gain, bar_seconds, fade_seconds);
        }
    }

    pub fn theme(&self, clip: &Clip, gain: f32, fade_seconds: f32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.theme(clip, gain, fade_seconds);
        }
    }

    pub fn theme_playing(&self, on: bool) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.theme_playing(on);
        }
    }

    pub fn music_intensity(&self, section: usize) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.music_intensity(section);
        }
    }

    pub fn play(&self, clip: &Clip, bus: Bus, gain: f32, pan: f32, pitch: f32, looping: bool) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.play(clip, bus, gain, pan, pitch, looping);
        }
    }

    pub fn duck_music(&self, seconds: f32, gain: f32) {
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.duck_music(seconds, gain);
        }
    }
}

fn stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer: Arc<Mutex<Mixer>>,
) -> Result<cpal::Stream, cpal::BuildStreamError> {
    let channels = config.channels as usize;
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            // Never wait for the game thread inside the device callback.
            let mut mixer = mixer.try_lock().ok();
            for frame in data.chunks_mut(channels) {
                let sample = mixer.as_mut().map_or([0.0; 2], |m| m.next_frame());
                for (channel, out) in frame.iter_mut().enumerate() {
                    let value = if channels == 1 {
                        (sample[0] + sample[1]) * 0.5
                    } else if channel < 2 {
                        sample[channel]
                    } else {
                        0.0
                    };
                    *out = T::from_sample(value);
                }
            }
        },
        |error| log::warn!("Audio output error: {error}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clip() -> Clip {
        Clip { frames: vec![[0.5; 2]; 4].into(), rate: 24000 }
    }

    #[test]
    fn title_theme_fades_to_score_restarts_and_respects_music_mute() {
        let score = Clip { frames: vec![[0.1; 2]; 2000].into(), rate: 1000 };
        let theme = Clip {
            frames: (0..2000).map(|i| [i as f32 / 2000.0; 2]).collect::<Vec<_>>().into(),
            rate: 1000,
        };
        let mut mixer = Mixer::new(1000);
        mixer.adaptive_music(&[score], 1.0, 0.1, 0.01);
        mixer.theme(&theme, 1.0, 0.1);
        for _ in 0..50 {
            mixer.next_frame();
        }
        assert!((mixer.next_frame()[0] - 0.1f32.tanh()).abs() < 0.001);
        mixer.theme_playing(true);
        for _ in 0..300 {
            mixer.next_frame();
        }
        let mut previous = mixer.next_frame()[0];
        assert!((previous - 0.15f32.tanh()).abs() < 0.002);
        mixer.theme_playing(false);
        for _ in 0..110 {
            let next = mixer.next_frame()[0];
            assert!((next - previous).abs() < 0.01, "theme exit must crossfade");
            previous = next;
        }
        assert!((previous - 0.1f32.tanh()).abs() < 0.001);
        assert_eq!(mixer.theme.as_ref().unwrap().position, 0.0);
        mixer.theme_playing(true);
        mixer.volumes(1.0, 0.0, 1.0);
        for _ in 0..200 {
            assert_eq!(mixer.next_frame(), [0.0; 2]);
        }
    }

    #[test]
    fn adaptive_music_waits_for_a_bar_and_crossfades_without_restarting_the_clock() {
        let clips =
            [0.1, 0.7].map(|value| Clip { frames: vec![[value; 2]; 400].into(), rate: 1000 });
        let mut mixer = Mixer::new(1000);
        mixer.adaptive_music(&clips, 1.0, 0.1, 0.02);
        for _ in 0..50 {
            mixer.next_frame();
        }
        mixer.music_intensity(1);
        for _ in 0..50 {
            assert!((mixer.next_frame()[0] - 0.1f32.tanh()).abs() < 0.001);
        }
        let mut previous = mixer.next_frame()[0];
        assert!((previous - 0.1f32.tanh()).abs() < 0.001);
        // A new request during a fade is queued rather than cutting it off.
        mixer.music_intensity(0);
        for _ in 0..20 {
            let next = mixer.next_frame()[0];
            assert!((next - previous).abs() < 0.05);
            previous = next;
        }
        assert!((previous - 0.7f32.tanh()).abs() < 0.001);
        for _ in 0..100 {
            mixer.next_frame();
        }
        assert!((mixer.next_frame()[0] - 0.1f32.tanh()).abs() < 0.001);
        assert!(mixer.voices.is_empty(), "switching never adds ordinary voices");
        assert_eq!(mixer.adaptive.as_ref().unwrap().clock, 222);
    }

    #[test]
    fn adaptive_music_survives_voice_pressure_loops_resamples_and_respects_mute() {
        let clips = [clip(), clip()];
        let mut mixer = Mixer::new(48000);
        mixer.adaptive_music(&clips, 0.5, 0.1, 0.01);
        for _ in 0..40 {
            mixer.play(&clip(), Bus::Effects, 1.0, 0.0, 1.0, true);
        }
        mixer.volumes(1.0, 1.0, 0.0);
        for _ in 0..10000 {
            mixer.next_frame();
        }
        assert!((mixer.next_frame()[0] - 0.25f32.tanh()).abs() < 0.001);
        mixer.volumes(1.0, 0.0, 0.0);
        assert_eq!(mixer.next_frame(), [0.0; 2]);
        mixer.volumes(0.0, 1.0, 1.0);
        assert_eq!(mixer.next_frame(), [0.0; 2]);
    }

    #[test]
    fn impact_duck_recovers_and_respects_user_bus_levels() {
        let mut mixer = Mixer::new(1000);
        mixer.volumes(0.7, 0.6, 0.0);
        mixer.play(&clip(), Bus::Music, 1.0, 0.0, 1.0, true);
        let normal = mixer.next_frame()[0];
        mixer.duck_music(0.1, 0.25);
        for _ in 0..50 {
            mixer.next_frame();
        }
        assert!(mixer.next_frame()[0] < normal * 0.4);
        for _ in 0..1500 {
            mixer.next_frame();
        }
        assert!((mixer.next_frame()[0] - normal).abs() < 0.001);
        mixer.volumes(0.0, 0.6, 1.0);
        mixer.duck_music(0.1, 0.2);
        assert_eq!(mixer.next_frame(), [0.0; 2]);
    }

    #[test]
    fn buses_pan_resampling_and_live_mute_work_without_a_device() {
        let mut mixer = Mixer::new(48000);
        mixer.play(&clip(), Bus::Effects, 1.0, -1.0, 1.0, false);
        assert_eq!(mixer.next_frame()[1], 0.0);
        mixer.volumes(1.0, 1.0, 0.0);
        assert_eq!(mixer.next_frame(), [0.0; 2]);
        mixer.volumes(1.0, 1.0, 1.0);
        assert!(mixer.next_frame()[0] > 0.0);
        for _ in 0..5 {
            mixer.next_frame();
        }
        assert_eq!(mixer.next_frame(), [0.0; 2]);
        mixer.play(&clip(), Bus::Music, 1.0, 0.0, 1.0, true);
        for _ in 0..100 {
            assert!(mixer.next_frame()[0] > 0.0);
        }
        mixer.volumes(0.0, 1.0, 1.0);
        assert_eq!(mixer.next_frame(), [0.0; 2]);
    }

    #[test]
    fn mixer_bounds_polyphony_and_output_and_rejects_invalid_waves() {
        let mut mixer = Mixer::new(24000);
        for _ in 0..100 {
            mixer.play(&clip(), Bus::Effects, 2.0, 0.0, 1.0, false);
        }
        assert_eq!(mixer.voices.len(), 32);
        assert!(mixer.next_frame().iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        assert!(Clip::wav(b"not audio").is_err());
        assert!(Clip::wav(b"RIFF\0\0\0\0WAVEdata\xff\xff\xff\xff").is_err());
    }
}
