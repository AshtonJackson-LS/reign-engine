//! Rigidbodies and box colliders between scene objects.
//!
//! Deterministic, like the rest of the simulation: positions and velocities
//! are fixed-point, and each tick moves bodies one whole pixel at a time
//! (x, then y) and stops at the first pixel that would overlap another
//! collider, so nothing tunnels through thin geometry.
//!
//! - An object with a [`BoxCollider`] and no [`Rigidbody`] is static
//!   scenery: it blocks, but never moves.
//! - A **Dynamic** rigidbody falls with gravity, keeps its velocity, and
//!   stops (or bounces) when it hits a collider.
//! - A **Kinematic** rigidbody moves by its velocity alone, through
//!   anything, and pushes nothing, but dynamic bodies collide with it.
//! - A **Static** rigidbody never moves.
//!
//! Dynamic bodies move in object-id order, each seeing the others where
//! they already are this tick, so stacks settle the same way every time.
//!
//! A [`TilemapCollider`] makes the tilemap of the object's
//! [`TilemapRenderer`] solid: the asset's collision layer, on a grid of
//! [`TILE`]-pixel cells with its top-left at the object's position. `#`
//! cells block like box colliders; `-` cells are one-way platforms, solid
//! only to a body coming down onto their top edge. Slopes (`/`, `\`)
//! collide as whole solid cells for now.

use std::sync::Arc;

use reign::assets::{Handle, Tilemap};
use reign::core::{Fx, FxVec2, TICK_HZ};
use reign::scene::{Bounds, Component, ObjectId, Reflect, Scene};
use serde::{Deserialize, Serialize};

use crate::DB;
use crate::components::TilemapRenderer;

/// A tilemap cell's width and height in world pixels.
pub const TILE: i32 = reign::physics::TILE;

/// Downward pull in world pixels per second squared, before a body's
/// `gravity_scale`.
pub const GRAVITY: i32 = 640;
/// A collider with no size and nothing to fit is this big.
const DEFAULT_SIZE: [i32; 2] = [16, 16];
/// Bounces slower than this (px/s) come to rest instead.
const REST_SPEED: i32 = 24;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum BodyType {
    /// Falls, collides and bounces.
    #[default]
    Dynamic,
    /// Moves by its velocity only; blocks dynamic bodies.
    Kinematic,
    /// Never moves.
    Static,
}

/// Makes an object move under physics. Needs a [`BoxCollider`] to collide.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct Rigidbody {
    pub body_type: BodyType,
    /// Multiplies gravity: 0 floats, 2 falls twice as hard, -1 rises.
    pub gravity_scale: Fx,
    /// World pixels per second; set it to launch the body.
    pub velocity: FxVec2,
    /// Percent of speed kept when it hits something.
    #[reflect(range(0, 100))]
    pub bounciness: u8,
    /// Standing on something: its last fall this tick was stopped. Set by
    /// physics each tick, for scripts to read.
    #[reflect(read_only)]
    pub grounded: bool,
}

impl Default for Rigidbody {
    fn default() -> Self {
        Rigidbody {
            body_type: BodyType::Dynamic,
            gravity_scale: Fx::ONE,
            velocity: FxVec2::ZERO,
            bounciness: 0,
            grounded: false,
        }
    }
}

/// A solid box centred on the object's position (plus `offset`). A size
/// of 0 fits the object's sprite.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
#[component(bounds = "BoxCollider::bounds_hint")]
pub struct BoxCollider {
    pub size: [u16; 2],
    pub offset: [i16; 2],
    /// Blocks nothing; reports overlaps as trigger events instead.
    pub is_trigger: bool,
    /// Which layer (0-31) this collider is on.
    #[reflect(range(0, 31))]
    pub layer: u8,
    /// The layers it collides with, one bit each (bit 0 is layer 0). Two
    /// colliders interact only if each one's mask has the other's layer.
    pub mask: u32,
}

impl Default for BoxCollider {
    fn default() -> Self {
        BoxCollider { size: [0, 0], offset: [0, 0], is_trigger: false, layer: 0, mask: u32::MAX }
    }
}

impl BoxCollider {
    /// Sized, offset or otherwise like the default.
    pub fn sized(w: u16, h: u16) -> Self {
        BoxCollider { size: [w, h], ..BoxCollider::default() }
    }

    fn bounds_hint(&self) -> Option<Bounds> {
        let [w, h] = self.size.map(i32::from);
        let [ox, oy] = self.offset.map(i32::from);
        (w > 0 && h > 0).then(|| Bounds::new(ox - w / 2, oy - h / 2, w, h))
    }
}

/// Makes the tilemap drawn by the object's [`TilemapRenderer`] solid:
/// `#` tiles block rigidbodies, `-` tiles are one-way platforms. A
/// rigidbody touching it gets CollisionEnter/Exit with this object.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct TilemapCollider {
    /// Blocks nothing; reports bodies overlapping any collision tile as
    /// trigger events instead.
    pub is_trigger: bool,
    /// Which layer (0-31) the tiles are on.
    #[reflect(range(0, 31))]
    pub layer: u8,
    /// The layers it collides with, one bit each (bit 0 is layer 0).
    pub mask: u32,
}

impl Default for TilemapCollider {
    fn default() -> Self {
        TilemapCollider { is_trigger: false, layer: 0, mask: u32::MAX }
    }
}

/// A collider's box in world pixels, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }

    fn moved(self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..self }
    }
}

/// Two objects touching (or overlapping, for triggers) after a tick,
/// lowest id first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Contact {
    pub a: ObjectId,
    pub b: ObjectId,
    /// One of them is a trigger: they overlap rather than collide.
    pub trigger: bool,
}

impl Contact {
    /// The object in the contact that isn't `me`.
    pub fn other(&self, me: ObjectId) -> Option<ObjectId> {
        if self.a == me {
            Some(self.b)
        } else if self.b == me {
            Some(self.a)
        } else {
            None
        }
    }
}

/// Whether two colliders' layers and masks let them interact.
fn layers_meet(layer_a: u8, mask_a: u32, layer_b: u8, mask_b: u32) -> bool {
    mask_a & (1 << (layer_b & 31)) != 0 && mask_b & (1 << (layer_a & 31)) != 0
}

/// One collider this tick.
#[derive(Clone, Copy)]
struct Shape {
    id: ObjectId,
    rect: Rect,
    trigger: bool,
    layer: u8,
    mask: u32,
    /// It has a Rigidbody that can move (Dynamic or Kinematic).
    moves: bool,
}

impl Shape {
    fn interacts(&self, other: &Shape) -> bool {
        layers_meet(self.layer, self.mask, other.layer, other.mask)
    }

    /// Blocks `other` from moving into it.
    fn blocks(&self, other: &Shape) -> bool {
        !self.trigger && !other.trigger && self.interacts(other)
    }
}

/// What a collision-layer cell does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Empty,
    /// Blocks from every side (`#`; slopes `/` and `\` too, for now).
    Solid,
    /// Blocks only a body coming down onto its top edge (`-`).
    OneWay,
}

/// One tilemap collider this tick.
#[derive(Clone)]
struct Grid {
    id: ObjectId,
    /// The map's top-left corner in world pixels.
    x: i32,
    y: i32,
    /// Size in cells.
    w: i32,
    h: i32,
    map: Arc<Tilemap>,
    trigger: bool,
    layer: u8,
    mask: u32,
    moves: bool,
}

impl Grid {
    fn kind(&self, tx: i32, ty: i32) -> TileKind {
        if tx < 0 || ty < 0 {
            return TileKind::Empty;
        }
        let Some(row) = self.map.collision.get(ty as usize) else { return TileKind::Empty };
        let c = if row.is_ascii() {
            row.as_bytes().get(tx as usize).map(|&b| b as char)
        } else {
            row.chars().nth(tx as usize)
        };
        match c {
            Some('#' | '/' | '\\') => TileKind::Solid,
            Some('-') => TileKind::OneWay,
            _ => TileKind::Empty,
        }
    }

    /// The cell's top edge in world pixels.
    fn top(&self, ty: i32) -> i32 {
        self.y + ty * TILE
    }

    fn cell_rect(&self, tx: i32, ty: i32) -> Rect {
        Rect { x: self.x + tx * TILE, y: self.top(ty), w: TILE, h: TILE }
    }

    /// The non-empty cells `r` covers.
    fn cells(&self, r: Rect) -> impl Iterator<Item = (i32, i32, TileKind)> + '_ {
        let tx0 = (r.x - self.x).div_euclid(TILE).max(0);
        let tx1 = (r.x + r.w - 1 - self.x).div_euclid(TILE).min(self.w - 1);
        let ty0 = (r.y - self.y).div_euclid(TILE).max(0);
        let ty1 = (r.y + r.h - 1 - self.y).div_euclid(TILE).min(self.h - 1);
        (ty0..=ty1)
            .flat_map(move |ty| (tx0..=tx1).map(move |tx| (tx, ty)))
            .map(|(tx, ty)| (tx, ty, self.kind(tx, ty)))
            .filter(|&(_, _, k)| k != TileKind::Empty)
    }

    fn interacts(&self, s: &Shape) -> bool {
        layers_meet(self.layer, self.mask, s.layer, s.mask)
    }

    /// Stops `s` from moving into it.
    fn blocks(&self, s: &Shape) -> bool {
        !self.trigger && !s.trigger && self.interacts(s)
    }

    /// A box at `now` can't move one pixel to `next`: `next` enters a
    /// solid cell `now` isn't already inside (so stuck bodies can work
    /// free), or comes down onto the top of a one-way cell.
    fn stops(&self, next: Rect, now: Rect) -> bool {
        let down = next.y > now.y;
        self.cells(next).any(|(tx, ty, kind)| match kind {
            TileKind::Solid => !now.overlaps(&self.cell_rect(tx, ty)),
            TileKind::OneWay => down && now.y + now.h == self.top(ty),
            TileKind::Empty => false,
        })
    }

    /// `r` overlaps a collision cell (as a trigger sees it), or touches
    /// it: edge to edge with a solid cell, or standing on a one-way one.
    fn touches(&self, r: Rect, trigger: bool) -> bool {
        if trigger {
            return self.cells(r).next().is_some();
        }
        let solid = |q: Rect| self.cells(q).any(|(_, _, k)| k == TileKind::Solid);
        [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dy)| solid(r.moved(dx, dy)))
            || self
                .cells(r.moved(0, 1))
                .any(|(_, ty, k)| k == TileKind::OneWay && r.y + r.h == self.top(ty))
    }
}

/// Loads the tilemap asset behind a handle, if it loads.
pub trait TilemapSource: Fn(Handle<Tilemap>) -> Option<Arc<Tilemap>> {}
impl<F: Fn(Handle<Tilemap>) -> Option<Arc<Tilemap>>> TilemapSource for F {}

/// The project's tilemaps, from the asset database.
pub fn project_tilemaps(handle: Handle<Tilemap>) -> Option<Arc<Tilemap>> {
    DB.get()?.load(handle).ok()
}

fn grids(scene: &Scene, tilemaps: &impl TilemapSource) -> Vec<Grid> {
    scene
        .iter::<TilemapCollider>()
        .filter_map(|(id, c)| {
            let map = tilemaps(scene.get::<TilemapRenderer>(id)?.tilemap)?;
            let p = scene.world_position(id);
            Some(Grid {
                id,
                x: p.x.round_px(),
                y: p.y.round_px(),
                w: map.width() as i32,
                h: map.height() as i32,
                map,
                trigger: c.is_trigger,
                layer: c.layer,
                mask: c.mask,
                moves: scene.get::<Rigidbody>(id).is_some_and(|b| b.body_type != BodyType::Static),
            })
        })
        .collect()
}

/// Outlines of every tilemap collider's tiles inside `view`, as one-pixel
/// rectangles: the outer edges of solid areas, and the top edge of each
/// one-way tile (`true`).
pub fn tile_outlines(scene: &Scene, view: Rect) -> Vec<(Rect, bool)> {
    outlines(scene, view, &project_tilemaps)
}

fn outlines(scene: &Scene, view: Rect, tilemaps: &impl TilemapSource) -> Vec<(Rect, bool)> {
    let mut out = Vec::new();
    for g in grids(scene, tilemaps) {
        for (tx, ty, kind) in g.cells(view) {
            let c = g.cell_rect(tx, ty);
            match kind {
                TileKind::OneWay => out.push((Rect { h: 1, ..c }, true)),
                TileKind::Solid => {
                    let open = |dx, dy| g.kind(tx + dx, ty + dy) != TileKind::Solid;
                    if open(0, -1) {
                        out.push((Rect { h: 1, ..c }, false));
                    }
                    if open(0, 1) {
                        out.push((Rect { y: c.y + c.h - 1, h: 1, ..c }, false));
                    }
                    if open(-1, 0) {
                        out.push((Rect { w: 1, ..c }, false));
                    }
                    if open(1, 0) {
                        out.push((Rect { x: c.x + c.w - 1, w: 1, ..c }, false));
                    }
                }
                TileKind::Empty => {}
            }
        }
    }
    out
}

fn shapes(scene: &Scene, fit: &impl Fn(&Scene, ObjectId) -> Option<[i32; 2]>) -> Vec<Shape> {
    scene
        .iter::<BoxCollider>()
        .map(|(id, c)| {
            let fitted = fit(scene, id).unwrap_or(DEFAULT_SIZE);
            let w = if c.size[0] > 0 { c.size[0] as i32 } else { fitted[0] };
            let h = if c.size[1] > 0 { c.size[1] as i32 } else { fitted[1] };
            let p = scene.world_position(id);
            let (x, y) = (p.x.round_px() + c.offset[0] as i32, p.y.round_px() + c.offset[1] as i32);
            let moves = scene.get::<Rigidbody>(id).is_some_and(|b| b.body_type != BodyType::Static);
            Shape {
                id,
                rect: Rect { x: x - w / 2, y: y - h / 2, w, h },
                trigger: c.is_trigger,
                layer: c.layer,
                mask: c.mask,
                moves,
            }
        })
        .collect()
}

/// Every active collider's box. `fit` gives an object's sprite size, for
/// colliders sized 0.
pub fn colliders(
    scene: &Scene,
    fit: &impl Fn(&Scene, ObjectId) -> Option<[i32; 2]>,
) -> Vec<(ObjectId, Rect)> {
    shapes(scene, fit).into_iter().map(|s| (s.id, s.rect)).collect()
}

/// Who touches whom: solid colliders touching edge to edge (or
/// overlapping), and triggers overlapping anything. At least one of each
/// pair has a moving Rigidbody, so scenery never reports touching scenery.
fn contacts(shapes: &[Shape], grids: &[Grid]) -> Vec<Contact> {
    let mut out = Vec::new();
    for g in grids {
        for s in shapes {
            if s.id == g.id || !(s.moves || g.moves) || !g.interacts(s) {
                continue;
            }
            let trigger = s.trigger || g.trigger;
            if g.touches(s.rect, trigger) {
                let (lo, hi) = if s.id < g.id { (s.id, g.id) } else { (g.id, s.id) };
                out.push(Contact { a: lo, b: hi, trigger });
            }
        }
    }
    for (i, a) in shapes.iter().enumerate() {
        for b in &shapes[i + 1..] {
            if !(a.moves || b.moves) || !a.interacts(b) {
                continue;
            }
            let trigger = a.trigger || b.trigger;
            let touching = if trigger {
                a.rect.overlaps(&b.rect)
            } else {
                a.rect.overlaps(&b.rect)
                    || [(1, 0), (-1, 0), (0, 1), (0, -1)]
                        .iter()
                        .any(|&(dx, dy)| a.rect.moved(dx, dy).overlaps(&b.rect))
            };
            if touching {
                let (lo, hi) = if a.id < b.id { (a.id, b.id) } else { (b.id, a.id) };
                out.push(Contact { a: lo, b: hi, trigger });
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Advances every rigidbody by one fixed tick, and returns who touches
/// whom afterwards. Tilemaps load from the project's assets.
pub fn step(
    scene: &mut Scene,
    fit: &impl Fn(&Scene, ObjectId) -> Option<[i32; 2]>,
) -> Vec<Contact> {
    step_with(scene, fit, &project_tilemaps)
}

/// [`step`], with tilemaps from `tilemaps` instead of the project.
pub fn step_with(
    scene: &mut Scene,
    fit: &impl Fn(&Scene, ObjectId) -> Option<[i32; 2]>,
    tilemaps: &impl TilemapSource,
) -> Vec<Contact> {
    let mut boxes = shapes(scene, fit);
    let mut grids = grids(scene, tilemaps);
    let bodies: Vec<(ObjectId, Rigidbody)> =
        scene.iter::<Rigidbody>().map(|(id, b)| (id, b.clone())).collect();
    let hz = TICK_HZ as i32;

    for (id, mut body) in bodies {
        match body.body_type {
            BodyType::Static => continue,
            BodyType::Kinematic => {
                let from = scene.world_position(id);
                let to = from + FxVec2::new(body.velocity.x / hz, body.velocity.y / hz);
                scene.set_world_position(id, to);
                let (dx, dy) =
                    (to.x.round_px() - from.x.round_px(), to.y.round_px() - from.y.round_px());
                if let Some(s) = boxes.iter_mut().find(|s| s.id == id) {
                    s.rect = s.rect.moved(dx, dy);
                }
                if let Some(g) = grids.iter_mut().find(|g| g.id == id) {
                    g.x += dx;
                    g.y += dy;
                }
                continue;
            }
            BodyType::Dynamic => {}
        }

        body.velocity.y += Fx::from_px(GRAVITY) * body.gravity_scale / hz;
        body.grounded = false;
        let mut pos = scene.world_position(id);
        let me = boxes.iter().position(|s| s.id == id);
        let blocked = |rect: Rect, now: Rect, boxes: &[Shape]| {
            let mine = &boxes[me.expect("only checked with a collider")];
            // Only things it isn't already inside block it, so overlapping
            // objects can separate. Triggers and other layers never block.
            boxes.iter().any(|s| {
                s.id != id && s.blocks(mine) && rect.overlaps(&s.rect) && !now.overlaps(&s.rect)
            }) || grids.iter().any(|g| g.id != id && g.blocks(mine) && g.stops(rect, now))
        };

        for axis in [0, 1] {
            let v = if axis == 0 { body.velocity.x } else { body.velocity.y };
            let start = if axis == 0 { pos.x } else { pos.y };
            let target = start + v / hz;
            let pixels = target.round_px() - start.round_px();
            let mut hit = false;
            let mut moved = 0;
            if let Some(i) = me {
                let step = pixels.signum();
                // Resting against something: even a sub-pixel push into it
                // is a hit, so bodies settle on whole pixels.
                if pixels == 0 && v != Fx::ZERO {
                    let now = boxes[i].rect;
                    let dir = v.signum();
                    let next = if axis == 0 { now.moved(dir, 0) } else { now.moved(0, dir) };
                    hit = blocked(next, now, &boxes);
                }
                while !hit && moved != pixels {
                    let now = boxes[i].rect;
                    let next = if axis == 0 { now.moved(step, 0) } else { now.moved(0, step) };
                    if blocked(next, now, &boxes) {
                        hit = true;
                        break;
                    }
                    boxes[i].rect = next;
                    moved += step;
                }
            }
            let end = if hit { Fx::from_px(start.round_px() + moved) } else { target };
            if axis == 0 {
                pos.x = end
            } else {
                pos.y = end
            }
            if hit && axis == 1 && v > Fx::ZERO {
                body.grounded = true;
            }
            if hit {
                let bounced = -v * body.bounciness as i32 / 100;
                let bounced =
                    if bounced.abs() < Fx::from_px(REST_SPEED) { Fx::ZERO } else { bounced };
                if axis == 0 { body.velocity.x = bounced } else { body.velocity.y = bounced }
            }
        }

        scene.set_world_position(id, pos);
        if let Some(rb) = scene.get_mut::<Rigidbody>(id) {
            rb.velocity = body.velocity;
            rb.grounded = body.grounded;
        }
    }
    contacts(&boxes, &grids)
}

#[cfg(test)]
#[path = "tilemap_tests.rs"]
mod tilemap_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn fit16(_: &Scene, _: ObjectId) -> Option<[i32; 2]> {
        Some([16, 16])
    }

    fn ground(scene: &mut Scene, y: i32) -> ObjectId {
        let id = scene.spawn("Ground");
        scene.set_world_position(id, FxVec2::from_px(0, y));
        scene.insert(id, BoxCollider::sized(400, 16));
        id
    }

    fn crate_at(scene: &mut Scene, x: i32, y: i32) -> ObjectId {
        let id = scene.spawn("Box");
        scene.set_world_position(id, FxVec2::from_px(x, y));
        scene.insert(id, BoxCollider::default());
        scene.insert(id, Rigidbody::default());
        id
    }

    fn run(scene: &mut Scene, ticks: u32) {
        for _ in 0..ticks {
            step(scene, &fit16);
        }
    }

    #[test]
    fn a_box_falls_and_lands_on_the_ground() {
        let mut scene = Scene::new();
        ground(&mut scene, 100);
        let b = crate_at(&mut scene, 0, 0);
        run(&mut scene, 240);
        // Ground's top is 92; the box's centre sits 8 above that.
        assert_eq!(scene.world_position(b), FxVec2::from_px(0, 84));
        assert_eq!(scene.get::<Rigidbody>(b).unwrap().velocity.y, Fx::ZERO);
        assert!(scene.get::<Rigidbody>(b).unwrap().grounded, "resting on the ground");
    }

    #[test]
    fn boxes_stack() {
        let mut scene = Scene::new();
        ground(&mut scene, 100);
        let low = crate_at(&mut scene, 0, 40);
        let high = crate_at(&mut scene, 4, 0);
        run(&mut scene, 300);
        assert_eq!(scene.world_position(low).y, Fx::from_px(84));
        assert_eq!(scene.world_position(high).y, Fx::from_px(68));
    }

    #[test]
    fn walls_stop_sideways_motion() {
        let mut scene = Scene::new();
        let wall = scene.spawn("Wall");
        scene.set_world_position(wall, FxVec2::from_px(100, 0));
        scene.insert(wall, BoxCollider::sized(16, 200));
        let b = crate_at(&mut scene, 0, 0);
        let rb = scene.get_mut::<Rigidbody>(b).unwrap();
        rb.gravity_scale = Fx::ZERO;
        rb.velocity = FxVec2::from_px(600, 0);
        run(&mut scene, 60);
        // Wall's left edge is 92; the box's right edge stops there.
        assert_eq!(scene.world_position(b).x, Fx::from_px(84));
    }

    #[test]
    fn bouncy_boxes_bounce_then_settle() {
        let mut scene = Scene::new();
        ground(&mut scene, 100);
        let b = crate_at(&mut scene, 0, 0);
        scene.get_mut::<Rigidbody>(b).unwrap().bounciness = 70;
        let mut rose = false;
        for _ in 0..600 {
            let before = scene.world_position(b).y;
            step(&mut scene, &fit16);
            rose |= scene.world_position(b).y < before;
        }
        assert!(rose, "it bounced");
        assert_eq!(scene.world_position(b).y, Fx::from_px(84), "and came to rest");
    }

    #[test]
    fn kinematic_bodies_ignore_gravity_and_collisions() {
        let mut scene = Scene::new();
        ground(&mut scene, 100);
        let k = crate_at(&mut scene, 0, 0);
        let rb = scene.get_mut::<Rigidbody>(k).unwrap();
        rb.body_type = BodyType::Kinematic;
        rb.velocity = FxVec2::from_px(0, 240);
        run(&mut scene, 120);
        assert_eq!(scene.world_position(k).y, Fx::from_px(240));
    }

    #[test]
    fn the_same_scene_always_ends_the_same() {
        let build = || {
            let mut scene = Scene::new();
            ground(&mut scene, 200);
            for i in 0..6 {
                let b = crate_at(&mut scene, i * 7 - 20, -i * 30);
                scene.get_mut::<Rigidbody>(b).unwrap().bounciness = (i * 15) as u8;
            }
            run(&mut scene, 500);
            scene
        };
        assert!(build() == build());
    }
}
