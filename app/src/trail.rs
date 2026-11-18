//! Drawing a [`TrailRenderer`]: the points it has kept, joined into a ribbon
//! that narrows and fades with age.
//!
//! The points are game state (see the component); this is only drawing, so
//! it works in floats.

use reign::core::FxVec2;
use reign::render::{Blend, Color, Renderer};

use crate::components::TrailRenderer;

/// One piece of the ribbon: a straight stretch between two points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub width: f32,
    /// sRGB, 0 to 255 a channel.
    pub color: [u8; 4],
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The ribbon's segments, oldest first, ending at `head` (where the object
/// is now). A trail with no points yet has no segments.
pub fn segments(trail: &TrailRenderer, head: FxVec2) -> Vec<Segment> {
    let lifetime = trail.lifetime_ticks().max(1) as f32;
    // (position, how far along the ribbon: 0 at the object, 1 at the oldest).
    let mut path: Vec<((f32, f32), f32)> = trail
        .points
        .iter()
        .map(|p| ((p.at.x.to_f32(), p.at.y.to_f32()), (p.age as f32 / lifetime).min(1.0)))
        .collect();
    path.push(((head.x.to_f32(), head.y.to_f32()), 0.0));
    let (w0, w1) = (trail.start_width.to_f32(), trail.end_width.to_f32());
    let (c0, c1) = (trail.start_color, trail.end_color);
    path.windows(2)
        .map(|pair| {
            let ((from, t_from), (to, t_to)) = (pair[0], pair[1]);
            let t = (t_from + t_to) / 2.0;
            let channel = |a: u8, b: u8| lerp(f32::from(a), f32::from(b), t).round() as u8;
            Segment {
                from,
                to,
                width: lerp(w0, w1, t),
                color: [
                    channel(c0.r, c1.r),
                    channel(c0.g, c1.g),
                    channel(c0.b, c1.b),
                    channel(c0.a, c1.a),
                ],
            }
        })
        .collect()
}

/// Queues `trail` for drawing, ending at the object's position `head`.
pub fn draw(r: &mut Renderer, trail: &TrailRenderer, head: FxVec2) {
    if trail.points.is_empty() {
        return;
    }
    if trail.additive {
        r.set_blend(Blend::Add);
    }
    for s in segments(trail, head) {
        if s.color[3] == 0 || s.width <= 0.0 {
            continue;
        }
        let [cr, cg, cb, ca] = s.color;
        r.line(s.from, s.to, s.width, Color::rgba8(cr, cg, cb, ca));
    }
    if trail.additive {
        r.set_blend(Blend::Alpha);
    }
}

#[cfg(test)]
mod tests {
    use reign::core::Fx;
    use reign::scene::Rgba8;

    use super::*;
    use crate::components::TrailPoint;

    fn trail() -> TrailRenderer {
        TrailRenderer {
            time: Fx::ONE,
            start_width: Fx::from_px(10),
            end_width: Fx::ZERO,
            start_color: Rgba8::rgba(200, 100, 0, 255),
            end_color: Rgba8::rgba(0, 0, 0, 0),
            ..TrailRenderer::default()
        }
    }

    #[test]
    fn no_points_no_segments_to_draw() {
        let t = trail();
        // Just the head: one point, so nothing to join.
        assert!(segments(&t, FxVec2::from_px(5, 5)).is_empty());
    }

    #[test]
    fn narrows_and_fades_from_the_object_to_the_oldest_point() {
        let mut t = trail();
        let life = t.lifetime_ticks();
        t.points = vec![
            TrailPoint { at: FxVec2::from_px(0, 0), age: life },
            TrailPoint { at: FxVec2::from_px(10, 0), age: life / 2 },
        ];
        let s = segments(&t, FxVec2::from_px(20, 0));
        assert_eq!(s.len(), 2);
        // The newest segment is wider and more opaque than the oldest.
        assert!(s[1].width > s[0].width);
        assert!(s[1].color[3] > s[0].color[3]);
        assert_eq!(s[1].from, (10.0, 0.0));
        assert_eq!(s[1].to, (20.0, 0.0));
    }
}
