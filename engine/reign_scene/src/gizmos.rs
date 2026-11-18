//! Gizmos: lines a component draws in the editor's Scene view to show what
//! it does, like Unity's `OnDrawGizmos`. A collider's outline, a trigger's
//! range or a patrol path is easier to set up when you can see it.
//!
//! A [`Gizmos`] only collects lines in world pixels. The editor draws them
//! over the stopped scene and never in Play or an exported game, so gizmos
//! are not game state and don't touch the simulation (they may use floats).

use crate::Rgba8;

/// One line of a gizmo, in world pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GizmoLine {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub color: Rgba8,
}

/// The sides a circle is drawn with.
const CIRCLE_SIDES: usize = 32;

/// A pile of lines to draw. Set the colour, then draw shapes in it:
///
/// ```
/// use reign_scene::{Gizmos, Rgba8};
/// let mut g = Gizmos::new();
/// g.set_color(Rgba8::rgb(255, 200, 0));
/// g.circle((100.0, 50.0), 24.0);
/// g.arrow((100.0, 50.0), (160.0, 50.0));
/// assert!(!g.lines().is_empty());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Gizmos {
    color: Rgba8,
    lines: Vec<GizmoLine>,
}

impl Default for Gizmos {
    fn default() -> Self {
        Gizmos { color: Rgba8::WHITE, lines: Vec::new() }
    }
}

impl Gizmos {
    pub fn new() -> Self {
        Self::default()
    }

    /// The colour shapes are drawn in from now on (white to start with).
    pub fn set_color(&mut self, color: Rgba8) {
        self.color = color;
    }

    pub fn color(&self) -> Rgba8 {
        self.color
    }

    /// Every line drawn so far, in order.
    pub fn lines(&self) -> &[GizmoLine] {
        &self.lines
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }

    pub fn line(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.lines.push(GizmoLine { from, to, color: self.color });
    }

    /// Lines joining `points` in order; with `closed`, the last joins the first.
    pub fn polyline(&mut self, points: &[(f32, f32)], closed: bool) {
        for pair in points.windows(2) {
            self.line(pair[0], pair[1]);
        }
        if closed && points.len() > 2 {
            self.line(points[points.len() - 1], points[0]);
        }
    }

    /// A rectangle's outline: top-left corner, then width and height.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.polyline(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], true);
    }

    /// A circle's outline, drawn with 32 sides.
    pub fn circle(&mut self, center: (f32, f32), radius: f32) {
        let point = |i: usize| {
            let a = i as f32 / CIRCLE_SIDES as f32 * std::f32::consts::TAU;
            (center.0 + radius * a.cos(), center.1 + radius * a.sin())
        };
        for i in 0..CIRCLE_SIDES {
            self.line(point(i), point(i + 1));
        }
    }

    /// A plus sign `half` pixels each way from `center`: marks a point.
    pub fn cross(&mut self, center: (f32, f32), half: f32) {
        self.line((center.0 - half, center.1), (center.0 + half, center.1));
        self.line((center.0, center.1 - half), (center.0, center.1 + half));
    }

    /// A line with a head at `to`.
    pub fn arrow(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.line(from, to);
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let len = dx.hypot(dy);
        if len <= f32::EPSILON {
            return;
        }
        // The head's two barbs sweep back 30 degrees either side.
        let head = len * 0.25;
        let (ux, uy) = (dx / len, dy / len);
        let (sin, cos) = (0.5_f32, 0.866_f32);
        for side in [-1.0_f32, 1.0] {
            let (bx, by) = (ux * cos - side * uy * sin, side * ux * sin + uy * cos);
            self.line(to, (to.0 - bx * head, to.1 - by * head));
        }
    }

    /// A grid of lines every `cell` pixels over the rectangle: top-left
    /// corner, then width and height. Does nothing for more than 1000 lines
    /// a side (a grid you couldn't see anyway).
    pub fn grid(&mut self, x: f32, y: f32, w: f32, h: f32, cell: f32) {
        if cell <= 0.0 || w / cell > 1000.0 || h / cell > 1000.0 {
            return;
        }
        let mut gx = (x / cell).ceil() * cell;
        while gx <= x + w {
            self.line((gx, y), (gx, y + h));
            gx += cell;
        }
        let mut gy = (y / cell).ceil() * cell;
        while gy <= y + h {
            self.line((x, gy), (x + w, gy));
            gy += cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_are_lines_in_the_current_colour() {
        let mut g = Gizmos::new();
        assert_eq!(g.color(), Rgba8::WHITE);
        g.rect(0.0, 0.0, 10.0, 5.0);
        assert_eq!(g.lines().len(), 4);
        g.set_color(Rgba8::rgb(1, 2, 3));
        g.circle((0.0, 0.0), 10.0);
        assert_eq!(g.lines().len(), 4 + CIRCLE_SIDES);
        assert_eq!(g.lines()[0].color, Rgba8::WHITE);
        assert_eq!(g.lines().last().unwrap().color, Rgba8::rgb(1, 2, 3));
    }

    #[test]
    fn a_circle_closes_on_itself_at_its_radius() {
        let mut g = Gizmos::new();
        g.circle((5.0, 5.0), 10.0);
        let (first, last) = (g.lines()[0], *g.lines().last().unwrap());
        assert!((first.from.0 - 15.0).abs() < 1e-4 && (first.from.1 - 5.0).abs() < 1e-4);
        assert!((last.to.0 - first.from.0).abs() < 1e-3 && (last.to.1 - first.from.1).abs() < 1e-3);
        for l in g.lines() {
            let d = (l.from.0 - 5.0).hypot(l.from.1 - 5.0);
            assert!((d - 10.0).abs() < 1e-3, "{d}");
        }
    }

    #[test]
    fn polylines_only_close_with_three_points_or_more() {
        let mut g = Gizmos::new();
        g.polyline(&[(0.0, 0.0), (1.0, 0.0)], true);
        assert_eq!(g.lines().len(), 1);
        g.polyline(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)], true);
        assert_eq!(g.lines().len(), 1 + 3);
    }

    #[test]
    fn an_arrow_has_a_shaft_and_two_barbs_back_from_its_tip() {
        let mut g = Gizmos::new();
        g.arrow((0.0, 0.0), (40.0, 0.0));
        assert_eq!(g.lines().len(), 3);
        for barb in &g.lines()[1..] {
            assert_eq!(barb.from, (40.0, 0.0));
            assert!(barb.to.0 < 40.0, "swept back from the tip");
        }
        // A zero-length arrow draws just its (empty) shaft.
        let mut g = Gizmos::new();
        g.arrow((3.0, 3.0), (3.0, 3.0));
        assert_eq!(g.lines().len(), 1);
    }

    #[test]
    fn grid_lines_sit_on_multiples_of_the_cell_and_huge_grids_are_skipped() {
        let mut g = Gizmos::new();
        g.grid(5.0, 5.0, 30.0, 20.0, 16.0);
        // x = 16, 32; y = 16.
        assert_eq!(g.lines().len(), 3);
        assert!(g.lines().iter().all(|l| l.from.0 % 16.0 == 0.0 || l.from.1 % 16.0 == 0.0));
        let mut g = Gizmos::new();
        g.grid(0.0, 0.0, 1_000_000.0, 10.0, 1.0);
        assert!(g.is_empty());
    }
}
