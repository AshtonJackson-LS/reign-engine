//! Picking and moving objects in the Game view while not playing.
//!
//! Click selects the topmost object under the cursor: the one with the
//! highest `z`, and of those the one drawn last (later in hierarchy order).
//! Its click box is the first component's [`Bounds`], or a default box.
//! Dragging moves the selected object, snapped to whole pixels, or to the
//! 16-pixel tile grid while Ctrl is held.

use egui::{Color32, Pos2, Rect, Response, Stroke, StrokeKind, vec2};
use reign_core::FxVec2;
use reign_render::{VIEW_HEIGHT, VIEW_WIDTH};
use reign_scene::{Bounds, ObjectId, Scene};

/// The click box for objects whose components don't give one: a 16-pixel
/// square standing on the position.
pub const DEFAULT_BOUNDS: Bounds = Bounds::new(-8, -16, 16, 16);
pub const TILE: i32 = 16;

const SELECTED: Color32 = Color32::from_rgb(255, 196, 64);
const HOVERED: Color32 = Color32::from_rgba_premultiplied(160, 200, 255, 160);

/// How world pixels map onto the Game view.
#[derive(Clone, Copy, Debug)]
pub struct ViewMap {
    /// Where the frame is drawn, in points.
    pub image: Rect,
    /// The world pixel at the frame's top-left.
    pub origin: (f32, f32),
    pub zoom: f32,
}

impl ViewMap {
    pub fn to_world(self, p: Pos2) -> (f32, f32) {
        let u = (p - self.image.min) / self.image.size();
        (
            self.origin.0 + u.x * VIEW_WIDTH as f32 / self.zoom,
            self.origin.1 + u.y * VIEW_HEIGHT as f32 / self.zoom,
        )
    }

    pub fn to_screen(self, (x, y): (f32, f32)) -> Pos2 {
        let u = vec2(
            (x - self.origin.0) * self.zoom / VIEW_WIDTH as f32,
            (y - self.origin.1) * self.zoom / VIEW_HEIGHT as f32,
        );
        self.image.min + u * self.image.size()
    }

    fn rect(self, (x, y, w, h): (f32, f32, f32, f32)) -> Rect {
        Rect::from_min_max(self.to_screen((x, y)), self.to_screen((x + w, y + h)))
    }
}

/// An object's position in world pixels.
fn position(scene: &Scene, id: ObjectId) -> (f32, f32) {
    let p = scene.world_position(id);
    (p.x.to_f32(), p.y.to_f32())
}

/// The object's click box in world pixels: x, y, w, h.
pub fn world_box(scene: &Scene, id: ObjectId) -> (f32, f32, f32, f32) {
    let b = scene.bounds(id).unwrap_or(DEFAULT_BOUNDS);
    let (x, y) = position(scene, id);
    (x + b.x as f32, y + b.y as f32, b.w as f32, b.h as f32)
}

/// The topmost active object whose box holds `(x, y)`.
pub fn pick(scene: &Scene, (x, y): (f32, f32)) -> Option<ObjectId> {
    let order = scene.walk();
    order
        .iter()
        .enumerate()
        .filter(|(_, id)| scene.is_active(**id))
        .filter(|(_, id)| {
            let (bx, by, bw, bh) = world_box(scene, **id);
            (bx..bx + bw).contains(&x) && (by..by + bh).contains(&y)
        })
        .max_by_key(|(i, id)| (scene.transform(**id).map_or(0, |t| t.z), *i))
        .map(|(_, id)| *id)
}

/// A move in progress: the object, and where it was grabbed relative to
/// its position.
#[derive(Clone, Copy, Debug)]
pub struct Drag {
    id: ObjectId,
    grab: (f32, f32),
}

/// Handles the Game view's pointer while not playing: select and move.
/// Returns true when the scene changed.
pub fn interact(
    response: &Response,
    map: &ViewMap,
    scene: &mut Scene,
    selected: &mut Option<ObjectId>,
    drag: &mut Option<Drag>,
    tile_snap: bool,
) -> bool {
    let pointer = response.interact_pointer_pos();
    if response.clicked()
        && let Some(p) = pointer
    {
        *selected = pick(scene, map.to_world(p));
    }
    if response.drag_started_by(egui::PointerButton::Primary)
        && let Some(p) = response.ctx.input(|i| i.pointer.press_origin()).or(pointer)
    {
        // Measured from where the button went down, not from where the
        // drag threshold was crossed, so the object tracks the cursor.
        let at = map.to_world(p);
        // Grab the selection if the press is on it, else whatever is there.
        let on_selected = selected.filter(|id| {
            let (bx, by, bw, bh) = world_box(scene, *id);
            (bx..bx + bw).contains(&at.0) && (by..by + bh).contains(&at.1)
        });
        let target = on_selected.or_else(|| pick(scene, at));
        *selected = target.or(*selected);
        *drag = target.map(|id| {
            let (x, y) = position(scene, id);
            Drag { id, grab: (x - at.0, y - at.1) }
        });
    }
    let mut moved = false;
    if response.dragged_by(egui::PointerButton::Primary)
        && let (Some(d), Some(p)) = (*drag, pointer)
        && scene.contains(d.id)
    {
        let at = map.to_world(p);
        let (mut x, mut y) = ((at.0 + d.grab.0).round() as i32, (at.1 + d.grab.1).round() as i32);
        if tile_snap {
            let snap = |v: i32| (v as f32 / TILE as f32).round() as i32 * TILE;
            (x, y) = (snap(x), snap(y));
        }
        let target = FxVec2::from_px(x, y);
        if scene.world_position(d.id) != target {
            scene.set_world_position(d.id, target);
            moved = true;
        }
    }
    if response.drag_stopped() {
        *drag = None;
    }
    moved
}

/// Outlines the hovered and selected objects and draws the move handle.
pub fn draw(
    painter: &egui::Painter,
    map: &ViewMap,
    scene: &Scene,
    selected: Option<ObjectId>,
    hover: Option<Pos2>,
) {
    let painter = painter.with_clip_rect(map.image);
    if let Some(hovered) = hover.and_then(|p| pick(scene, map.to_world(p)))
        && Some(hovered) != selected
    {
        painter.rect_stroke(
            map.rect(world_box(scene, hovered)),
            0.0,
            Stroke::new(1.0, HOVERED),
            StrokeKind::Outside,
        );
    }
    let Some(id) = selected.filter(|id| scene.contains(*id)) else { return };
    painter.rect_stroke(
        map.rect(world_box(scene, id)),
        0.0,
        Stroke::new(1.5, SELECTED),
        StrokeKind::Outside,
    );
    // The pivot with two arrows: drag anywhere on the object to move it.
    let pivot = map.to_screen(position(scene, id));
    let x_axis = Color32::from_rgb(230, 90, 90);
    let y_axis = Color32::from_rgb(110, 210, 110);
    let len = 22.0;
    painter.arrow(pivot, vec2(len, 0.0), Stroke::new(2.0, x_axis));
    painter.arrow(pivot, vec2(0.0, -len), Stroke::new(2.0, y_axis));
    painter.rect_filled(Rect::from_center_size(pivot, vec2(6.0, 6.0)), 0.0, SELECTED);
    let label = scene.name(id).unwrap_or("?");
    painter.text(
        pivot + vec2(8.0, 6.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(12.0),
        SELECTED,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picking_and_gizmos_use_the_same_zoomed_world_coordinates() {
        let image = Rect::from_min_size(egui::pos2(80.0, 50.0), vec2(1280.0, 720.0));
        for zoom in [0.125, 0.5, 1.0, 2.0, 8.0] {
            let map = ViewMap { image, origin: (1234.0, -80.0), zoom };
            let size = (VIEW_WIDTH as f32 / zoom, VIEW_HEIGHT as f32 / zoom);
            assert_eq!(map.to_world(image.min), map.origin);
            assert_eq!(map.to_world(image.max), (1234.0 + size.0, -80.0 + size.1));
            let world = (1434.0, 40.0);
            let round_trip = map.to_world(map.to_screen(world));
            assert!((round_trip.0 - world.0).abs() < 0.001);
            assert!((round_trip.1 - world.1).abs() < 0.001);
        }
    }
}
