use reign_core::{Rng, checksum};
use reign_scene::{Component, ObjectId, Scene};
use serde::{Deserialize, Serialize};

use super::*;

/// A tiny game with no physics, tile map or player input. Its typed
/// resources are the log and RNG, cloned and hashed with the scene.
#[derive(Clone, Hash, PartialEq, Eq)]
struct World {
    scene: Scene,
    rng: Rng,
    tick: u64,
    state: BehaviourState,
    log: Vec<(&'static str, ObjectId, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Msg {
    Ping(u32),
    Pong(u32),
    Hello,
}

impl Game for World {
    type Message = Msg;
    type Event = ();

    fn scene(&self) -> &Scene {
        &self.scene
    }
    fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }
    fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }
    fn tick(&self) -> u64 {
        self.tick
    }
    fn behaviours(&mut self) -> &mut BehaviourState {
        &mut self.state
    }
}

/// Logs each update; counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Walker {
    steps: u32,
    started: bool,
    starts: u32,
}

impl Behaviour<World> for Walker {
    const ORDER: i32 = 10;
    fn start(ctx: &mut Ctx<'_, World>, me: ObjectId) {
        let actor = ctx.scene_mut().get_mut::<Self>(me).unwrap();
        actor.started = true;
        actor.starts += 1;
    }
    fn fixed_update(ctx: &mut Ctx<'_, World>, me: ObjectId) {
        let random = ctx.rng().range(0, 2) as u32;
        let actor = ctx.scene_mut().get_mut::<Self>(me).unwrap();
        assert!(actor.started, "start runs before the first update");
        actor.steps += 1;
        // Randomness only from the context, so it rolls back.
        actor.steps += random;
        let tick = ctx.tick();
        ctx.game.log.push(("walker", me, tick));
    }
}

/// Runs before Walkers, whatever was registered first.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Early {}

impl Behaviour<World> for Early {
    const ORDER: i32 = -10;
    fn fixed_update(ctx: &mut Ctx<'_, World>, me: ObjectId) {
        let tick = ctx.tick();
        ctx.game.log.push(("early", me, tick));
    }
}

/// Spawns a Walker every other tick and despawns the oldest every third,
/// so slots get reused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Churn {
    spawned: Vec<ObjectId>,
    destroyed: u32,
}

impl Behaviour<World> for Churn {
    const ORDER: i32 = 20;
    fn fixed_update(ctx: &mut Ctx<'_, World>, _: ObjectId) {
        let tick = ctx.tick();
        if tick.is_multiple_of(2) {
            let t = ctx.commands.spawn("walker");
            ctx.commands.insert(t, Walker::default());
        }
        if tick.is_multiple_of(3) {
            let oldest = ctx.scene().iter::<Walker>().next().map(|(id, _)| id);
            if let Some(id) = oldest {
                ctx.commands.despawn(id);
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Mortal {}

impl Behaviour<World> for Mortal {
    fn on_destroy(ctx: &mut Ctx<'_, World>, me: ObjectId) {
        let tick = ctx.tick();
        ctx.game.log.push(("destroyed", me, tick));
    }
}

/// Sends pings to `peer`; answers pings with pongs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
struct Talker {
    peer: Option<ObjectId>,
    heard: Vec<String>,
}

impl Behaviour<World> for Talker {
    fn fixed_update(ctx: &mut Ctx<'_, World>, me: ObjectId) {
        if let Some(peer) = ctx.scene().get::<Self>(me).unwrap().peer {
            ctx.send(peer, Msg::Ping(1));
            ctx.send(peer, Msg::Ping(2));
            ctx.broadcast(Msg::Hello);
        }
    }
    fn on_message(ctx: &mut Ctx<'_, World>, me: ObjectId, msg: &Envelope<Msg>) {
        let from = msg.from.map_or(-1, |f| f.index as i64);
        ctx.scene_mut()
            .get_mut::<Self>(me)
            .unwrap()
            .heard
            .push(format!("{:?} from {from} to {}", msg.body, me.index));
        if let (Msg::Ping(n), Some(from)) = (&msg.body, msg.from) {
            ctx.send(from, Msg::Pong(*n));
        }
    }
}

fn registry() -> Behaviours<World> {
    let mut b = Behaviours::new();
    // Registered out of order on purpose: ORDER decides.
    b.behaviour::<Walker>()
        .behaviour::<Churn>()
        .behaviour::<Early>()
        .behaviour::<Mortal>()
        .behaviour::<Talker>();
    b
}

fn schedule() -> Schedule<World> {
    fn tick(ctx: &mut Ctx<'_, World>) -> Flow {
        ctx.game.tick += 1;
        Flow::Continue
    }
    fn none(_: &mut Ctx<'_, World>) -> Flow {
        Flow::Continue
    }
    Schedule::standard(tick, none, none)
}

fn world() -> World {
    let mut scene = Scene::new();
    for i in 0..3 {
        let id = scene.spawn(format!("walker {i}"));
        scene.insert(id, Walker::default());
    }
    let early = scene.spawn("early");
    scene.insert(early, Early {});
    let churn = scene.spawn("churn");
    scene.insert(churn, Churn::default());
    World { scene, rng: Rng::new(7), tick: 0, state: BehaviourState::default(), log: Vec::new() }
}

fn step(world: &mut World) {
    schedule().run(&registry(), world, None);
}

#[test]
fn types_run_in_order_then_objects_by_id() {
    assert_eq!(registry().names(), ["Early", "Mortal", "Talker", "Walker", "Churn"]);
    let mut w = world();
    step(&mut w);
    let kinds: Vec<_> = w.log.iter().map(|(k, id, _)| (*k, id.index)).collect();
    assert_eq!(kinds, [("early", 3), ("walker", 0), ("walker", 1), ("walker", 2)]);
}

#[test]
fn execution_order_is_stable_after_spawn_and_despawn_churn() {
    let mut a = world();
    for _ in 0..200 {
        step(&mut a);
        // Every stage visits types by order, then ids ascending.
        let order = registry().execution_order(&a.scene);
        let mut sorted = order.clone();
        let rank = |name: &str| registry().names().iter().position(|n| *n == name).unwrap();
        sorted.sort_by_key(|(name, id)| (rank(name), *id));
        assert_eq!(order, sorted);
        let walkers: Vec<_> = a.log.iter().filter(|e| e.0 == "walker" && e.2 == a.tick).collect();
        assert!(walkers.windows(2).all(|w| w[0].1 < w[1].1), "walkers by id");
    }
    assert!(!a.scene.iter::<Walker>().next().is_none(), "churn left walkers");
    // A fresh run from the same start lands on the same state, ids and all.
    let mut b = world();
    for _ in 0..200 {
        step(&mut b);
    }
    assert!(a == b);
    assert_eq!(checksum(&a), checksum(&b));
}

#[test]
fn spawned_behaviours_start_at_the_end_of_their_tick_and_once() {
    let mut w = world();
    step(&mut w); // tick 1: odd, nothing spawned
    step(&mut w); // tick 2: spawns one
    let newest = w.scene.iter::<Walker>().last().map(|(id, c)| (id, c.clone())).unwrap();
    assert!(newest.1.started && newest.1.steps == 0, "started, not yet updated");
    step(&mut w);
    let after = w.scene.get::<Walker>(newest.0).unwrap();
    assert!(after.steps >= 1);
    // Deactivating and reactivating doesn't start it again.
    w.scene.object_mut(newest.0).unwrap().active = false;
    step(&mut w);
    w.scene.object_mut(newest.0).unwrap().active = true;
    step(&mut w);
    assert_eq!(w.scene.get::<Walker>(newest.0).unwrap().starts, 1);
}

#[test]
fn messages_arrive_in_send_order_within_the_tick() {
    let mut w = world();
    let a = w.scene.spawn("a");
    let b = w.scene.spawn("b");
    w.scene.insert(a, Talker { peer: Some(b), heard: vec![] });
    w.scene.insert(b, Talker::default());
    step(&mut w);
    let heard_a = &w.scene.get::<Talker>(a).unwrap().heard;
    let heard_b = &w.scene.get::<Talker>(b).unwrap().heard;
    let (ai, bi) = (a.index, b.index);
    assert_eq!(
        heard_b,
        &[
            format!("Ping(1) from {ai} to {bi}"),
            format!("Ping(2) from {ai} to {bi}"),
            format!("Hello from {ai} to {bi}"),
        ]
    );
    // The broadcast reaches a too, then the pongs b sent while it was
    // being delivered to, in the order b sent them.
    assert_eq!(
        heard_a,
        &[
            format!("Hello from {ai} to {ai}"),
            format!("Pong(1) from {bi} to {ai}"),
            format!("Pong(2) from {bi} to {ai}"),
        ]
    );
}

#[test]
fn on_destroy_runs_before_despawn_for_the_object_and_its_children() {
    let mut w = world();
    let parent = w.scene.spawn("parent");
    let child = w.scene.spawn_child(parent, "child");
    w.scene.insert(parent, Mortal {});
    w.scene.insert(child, Mortal {});
    let behaviours = registry();
    let mut commands = Commands::new();
    commands.despawn(parent);
    let mut outbox = Vec::new();
    let mut events = Vec::new();
    let mut ctx = Ctx {
        game: &mut w,
        commands: &mut commands,
        outbox: &mut outbox,
        events: &mut events,
        assets: None,
        current: None,
    };
    behaviours.apply_commands(&mut ctx);
    let destroyed: Vec<_> = w.log.iter().filter(|e| e.0 == "destroyed").map(|e| e.1).collect();
    assert_eq!(destroyed, [parent, child]);
    assert!(!w.scene.contains(parent) && !w.scene.contains(child));
}

#[test]
fn snapshot_and_restore_with_live_behaviours_replays_identically() {
    let mut w = world();
    let a = w.scene.spawn("a");
    let b = w.scene.spawn("b");
    w.scene.insert(a, Talker { peer: Some(b), heard: vec![] });
    w.scene.insert(b, Talker::default());
    for _ in 0..50 {
        step(&mut w);
    }
    let snapshot = w.clone();
    for _ in 0..50 {
        step(&mut w);
    }
    let first = checksum(&w);
    let mut replay = snapshot.clone();
    for _ in 0..50 {
        step(&mut replay);
    }
    assert_eq!(checksum(&replay), first, "rollback replays to the same state");
    assert!(replay == w);
    // And the snapshot really was different: Behaviour state is in it.
    assert_ne!(checksum(&snapshot), first);
}

#[test]
fn systems_run_by_stage_order_then_stable_registration() {
    fn a(ctx: &mut Ctx<'_, World>) -> Flow {
        ctx.game.log.push(("a", ObjectId::new(0, 0), 0));
        Flow::Continue
    }
    fn b(ctx: &mut Ctx<'_, World>) -> Flow {
        ctx.game.log.push(("b", ObjectId::new(0, 0), 0));
        Flow::Continue
    }
    fn c(ctx: &mut Ctx<'_, World>) -> Flow {
        ctx.game.log.push(("c", ObjectId::new(0, 0), 0));
        Flow::Continue
    }
    let schedule = Schedule::new()
        .then(Stage::Systems("first"))
        .then(Stage::Systems("second"))
        .system(System::new("c", "second", -100, c))
        .system(System::new("b", "first", 10, b))
        .system(System::new("a", "first", 0, a))
        .system(System::new("c", "first", 10, c));
    assert_eq!(schedule.systems("first").collect::<Vec<_>>(), [("a", 0), ("b", 10), ("c", 10)]);
    let mut w = world();
    let snapshot = w.clone();
    schedule.run(&registry(), &mut w, None);
    assert_eq!(w.log.iter().map(|e| e.0).collect::<Vec<_>>(), ["a", "b", "c", "c"]);
    let mut replay = snapshot;
    schedule.run(&registry(), &mut replay, None);
    assert_eq!(checksum(&w), checksum(&replay));
}

#[test]
fn end_tick_skips_remaining_systems_but_flushes_commands_and_messages() {
    fn stop(ctx: &mut Ctx<'_, World>) -> Flow {
        let id = ctx.commands.spawn("new walker");
        ctx.commands.insert(id, Walker::default());
        ctx.broadcast(Msg::Hello);
        Flow::EndTick
    }
    fn never(_: &mut Ctx<'_, World>) -> Flow {
        panic!("a stopped tick must not reach another system");
    }
    let schedule = Schedule::new()
        .then(Stage::Systems("input"))
        .then(Stage::Systems("later"))
        .system(System::new("stop", "input", 0, stop))
        .system(System::new("never", "input", 1, never))
        .system(System::new("later", "later", 0, never));
    let mut w = world();
    let talker = w.scene.spawn("listener");
    w.scene.insert(talker, Talker::default());
    schedule.run(&registry(), &mut w, None);
    assert_eq!(w.scene.get::<Talker>(talker).unwrap().heard.len(), 1);
    assert!(w.scene.iter::<Walker>().last().unwrap().1.started);
    assert_eq!(w.scene.iter::<Walker>().last().unwrap().1.steps, 0);
}

#[test]
#[should_panic(expected = "missing stage")]
fn system_stage_typos_fail_loudly() {
    let schedule = Schedule::new().system(System::new("missing", "typo", 0, |_| Flow::Continue));
    schedule.run(&registry(), &mut world(), None);
}

#[test]
#[should_panic(expected = "duplicate system")]
fn duplicate_system_names_in_one_stage_are_rejected() {
    Schedule::<World>::new()
        .system(System::new("work", "tick", 0, |_| Flow::Continue))
        .system(System::new("work", "tick", 1, |_| Flow::Continue));
}

#[test]
fn no_physics_game_uses_optional_input_and_replays_systems() {
    fn tick(ctx: &mut Ctx<'_, World>) -> Flow {
        assert!(ctx.input(0).is_none());
        ctx.game.tick += 1;
        Flow::Continue
    }
    let schedule = Schedule::new()
        .then(Stage::Start)
        .then(Stage::Systems("clock"))
        .then(Stage::FixedUpdate)
        .system(System::new("tick", "clock", 0, tick));
    let mut original = world();
    for _ in 0..20 {
        schedule.run(&registry(), &mut original, None);
    }
    let snapshot = original.clone();
    for _ in 0..50 {
        schedule.run(&registry(), &mut original, None);
    }
    let mut replay = snapshot;
    for _ in 0..50 {
        schedule.run(&registry(), &mut replay, None);
    }
    assert_eq!(checksum(&original), checksum(&replay));
    assert_eq!(original.tick, 70);
}
