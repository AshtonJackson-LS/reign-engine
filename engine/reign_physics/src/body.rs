use reign_core::{Fx, FxVec2};

use crate::{Aabb, Surface, TILE, TileMap};

/// An axis-aligned box that moves through a [`TileMap`].
///
/// Walls and ceilings collide with the box. Floors collide with its bottom
/// row, and slopes with a single foot point at the bottom centre, which is
/// what lets a body walk up a ramp or land on a platform it jumped through.
///
/// On a 45-degree ramp the box's leading corner pokes into the ground ahead
/// by up to half its width. So while grounded, wall and ceiling checks skip
/// the bottom `step_height` pixels of the box.
#[derive(
    Clone,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    reign_scene::Reflect,
)]
pub struct Body {
    /// Top-left corner in whole world pixels.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    #[serde(default)]
    #[reflect(read_only)]
    remainder: FxVec2,
}

/// Most pixels a body is lifted per pixel walked, enough for 45-degree slopes.
const MAX_STEP_UP: i32 = 2;

impl Body {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Body { x, y, w, h, remainder: FxVec2::ZERO }
    }

    /// Exact position including the subpixel remainder, for interpolation.
    pub fn pos(&self) -> FxVec2 {
        FxVec2::new(Fx::from_px(self.x) + self.remainder.x, Fx::from_px(self.y) + self.remainder.y)
    }

    /// The box in whole pixels, for hit and hurt checks.
    pub fn aabb(&self) -> Aabb {
        Aabb::new(self.x, self.y, self.w, self.h)
    }

    fn foot_x(&self, x: i32) -> i32 {
        x + self.w / 2
    }

    fn foot_in_slope(&self, map: &TileMap, x: i32, y: i32) -> bool {
        map.slope_at(self.foot_x(x), y + self.h - 1)
    }

    /// How far a 45-degree ramp can push the box's corner into the ground.
    fn step_height(&self) -> i32 {
        self.w / 2 + 1
    }

    /// The box minus its bottom `step_height` pixels, for wall and ceiling
    /// checks that must ignore a ramp poking into the corner.
    fn upper_hits_solid(&self, map: &TileMap, x: i32, y: i32) -> bool {
        map.rect_hits_solid(x, y, self.w, (self.h - self.step_height()).max(1))
    }

    fn hits_wall(&self, map: &TileMap, x: i32, y: i32, grounded: bool) -> bool {
        if grounded {
            self.upper_hits_solid(map, x, y)
        } else {
            map.rect_hits_solid(x, y, self.w, self.h)
        }
    }

    /// Whether the body's bottom would rest in something at (`x`, `y`).
    /// With the foot over a slope, only the slope counts, so the body follows
    /// the ramp instead of hanging off the flat ground beside it.
    fn blocked_below(&self, map: &TileMap, x: i32, y: i32, drop_through: bool) -> bool {
        let bottom = y + self.h - 1;
        let foot = self.foot_x(x);
        if map.slope_tile_at(foot, bottom) || map.slope_tile_at(foot, bottom + 1) {
            return self.foot_in_slope(map, x, y);
        }
        map.rect_hits_solid(x, bottom, self.w, 1)
            || self.foot_in_slope(map, x, y)
            || (!drop_through && map.one_way_top(x, self.w, bottom))
    }

    /// Standing on something: solid ground, a slope or a one-way platform.
    pub fn on_ground(&self, map: &TileMap, drop_through: bool) -> bool {
        self.blocked_below(map, self.x, self.y + 1, drop_through)
    }

    /// Feet resting exactly on `surface`.
    pub fn standing_on(&self, surface: Surface) -> bool {
        self.y + self.h == surface.y
            && self.x < surface.x + surface.w
            && surface.x < self.x + self.w
    }

    /// Standing on a one-way platform and nothing firmer.
    pub fn on_one_way_only(&self, map: &TileMap) -> bool {
        self.on_ground(map, false) && !self.on_ground(map, true)
    }

    /// Side of a solid wall the body is touching: -1 left, 1 right, 0 none.
    pub fn wall_side(&self, map: &TileMap) -> i32 {
        if self.upper_hits_solid(map, self.x + 1, self.y) {
            1
        } else if self.upper_hits_solid(map, self.x - 1, self.y) {
            -1
        } else {
            0
        }
    }

    /// Moves horizontally. Walks up slopes, and with `grounded` also follows
    /// slopes down instead of stepping off them. Returns true if a wall
    /// stopped the move.
    pub fn move_x(&mut self, map: &TileMap, amount: Fx, grounded: bool) -> bool {
        self.remainder.x += amount;
        let mut pixels = self.remainder.x.round_px();
        self.remainder.x -= Fx::from_px(pixels);
        let step = pixels.signum();
        while pixels != 0 {
            let nx = self.x + step;
            let mut ny = self.y;
            let mut lifted = 0;
            while self.foot_in_slope(map, nx, ny) && lifted < MAX_STEP_UP {
                ny -= 1;
                lifted += 1;
            }
            if self.hits_wall(map, nx, ny, grounded) || self.foot_in_slope(map, nx, ny) {
                self.remainder.x = Fx::ZERO;
                return true;
            }
            self.x = nx;
            self.y = ny;
            if grounded && lifted == 0 {
                for _ in 0..MAX_STEP_UP {
                    if !self.on_ground(map, false)
                        && self.blocked_below(map, self.x, self.y + 2, false)
                    {
                        self.y += 1;
                    }
                }
            }
            pixels -= step;
        }
        false
    }

    /// Moves vertically. Returns true if something stopped the move: a
    /// ceiling going up, or the ground going down.
    pub fn move_y(&mut self, map: &TileMap, amount: Fx, drop_through: bool) -> bool {
        self.remainder.y += amount;
        let mut pixels = self.remainder.y.round_px();
        self.remainder.y -= Fx::from_px(pixels);
        let step = pixels.signum();
        while pixels != 0 {
            let ny = self.y + step;
            let blocked = if step > 0 {
                self.blocked_below(map, self.x, ny, drop_through)
            } else {
                self.upper_hits_solid(map, self.x, ny)
            };
            if blocked {
                self.remainder.y = Fx::ZERO;
                return true;
            }
            self.y = ny;
            pixels -= step;
        }
        false
    }

    /// The tile row index the body's feet are in, for debugging and tests.
    pub fn feet_tile_row(&self) -> i32 {
        (self.y + self.h - 1).div_euclid(TILE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8x4 tiles: a floor, a one-way platform, a wall and a ramp.
    fn room() -> TileMap {
        TileMap::from_ascii(&[
            "........", //
            "..--....", //
            ".....#./", //
            "########",
        ])
    }

    fn px(v: i32) -> Fx {
        Fx::from_px(v)
    }

    #[test]
    fn falls_and_lands_on_the_floor() {
        let map = room();
        let mut body = Body::new(2, 0, 10, 12);
        assert!(body.move_y(&map, px(100), false));
        assert_eq!(body.y + body.h, 48);
        assert!(body.on_ground(&map, false));
    }

    #[test]
    fn walls_stop_horizontal_moves() {
        let map = room();
        let mut body = Body::new(60, 36, 10, 12);
        assert!(body.move_x(&map, px(40), true));
        assert_eq!(body.x + body.w, 80);
        assert_eq!(body.wall_side(&map), 1);
    }

    #[test]
    fn one_way_lets_bodies_up_and_catches_them_coming_down() {
        let map = room();
        let mut body = Body::new(34, 30, 10, 12);
        assert!(!body.move_y(&map, px(-30), false), "jumps up through the platform");
        assert!(body.move_y(&map, px(20), false), "lands on it falling back");
        assert_eq!(body.y + body.h, 16);
        assert!(body.on_one_way_only(&map));
        assert!(!body.move_y(&map, px(10), true), "drops through when asked");
    }

    #[test]
    fn walks_up_a_slope() {
        let map = room();
        // Standing on the floor just left of the ramp tile at x 112..128.
        let mut body = Body::new(98, 36, 10, 12);
        assert!(!body.move_x(&map, px(12), true));
        assert!(body.y < 36, "body rose onto the ramp, y = {}", body.y);
        assert!(body.on_ground(&map, false));
    }

    #[test]
    fn walks_over_a_two_tile_hill() {
        // A ramp two tiles high, a plateau, and a ramp back down. The box's
        // leading corner reaches over solid tiles that sit under slopes.
        let map = TileMap::from_ascii(&[
            "..............",  //
            "...../##\\.....", //
            "..../####\\....", //
            "##############",
        ]);
        let mut body = Body::new(20, 18, 14, 30);
        let mut highest = body.y;
        let mut lowest_while_descending = 0;
        for _ in 0..170 {
            body.move_x(&map, Fx::from_px(1), body.on_ground(&map, false));
            body.move_y(&map, Fx::from_px(1), false);
            highest = highest.min(body.y);
            if body.x > 110 && body.x < 150 {
                assert!(body.on_ground(&map, false), "follows the ramp down at x = {}", body.x);
                lowest_while_descending = lowest_while_descending.max(body.y);
            }
        }
        assert!(body.x > 170, "crossed the hill, x = {}", body.x);
        assert_eq!(highest + body.h, 16, "stood on the plateau at the top");
        assert_eq!(body.y + body.h, 48, "back on the floor");
    }

    #[test]
    fn subpixel_moves_accumulate() {
        let map = room();
        let mut body = Body::new(10, 36, 10, 12);
        for _ in 0..4 {
            body.move_x(&map, Fx::from_raw(64), true);
        }
        assert_eq!(body.x, 11);
    }
}
