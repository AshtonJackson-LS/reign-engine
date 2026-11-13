//! Edit, Play, Pause, Step and Stop, and the time scale.
//!
//! The editor never changes the tick length: time scale only changes how
//! much real time feeds the fixed clock, so more or fewer whole ticks run
//! per frame. A run is bracketed by a snapshot taken on Play and restored
//! on Stop, so play-testing never leaves the edited state changed.

use std::time::Duration;

use reign_core::FixedClock;

/// Time scales offered in the toolbar.
pub const TIME_SCALES: [f32; 5] = [0.1, 0.25, 0.5, 1.0, 2.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    /// Not running; the state is the one being edited.
    Edit,
    /// Running from the snapshot taken on Play.
    Playing,
    /// Stopped mid-run. Step runs one tick at a time.
    Paused,
}

/// The play state machine, separate from any window so it can be tested.
/// `S` is the app's snapshot.
pub struct PlayControl<S> {
    mode: PlayMode,
    scale: f32,
    tick_hz: u32,
    clock: FixedClock,
    snapshot: Option<S>,
    step: bool,
}

impl<S> PlayControl<S> {
    /// Starts in edit mode at 1x.
    pub fn new(tick_hz: u32) -> Self {
        PlayControl {
            mode: PlayMode::Edit,
            scale: 1.0,
            tick_hz,
            clock: FixedClock::new(tick_hz),
            snapshot: None,
            step: false,
        }
    }

    pub fn mode(&self) -> PlayMode {
        self.mode
    }

    /// Playing or paused: a run is in progress and Stop will restore.
    pub fn in_run(&self) -> bool {
        self.mode != PlayMode::Edit
    }

    /// From edit mode, takes the snapshot and starts running; from pause,
    /// resumes.
    pub fn play(&mut self, snapshot: impl FnOnce() -> S) {
        match self.mode {
            PlayMode::Edit => {
                self.snapshot = Some(snapshot());
                self.start_clock();
                self.mode = PlayMode::Playing;
            }
            PlayMode::Paused => {
                self.start_clock();
                self.mode = PlayMode::Playing;
            }
            PlayMode::Playing => {}
        }
    }

    pub fn pause(&mut self) {
        if self.mode == PlayMode::Playing {
            self.mode = PlayMode::Paused;
        }
    }

    /// Runs exactly one tick on the next frame and stays paused. From edit
    /// mode this starts a run first (taking the snapshot), like Play.
    pub fn step(&mut self, snapshot: impl FnOnce() -> S) {
        if self.mode == PlayMode::Edit {
            self.snapshot = Some(snapshot());
        }
        self.mode = PlayMode::Paused;
        self.step = true;
    }

    /// Ends the run and hands back the snapshot taken on Play, to restore.
    pub fn stop(&mut self) -> Option<S> {
        self.mode = PlayMode::Edit;
        self.step = false;
        self.snapshot.take()
    }

    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Sets how fast the run goes relative to real time; clamped to
    /// 0.01..=8.
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.01, 8.0);
    }

    /// The fixed tick length. No editor setting changes it.
    pub fn tick_duration(&self) -> Duration {
        self.clock.tick_duration()
    }

    /// How many ticks to run this frame, given the real time since the
    /// last one. Only a running game or a pending step yields any.
    pub fn advance(&mut self, elapsed: Duration) -> u32 {
        match self.mode {
            PlayMode::Playing => self.clock.advance(elapsed.mul_f32(self.scale)),
            PlayMode::Edit | PlayMode::Paused => u32::from(std::mem::take(&mut self.step)),
        }
    }

    /// Interpolation between the last tick and the next. Stopped, the frame
    /// shows the last tick exactly.
    pub fn alpha(&self) -> f32 {
        if self.mode == PlayMode::Playing { self.clock.alpha() } else { 1.0 }
    }

    fn start_clock(&mut self) {
        self.clock = FixedClock::new(self.tick_hz);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reign_core::TICK_HZ;

    /// Exactly one tick of real time.
    fn tick() -> Duration {
        Duration::from_secs(1) / TICK_HZ
    }

    #[test]
    fn starts_in_edit_mode_and_runs_nothing() {
        let mut play = PlayControl::<u32>::new(TICK_HZ);
        assert_eq!(play.mode(), PlayMode::Edit);
        assert_eq!(play.advance(Duration::from_secs(1)), 0);
    }

    #[test]
    fn play_snapshots_once_and_stop_hands_it_back() {
        let mut play = PlayControl::new(TICK_HZ);
        play.play(|| 7);
        play.pause();
        play.play(|| unreachable!("resuming must not snapshot again"));
        assert_eq!(play.mode(), PlayMode::Playing);
        assert_eq!(play.stop(), Some(7));
        assert_eq!(play.mode(), PlayMode::Edit);
        assert_eq!(play.stop(), None);
    }

    #[test]
    fn step_runs_exactly_one_tick() {
        let mut play = PlayControl::new(TICK_HZ);
        play.step(|| 1);
        assert_eq!(play.mode(), PlayMode::Paused);
        // However much time passes, a step is one tick, then nothing.
        assert_eq!(play.advance(Duration::from_secs(5)), 1);
        assert_eq!(play.advance(Duration::from_secs(5)), 0);
        play.step(|| unreachable!("already in a run"));
        assert_eq!(play.advance(Duration::ZERO), 1);
        assert_eq!(play.stop(), Some(1));
    }

    #[test]
    fn step_while_playing_pauses() {
        let mut play = PlayControl::new(TICK_HZ);
        play.play(|| 0);
        play.step(|| unreachable!());
        assert_eq!(play.mode(), PlayMode::Paused);
        assert_eq!(play.advance(tick() * 10), 1);
    }

    #[test]
    fn time_scale_changes_ticks_per_frame_not_tick_length() {
        for scale in TIME_SCALES {
            let mut play = PlayControl::<()>::new(TICK_HZ);
            play.set_scale(scale);
            play.play(|| ());
            assert_eq!(play.tick_duration(), tick());
            // Four ticks of real time per frame, for 120 frames.
            let ticks: u32 = (0..120).map(|_| play.advance(tick() * 4)).sum();
            let expected = (480.0 * scale).round() as u32;
            assert!(ticks.abs_diff(expected) <= 1, "{scale}x ran {ticks}, expected {expected}");
            assert_eq!(play.tick_duration(), tick());
        }
    }
}
