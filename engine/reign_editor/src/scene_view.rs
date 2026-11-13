use reign_render::{VIEW_HEIGHT, VIEW_WIDTH};

/// An editor camera, independent of the simulation and the player's camera.
#[derive(Clone, Copy, Debug)]
pub struct SceneView {
    pub origin: (f32, f32),
    pub zoom: f32,
}

impl SceneView {
    pub fn new(origin: (f32, f32)) -> Self {
        Self { origin, zoom: 1.0 }
    }

    pub fn size(self) -> (f32, f32) {
        (VIEW_WIDTH as f32 / self.zoom, VIEW_HEIGHT as f32 / self.zoom)
    }

    pub(crate) fn pan(&mut self, delta: egui::Vec2, image: egui::Rect) {
        let (w, h) = self.size();
        self.origin.0 -= delta.x * w / image.width();
        self.origin.1 -= delta.y * h / image.height();
    }

    pub(crate) fn zoom_at(&mut self, factor: f32, uv: egui::Vec2) {
        let before = self.size();
        self.zoom = (self.zoom * factor).clamp(0.125, 8.0);
        let after = self.size();
        self.origin.0 += uv.x * (before.0 - after.0);
        self.origin.1 += uv.y * (before.1 - after.1);
    }

    pub(crate) fn focus(&mut self, (x, y, w, h): (f32, f32, f32, f32)) {
        self.zoom =
            (VIEW_WIDTH as f32 / (w + 64.0)).min(VIEW_HEIGHT as f32 / (h + 64.0)).clamp(0.125, 2.0);
        let size = self.size();
        self.origin = (x + w / 2.0 - size.0 / 2.0, y + h / 2.0 - size.1 / 2.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2, vec2};

    #[test]
    fn zoom_keeps_the_world_point_under_the_cursor() {
        let mut view = SceneView::new((300.0, -50.0));
        let uv = vec2(0.3, 0.7);
        let point = |v: SceneView| {
            let size = v.size();
            (v.origin.0 + uv.x * size.0, v.origin.1 + uv.y * size.1)
        };
        let before = point(view);
        for factor in [2.0, 0.01, 1000.0] {
            view.zoom_at(factor, uv);
            let after = point(view);
            assert!((after.0 - before.0).abs() < 0.001);
            assert!((after.1 - before.1).abs() < 0.001);
        }
    }

    #[test]
    fn pan_uses_zoom_and_display_size() {
        let mut view = SceneView::new((100.0, 200.0));
        view.zoom = 0.5;
        view.pan(vec2(100.0, -50.0), Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 720.0)));
        assert_eq!(view.origin, (0.0, 250.0));
    }

    #[test]
    fn focus_centres_the_object_and_fits_its_bounds() {
        let mut view = SceneView::new((0.0, 0.0));
        view.focus((2000.0, 400.0, 500.0, 600.0));
        let size = view.size();
        assert!((view.origin.0 + size.0 / 2.0 - 2250.0).abs() < 0.001);
        assert!((view.origin.1 + size.1 / 2.0 - 700.0).abs() < 0.001);
        assert!(size.0 >= 500.0 && size.1 >= 600.0);
    }
}
