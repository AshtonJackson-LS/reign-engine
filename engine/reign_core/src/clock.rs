//! Fixed-timestep clock: turns variable frame times into whole simulation ticks.

use std::time::Duration;

#[derive(Debug)]
pub struct FixedClock {
    tick: Duration,
    accumulator: Duration,
    max_ticks_per_frame: u32,
}

impl FixedClock {
    pub fn new(tick_hz: u32) -> Self {
        FixedClock {
            tick: Duration::from_secs(1) / tick_hz,
            accumulator: Duration::ZERO,
            // A long stall (window drag, breakpoint) should not fast-forward the game.
            max_ticks_per_frame: (tick_hz / 10).max(1),
        }
    }

    pub fn tick_duration(&self) -> Duration {
        self.tick
    }

    /// Adds elapsed real time and returns how many ticks to simulate now.
    pub fn advance(&mut self, elapsed: Duration) -> u32 {
        self.accumulator += elapsed;
        let mut ticks = 0;
        while self.accumulator >= self.tick {
            self.accumulator -= self.tick;
            ticks += 1;
        }
        if ticks > self.max_ticks_per_frame {
            self.accumulator = Duration::ZERO;
            ticks = self.max_ticks_per_frame;
        }
        ticks
    }

    /// How far between the last tick and the next one we are, in 0..1.
    /// Rendering uses it to interpolate; the simulation never sees it.
    pub fn alpha(&self) -> f32 {
        self.accumulator.as_secs_f32() / self.tick.as_secs_f32()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_time_into_ticks() {
        let mut clock = FixedClock::new(120);
        let tick = clock.tick_duration();
        assert_eq!(clock.advance(tick * 3 + tick / 2), 3);
        assert!((clock.alpha() - 0.5).abs() < 0.01);
    }

    #[test]
    fn caps_catch_up_after_a_stall() {
        let mut clock = FixedClock::new(120);
        assert_eq!(clock.advance(Duration::from_secs(5)), 12);
        assert_eq!(clock.alpha(), 0.0);
    }
}
