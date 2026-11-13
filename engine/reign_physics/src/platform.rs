//! Platforms that move back and forth and carry whatever stands on them.

use reign_core::{Fx, FxVec2};

use crate::{Body, Surface, TileMap};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MovingPlatform {
    pub w: i32,
    /// The two ends of the path; the platform's top-left travels between them.
    pub from: FxVec2,
    pub to: FxVec2,
    /// Ticks to travel from one end to the other.
    pub travel_ticks: u32,
    /// Ticks spent waiting at each end.
    pub pause_ticks: u32,
    tick: u32,
    pos: FxVec2,
    prev_pos: FxVec2,
}

impl MovingPlatform {
    pub fn new(w: i32, from: FxVec2, to: FxVec2, travel_ticks: u32, pause_ticks: u32) -> Self {
        MovingPlatform {
            w,
            from,
            to,
            travel_ticks,
            pause_ticks,
            tick: 0,
            pos: from,
            prev_pos: from,
        }
    }

    pub fn pos(&self) -> FxVec2 {
        self.pos
    }

    /// Position before the last step, for render interpolation.
    pub fn prev_pos(&self) -> FxVec2 {
        self.prev_pos
    }

    /// Stays put for a tick, as during hitstop, so rendering doesn't replay
    /// the last move.
    pub fn hold(&mut self) {
        self.prev_pos = self.pos;
    }

    /// Whole-pixel top-left, as bodies see it.
    pub fn px(&self) -> (i32, i32) {
        (self.pos.x.floor_px(), self.pos.y.floor_px())
    }

    pub fn surface(&self) -> Surface {
        let (x, y) = self.px();
        Surface { x, y, w: self.w }
    }

    /// Where along the path the platform is at `tick`, from 0 to `travel_ticks`.
    fn progress(&self, tick: u32) -> u32 {
        let leg = self.travel_ticks + self.pause_ticks;
        let t = tick % (2 * leg);
        if t < self.travel_ticks {
            t
        } else if t < leg {
            self.travel_ticks
        } else if t < leg + self.travel_ticks {
            self.travel_ticks - (t - leg)
        } else {
            0
        }
    }

    /// Advances one tick and moves any `riders` standing on it along with it.
    /// Returns the whole-pixel move.
    pub fn step(&mut self, map: &TileMap, riders: &mut [&mut Body]) -> (i32, i32) {
        let before = self.px();
        let surface = self.surface();
        let carried: Vec<bool> = riders.iter().map(|b| b.standing_on(surface)).collect();

        self.prev_pos = self.pos;
        self.tick += 1;
        let p = self.progress(self.tick) as i32;
        let n = self.travel_ticks as i32;
        let lerp = |a: Fx, b: Fx| a + (b - a) * p / n;
        self.pos = FxVec2::new(lerp(self.from.x, self.to.x), lerp(self.from.y, self.to.y));

        let after = self.px();
        let (dx, dy) = (after.0 - before.0, after.1 - before.1);
        for (body, ride) in riders.iter_mut().zip(carried) {
            if ride {
                // The map still holds last tick's surface, so a rider never
                // lands on the platform's old position while being carried.
                body.move_x(map, Fx::from_px(dx), true);
                body.move_y(map, Fx::from_px(dy), true);
            }
        }
        (dx, dy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room() -> TileMap {
        TileMap::from_ascii(&["..........", "..........", "..........", "##########"])
    }

    #[test]
    fn travels_pauses_and_returns() {
        let map = room();
        let mut p =
            MovingPlatform::new(32, FxVec2::from_px(0, 20), FxVec2::from_px(40, 20), 40, 10);
        for _ in 0..40 {
            p.step(&map, &mut []);
        }
        assert_eq!(p.px(), (40, 20));
        for _ in 0..10 {
            p.step(&map, &mut []);
        }
        assert_eq!(p.px(), (40, 20), "paused at the far end");
        for _ in 0..40 {
            p.step(&map, &mut []);
        }
        assert_eq!(p.px(), (0, 20), "back at the start");
    }

    #[test]
    fn carries_a_body_standing_on_it() {
        let mut map = room();
        let mut p = MovingPlatform::new(32, FxVec2::from_px(0, 30), FxVec2::from_px(40, 10), 40, 0);
        let mut body = Body::new(8, 30 - 12, 10, 12);
        map.surfaces_mut().clear();
        map.surfaces_mut().push(p.surface());
        assert!(body.on_ground(&map, false));
        for _ in 0..40 {
            p.step(&map, &mut [&mut body]);
            map.surfaces_mut()[0] = p.surface();
            assert!(body.on_ground(&map, false), "still riding at {:?}", p.px());
        }
        assert_eq!((body.x, body.y + body.h), (48, 10));
    }
}
