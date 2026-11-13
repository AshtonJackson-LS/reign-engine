//! Collision tiles. This is the invisible layer under the art: rooms are
//! drawn with freely placed sprites, and this grid only says what is solid.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Tile size in world pixels.
pub const TILE: i32 = 16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Tile {
    #[default]
    Empty,
    Solid,
    /// Solid only from above; bodies jump up through it and can drop down.
    OneWay,
    /// 45-degree floor rising to the right, like `/`.
    SlopeUpRight,
    /// 45-degree floor rising to the left, like `\`.
    SlopeUpLeft,
}

/// A thin surface that is solid only from above, like a one-way tile but
/// anywhere and any width. Moving platforms update theirs every tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Surface {
    pub x: i32,
    /// The pixel row a body lands on.
    pub y: i32,
    pub w: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileMap {
    width: i32,
    height: i32,
    tiles: Arc<[Tile]>,
    collision_hash: u64,
    surfaces: Vec<Surface>,
}

// Keep the original field-by-field fingerprint for asset equivalence tests.
impl Hash for TileMap {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.width.hash(state);
        self.height.hash(state);
        self.tiles.hash(state);
        self.surfaces.hash(state);
    }
}

impl TileMap {
    /// Builds a map from rows of characters: `#` solid, `-` one-way,
    /// `/` and `\` slopes, anything else empty. Short rows are padded.
    pub fn from_ascii(rows: &[&str]) -> Self {
        let width = rows.iter().map(|r| r.chars().count()).max().unwrap_or(0) as i32;
        let height = rows.len() as i32;
        let mut tiles = vec![Tile::Empty; (width * height) as usize];
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                tiles[y * width as usize + x] = match c {
                    '#' => Tile::Solid,
                    '-' => Tile::OneWay,
                    '/' => Tile::SlopeUpRight,
                    '\\' => Tile::SlopeUpLeft,
                    _ => Tile::Empty,
                };
            }
        }
        let collision_hash = reign_core::checksum(&(width, height, &tiles));
        TileMap { width, height, tiles: tiles.into(), collision_hash, surfaces: Vec::new() }
    }

    /// Hash static collision in constant time, and moving surfaces in full.
    /// Use this in a world's rollback checksum; `Hash` retains the original
    /// field-by-field map fingerprint for recording/asset compatibility.
    pub fn hash_cached<H: Hasher>(&self, state: &mut H) {
        self.collision_hash.hash(state);
        self.surfaces.hash(state);
    }

    /// Edits collision and refreshes its digest. Out-of-bounds edits fail.
    /// Snapshots share immutable tiles until an edit makes them unique.
    pub fn set_tile(&mut self, x: i32, y: i32, tile: Tile) -> bool {
        if x < 0 || y < 0 || x >= self.width || y >= self.height {
            return false;
        }
        let index = (y * self.width + x) as usize;
        if self.tiles[index] != tile {
            Arc::make_mut(&mut self.tiles)[index] = tile;
            self.collision_hash = reign_core::checksum(&(self.width, self.height, &*self.tiles));
        }
        true
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// Visibility and melee obstruction between two world points. One-way
    /// platforms pass rays; solid tiles and the solid part of slopes stop them.
    pub fn line_clear(&self, from: (i32, i32), to: (i32, i32)) -> bool {
        let (mut x, mut y) = from;
        let dx = (to.0 - x).abs();
        let dy = -(to.1 - y).abs();
        let sx = (to.0 - x).signum();
        let sy = (to.1 - y).signum();
        let mut error = dx + dy;
        loop {
            if self.rect_hits_solid(x, y, 1, 1) || self.slope_at(x, y) {
                return false;
            }
            if (x, y) == to {
                return true;
            }
            let twice = error * 2;
            if twice >= dy {
                error += dy;
                x += sx;
            }
            if twice <= dx {
                error += dx;
                y += sy;
            }
        }
    }

    /// Outside the map, the sides and floor are solid and the sky is open, so
    /// bodies can't fall out of a room but can jump above its top edge.
    pub fn tile(&self, tx: i32, ty: i32) -> Tile {
        if ty < 0 && tx >= 0 && tx < self.width {
            return Tile::Empty;
        }
        if tx < 0 || ty < 0 || tx >= self.width || ty >= self.height {
            return Tile::Solid;
        }
        self.tiles[(ty * self.width + tx) as usize]
    }

    /// A solid tile with a slope on top is the slope's body. It collides
    /// like the slope (through the feet only), otherwise a box walking up a
    /// ramp would catch its leading corner on the tile below the next slope.
    fn slope_backed(&self, tx: i32, ty: i32) -> bool {
        self.tile(tx, ty) == Tile::Solid
            && matches!(self.tile(tx, ty - 1), Tile::SlopeUpRight | Tile::SlopeUpLeft)
    }

    /// True if the rectangle overlaps any fully solid tile.
    pub fn rect_hits_solid(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        let (tx0, ty0) = (x.div_euclid(TILE), y.div_euclid(TILE));
        let (tx1, ty1) = ((x + w - 1).div_euclid(TILE), (y + h - 1).div_euclid(TILE));
        (ty0..=ty1).any(|ty| {
            (tx0..=tx1).any(|tx| self.tile(tx, ty) == Tile::Solid && !self.slope_backed(tx, ty))
        })
    }

    /// True if the pixel is inside a slope: the solid part of a slope tile,
    /// or a solid tile underneath one.
    pub fn slope_at(&self, px: i32, py: i32) -> bool {
        let (tx, ty) = (px.div_euclid(TILE), py.div_euclid(TILE));
        let (lx, ly) = (px.rem_euclid(TILE), py.rem_euclid(TILE));
        match self.tile(tx, ty) {
            Tile::SlopeUpRight => ly >= TILE - 1 - lx,
            Tile::SlopeUpLeft => ly >= lx,
            Tile::Solid => self.slope_backed(tx, ty),
            _ => false,
        }
    }

    /// True if the pixel lies in a slope tile, solid part or not.
    pub fn slope_tile_at(&self, px: i32, py: i32) -> bool {
        matches!(
            self.tile(px.div_euclid(TILE), py.div_euclid(TILE)),
            Tile::SlopeUpRight | Tile::SlopeUpLeft
        )
    }

    /// Surfaces that are not tiles, such as moving platforms.
    pub fn surfaces(&self) -> &[Surface] {
        &self.surfaces
    }

    pub fn surfaces_mut(&mut self) -> &mut Vec<Surface> {
        &mut self.surfaces
    }

    /// True if pixel row `row` is the top row of a one-way tile or a surface
    /// anywhere under the span `x..x + w`.
    pub fn one_way_top(&self, x: i32, w: i32, row: i32) -> bool {
        let on_surface = self.surfaces.iter().any(|s| s.y == row && x < s.x + s.w && s.x < x + w);
        if on_surface {
            return true;
        }
        if row.rem_euclid(TILE) != 0 {
            return false;
        }
        let ty = row.div_euclid(TILE);
        (x.div_euclid(TILE)..=(x + w - 1).div_euclid(TILE))
            .any(|tx| self.tile(tx, ty) == Tile::OneWay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached(map: &TileMap) -> u64 {
        let mut h = reign_core::checksum::Fnv64::default();
        map.hash_cached(&mut h);
        h.finish()
    }

    #[test]
    fn cached_collision_tracks_edits_surfaces_and_snapshot_restore() {
        let mut map = TileMap::from_ascii(&[".#", "-/"]);
        let snapshot = map.clone();
        let before = cached(&map);
        assert!(map.set_tile(0, 0, Tile::Solid));
        assert_ne!(cached(&map), before);
        assert_eq!(cached(&snapshot), before);
        assert_eq!(snapshot.tile(0, 0), Tile::Empty);
        assert!(map.set_tile(0, 0, Tile::Empty));
        assert_eq!(cached(&map), before);
        assert_eq!(map, snapshot);
        assert!(!map.set_tile(-1, 0, Tile::Solid));
        assert_eq!(cached(&map), before);
        map.surfaces_mut().push(Surface { x: 0, y: 20, w: 16 });
        assert_ne!(cached(&map), before);
        map.surfaces_mut().clear();
        assert_eq!(cached(&map), before);
        let rebuilt = TileMap::from_ascii(&[".#", "-/"]);
        assert_eq!(cached(&map), cached(&rebuilt));
        assert_eq!(reign_core::checksum(&map), reign_core::checksum(&rebuilt));
    }

    #[test]
    fn sight_and_melee_stop_at_walls_but_pass_one_way_platforms() {
        let map = TileMap::from_ascii(&["..#..", "..#..", "-----", "....."]);
        assert!(!map.line_clear((8, 8), (72, 8)));
        assert!(!map.line_clear((72, 8), (8, 8)));
        assert!(map.line_clear((8, 8), (8, 56)));
        assert!(map.line_clear((8, 8), (8, 8)));
        assert!(!map.line_clear((40, 8), (40, 8)));
    }

    #[test]
    fn parses_ascii_and_pads_rows() {
        let map = TileMap::from_ascii(&["#-/", "\\"]);
        assert_eq!((map.width(), map.height()), (3, 2));
        assert_eq!(map.tile(1, 0), Tile::OneWay);
        assert_eq!(map.tile(0, 1), Tile::SlopeUpLeft);
        assert_eq!(map.tile(2, 1), Tile::Empty);
    }

    #[test]
    fn outside_is_walled_but_open_above() {
        let map = TileMap::from_ascii(&["..", ".."]);
        assert_eq!(map.tile(0, -1), Tile::Empty);
        assert_eq!(map.tile(-1, 0), Tile::Solid);
        assert_eq!(map.tile(0, 2), Tile::Solid);
    }

    #[test]
    fn slope_surface_is_diagonal() {
        let map = TileMap::from_ascii(&["/\\"]);
        // `/`: the top-right pixel is solid, the top-left is not.
        assert!(map.slope_at(15, 0));
        assert!(!map.slope_at(0, 0));
        assert!(map.slope_at(0, 15));
        // `\`: mirrored.
        assert!(map.slope_at(16, 0));
        assert!(!map.slope_at(31, 0));
    }
}
