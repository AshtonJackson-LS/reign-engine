use std::collections::HashMap;

use reign_assets::particles::*;
use reign_assets::{Grid, Sprite, SpriteSheet};

use super::*;

const FRAME: f32 = 1.0 / 60.0;

/// Effects by GUID, for sub-emitters.
struct Library(HashMap<Guid, Arc<ParticleEffect>>);

impl Effects for Library {
    fn effect(&self, handle: Handle<ParticleEffect>) -> Option<Arc<ParticleEffect>> {
        self.0.get(&handle.guid).cloned()
    }
}

fn run(world: &mut ParticleWorld, seconds: f32) {
    run_with(world, seconds, &NoEffects, &no_ground);
}

fn run_with(world: &mut ParticleWorld, seconds: f32, effects: &dyn Effects, ground: &dyn Ground) {
    for _ in 0..(seconds / FRAME).round() as u32 {
        world.step(FRAME, effects, ground);
    }
}

/// A one-shot burst of `count` particles that live exactly `life` seconds.
fn burst(count: u32, life: f32) -> ParticleEffect {
    ParticleEffect {
        main: Main {
            looping: false,
            duration: 0.1,
            lifetime: Range(life, life),
            ..Main::default()
        },
        emission: Emission {
            rate: 0.0,
            bursts: vec![Burst { count: CountRange(count, count), ..Burst::default() }],
            ..Emission::default()
        },
        ..ParticleEffect::default()
    }
}

/// Where each particle is, after a step.
fn positions(world: &ParticleWorld) -> Vec<(f32, f32)> {
    world.systems.iter().flat_map(|s| s.particles.iter().map(|p| s.world_pos(p))).collect()
}

fn velocities(world: &ParticleWorld) -> Vec<(f32, f32)> {
    world.systems.iter().flat_map(|s| s.particles.iter().map(|p| (p.vx, p.vy))).collect()
}

#[test]
fn a_burst_emits_its_count_and_the_effect_ends_with_its_particles() {
    let mut world = ParticleWorld::new();
    let id = world.play_effect(Arc::new(burst(25, 0.5)), Spawn::at(10.0, 20.0));
    world.step(FRAME, &NoEffects, &no_ground);
    assert_eq!(world.particle_count(), 25);
    assert!(world.is_alive(id));
    run(&mut world, 0.3);
    assert_eq!(world.particle_count(), 25, "still alive at 0.3 s");
    assert!(!world.is_emitting(id), "a one-shot stops emitting after its cycle");
    run(&mut world, 0.3);
    assert_eq!(world.particle_count(), 0);
    assert!(!world.is_alive(id), "the effect is gone once its particles are");
    assert_eq!(world.system_count(), 0);
}

#[test]
fn the_rate_emits_that_many_a_second_and_looping_goes_on_until_stopped() {
    let effect = ParticleEffect {
        main: Main { lifetime: Range(10.0, 10.0), duration: 0.5, ..Main::default() },
        emission: Emission { rate: 30.0, ..Emission::default() },
        ..ParticleEffect::default()
    };
    let mut world = ParticleWorld::new();
    let id = world.play_effect(Arc::new(effect), Spawn::default());
    run(&mut world, 2.0);
    let n = world.particle_count() as i32;
    assert!((59..=61).contains(&n), "{n} after 2 s at 30 a second");
    world.stop(id);
    run(&mut world, 1.0);
    assert_eq!(world.particle_count() as i32, n, "stopped: no more");
    assert!(world.is_alive(id), "the particles live on");
}

#[test]
fn the_start_delay_and_timed_bursts_fire_on_time_and_repeat_each_cycle() {
    let effect = ParticleEffect {
        main: Main {
            duration: 1.0,
            start_delay: 0.5,
            lifetime: Range(0.05, 0.05),
            ..Main::default()
        },
        emission: Emission {
            rate: 0.0,
            bursts: vec![Burst {
                time: 0.25,
                count: CountRange(4, 4),
                cycles: 2,
                interval: 0.25,
                ..Burst::default()
            }],
            ..Emission::default()
        },
        ..ParticleEffect::default()
    };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    let mut births = Vec::new();
    let mut before = 0;
    for frame in 0..180 {
        world.step(FRAME, &NoEffects, &no_ground);
        let now = world.particle_count();
        if now > before {
            births.push(frame + 1);
        }
        before = now;
    }
    // Delay 0.5, then 0.25 and 0.5 into each 1 s cycle: 0.75, 1.0, 1.75,
    // 2.0, 2.75 and 3.0 s, on the frame each is due.
    assert_eq!(births, vec![45, 60, 105, 120, 165, 180]);
}

#[test]
fn bursts_pick_counts_in_range_and_obey_their_probability() {
    let mut effect = burst(0, 1.0);
    effect.emission.bursts[0].count = CountRange(3, 7);
    let mut counts = std::collections::BTreeSet::new();
    for seed in 0..60 {
        let mut world = ParticleWorld::new();
        world.play_effect(Arc::new(effect.clone()), Spawn::default().seed(seed));
        world.step(FRAME, &NoEffects, &no_ground);
        counts.insert(world.particle_count());
    }
    assert_eq!(counts.into_iter().collect::<Vec<_>>(), vec![3, 4, 5, 6, 7]);
    effect.emission.bursts[0].probability = 0.0;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    assert_eq!(world.particle_count(), 0);
}

#[test]
fn no_more_than_the_maximum_are_alive() {
    let mut effect = burst(1000, 1.0);
    effect.main.max_particles = 40;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    assert_eq!(world.particle_count(), 40);
}

#[test]
fn particles_head_the_shapes_way_and_the_emitter_turns_and_mirrors_it() {
    let mut effect = burst(30, 1.0);
    effect.shape = Shape { kind: ShapeKind::Point, angle: 0.0, spread: 0.0, ..Shape::default() };
    effect.main.speed = Range(100.0, 100.0);
    let dir = |spawn: Spawn| {
        let mut world = ParticleWorld::new();
        world.play_effect(Arc::new(effect.clone()), spawn);
        world.step(FRAME, &NoEffects, &no_ground);
        let v = velocities(&world);
        assert_eq!(v.len(), 30);
        let (vx, vy) = v[0];
        assert!(v.iter().all(|&(x, y)| (x - vx).abs() < 1e-3 && (y - vy).abs() < 1e-3));
        (vx.round(), vy.round())
    };
    assert_eq!(dir(Spawn::default()), (100.0, 0.0));
    assert_eq!(dir(Spawn::default().flip_x(true)), (-100.0, 0.0));
    assert_eq!(dir(Spawn::default().angle(90.0)), (0.0, 100.0), "turned to point down");
    assert_eq!(dir(Spawn::default().angle(-90.0).flip_x(true)), (0.0, -100.0));
    assert_eq!(dir(Spawn::default().scale(2.0)), (200.0, 0.0));
}

#[test]
fn the_spread_keeps_directions_inside_the_cone() {
    let mut effect = burst(400, 1.0);
    effect.shape = Shape { kind: ShapeKind::Point, angle: -90.0, spread: 60.0, ..Shape::default() };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    let angles: Vec<f32> =
        velocities(&world).into_iter().map(|(x, y)| y.atan2(x).to_degrees()).collect();
    assert!(angles.iter().all(|a| (-120.01..=-59.99).contains(a)), "{angles:?}");
    let min = angles.iter().cloned().fold(f32::MAX, f32::min);
    let max = angles.iter().cloned().fold(f32::MIN, f32::max);
    assert!(min < -115.0 && max > -65.0, "fills the cone: {min}..{max}");
}

#[test]
fn shapes_place_particles_where_they_say() {
    let place = |shape: Shape| {
        let mut effect = burst(300, 1.0);
        effect.shape = shape;
        effect.main.speed = Range(0.0, 0.0);
        let mut world = ParticleWorld::new();
        world.play_effect(Arc::new(effect), Spawn::at(100.0, 50.0));
        world.step(FRAME, &NoEffects, &no_ground);
        positions(&world).into_iter().map(|(x, y)| (x - 100.0, y - 50.0)).collect::<Vec<_>>()
    };
    let ring = place(Shape {
        kind: ShapeKind::Circle,
        radius: 10.0,
        spread: 360.0,
        edge: true,
        ..Shape::default()
    });
    assert!(ring.iter().all(|(x, y)| (x.hypot(*y) - 10.0).abs() < 0.01));
    let disc =
        place(Shape { kind: ShapeKind::Circle, radius: 10.0, spread: 360.0, ..Shape::default() });
    assert!(disc.iter().all(|(x, y)| x.hypot(*y) <= 10.01));
    assert!(disc.iter().any(|(x, y)| x.hypot(*y) < 5.0), "inside, not just the edge");
    let flat = place(Shape {
        kind: ShapeKind::Circle,
        radius: 10.0,
        spread: 360.0,
        edge: true,
        squash: 0.25,
        ..Shape::default()
    });
    assert!(flat.iter().all(|(_, y)| y.abs() <= 2.51), "squashed into a flat ring");
    let boxed =
        place(Shape { kind: ShapeKind::Box, size: [20.0, 4.0], spread: 0.0, ..Shape::default() });
    assert!(boxed.iter().all(|(x, y)| x.abs() <= 10.0 && y.abs() <= 2.0));
    // A line square to an upward angle lies along x.
    let line =
        place(Shape { kind: ShapeKind::Line, angle: -90.0, size: [30.0, 0.0], ..Shape::default() });
    assert!(line.iter().all(|(x, y)| x.abs() <= 15.01 && y.abs() < 0.01));
    assert!(line.iter().any(|(x, _)| x.abs() > 10.0));
    let point = place(Shape { kind: ShapeKind::Point, ..Shape::default() });
    assert!(point.iter().all(|&(x, y)| x == 0.0 && y == 0.0));
}

#[test]
fn gravity_pulls_drag_slows_and_wind_pushes() {
    let mut effect = burst(1, 5.0);
    effect.shape = Shape { kind: ShapeKind::Point, angle: 0.0, spread: 0.0, ..Shape::default() };
    effect.main.speed = Range(0.0, 0.0);
    effect.forces.gravity = 100.0;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect.clone()), Spawn::default());
    run(&mut world, 1.0);
    let (_, vy) = velocities(&world)[0];
    assert!((vy - 100.0).abs() < 2.0, "{vy} after a second of gravity 100");
    let (_, y) = positions(&world)[0];
    assert!((45.0..55.0).contains(&y), "fell about half g t squared: {y}");

    effect.forces = Forces { drag: 0.5, ..Forces::default() };
    effect.main.speed = Range(100.0, 100.0);
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect.clone()), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    run(&mut world, 1.0);
    let (vx, _) = velocities(&world)[0];
    assert!((48.0..52.0).contains(&vx), "half its speed lost in a second: {vx}");

    effect.forces = Forces { wind: [20.0, 0.0], ..Forces::default() };
    effect.main.speed = Range(0.0, 0.0);
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    run(&mut world, 1.0);
    assert!((velocities(&world)[0].0 - 20.0).abs() < 1.0);
}

#[test]
fn velocity_over_life_scales_speed_and_adds_its_own() {
    let mut effect = burst(1, 2.0);
    effect.shape = Shape { kind: ShapeKind::Point, angle: 0.0, spread: 0.0, ..Shape::default() };
    effect.main.speed = Range(60.0, 60.0);
    effect.velocity =
        VelocityOverLife { enabled: true, speed: Curve::constant(0.0), linear: [0.0, -30.0] };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    run(&mut world, 1.0);
    let (x, y) = positions(&world)[0];
    assert!(x.abs() < 0.01, "its own speed is scaled to nothing: {x}");
    assert!((y + 30.0).abs() < 1.0, "the linear part still moves it: {y}");
}

/// One particle dropped straight down from 10 pixels above a floor at 0.
fn dropped(mode: CollisionMode) -> ParticleEffect {
    let mut effect = burst(1, 3.0);
    effect.shape = Shape { kind: ShapeKind::Point, angle: 90.0, spread: 0.0, ..Shape::default() };
    effect.main.speed = Range(200.0, 200.0);
    effect.collision = Collision {
        enabled: true,
        mode,
        floor: 10.0,
        bounce: 0.5,
        friction: 0.0,
        ..Collision::default()
    };
    effect
}

#[test]
fn particles_bounce_stick_or_die_on_the_floor() {
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(dropped(CollisionMode::Bounce)), Spawn::at(0.0, -10.0));
    let mut lowest = f32::MIN;
    let mut rose = false;
    for _ in 0..30 {
        world.step(FRAME, &NoEffects, &no_ground);
        let (_, y) = positions(&world)[0];
        lowest = lowest.max(y);
        rose |= velocities(&world)[0].1 < 0.0;
    }
    assert!(lowest <= 0.0001, "never went through the floor: {lowest}");
    assert!(rose, "bounced back up");

    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(dropped(CollisionMode::Stick)), Spawn::at(5.0, -10.0));
    run(&mut world, 0.5);
    assert_eq!(positions(&world), vec![(5.0, 0.0)], "stuck where it landed");
    run(&mut world, 1.0);
    assert_eq!(positions(&world), vec![(5.0, 0.0)], "and stays");

    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(dropped(CollisionMode::Die)), Spawn::at(0.0, -10.0));
    run(&mut world, 0.5);
    assert_eq!(world.particle_count(), 0);
}

#[test]
fn the_game_can_say_where_the_floor_is_and_where_the_level_is_solid() {
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(dropped(CollisionMode::Stick)), Spawn::at(0.0, -10.0).floor(40.0));
    run(&mut world, 1.0);
    let (x, y) = positions(&world)[0];
    assert!(x.abs() < 1e-4 && y == 40.0, "{x}, {y}");

    // Solid from y = 20 down, and a wall at x >= 30.
    let ground = |x: f32, y: f32| y >= 20.0 || x >= 30.0;
    let mut effect = dropped(CollisionMode::Stick);
    effect.collision.floor = 1000.0;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect.clone()), Spawn::at(0.0, 0.0));
    run_with(&mut world, 1.0, &NoEffects, &ground);
    let (x, y) = positions(&world)[0];
    assert!(x.abs() < 1e-3 && (19.9..20.0).contains(&y), "stopped on the ground: {x}, {y}");

    effect.shape.angle = 0.0;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect.clone()), Spawn::at(0.0, 0.0));
    run_with(&mut world, 1.0, &NoEffects, &ground);
    let (x, _) = positions(&world)[0];
    assert!((29.9..30.0).contains(&x), "stopped at the wall: {x}");

    // Without `world`, the level is ignored.
    effect.collision.world = false;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(0.0, 0.0));
    run_with(&mut world, 0.5, &NoEffects, &ground);
    assert!(positions(&world)[0].0 > 50.0);
}

#[test]
fn hits_cost_life_and_resting_particles_dont_keep_hitting() {
    let mut effect = dropped(CollisionMode::Bounce);
    effect.collision.lifetime_loss = 0.5;
    effect.collision.bounce = 0.0;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(0.0, -10.0));
    run(&mut world, 0.2);
    let life = world.systems[0].particles[0].life;
    assert!((1.5..1.7).contains(&life), "half its remaining life gone: {life}");
    run(&mut world, 0.5);
    assert_eq!(world.systems[0].particles[0].life, life, "lying still costs nothing");
}

#[test]
fn sub_emitters_start_on_birth_collision_and_death() {
    let splat = Handle::<ParticleEffect>::new(Guid(1));
    let puff = Handle::<ParticleEffect>::new(Guid(2));
    let trail = Handle::<ParticleEffect>::new(Guid(3));
    let mut library = HashMap::new();
    let mut one = burst(1, 0.2);
    one.collision.enabled = false;
    library.insert(splat.guid, Arc::new(one.clone()));
    library.insert(puff.guid, Arc::new(burst(3, 0.2)));
    let mut trailing = one;
    trailing.main.looping = true;
    trailing.emission = Emission { rate: 60.0, bursts: Vec::new(), ..Emission::default() };
    library.insert(trail.guid, Arc::new(trailing));
    let library = Library(library);

    let mut effect = dropped(CollisionMode::Die);
    effect.main.lifetime = Range(3.0, 3.0);
    let sub = |trigger, effect| SubEmitter { trigger, effect, ..SubEmitter::default() };
    effect.sub_emitters = vec![
        sub(SubTrigger::Collision, splat),
        sub(SubTrigger::Death, puff),
        sub(SubTrigger::Birth, trail),
    ];
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(0.0, -10.0));
    world.step(FRAME, &library, &no_ground);
    assert_eq!(world.system_count(), 2, "the trail started with the particle");
    run_with(&mut world, 0.04, &library, &no_ground);
    let trail_particles = world.systems[1].particles.len();
    assert!(trail_particles >= 1, "the trail emits while it follows");
    run_with(&mut world, 0.1, &library, &no_ground);
    // The drop hit the floor and died: a splat (collision), a puff (death),
    // and the trail stopped emitting.
    let counts: Vec<usize> = world.systems.iter().map(|s| s.particles.len()).collect();
    assert!(world.systems.iter().all(|s| s.id != SystemId(1)), "the drop's effect is over");
    assert!(counts.contains(&1) && counts.contains(&3), "{counts:?}");
    assert!(world.systems.iter().all(|s| !s.emitting || s.follow.is_none()));
    run_with(&mut world, 1.0, &library, &no_ground);
    assert_eq!(world.system_count(), 0, "everything finishes");
}

#[test]
fn sub_emitters_inherit_velocity_colour_and_the_emitters_mirror() {
    let child = Handle::<ParticleEffect>::new(Guid(9));
    let mut library = HashMap::new();
    let mut c = burst(1, 1.0);
    c.main.speed = Range(0.0, 0.0);
    library.insert(child.guid, Arc::new(c));
    let library = Library(library);
    let mut effect = burst(1, 0.05);
    effect.shape = Shape { kind: ShapeKind::Point, angle: 0.0, spread: 0.0, ..Shape::default() };
    effect.main.speed = Range(100.0, 100.0);
    effect.main.color = ColorRange([255, 0, 0, 255], [255, 0, 0, 255]);
    effect.sub_emitters = vec![SubEmitter {
        trigger: SubTrigger::Death,
        effect: child,
        inherit_velocity: 0.5,
        inherit_color: true,
        ..SubEmitter::default()
    }];
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default().flip_x(true));
    run_with(&mut world, 0.1, &library, &no_ground);
    let s = &world.systems[0];
    assert!(s.spawn.flip_x, "mirrored like its parent");
    let p = &s.particles[0];
    assert!((p.vx + 50.0).abs() < 1.0, "half the parent's leftward speed: {}", p.vx);
    assert_eq!(p.color, [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn sub_emitters_dont_start_each_other_forever() {
    let me = Handle::<ParticleEffect>::new(Guid(7));
    let mut effect = burst(1, 0.01);
    effect.sub_emitters =
        vec![SubEmitter { trigger: SubTrigger::Death, effect: me, ..SubEmitter::default() }];
    let library = Library(HashMap::from([(me.guid, Arc::new(effect))]));
    let mut world = ParticleWorld::new();
    world.play(&library, me, Spawn::default()).unwrap();
    run_with(&mut world, 2.0, &library, &no_ground);
    assert_eq!(world.system_count(), 0, "a chain is cut off after a few levels");
}

#[test]
fn local_effects_move_with_their_emitter_and_world_ones_stay() {
    let mut effect = burst(5, 2.0);
    effect.main.speed = Range(0.0, 0.0);
    let mut world = ParticleWorld::new();
    let stay = world.play_effect(Arc::new(effect.clone()), Spawn::default());
    effect.main.space = Space::Local;
    let follow = world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    world.move_to(stay, 50.0, 0.0);
    world.move_to(follow, 50.0, 0.0);
    world.step(FRAME, &NoEffects, &no_ground);
    let s = &world.systems;
    assert!(s[0].particles.iter().all(|p| s[0].world_pos(p) == (0.0, 0.0)));
    assert!(s[1].particles.iter().all(|p| s[1].world_pos(p) == (50.0, 0.0)));
}

#[test]
fn the_same_seed_plays_the_same_effect() {
    let mut effect = burst(50, 1.0);
    effect.shape.spread = 180.0;
    effect.main.speed = Range(10.0, 90.0);
    let play = |seed| {
        let mut world = ParticleWorld::new();
        world.play_effect(Arc::new(effect.clone()), Spawn::default().seed(seed));
        run(&mut world, 0.5);
        positions(&world)
    };
    assert_eq!(play(5), play(5));
    assert_ne!(play(5), play(6));
}

#[test]
fn effects_played_from_files_follow_their_changes() {
    let handle = Handle::<ParticleEffect>::new(Guid(5));
    let mut first = burst(1, 10.0);
    first.main.looping = true;
    first.emission.rate = 0.0;
    first.emission.bursts.clear();
    let mut world = ParticleWorld::new();
    let library = Library(HashMap::from([(handle.guid, Arc::new(first.clone()))]));
    let id = world.play(&library, handle, Spawn::default()).unwrap();
    run_with(&mut world, 0.5, &library, &no_ground);
    assert_eq!(world.particle_count(), 0);
    let mut changed = first;
    changed.emission.rate = 60.0;
    let library = Library(HashMap::from([(handle.guid, Arc::new(changed))]));
    run_with(&mut world, 0.5, &library, &no_ground);
    assert!(world.particle_count() >= 28, "the new rate applies at once");
    assert!(world.is_alive(id));
    assert!(world.play(&NoEffects, handle, Spawn::default()).is_none(), "missing: no effect");
}

#[test]
fn a_huge_or_bad_frame_time_is_handled() {
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(burst(3, 0.5)), Spawn::default());
    world.step(f32::NAN, &NoEffects, &no_ground);
    world.step(-1.0, &NoEffects, &no_ground);
    assert_eq!(world.particle_count(), 0, "nothing happened");
    world.step(1000.0, &NoEffects, &no_ground);
    // Only a quarter second was stepped: the burst fired and is still alive.
    assert_eq!(world.particle_count(), 3);
}

// Drawing.

fn quads(world: &ParticleWorld) -> Vec<Quad> {
    world.quads(&mut |_| None)
}

/// One particle at (10, 20), standing still, drawn as the effect says.
fn still(render: Render) -> ParticleWorld {
    let mut effect = burst(1, 1.0);
    effect.main.speed = Range(0.0, 0.0);
    effect.main.size = Range(4.0, 4.0);
    effect.render = render;
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(10.0, 20.0));
    world.step(FRAME, &NoEffects, &no_ground);
    world
}

#[test]
fn squares_glows_and_streaks_are_sized_and_placed() {
    let q = quads(&still(Render::default()));
    assert_eq!(q.len(), 1);
    assert_eq!((q[0].x, q[0].y, q[0].w, q[0].h), (8.0, 18.0, 4.0, 4.0));
    assert_eq!(q[0].image, QuadImage::Solid);

    let q = quads(&still(Render { aspect: 3.0, ..Render::default() }));
    assert_eq!((q[0].w, q[0].h), (12.0, 4.0), "a flat splat");

    let q = quads(&still(Render {
        mode: RenderMode::Glow,
        blend: BlendMode::Add,
        ..Render::default()
    }));
    assert_eq!((q[0].image, q[0].blend, q[0].w), (QuadImage::Glow, BlendMode::Add, 4.0));

    // A streak moving right at 300 px/s, 0.012 s long: a 4-pixel line
    // trailing left of its head, one pixel thick.
    let mut effect = burst(1, 1.0);
    effect.shape = Shape { kind: ShapeKind::Point, angle: 0.0, spread: 0.0, ..Shape::default() };
    effect.main.speed = Range(300.0, 300.0);
    effect.main.size = Range(1.0, 1.0);
    effect.render = Render { mode: RenderMode::Streak, ..Render::default() };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(0.0, 0.0));
    world.step(FRAME, &NoEffects, &no_ground);
    let q = quads(&world);
    assert_eq!(q.len(), 4);
    let xs: Vec<f32> = q.iter().map(|q| q.x).collect();
    assert_eq!(xs, vec![5.0, 4.0, 3.0, 2.0]);
    assert!(q.iter().all(|q| q.y == 0.0 && q.w == 1.0));
}

#[test]
fn colour_and_size_over_life_apply_and_faded_particles_arent_drawn() {
    let mut effect = burst(1, 1.0);
    effect.main.speed = Range(0.0, 0.0);
    effect.main.size = Range(8.0, 8.0);
    effect.main.color = ColorRange([255, 0, 0, 255], [255, 0, 0, 255]);
    effect.color.enabled = true;
    effect.size = SizeOverLife { enabled: true, curve: Curve(vec![(0.0, 1.0), (1.0, 0.0)]) };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    run(&mut world, 0.5 - FRAME);
    let q = quads(&world);
    assert!((q[0].color[3] - 0.5).abs() < 0.03, "{:?}", q[0].color);
    assert_eq!(q[0].color[0], 1.0);
    assert_eq!(q[0].w, 4.0, "half its size at half its life");
    run(&mut world, 0.48);
    assert!(quads(&world).is_empty(), "too small and faint to draw");
}

#[test]
fn sprite_particles_play_through_their_sprites_over_life() {
    let texture = Handle::new(Guid(77));
    let sheet = Arc::new(SpriteSheet {
        texture,
        density: 2,
        sprites: (0..4)
            .map(|i| Sprite { name: format!("f{i}"), rect: [i * 8, 0, 8, 8], pivot: [4, 4] })
            .collect(),
        ..SpriteSheet::default()
    });
    let handle = Handle::new(Guid(78));
    let mut effect = burst(1, 1.0);
    effect.main.speed = Range(0.0, 0.0);
    effect.main.size = Range(1.0, 1.0);
    effect.render = Render {
        mode: RenderMode::Sprite,
        sheet: handle,
        frames: FrameMode::OverLife,
        ..Render::default()
    };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect.clone()), Spawn::at(10.0, 10.0));
    let mut lookup = |h: Handle<SpriteSheet>| (h == handle).then(|| sheet.clone());
    let mut seen = Vec::new();
    for _ in 0..4 {
        world.step(0.25 - 0.001, &NoEffects, &no_ground);
        let q = world.quads(&mut lookup);
        let QuadImage::Sprite { src, .. } = q[0].image else { panic!("a sprite") };
        assert_eq!((q[0].x, q[0].y, q[0].w), (8.0, 8.0, 4.0), "density 2: half size, on its pivot");
        seen.push(src[0]);
    }
    assert_eq!(seen, vec![0.0, 8.0, 16.0, 24.0]);

    // Named sprites only, and mirrored with the emitter.
    effect.render.sprites = vec!["f2".into()];
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(10.0, 10.0).flip_x(true));
    world.step(FRAME, &NoEffects, &no_ground);
    let q = world.quads(&mut lookup);
    assert!(q[0].flip);
    assert_eq!(q[0].image, QuadImage::Sprite { texture, src: [16.0, 0.0, 8.0, 8.0] });
    // No sheet: nothing drawn, no panic.
    assert!(world.quads(&mut |_| None).is_empty());
}

#[test]
fn grid_sheets_use_their_cells() {
    let sheet = Arc::new(SpriteSheet {
        grid: Some(Grid { cell: [8, 8], columns: 2, rows: 1, ..Grid::default() }),
        cell_pivot: [0, 0],
        ..SpriteSheet::default()
    });
    let mut effect = burst(1, 1.0);
    effect.main.size = Range(1.0, 1.0);
    effect.render = Render {
        mode: RenderMode::Sprite,
        sheet: Handle::new(Guid(3)),
        frames: FrameMode::Fps,
        fps: 4.0,
        ..Render::default()
    };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    let first = world.quads(&mut |_| Some(sheet.clone()));
    run(&mut world, 0.25);
    let second = world.quads(&mut |_| Some(sheet.clone()));
    assert_ne!(first[0].image, second[0].image, "the next cell after a quarter second at 4 fps");
}

#[test]
fn lights_follow_some_particles_and_fade() {
    let mut effect = burst(200, 1.0);
    effect.main.speed = Range(0.0, 0.0);
    effect.main.color = ColorRange([255, 128, 0, 255], [255, 128, 0, 255]);
    effect.lights = Lights { enabled: true, ratio: 0.25, intensity: 2.0, ..Lights::default() };
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::default());
    world.step(FRAME, &NoEffects, &no_ground);
    let lights = world.lights();
    assert!((30..70).contains(&lights.len()), "about a quarter: {}", lights.len());
    assert!(lights.iter().all(|l| (l.color[1] - 128.0 / 255.0).abs() < 1e-3));
    let bright = lights[0].intensity;
    run(&mut world, 0.5);
    assert!(world.lights()[0].intensity < bright * 0.6, "fading with life");
}

#[test]
fn the_glow_image_is_round_and_soft() {
    let img = glow_image();
    assert_eq!(img.len(), (GLOW_SIZE * GLOW_SIZE * 4) as usize);
    let alpha = |x: u32, y: u32| img[((y * GLOW_SIZE + x) * 4 + 3) as usize];
    let mid = GLOW_SIZE / 2;
    assert!(alpha(mid, mid) > 200);
    assert_eq!(alpha(0, 0), 0, "nothing in the corners");
    assert!(alpha(mid, mid) > alpha(mid + 6, mid) && alpha(mid + 6, mid) > alpha(mid + 12, mid));
}

#[test]
fn effects_draw_by_order_and_a_range_picks_some() {
    let mut world = ParticleWorld::new();
    let at = |order: i32, x: f32| {
        let mut e = burst(1, 1.0);
        e.main.speed = Range(0.0, 0.0);
        e.main.size = Range(1.0, 1.0);
        e.render.order = order;
        (Arc::new(e), Spawn::at(x, 0.0))
    };
    for (order, x) in [(5, 1.0), (-1, 2.0), (0, 3.0), (-1, 4.0)] {
        let (e, spawn) = at(order, x);
        world.play_effect(e, spawn);
    }
    world.step(FRAME, &NoEffects, &no_ground);
    // One-pixel squares centred on x land on whole pixel x.
    let xs = |q: Vec<Quad>| q.iter().map(|q| q.x).collect::<Vec<_>>();
    assert_eq!(xs(quads(&world)), vec![2.0, 4.0, 3.0, 1.0], "by order, then as played");
    assert_eq!(xs(world.quads_in(i32::MIN..=-1, &mut |_| None)), vec![2.0, 4.0]);
    assert_eq!(xs(world.quads_in(0..=i32::MAX, &mut |_| None)), vec![3.0, 1.0]);
}

#[test]
fn the_shape_offset_moves_turns_and_mirrors_with_the_effect() {
    let mut effect = burst(1, 1.0);
    effect.main.speed = Range(0.0, 0.0);
    effect.shape = Shape { kind: ShapeKind::Point, offset: [3.0, -1.0], ..Shape::default() };
    let place = |spawn: Spawn| {
        let mut world = ParticleWorld::new();
        world.play_effect(Arc::new(effect.clone()), spawn);
        world.step(FRAME, &NoEffects, &no_ground);
        let (x, y) = positions(&world)[0];
        ((x * 100.0).round() / 100.0, (y * 100.0).round() / 100.0)
    };
    assert_eq!(place(Spawn::at(10.0, 20.0)), (13.0, 19.0));
    assert_eq!(place(Spawn::at(10.0, 20.0).flip_x(true)), (7.0, 19.0));
    assert_eq!(place(Spawn::at(10.0, 20.0).angle(90.0)), (11.0, 23.0), "turned a quarter");
}

#[test]
fn sub_emitters_play_upright_whatever_their_parent_was_turned_to() {
    let child = Handle::<ParticleEffect>::new(Guid(11));
    let mut c = burst(1, 1.0);
    c.main.speed = Range(0.0, 0.0);
    c.shape = Shape { kind: ShapeKind::Point, offset: [0.0, -1.0], ..Shape::default() };
    let library = Library(HashMap::from([(child.guid, Arc::new(c))]));
    let mut effect = burst(1, 0.02);
    effect.main.speed = Range(0.0, 0.0);
    effect.sub_emitters =
        vec![SubEmitter { trigger: SubTrigger::Death, effect: child, ..SubEmitter::default() }];
    let mut world = ParticleWorld::new();
    world.play_effect(Arc::new(effect), Spawn::at(10.0, 20.0).angle(-30.0));
    run_with(&mut world, 0.1, &library, &no_ground);
    let (x, y) = positions(&world)[0];
    assert!((x - 10.0).abs() < 1e-4 && (y - 19.0).abs() < 1e-4, "straight up: {x}, {y}");
    assert_eq!(world.systems[0].spawn.angle, 0.0);
}

#[test]
fn a_flat_splat_draws_even_when_thinner_than_a_pixel() {
    let mut world = still(Render { aspect: 20.0, ..Render::default() });
    world.systems[0].particles[0].size = 0.1;
    let q = quads(&world);
    assert_eq!((q[0].w, q[0].h), (2.0, 1.0), "2 wide, at least a pixel high");
    world.systems[0].particles[0].size = 0.02;
    assert!(quads(&world).is_empty(), "under half a pixel both ways: not drawn");
}
