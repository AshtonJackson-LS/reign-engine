//! Gizmos in the editor's Scene view: lines that show what a component does
//! while the game is stopped, like Unity's. Built-in ones here (the camera's
//! view, particle emitters, an optional grid), and any a project's own
//! components add with [`reign::behaviour::Behaviour::gizmos`].
//!
//! Never drawn in Play or in an exported game, and never part of the
//! simulation: they are floats in world pixels, drawn over the finished frame.

use reign::behaviour::Behaviours;
use reign::render::{Color, Layer, Renderer, VIEW_HEIGHT, VIEW_WIDTH};
use reign::scene::{Gizmos, Rgba8, Scene};

use crate::components::{Camera, ParticleSystem};
use crate::physics::TILE;
use crate::runtime::World;

/// What the Scene view shows.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Gizmos at all: the built-in ones and the components'.
    pub show: bool,
    /// A grid of tiles, over the part of the world in view.
    pub grid: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { show: true, grid: false }
    }
}

const CAMERA: Rgba8 = Rgba8::rgba(90, 170, 255, 230);
const EMITTER: Rgba8 = Rgba8::rgba(255, 130, 220, 230);
const GRID: Rgba8 = Rgba8::rgba(255, 255, 255, 28);

/// Every gizmo for `scene`: the built-in ones, then each Behaviour's. `view`
/// is the world rectangle on screen (x, y, w, h), which the grid fills.
pub fn collect(
    scene: &Scene,
    behaviours: &Behaviours<World>,
    view: [f32; 4],
    options: Options,
) -> Gizmos {
    let mut g = Gizmos::new();
    if !options.show {
        return g;
    }
    if options.grid {
        g.set_color(GRID);
        g.grid(view[0], view[1], view[2], view[3], TILE as f32);
    }
    // What each camera sees: the view's rectangle, centred on the camera.
    g.set_color(CAMERA);
    for (id, _) in scene.iter::<Camera>() {
        let p = scene.world_position(id);
        let (w, h) = (VIEW_WIDTH as f32, VIEW_HEIGHT as f32);
        g.rect(p.x.to_f32() - w / 2.0, p.y.to_f32() - h / 2.0, w, h);
        g.cross((p.x.to_f32(), p.y.to_f32()), 4.0);
    }
    g.set_color(EMITTER);
    for (id, _) in scene.iter::<ParticleSystem>() {
        let p = scene.world_position(id);
        g.cross((p.x.to_f32(), p.y.to_f32()), 5.0);
        g.circle((p.x.to_f32(), p.y.to_f32()), 3.0);
    }
    g.set_color(Rgba8::WHITE);
    behaviours.draw_gizmos(scene, &mut g);
    g
}

/// Queues `gizmos` over the frame, one screen pixel wide whatever the
/// editor's `zoom`.
pub fn draw(r: &mut Renderer, gizmos: &Gizmos, zoom: f32) {
    if gizmos.is_empty() {
        return;
    }
    r.set_layer(Layer::Overlay);
    let width = 1.0 / zoom.max(f32::EPSILON);
    for l in gizmos.lines() {
        let c = l.color;
        r.line(l.from, l.to, width, Color::rgba8(c.r, c.g, c.b, c.a));
    }
    r.set_layer(Layer::World);
}

#[cfg(test)]
mod tests {
    use reign::behaviour::Behaviour;
    use reign::core::FxVec2;
    use reign::scene::{Component, ObjectId};
    use serde::{Deserialize, Serialize};

    use super::*;

    /// Draws a circle round its object, as a component's gizmos would.
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
    #[serde(default)]
    struct Range {
        radius: i32,
    }

    impl Behaviour<World> for Range {
        fn gizmos(scene: &Scene, me: ObjectId, gizmos: &mut Gizmos) {
            let p = scene.world_position(me);
            let radius = scene.get::<Range>(me).map_or(0, |r| r.radius) as f32;
            gizmos.circle((p.x.to_f32(), p.y.to_f32()), radius);
        }
    }

    fn scene() -> Scene {
        let mut scene = Scene::new();
        let cam = scene.spawn("Cam");
        scene.insert(cam, Camera::default());
        let guard = scene.spawn("Guard");
        scene.set_world_position(guard, FxVec2::from_px(100, 50));
        scene.insert(guard, Range { radius: 30 });
        scene
    }

    #[test]
    fn a_components_gizmos_and_the_built_in_ones_are_collected() {
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<Range>();
        let view = [0.0, 0.0, 640.0, 360.0];
        let g = collect(&scene(), &behaviours, view, Options::default());
        // The camera's rectangle (4) and cross (2), then the guard's circle.
        assert_eq!(g.lines().len(), 4 + 2 + 32);
        let circle = &g.lines()[6..];
        assert!(circle.iter().all(|l| l.color == Rgba8::WHITE));
        assert!((circle[0].from.0 - 130.0).abs() < 1e-3, "centred on the guard, radius 30");
    }

    #[test]
    fn hidden_gizmos_collect_nothing_and_the_grid_is_opt_in() {
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<Range>();
        let view = [0.0, 0.0, 64.0, 64.0];
        let off = Options { show: false, grid: true };
        assert!(collect(&scene(), &behaviours, view, off).is_empty());
        let plain = collect(&scene(), &behaviours, view, Options::default());
        let grid = collect(&scene(), &behaviours, view, Options { show: true, grid: true });
        assert!(grid.lines().len() > plain.lines().len());
    }
}
