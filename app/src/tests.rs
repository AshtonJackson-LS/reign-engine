//! Tests for the editor app: projects, the create menu, physics and Behaviours.

use reign::assets::AssetDb;
use reign::core::FxVec2;
use reign::editor::EditorApp;
use reign::scene::{Bounds, ComponentRegistry, Rgba8, Scene};

use crate::components::{SpriteRenderer, register_builtins, sprite_size};
use crate::editor::{Project, ReignEngine};
use crate::runtime::Scripts;
use crate::{physics, starter};

fn registry() -> ComponentRegistry {
    let mut r = ComponentRegistry::new();
    register_builtins(&mut r);
    r
}

fn engine(project: Project) -> ReignEngine {
    ReignEngine::new(project, Scripts::none(), None)
}

fn temp_project(name: &str) -> Project {
    let dir = std::env::temp_dir().join(format!("reign-engine-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    Project::open(dir).expect("project opens")
}

#[test]
fn new_projects_get_the_shapes_sheet() {
    let project = temp_project("shapes");
    let db = AssetDb::loose(project.assets()).expect("assets load");
    assert_eq!(db.path(starter::SHAPES_SHEET).as_deref(), Some("Reign/Shapes.sheet.ron"));
    let sheet = db.load(starter::shapes_sheet()).expect("sheet loads");
    assert_eq!(sheet.sprites.len(), starter::SHAPES.len());
    assert_eq!(sheet.texture.guid, starter::SHAPES_TEXTURE);
    let texture = db.load(sheet.texture).expect("texture loads");
    let square = sheet.sprite("square").expect("square");
    assert_eq!(square.pivot, [16, 16]);
    // The square is solid; the circle's corner is empty.
    let px = |x: u32, y: u32| texture.image.rgba[((y * texture.image.width + x) * 4 + 3) as usize];
    assert_eq!(px(0, 0), 255);
    let circle = sheet.sprite("circle").expect("circle");
    assert_eq!(px(circle.rect[0], 0), 0);
    assert_eq!(px(circle.rect[0] + 16, 16), 255);
    let _ = std::fs::remove_dir_all(&project.root);
}

#[test]
fn create_menu_items_all_make_objects() {
    let project = temp_project("create");
    let root = project.root.clone();
    let mut app = engine(project);
    let parent = app.world.scene.spawn("Parent");
    for item in app.create_items() {
        let id = app.create(&item, None, (40, -12)).unwrap_or_else(|| panic!("{item}"));
        assert_eq!(app.world.scene.world_position(id), FxVec2::from_px(40, -12), "{item}");
    }
    let square = app.create("2D Object/Sprites/Square", Some(parent), (8, 8)).expect("square");
    assert_eq!(app.world.scene.parent(square), Some(parent));
    let sr = app.world.scene.get::<SpriteRenderer>(square).expect("renderer");
    assert_eq!((sr.sprite.as_str(), sr.size, sr.color), ("square", [32, 32], Rgba8::WHITE));
    assert_eq!(app.world.scene.bounds(square), Some(Bounds::new(-16, -16, 32, 32)));
    assert!(app.create("2D Object/Sprites/Nope", None, (0, 0)).is_none());

    // Colour and size survive a save and load.
    app.world.scene.get_mut::<SpriteRenderer>(square).unwrap().color = Rgba8::rgb(228, 46, 52);
    let text = app.world.scene.to_ron().expect("saves");
    let back = Scene::from_ron(&text, &app.registry).expect("loads");
    assert_eq!(back.get::<SpriteRenderer>(square).unwrap().color, Rgba8::rgb(228, 46, 52));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_created_box_falls_onto_a_created_platform() {
    let project = temp_project("physics");
    let root = project.root.clone();
    let mut app = engine(project);
    let platform = app.create("2D Object/Physics/Static Platform", None, (0, 100)).unwrap();
    let crate_id = app.create("2D Object/Physics/Dynamic Box", None, (10, 0)).unwrap();
    assert!(app.world.scene.get::<physics::Rigidbody>(platform).is_none());
    for _ in 0..360 {
        physics::step(&mut app.world.scene, &sprite_size);
    }
    // Platform top: 100 - 8 = 92. A 24px box rests with its centre 12 above.
    assert_eq!(app.world.scene.world_position(crate_id), FxVec2::from_px(10, 80));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn old_sprite_renderers_load_white() {
    // Saved before SpriteRenderer had colour and size.
    let text = r#"(
        version: 1,
        objects: [
            ( id: (0, 0), name: "Old", components: {"SpriteRenderer": (sheet: "00000000000000000000000000000000", sprite: "square")} ),
        ],
    )"#;
    let scene = Scene::from_ron(text, &registry()).expect("loads");
    let (_, sr) = scene.iter::<SpriteRenderer>().next().expect("renderer");
    assert_eq!((sr.color, sr.size), (Rgba8::WHITE, [0, 0]));
}

mod behaviours {
    use crate::prelude::*;
    use crate::runtime::{Keyboard, schedule};
    use reign::app::winit::keyboard::KeyCode;
    use reign::behaviour::Behaviours;
    use reign::core::InputFrame;

    /// The same shape as the New Component template.
    #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
    #[serde(default)]
    pub struct Mover {
        pub speed: Fx,
        pub started: bool,
    }

    impl Default for Mover {
        fn default() -> Self {
            Mover { speed: Fx::from_px(120), started: false }
        }
    }

    impl Behaviour<World> for Mover {
        fn start(ctx: &mut Ctx, me: ObjectId) {
            if let Some(m) = ctx.get_mut::<Mover>(me) {
                m.started = true;
            }
        }

        fn fixed_update(ctx: &mut Ctx, me: ObjectId) {
            let Some(speed) = ctx.get::<Mover>(me).map(|c| c.speed) else { return };
            let step = speed / TICKS_PER_SECOND;
            ctx.translate(me, FxVec2::new(step * ctx.axis_x(), step * ctx.axis_y()));
        }
    }

    fn held(keys: &[KeyCode]) -> InputFrame {
        let mut keyboard = Keyboard::default();
        keyboard.press_for_tests(keys);
        keyboard.frame(None)
    }

    #[test]
    fn behaviours_start_then_move_with_the_keys() {
        let mut scene = Scene::new();
        let id = scene.spawn("Player");
        scene.insert(id, Mover::default());
        let mut world = World::new(scene);
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<Mover>();
        let schedule = schedule();

        // One second holding right and down: 120 px each way.
        let input = held(&[KeyCode::ArrowRight, KeyCode::KeyS]);
        for _ in 0..TICKS_PER_SECOND {
            world.step(input, &behaviours, &schedule);
        }
        assert!(world.scene.get::<Mover>(id).unwrap().started);
        assert_eq!(world.scene.world_position(id), FxVec2::from_px(120, 120));

        // Nothing held: it stays put.
        for _ in 0..10 {
            world.step(InputFrame::default(), &behaviours, &schedule);
        }
        assert_eq!(world.scene.world_position(id), FxVec2::from_px(120, 120));
    }

    #[test]
    fn physics_runs_inside_the_schedule() {
        let mut scene = Scene::new();
        let ground = scene.spawn("Ground");
        scene.set_world_position(ground, FxVec2::from_px(0, 100));
        scene.insert(ground, BoxCollider::sized(200, 16));
        let b = scene.spawn("Box");
        scene.insert(b, BoxCollider::sized(16, 16));
        scene.insert(b, Rigidbody::default());
        let mut world = World::new(scene);
        let behaviours = Behaviours::new();
        let schedule = schedule();
        for _ in 0..240 {
            world.step(InputFrame::default(), &behaviours, &schedule);
        }
        assert_eq!(world.scene.world_position(b), FxVec2::from_px(0, 84));
    }
}

/// The templates Scripts → New Component writes, compiled and run as-is.
mod templates {
    mod platformer {
        include!("../../engine/reign_build/templates/platformer_movement.rs");
    }
    mod top_down {
        include!("../../engine/reign_build/templates/top_down_movement.rs");
    }
    mod behaviour {
        include!("../../engine/reign_build/templates/behaviour.rs");
    }
    mod data {
        include!("../../engine/reign_build/templates/data.rs");
    }

    use crate::prelude::*;
    use crate::runtime::schedule;
    use platformer::PlatformerMovement;
    use reign::behaviour::{Behaviours, Schedule};
    use reign::core::InputFrame;
    use top_down::TopDownMovement;

    fn frame(buttons: &[Button]) -> InputFrame {
        let mut f = InputFrame::default();
        for b in buttons {
            f.set(*b, true);
        }
        f
    }

    struct Sim {
        world: World,
        behaviours: Behaviours<World>,
        schedule: Schedule<World>,
        player: ObjectId,
    }

    impl Sim {
        fn run(&mut self, ticks: u32, buttons: &[Button]) {
            for _ in 0..ticks {
                self.world.step(frame(buttons), &self.behaviours, &self.schedule);
            }
        }

        fn pos(&self) -> FxVec2 {
            self.world.scene.world_position(self.player)
        }

        fn body(&self) -> &Rigidbody {
            self.world.scene.get::<Rigidbody>(self.player).unwrap()
        }

        /// Runs until it lands again, returning the highest point (lowest y).
        fn peak_until_landed(&mut self, buttons: &[Button]) -> i32 {
            let mut top = self.pos().y.round_px();
            for _ in 0..600 {
                self.run(1, buttons);
                top = top.min(self.pos().y.round_px());
                if self.body().grounded {
                    break;
                }
            }
            top
        }
    }

    /// Ground with its top at y = 92, and a 16 px player above it.
    fn platformer() -> Sim {
        let mut scene = Scene::new();
        let ground = scene.spawn("Ground");
        scene.set_world_position(ground, FxVec2::from_px(0, 100));
        scene.insert(ground, BoxCollider::sized(2000, 16));
        let player = scene.spawn("Player");
        scene.set_world_position(player, FxVec2::from_px(0, 40));
        scene.insert(player, BoxCollider::sized(16, 16));
        scene.insert(player, Rigidbody::default());
        scene.insert(player, PlatformerMovement::default());
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<PlatformerMovement>();
        Sim { world: World::new(scene), behaviours, schedule: schedule(), player }
    }

    #[test]
    fn platformer_lands_runs_and_stops() {
        let mut sim = platformer();
        sim.run(120, &[]);
        assert!(sim.body().grounded);
        assert_eq!(sim.pos().y, Fx::from_px(84));

        sim.run(120, &[Button::Right]);
        let x = sim.pos().x;
        assert!(x > Fx::from_px(100), "ran right: {x:?}");
        assert_eq!(sim.body().velocity.x, Fx::from_px(140), "at top speed");

        sim.run(60, &[]);
        assert_eq!(sim.body().velocity.x, Fx::ZERO, "slowed to a stop");
        assert_eq!(sim.pos().y, Fx::from_px(84), "stayed on the ground");
    }

    #[test]
    fn holding_jump_goes_higher_than_tapping_it() {
        let mut sim = platformer();
        sim.run(120, &[]);
        let held = sim.peak_until_landed(&[Button::Jump]);
        sim.run(30, &[]);

        sim.run(2, &[Button::Jump]);
        let tapped = sim.peak_until_landed(&[]);
        assert!(held < 84 - 40, "a held jump rises well: peak y {held}");
        assert!(tapped > held + 20, "a tap is a short hop: {tapped} vs {held}");
        assert_eq!(sim.pos().y, Fx::from_px(84), "lands back on the ground");
    }

    #[test]
    fn a_jump_pressed_just_before_landing_still_happens() {
        let mut sim = platformer();
        // Falling from y = 40: press Jump a few ticks before touching down.
        let mut ticks_to_land = 0;
        let mut probe = platformer();
        while !probe.body().grounded {
            probe.run(1, &[]);
            ticks_to_land += 1;
        }
        sim.run(ticks_to_land - 5, &[]);
        sim.run(1, &[Button::Jump]);
        sim.run(30, &[Button::Jump]);
        assert!(sim.pos().y < Fx::from_px(60), "buffered jump fired: {:?}", sim.pos().y);
    }

    #[test]
    fn no_double_jump() {
        let mut sim = platformer();
        sim.run(120, &[]);
        sim.run(20, &[Button::Jump]);
        let rising = sim.body().velocity.y;
        sim.run(1, &[]);
        sim.run(1, &[Button::Jump]);
        assert!(sim.body().velocity.y > rising, "a second press mid-air doesn't jump again");
    }

    fn top_down(with_body: bool) -> Sim {
        let mut scene = Scene::new();
        let player = scene.spawn("Player");
        scene.insert(player, TopDownMovement::default());
        if with_body {
            scene.insert(player, BoxCollider::sized(16, 16));
            scene.insert(player, Rigidbody { gravity_scale: Fx::ZERO, ..Rigidbody::default() });
            let wall = scene.spawn("Wall");
            scene.set_world_position(wall, FxVec2::from_px(60, 0));
            scene.insert(wall, BoxCollider::sized(16, 400));
        }
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<TopDownMovement>();
        Sim { world: World::new(scene), behaviours, schedule: schedule(), player }
    }

    #[test]
    fn top_down_moves_evenly_on_diagonals() {
        let mut straight = top_down(false);
        straight.run(240, &[Button::Right]);
        let mut diagonal = top_down(false);
        diagonal.run(240, &[Button::Right, Button::Down]);
        let d = diagonal.pos();
        assert_eq!(d.x, d.y);
        // Same distance overall: x√2 ≈ the straight run.
        let along = d.x.to_f32() * std::f32::consts::SQRT_2;
        let straight_x = straight.pos().x.to_f32();
        assert!((along - straight_x).abs() < straight_x * 0.02, "{along} vs {straight_x}");
        assert_eq!(straight.pos().y, Fx::ZERO);
    }

    #[test]
    fn top_down_with_a_rigidbody_stops_at_walls() {
        let mut sim = top_down(true);
        sim.run(240, &[Button::Right]);
        // Wall's left edge is 52; the player's right edge stops there.
        assert_eq!(sim.pos(), FxVec2::from_px(44, 0));
    }

    #[test]
    fn the_basic_templates_compile_and_register() {
        let mut r = reign::scene::ComponentRegistry::new();
        r.register::<behaviour::MyBehaviour>().register::<data::MyData>();
        let mut b = Behaviours::<World>::new();
        b.behaviour::<behaviour::MyBehaviour>();
        assert_eq!(b.names(), ["MyBehaviour"]);
    }
}

/// Collision and trigger events reaching Behaviours.
mod contacts {
    use crate::prelude::*;
    use crate::runtime::schedule;
    use reign::behaviour::Behaviours;
    use reign::core::InputFrame;

    /// Writes down every physics event it hears about.
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
    #[serde(default)]
    pub struct Recorder {
        pub hit: Vec<ObjectId>,
        pub left: Vec<ObjectId>,
        pub entered: Vec<ObjectId>,
        pub exited: Vec<ObjectId>,
    }

    impl Behaviour<World> for Recorder {
        fn on_message(ctx: &mut Ctx, me: ObjectId, msg: &Envelope<Message>) {
            let Some(r) = ctx.get_mut::<Recorder>(me) else { return };
            match msg.body {
                Message::CollisionEnter(o) => r.hit.push(o),
                Message::CollisionExit(o) => r.left.push(o),
                Message::TriggerEnter(o) => r.entered.push(o),
                Message::TriggerExit(o) => r.exited.push(o),
                Message::Custom { .. } => {}
            }
        }
    }

    struct Sim {
        world: World,
        behaviours: Behaviours<World>,
    }

    impl Sim {
        fn new(scene: Scene) -> Sim {
            let mut behaviours = Behaviours::new();
            behaviours.behaviour::<Recorder>();
            Sim { world: World::new(scene), behaviours }
        }

        fn run(&mut self, ticks: u32) {
            let schedule = schedule();
            for _ in 0..ticks {
                self.world.step(InputFrame::default(), &self.behaviours, &schedule);
            }
        }

        fn rec(&self, id: ObjectId) -> &Recorder {
            self.world.scene.get::<Recorder>(id).unwrap()
        }
    }

    fn ground(scene: &mut Scene) -> ObjectId {
        let g = scene.spawn("Ground");
        scene.set_world_position(g, FxVec2::from_px(0, 100));
        scene.insert(g, BoxCollider::sized(400, 16));
        scene.insert(g, Recorder::default());
        g
    }

    fn falling_box(scene: &mut Scene, collider: BoxCollider) -> ObjectId {
        let b = scene.spawn("Box");
        scene.insert(b, collider);
        scene.insert(b, Rigidbody::default());
        scene.insert(b, Recorder::default());
        b
    }

    #[test]
    fn landing_reports_one_enter_to_both_and_leaving_reports_exit() {
        let mut scene = Scene::new();
        let g = ground(&mut scene);
        let b = falling_box(&mut scene, BoxCollider::sized(16, 16));
        let mut sim = Sim::new(scene);
        sim.run(240);
        assert_eq!(sim.rec(b).hit, [g], "the box hit the ground once, resting there");
        assert_eq!(sim.rec(g).hit, [b], "and the ground heard about the box");
        assert!(sim.rec(b).left.is_empty());

        // Launch it upward: they stop touching.
        sim.world.scene.get_mut::<Rigidbody>(b).unwrap().velocity = FxVec2::from_px(0, -300);
        sim.run(10);
        assert_eq!(sim.rec(b).left, [g]);
        assert_eq!(sim.rec(g).left, [b]);
    }

    #[test]
    fn triggers_report_overlaps_and_block_nothing() {
        let mut scene = Scene::new();
        let g = ground(&mut scene);
        let zone = scene.spawn("Zone");
        scene.set_world_position(zone, FxVec2::from_px(0, 40));
        scene.insert(zone, BoxCollider { is_trigger: true, ..BoxCollider::sized(64, 16) });
        scene.insert(zone, Recorder::default());
        let b = falling_box(&mut scene, BoxCollider::sized(16, 16));
        let mut sim = Sim::new(scene);
        sim.run(240);
        assert_eq!(sim.rec(zone).entered, [b]);
        assert_eq!(sim.rec(zone).exited, [b]);
        assert_eq!(sim.rec(b).entered, [zone]);
        assert_eq!(sim.rec(b).exited, [zone]);
        assert!(sim.rec(zone).hit.is_empty(), "a trigger isn't a collision");
        assert_eq!(
            sim.world.scene.world_position(b).y,
            Fx::from_px(84),
            "it fell through and landed"
        );
        assert_eq!(sim.rec(b).hit, [g]);
    }

    #[test]
    fn colliders_on_ignored_layers_pass_through() {
        let mut scene = Scene::new();
        ground(&mut scene);
        // Layer 1, colliding only with layer 1: the ground is on layer 0.
        let ghost = falling_box(
            &mut scene,
            BoxCollider { layer: 1, mask: 1 << 1, ..BoxCollider::sized(16, 16) },
        );
        let mut sim = Sim::new(scene);
        sim.run(120);
        assert!(
            sim.world.scene.world_position(ghost).y > Fx::from_px(200),
            "fell through the ground"
        );
        assert!(sim.rec(ghost).hit.is_empty());
    }

    #[test]
    fn touching_lists_what_is_in_contact() {
        let mut scene = Scene::new();
        let g = ground(&mut scene);
        let b = falling_box(&mut scene, BoxCollider::sized(16, 16));
        let mut sim = Sim::new(scene);
        sim.run(240);
        let behaviours = Behaviours::<World>::new();
        let schedule = schedule();
        // Ask from inside a tick, as a Behaviour would.
        let mut world = sim.world.clone();
        world.step(InputFrame::default(), &behaviours, &schedule);
        // `touching` reads the contacts the last tick left behind.
        assert_eq!(crate::runtime::touching_for_tests(&world, b), [g]);
        assert_eq!(crate::runtime::touching_for_tests(&world, g), [b]);
    }
}

/// Named input actions reaching Behaviours.
mod actions {
    use crate::prelude::*;
    use crate::runtime::{Keyboard, schedule};
    use reign::app::winit::keyboard::KeyCode;
    use reign::behaviour::Behaviours;

    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
    #[serde(default)]
    pub struct Counter {
        pub held: u32,
        pub presses: u32,
        pub unknown: u32,
    }

    impl Behaviour<World> for Counter {
        fn fixed_update(ctx: &mut Ctx, me: ObjectId) {
            let held = ctx.action("Interact");
            let pressed = ctx.action_pressed("Interact");
            let unknown = ctx.action("No Such Action");
            if let Some(c) = ctx.get_mut::<Counter>(me) {
                c.held += held as u32;
                c.presses += pressed as u32;
                c.unknown += unknown as u32;
            }
        }
    }

    #[test]
    fn actions_by_name_follow_their_keys() {
        let mut scene = Scene::new();
        let id = scene.spawn("Counter");
        scene.insert(id, Counter::default());
        let mut world = World::new(scene);
        let mut behaviours = Behaviours::new();
        behaviours.behaviour::<Counter>();
        let schedule = schedule();

        let mut e = Keyboard::default();
        e.press_for_tests(&[KeyCode::KeyE]);
        let idle = Keyboard::default();
        for keyboard in [&e, &e, &e, &idle, &e] {
            world.step(keyboard.frame(None), &behaviours, &schedule);
        }
        let c = world.scene.get::<Counter>(id).unwrap();
        assert_eq!((c.held, c.presses, c.unknown), (4, 2, 0));
    }
}
