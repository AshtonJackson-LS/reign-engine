//! The outline of an effect's emission shape, like Unity's Shape module
//! gizmo: where particles are born and the volume they head out into. The
//! editor draws it over the Scene view and the Particles tab's preview.
//!
//! Placed as the simulation places particles (`new_particle` in `lib.rs`):
//! squashed, offset, turned by the spawn's angle, mirrored, scaled and moved
//! to the emitter. Only lines in world pixels; nothing here runs in a game.

use reign_assets::ParticleEffect;
use reign_assets::particles::ShapeKind;

use crate::Spawn;

/// One line of an outline, in world pixels.
pub type Segment = ((f32, f32), (f32, f32));

/// The shortest and longest the volume is drawn, in pixels (before scale).
const REACH_MIN: f32 = 16.0;
const REACH_MAX: f32 = 120.0;
/// How much of the reach the heading arrow covers.
const ARROW: f32 = 0.5;

/// How far the volume reaches: about how far a particle flies at its
/// average speed for its average life, ignoring forces, kept to a size that
/// reads on screen.
pub fn reach(effect: &ParticleEffect) -> f32 {
    let m = &effect.main;
    let flies = m.speed.lerp(0.5).abs() * m.lifetime.lerp(0.5).max(0.0);
    flies.clamp(REACH_MIN, REACH_MAX)
}

/// The emission shape of `effect` played at `spawn`:
///
/// - Cone, Line and Point: a closed fan from the mouth (a point for Point),
///   its sides opening at the spread, capped at [`reach`]. With no spread
///   it's a band (or, for a point, nothing but the arrow); with all 360
///   degrees, a disc.
/// - Circle: the circle, or the slice the spread leaves of it.
/// - Box: the box.
///
/// Then an arrow for the heading, unless particles head every way (or are
/// born on a circle, heading outward).
pub fn shape_outline(effect: &ParticleEffect, spawn: &Spawn) -> Vec<Segment> {
    let sh = &effect.shape;
    let (sin, cos) = spawn.angle.to_radians().sin_cos();
    let place = |(x, y): (f32, f32)| {
        let (x, y) = (x, y * sh.squash);
        let (x, y) = (x + sh.offset[0], y + sh.offset[1]);
        let (x, y) = (x * cos - y * sin, x * sin + y * cos);
        let x = if spawn.flip_x { -x } else { x };
        (spawn.x + x * spawn.scale, spawn.y + y * spawn.scale)
    };
    let dir = |degrees: f32, len: f32| {
        let a = degrees.to_radians();
        (a.cos() * len, a.sin() * len)
    };
    let spread = sh.spread.clamp(0.0, 360.0);
    let (lo, hi) = (sh.angle - spread / 2.0, sh.angle + spread / 2.0);
    let reach = reach(effect);
    // Arc points from `lo` to `hi`: one every 11.25 degrees, at least two.
    let steps = ((spread / 360.0 * 32.0).ceil() as usize).max(1);
    let t = |i: usize| i as f32 / steps as f32;
    let outline: Vec<(f32, f32)> = match sh.kind {
        ShapeKind::Box => {
            let (w, h) = (sh.size[0] / 2.0, sh.size[1] / 2.0);
            vec![(-w, -h), (w, -h), (w, h), (-w, h)]
        }
        ShapeKind::Circle => {
            let arc = (0..=steps).map(|i| dir(lo + (hi - lo) * t(i), sh.radius));
            if spread >= 360.0 {
                arc.take(steps).collect()
            } else {
                // A slice: from the middle round the arc and back.
                std::iter::once((0.0, 0.0)).chain(arc).collect()
            }
        }
        kind => {
            let width = match kind {
                ShapeKind::Cone => sh.radius,
                ShapeKind::Line => sh.size[0],
                _ => 0.0,
            };
            // The mouth's ends: the first on the `lo` side of the heading.
            let (ax, ay) = dir(sh.angle + 90.0, width / 2.0);
            let (m0, m1) = ((-ax, -ay), (ax, ay));
            let cap = (0..=steps).map(|i| {
                let k = t(i);
                let from = (m0.0 + (m1.0 - m0.0) * k, m0.1 + (m1.1 - m0.1) * k);
                let (dx, dy) = dir(lo + (hi - lo) * k, reach);
                (from.0 + dx, from.1 + dy)
            });
            if spread >= 360.0 && width == 0.0 {
                cap.take(steps).collect()
            } else if width == 0.0 && spread == 0.0 {
                Vec::new()
            } else {
                std::iter::once(m0).chain(cap).chain(std::iter::once(m1)).collect()
            }
        }
    };
    let points: Vec<_> = outline.into_iter().map(place).collect();
    let mut out: Vec<Segment> = points.windows(2).map(|w| (w[0], w[1])).collect();
    if points.len() > 2 {
        out.push((points[points.len() - 1], points[0]));
    }
    if sh.kind != ShapeKind::Circle && spread < 360.0 {
        let (hx, hy) = dir(sh.angle, reach * ARROW);
        out.extend(arrow(place((0.0, 0.0)), place((hx, hy))));
    }
    out
}

/// A line with a head at `to`: three segments, the line first.
fn arrow(from: (f32, f32), to: (f32, f32)) -> Vec<Segment> {
    let mut out = vec![(from, to)];
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = dx.hypot(dy);
    if len <= f32::EPSILON {
        return out;
    }
    // The head's two barbs sweep back 30 degrees either side.
    let head = len * 0.25;
    let (ux, uy) = (dx / len, dy / len);
    let (sin, cos) = (0.5_f32, 0.866_f32);
    for side in [-1.0_f32, 1.0] {
        let (bx, by) = (ux * cos - side * uy * sin, side * ux * sin + uy * cos);
        out.push((to, (to.0 - bx * head, to.1 - by * head)));
    }
    out
}

#[cfg(test)]
mod tests {
    use reign_assets::particles::{Main, Range, Shape};

    use super::*;

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    fn effect(shape: Shape) -> ParticleEffect {
        let main = Main { speed: Range(40.0, 40.0), lifetime: Range(1.0, 1.0), ..Main::default() };
        ParticleEffect { main, shape, ..ParticleEffect::default() }
    }

    #[test]
    fn a_box_is_its_four_sides_where_the_emitter_is() {
        let e = effect(Shape { kind: ShapeKind::Box, size: [20.0, 10.0], ..Shape::default() });
        let lines = shape_outline(&e, &Spawn::at(50.0, 60.0));
        // Four sides, then the arrow.
        assert_eq!(lines.len(), 4 + 3);
        assert!(near(lines[0].0, (40.0, 55.0)) && near(lines[0].1, (60.0, 55.0)));
        assert!(near(lines[3].1, (40.0, 55.0)), "closed");
    }

    #[test]
    fn a_box_turns_and_scales_with_the_spawn() {
        let e = effect(Shape { kind: ShapeKind::Box, size: [20.0, 10.0], ..Shape::default() });
        // Turned 90 degrees clockwise and doubled: 20 tall, 40 wide.
        let lines = shape_outline(&e, &Spawn::at(0.0, 0.0).angle(90.0).scale(2.0));
        assert!(near(lines[0].0, (10.0, -20.0)), "{:?}", lines[0]);
        assert!(near(lines[0].1, (10.0, 20.0)), "{:?}", lines[0]);
    }

    #[test]
    fn the_outline_reaches_about_as_far_as_particles_fly() {
        assert_eq!(reach(&effect(Shape::default())), 40.0);
        let slow = ParticleEffect::default();
        assert!(reach(&slow) >= REACH_MIN && reach(&slow) <= REACH_MAX);
    }
}
