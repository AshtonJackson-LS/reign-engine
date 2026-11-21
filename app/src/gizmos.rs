//! Gizmos in the editor's Scene view: lines that show what a component does
//! while the game is stopped, like Unity's. Built-in ones here (the camera's
//! view, colliders, lights, particle emitters' shapes, an optional grid), and
//! any a project's own components add with
//! [`reign::behaviour::Behaviour::gizmos`].
//!
//! Never drawn in Play or in an exported game, and never part of the
//! simulation: they are floats in world pixels, drawn over the finished frame.

use reign::assets::AssetDb;
use reign::assets::particles::ParticleEffect;
use reign::behaviour::Behaviours;
use reign::editor::egui;
use reign::particles::{Spawn, shape_outline};
use reign::render::{Color, Layer, Renderer, VIEW_HEIGHT, VIEW_WIDTH};
use reign::scene::{Gizmos, ObjectId, Rgba8, Scene};

use crate::components::{AmbientLight, Camera, Light2D, ParticleSystem, sprite_size};
use crate::physics::{self, TILE};
use crate::runtime::World;

/// What the Scene view shows: the Gizmos menu in its strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Gizmos at all; off hides every one below.
    pub show: bool,
    /// The world's x and y axes through (0, 0).
    pub origin: bool,
    /// A grid of tiles, over the part of the world in view.
    pub grid: bool,
    /// Collider outlines: boxes, triggers and the tilemaps' solid edges.
    pub colliders: bool,
    /// What each camera sees.
    pub cameras: bool,
    /// `Light2D`s' reach and the `AmbientLight`'s sun.
    pub lights: bool,
    /// Particle systems' emission shapes.
    pub particles: bool,
    /// What a project's own components draw (`Behaviour::gizmos`).
    pub scripts: bool,
    /// Only the selected object's gizmos (the origin, grid and tilemap
    /// edges still show; tilemap edges only with a tilemap selected).
    pub selected_only: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            show: true,
            origin: true,
            grid: false,
            colliders: true,
            cameras: true,
            lights: true,
            particles: true,
            scripts: true,
            selected_only: false,
        }
    }
}

/// The Gizmos menu: a switch for all of them, and a dropdown for each kind.
/// Stays open while boxes are ticked; a click outside closes it.
pub fn menu(ui: &mut egui::Ui, o: &mut Options) {
    use egui::containers::menu::{MenuButton, MenuConfig};
    ui.toggle_value(&mut o.show, "Gizmos").on_hover_text("Show or hide every gizmo");
    let config = MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    MenuButton::new("▾").config(config).ui(ui, |ui| {
        ui.checkbox(&mut o.show, "Show gizmos");
        ui.separator();
        ui.add_enabled_ui(o.show, |ui| {
            ui.checkbox(&mut o.colliders, "Colliders");
            ui.checkbox(&mut o.lights, "Lights");
            ui.checkbox(&mut o.particles, "Particle shapes");
            ui.checkbox(&mut o.cameras, "Cameras");
            ui.checkbox(&mut o.scripts, "Component gizmos")
                .on_hover_text("What your own components draw in Behaviour::gizmos");
            ui.separator();
            ui.checkbox(&mut o.origin, "World origin");
            ui.checkbox(&mut o.grid, "Tile grid");
            ui.separator();
            ui.checkbox(&mut o.selected_only, "Selected object only");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.small_button("All").clicked() {
                    *o = Options { grid: o.grid, origin: o.origin, ..Options::default() };
                }
                if ui.small_button("None").clicked() {
                    *o = Options {
                        colliders: false,
                        cameras: false,
                        lights: false,
                        particles: false,
                        scripts: false,
                        ..*o
                    };
                }
            });
        });
    });
}

const CAMERA: Rgba8 = Rgba8::rgba(90, 170, 255, 230);
const EMITTER: Rgba8 = Rgba8::rgba(255, 130, 220, 230);
const LIGHT: Rgba8 = Rgba8::rgba(255, 220, 110, 230);
/// A light that is switched off.
const LIGHT_OFF: Rgba8 = Rgba8::rgba(255, 220, 110, 90);
const COLLIDER: Rgba8 = Rgba8::rgba(120, 230, 140, 220);
const TRIGGER: Rgba8 = Rgba8::rgba(110, 200, 230, 200);
const ONE_WAY: Rgba8 = Rgba8::rgba(230, 220, 110, 220);
const GRID: Rgba8 = Rgba8::rgba(255, 255, 255, 28);
const ORIGIN: Rgba8 = Rgba8::rgba(255, 255, 255, 60);
/// How wide a particle effect's floor line is drawn, each side.
const FLOOR_HALF: f32 = 24.0;

/// Every gizmo for `scene`: the built-in ones, then each Behaviour's. `view`
/// is the world rectangle on screen (x, y, w, h), which the grid and origin
/// fill; `db` gives particle effects their shapes (without it emitters are
/// just marked); `selected` is the editor's selection, for
/// [`Options::selected_only`].
pub fn collect(
    scene: &Scene,
    behaviours: &Behaviours<World>,
    db: Option<&AssetDb>,
    view: [f32; 4],
    selected: Option<ObjectId>,
    options: Options,
) -> Gizmos {
    let mut g = Gizmos::new();
    if !options.show {
        return g;
    }
    let shown = |id: ObjectId| !options.selected_only || selected == Some(id);
    let [vx, vy, vw, vh] = view;
    if options.grid {
        g.set_color(GRID);
        g.grid(vx, vy, vw, vh, TILE as f32);
    }
    if options.origin {
        g.set_color(ORIGIN);
        g.line((vx, 0.0), (vx + vw, 0.0));
        g.line((0.0, vy), (0.0, vy + vh));
    }
    if options.colliders {
        let tilemap_selected =
            selected.is_some_and(|id| scene.get::<physics::TilemapCollider>(id).is_some());
        colliders(&mut g, scene, view, &shown, !options.selected_only || tilemap_selected);
    }
    // What each camera sees: the view's rectangle, centred on the camera.
    if options.cameras {
        g.set_color(CAMERA);
        for (id, _) in scene.iter::<Camera>().filter(|(id, _)| shown(*id)) {
            let p = scene.world_position(id);
            let (w, h) = (VIEW_WIDTH as f32, VIEW_HEIGHT as f32);
            g.rect(p.x.to_f32() - w / 2.0, p.y.to_f32() - h / 2.0, w, h);
            g.cross((p.x.to_f32(), p.y.to_f32()), 4.0);
        }
    }
    if options.particles {
        for (id, ps) in scene.iter::<ParticleSystem>().filter(|(id, _)| shown(*id)) {
            let p = scene.world_position(id);
            let at = (p.x.to_f32(), p.y.to_f32());
            let flip = scene.transform(id).is_some_and(|t| t.flip_x);
            g.set_color(EMITTER);
            g.cross(at, 5.0);
            if let Some(effect) = db.and_then(|db| db.load(ps.effect).ok()) {
                emitter(&mut g, &effect, at, flip);
            } else {
                g.circle(at, 3.0);
            }
        }
    }
    if options.lights {
        // A light: where it is, and how far it reaches.
        for (id, l) in scene.iter::<Light2D>().filter(|(id, _)| shown(*id)) {
            let p = scene.world_position(id);
            let at = (p.x.to_f32(), p.y.to_f32());
            g.set_color(if l.on { LIGHT } else { LIGHT_OFF });
            g.cross(at, 5.0);
            g.circle(at, l.radius.to_f32());
        }
        // The ambient light lights everything, so it's only marked: a sun.
        g.set_color(LIGHT);
        for (id, _) in scene.iter::<AmbientLight>().filter(|(id, _)| shown(*id)) {
            let p = scene.world_position(id);
            sun(&mut g, (p.x.to_f32(), p.y.to_f32()));
        }
    }
    if options.scripts {
        g.set_color(Rgba8::WHITE);
        behaviours.draw_gizmos_where(scene, shown, &mut g);
    }
    g
}

/// Box colliders' outlines (triggers in their own colour) of the objects
/// `shown` accepts, and with `tiles` the solid edges and one-way tops of
/// tilemaps inside `view`.
fn colliders(
    g: &mut Gizmos,
    scene: &Scene,
    view: [f32; 4],
    shown: &impl Fn(ObjectId) -> bool,
    tiles: bool,
) {
    let seen = physics::Rect {
        x: view[0].floor() as i32,
        y: view[1].floor() as i32,
        w: view[2].ceil() as i32 + 1,
        h: view[3].ceil() as i32 + 1,
    };
    let edges = if tiles { physics::tile_outlines(scene, seen) } else { Vec::new() };
    for (e, one_way) in edges {
        g.set_color(if one_way { ONE_WAY } else { COLLIDER });
        // Each edge is a strip a pixel thick: draw the line down its middle.
        let (x, y, w, h) = (e.x as f32, e.y as f32, e.w as f32, e.h as f32);
        if e.h == 1 {
            g.line((x, y + 0.5), (x + w, y + 0.5));
        } else {
            g.line((x + 0.5, y), (x + 0.5, y + h));
        }
    }
    let triggers: Vec<_> = scene
        .iter::<physics::BoxCollider>()
        .filter(|(_, c)| c.is_trigger)
        .map(|(id, _)| id)
        .collect();
    for (id, b) in physics::colliders(scene, &sprite_size).into_iter().filter(|(id, _)| shown(*id))
    {
        g.set_color(if triggers.contains(&id) { TRIGGER } else { COLLIDER });
        g.rect(b.x as f32 + 0.5, b.y as f32 + 0.5, b.w as f32 - 1.0, b.h as f32 - 1.0);
    }
}

/// A particle effect's emission shape at `at`, like Unity's Shape module:
/// see [`reign::particles::shape_outline`] (the Particles tab draws the
/// same). Then a line for the effect's own floor.
fn emitter(g: &mut Gizmos, effect: &ParticleEffect, at: (f32, f32), flip: bool) {
    let spawn = Spawn::at(at.0, at.1).flip_x(flip);
    for (from, to) in shape_outline(effect, &spawn) {
        g.line(from, to);
    }
    // The floor particles hit, when the effect has its own.
    if effect.collision.enabled && effect.collision.floor > 0.0 {
        let y = at.1 + effect.collision.floor;
        g.line((at.0 - FLOOR_HALF, y), (at.0 + FLOOR_HALF, y));
    }
}

/// A small sun: a ring and eight rays.
fn sun(g: &mut Gizmos, at: (f32, f32)) {
    g.circle(at, 4.0);
    for i in 0..8 {
        let a = i as f32 / 8.0 * std::f32::consts::TAU;
        let (c, s) = (a.cos(), a.sin());
        g.line((at.0 + c * 6.0, at.1 + s * 6.0), (at.0 + c * 9.0, at.1 + s * 9.0));
    }
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

    use reign::assets::particles::ShapeKind;

    use super::*;

    /// The defaults without the world origin's two lines, for counting.
    fn plain() -> Options {
        Options { origin: false, ..Options::default() }
    }

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
        let g = collect(&scene(), &behaviours, None, view, None, plain());
        // The camera's rectangle (4) and cross (2), then the guard's circle.
        assert_eq!(g.lines().len(), 4 + 2 + 32);
        let circle = &g.lines()[6..];
        assert!(circle.iter().all(|l| l.color == Rgba8::WHITE));
        assert!((circle[0].from.0 - 130.0).abs() < 1e-3, "centred on the guard, radius 30");
    }

    #[test]
    fn a_light_shows_where_it_is_and_how_far_it_reaches() {
        let mut scene = Scene::new();
        let lamp = scene.spawn("Lamp");
        scene.set_world_position(lamp, FxVec2::from_px(40, 30));
        scene.insert(lamp, Light2D { radius: reign::core::Fx::from_px(50), ..Light2D::default() });
        let g = collect(&scene, &Behaviours::new(), None, [0.0, 0.0, 640.0, 360.0], None, plain());
        // A cross (2 lines) and a circle (32) in the light's colour.
        assert_eq!(g.lines().len(), 2 + 32);
        assert!(g.lines().iter().all(|l| l.color == LIGHT));
        let circle = &g.lines()[2..];
        assert!((circle[0].from.0 - 90.0).abs() < 1e-3, "centred on the lamp, radius 50");
        assert!((circle[0].from.1 - 30.0).abs() < 1e-3);
    }

    /// The emitter gizmo for `effect` at (100, 100), without its cross.
    fn emitter_lines(effect: &ParticleEffect, flip: bool) -> Vec<reign::scene::GizmoLine> {
        let mut g = Gizmos::new();
        emitter(&mut g, effect, (100.0, 100.0), flip);
        g.lines().to_vec()
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3
    }

    #[test]
    fn a_cone_is_a_closed_fan_from_its_mouth_to_its_reach() {
        use reign::assets::particles::{Main, Range, Shape};
        // Straight up, 20 wide, 60 degrees of spread, flying 40 px.
        let effect = ParticleEffect {
            main: Main { speed: Range(40.0, 40.0), lifetime: Range(1.0, 1.0), ..Main::default() },
            shape: Shape {
                kind: ShapeKind::Cone,
                angle: -90.0,
                spread: 60.0,
                radius: 20.0,
                ..Shape::default()
            },
            ..ParticleEffect::default()
        };
        let lines = emitter_lines(&effect, false);
        // The outline: mouth end, 7 cap points (60 degrees), mouth end,
        // closed; then the arrow (3).
        let outline = &lines[..lines.len() - 3];
        assert_eq!(outline.len(), 9);
        assert!(near(outline[0].from, (100.0, 110.0)) || near(outline[0].from, (90.0, 100.0)));
        assert!(near(outline.last().unwrap().to, outline[0].from), "closed");
        // The cap's middle is straight up from the mouth's middle, 40 out.
        assert!(outline.iter().any(|l| near(l.from, (100.0, 60.0))), "{outline:?}");
        // The left side opens 30 degrees out from the left end of the mouth.
        let side = outline[0];
        let a = (side.to.1 - side.from.1).atan2(side.to.0 - side.from.0).to_degrees();
        assert!((a - -120.0).abs() < 1e-2, "{a}");
        assert!(near(lines[lines.len() - 3].to, (100.0, 80.0)), "the arrow, half the reach");
    }

    #[test]
    fn a_point_spraying_every_way_is_a_disc_and_one_way_is_an_arrow() {
        use reign::assets::particles::Shape;
        let shape = Shape { kind: ShapeKind::Point, spread: 360.0, ..Shape::default() };
        let effect = ParticleEffect { shape: shape.clone(), ..ParticleEffect::default() };
        assert_eq!(emitter_lines(&effect, false).len(), 32, "a disc, no arrow");
        let one_way = Shape { spread: 0.0, ..shape };
        let effect = ParticleEffect { shape: one_way, ..ParticleEffect::default() };
        assert_eq!(emitter_lines(&effect, false).len(), 3, "only the arrow");
    }

    #[test]
    fn an_emitter_is_squashed_offset_and_mirrored_like_its_particles() {
        use reign::assets::particles::Shape;
        let effect = ParticleEffect {
            shape: Shape {
                kind: ShapeKind::Box,
                size: [20.0, 10.0],
                spread: 0.0,
                squash: 0.5,
                offset: [30.0, 0.0],
                ..Shape::default()
            },
            ..ParticleEffect::default()
        };
        let box_lines = &emitter_lines(&effect, false)[..4];
        // Centred 30 right of the emitter, half as tall.
        assert!(near(box_lines[0].from, (120.0, 97.5)));
        let flipped = &emitter_lines(&effect, true)[..4];
        assert!(near(flipped[0].from, (80.0, 97.5)), "mirrored to the left");
    }

    #[test]
    fn a_partial_circle_and_its_floor_are_drawn() {
        use reign::assets::particles::Shape;
        let mut effect = ParticleEffect {
            shape: Shape {
                kind: ShapeKind::Circle,
                angle: 0.0,
                spread: 90.0,
                radius: 10.0,
                ..Shape::default()
            },
            ..ParticleEffect::default()
        };
        effect.collision.enabled = true;
        effect.collision.floor = 40.0;
        let lines = emitter_lines(&effect, false);
        // The arc, its two radii, the floor.
        let floor = lines.last().unwrap();
        assert!(near(floor.from, (100.0 - FLOOR_HALF, 140.0)));
        // A closed slice: from the middle out at -45 degrees, round to 45, back.
        let r = 10.0 * std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(lines[0].from, (100.0, 100.0)) && near(lines[0].to, (100.0 + r, 100.0 - r)));
        let back = lines[lines.len() - 2];
        assert!(near(back.from, (100.0 + r, 100.0 + r)) && near(back.to, (100.0, 100.0)));
    }

    #[test]
    fn colliders_are_outlined_triggers_apart_and_can_be_hidden() {
        use crate::physics::BoxCollider;
        let mut scene = Scene::new();
        let wall = scene.spawn("Wall");
        scene.set_world_position(wall, FxVec2::from_px(50, 50));
        scene.insert(wall, BoxCollider { size: [20, 10], ..BoxCollider::default() });
        let zone = scene.spawn("Zone");
        scene
            .insert(zone, BoxCollider { size: [8, 8], is_trigger: true, ..BoxCollider::default() });
        let view = [0.0, 0.0, 640.0, 360.0];
        let g = collect(&scene, &Behaviours::new(), None, view, None, plain());
        assert_eq!(g.lines().len(), 8);
        assert_eq!(g.lines().iter().filter(|l| l.color == TRIGGER).count(), 4);
        assert!(near(g.lines()[0].from, (40.5, 45.5)), "the wall's box, inside its pixels");
        let off = Options { colliders: false, ..plain() };
        assert!(collect(&scene, &Behaviours::new(), None, view, None, off).is_empty());
    }

    #[test]
    fn an_ambient_light_is_a_sun_and_a_switched_off_lamp_is_dim() {
        let mut scene = Scene::new();
        let sky = scene.spawn("Sky");
        scene.insert(sky, AmbientLight::default());
        let lamp = scene.spawn("Lamp");
        scene.insert(lamp, Light2D { on: false, ..Light2D::default() });
        let g = collect(&scene, &Behaviours::new(), None, [0.0; 4], None, plain());
        assert_eq!(g.lines().iter().filter(|l| l.color == LIGHT_OFF).count(), 2 + 32);
        assert_eq!(g.lines().iter().filter(|l| l.color == LIGHT).count(), 32 + 8);
    }

    #[test]
    fn hidden_gizmos_collect_nothing_and_the_grid_is_opt_in() {
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<Range>();
        let view = [0.0, 0.0, 64.0, 64.0];
        let off = Options { show: false, grid: true, ..plain() };
        assert!(collect(&scene(), &behaviours, None, view, None, off).is_empty());
        let without = collect(&scene(), &behaviours, None, view, None, plain());
        let grid =
            collect(&scene(), &behaviours, None, view, None, Options { grid: true, ..plain() });
        assert!(grid.lines().len() > without.lines().len());
    }

    /// A lamp, an emitter-less particle system, a box and a camera.
    fn busy_scene() -> (Scene, ObjectId) {
        let mut scene = Scene::new();
        let lamp = scene.spawn("Lamp");
        scene.insert(lamp, Light2D::default());
        let fx = scene.spawn("Sparks");
        scene.insert(fx, ParticleSystem::default());
        let wall = scene.spawn("Wall");
        scene.insert(wall, crate::physics::BoxCollider { size: [8, 8], ..Default::default() });
        let cam = scene.spawn("Cam");
        scene.insert(cam, Camera::default());
        (scene, fx)
    }

    #[test]
    fn each_kind_of_gizmo_has_its_own_switch() {
        let (scene, _) = busy_scene();
        let view = [0.0, 0.0, 640.0, 360.0];
        let count =
            |o: Options| collect(&scene, &Behaviours::new(), None, view, None, o).lines().len();
        let all = count(plain());
        // Lamp 2 + 32, sparks 2 + 32 (no effect: a marker), wall 4, camera 4 + 2.
        assert_eq!(all, 34 + 34 + 4 + 6);
        assert_eq!(count(Options { lights: false, ..plain() }), all - 34);
        assert_eq!(count(Options { particles: false, ..plain() }), all - 34);
        assert_eq!(count(Options { colliders: false, ..plain() }), all - 4);
        assert_eq!(count(Options { cameras: false, ..plain() }), all - 6);
        assert_eq!(count(Options::default()), all + 2, "the origin's two axes");
    }

    #[test]
    fn selected_only_keeps_just_the_selections_gizmos() {
        let (scene, fx) = busy_scene();
        let view = [0.0, 0.0, 640.0, 360.0];
        let only = Options { selected_only: true, ..plain() };
        let g = collect(&scene, &Behaviours::new(), None, view, Some(fx), only);
        assert_eq!(g.lines().len(), 34);
        assert!(g.lines().iter().all(|l| l.color == EMITTER));
        let none = collect(&scene, &Behaviours::new(), None, view, None, only);
        assert!(none.is_empty(), "nothing selected, nothing shown");
    }

    #[test]
    fn a_components_gizmos_follow_the_selection_too() {
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<Range>();
        let scene = scene();
        let guard = scene.iter::<Range>().next().unwrap().0;
        let view = [0.0, 0.0, 640.0, 360.0];
        let only = Options { selected_only: true, ..plain() };
        assert_eq!(collect(&scene, &behaviours, None, view, Some(guard), only).lines().len(), 32);
        let off = Options { scripts: false, ..plain() };
        assert_eq!(collect(&scene, &behaviours, None, view, None, off).lines().len(), 6);
    }
}
