//! Behaviours: Unity's MonoBehaviour, made deterministic.
//!
//! A Behaviour is a [`Component`] that also implements [`Behaviour`]: all of
//! its state lives in the component, so it is in the scene, cloned and
//! hashed with everything else, and rollback restores it exactly. The
//! engine runs every Behaviour through a fixed [`Schedule`] of stages each
//! tick; the game never calls an object's update by hand.
//!
//! # Order
//!
//! Within a stage, Behaviours run **by type, in [`Behaviour::ORDER`]**
//! (ties in registration order), **then by [`ObjectId`]**. Spawns and
//! despawns made during a tick go through [`Ctx::commands`] and are applied
//! together in the `Commands` stage, so the set of objects never changes
//! under a running stage, and ids are handed out in a fixed order on every
//! peer.
//!
//! # Borrowing
//!
//! Callbacks borrow their components through [`Ctx::scene_mut`] or split
//! component queries. Values stay in their stores, and Rust enforces the
//! lifetimes of all borrows. Structural edits use the command buffer.
//!
//! # Messages
//!
//! [`Ctx::send`] and [`Ctx::broadcast`] queue plain `Clone + Hash` data.
//! The `Messages` stage delivers the queue in send order; messages sent
//! while delivering are delivered in the same stage, after the ones already
//! queued. The schedule always flushes the queue before a tick ends, so no
//! message waits in hidden state between ticks.
//!
//! # Rollback rules
//!
//! - All state lives in components, or in the game's own hashed world.
//!   No statics, `Rc`/`RefCell`, closures or trait objects holding state.
//! - Randomness only from [`Ctx::rng`]. No floats, no `HashMap`s.
//! - Physics and other resources belong to the generic game world. The
//!   runtime requires no tile map, physics simulation, or player input.
//! - [`Ctx::emit`] output (sounds, particles) is for presentation only and
//!   must never feed back into the simulation: a rollback replays it.

use std::fmt::Debug;
use std::hash::Hash;

use reign_assets::AssetDb;
use reign_core::{InputBuffer, Rng};
use reign_scene::{Commands, Component, ObjectId, Scene};

/// The game's side of the runtime: where its scene and shared simulation
/// state live. Implemented by the game's world type.
pub trait Game: Clone + Hash + Sized + 'static {
    /// Messages Behaviours send each other. Plain data.
    type Message: Clone + Debug + PartialEq + Eq + Hash + Send + Sync + 'static;
    /// Presentation output: sounds, particles. Never read by the simulation.
    type Event: Clone + Debug + 'static;

    fn scene(&self) -> &Scene;
    fn scene_mut(&mut self) -> &mut Scene;
    /// Player `player`'s buffered input.
    fn input(&mut self, _player: usize) -> Option<&mut InputBuffer> {
        None
    }
    fn rng(&mut self) -> &mut Rng;
    /// The tick being simulated.
    fn tick(&self) -> u64;
    /// The runtime's own state, kept in the game's hashed world.
    fn behaviours(&mut self) -> &mut BehaviourState;
}

/// What the runtime remembers between ticks: which Behaviours have had
/// `start`. Part of the game's world, so it rolls back with it.
#[derive(Clone, Debug, Default)]
pub struct BehaviourState {
    /// (type, object), sorted.
    started: Vec<(&'static str, ObjectId)>,
    /// The scene's revision when `start` last ran: nothing new since then
    /// means nothing to start. A cache, so not part of `Hash` or `Eq`.
    checked: Option<u64>,
}

impl PartialEq for BehaviourState {
    fn eq(&self, other: &Self) -> bool {
        self.started == other.started
    }
}

impl Eq for BehaviourState {}

impl std::hash::Hash for BehaviourState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.started.hash(state);
    }
}

impl BehaviourState {
    fn is_started(&self, name: &'static str, id: ObjectId) -> bool {
        self.started.binary_search(&(name, id)).is_ok()
    }

    fn mark(&mut self, name: &'static str, id: ObjectId) {
        if let Err(at) = self.started.binary_search(&(name, id)) {
            self.started.insert(at, (name, id));
        }
    }
}

/// Who a message is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum To {
    /// Every Behaviour on this object.
    Object(ObjectId),
    /// Every Behaviour in the scene, in execution order.
    All,
}

/// A message on its way.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Envelope<M> {
    pub to: To,
    /// The object that sent it, if a Behaviour did.
    pub from: Option<ObjectId>,
    pub body: M,
}

/// A per-object script. Implement it on a `#[derive(Component)]` struct
/// that holds all its state, and register it with
/// [`Behaviours::behaviour`].
#[allow(unused_variables)]
pub trait Behaviour<G: Game>: Component {
    /// Execution order: lower runs earlier. Ties run in registration order.
    const ORDER: i32 = 0;

    /// Once, before the object's first `fixed_update` (or at the end of
    /// the tick that spawned it).
    fn start(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
    /// Every tick, before physics and combat.
    fn fixed_update(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
    /// Every tick, after combat and messages.
    fn late_update(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
    /// A message to this object, or a broadcast.
    fn on_message(ctx: &mut Ctx<'_, G>, me: ObjectId, msg: &Envelope<G::Message>) {}
    /// Just before the object is despawned through the command buffer.
    fn on_destroy(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
}

/// What a Behaviour or system can reach while the tick runs.
pub struct Ctx<'a, G: Game> {
    /// The game's typed, rollback-safe world and its optional resources.
    pub game: &'a mut G,
    /// Spawns, despawns and component changes, applied in the `Commands`
    /// stage.
    pub commands: &'a mut Commands,
    outbox: &'a mut Vec<Envelope<G::Message>>,
    events: &'a mut Vec<G::Event>,
    assets: Option<&'a AssetDb>,
    /// The object whose Behaviour is running, for `from`.
    current: Option<ObjectId>,
}

impl<'a, G: Game> Ctx<'a, G> {
    pub fn scene(&self) -> &Scene {
        self.game.scene()
    }

    pub fn scene_mut(&mut self) -> &mut Scene {
        self.game.scene_mut()
    }

    /// Player `player`'s buffered input.
    pub fn input(&mut self, player: usize) -> Option<&mut InputBuffer> {
        self.game.input(player)
    }

    /// The simulation's deterministic random numbers.
    pub fn rng(&mut self) -> &mut Rng {
        self.game.rng()
    }

    pub fn tick(&self) -> u64 {
        self.game.tick()
    }

    /// Assets by handle, when the game runs with a database.
    pub fn assets(&self) -> Option<&'a AssetDb> {
        self.assets
    }

    /// Queues `msg` for object `to`.
    pub fn send(&mut self, to: ObjectId, msg: G::Message) {
        self.outbox.push(Envelope { to: To::Object(to), from: self.current, body: msg });
    }

    /// Queues `msg` for every Behaviour.
    pub fn broadcast(&mut self, msg: G::Message) {
        self.outbox.push(Envelope { to: To::All, from: self.current, body: msg });
    }

    /// Presentation output for this tick.
    pub fn emit(&mut self, event: G::Event) {
        self.events.push(event);
    }
}

/// One registered Behaviour type, with its type erased.
struct Entry<G: Game> {
    name: &'static str,
    order: i32,
    ids: fn(&Scene) -> Vec<ObjectId>,
    has: fn(&Scene, ObjectId) -> bool,
    call: fn(&mut Ctx<'_, G>, ObjectId, Call<'_, G::Message>),
}

#[derive(Clone, Copy)]
enum Call<'m, M> {
    Start,
    Fixed,
    Late,
    Message(&'m Envelope<M>),
    Destroy,
}

fn call<G: Game, T: Behaviour<G>>(ctx: &mut Ctx<'_, G>, id: ObjectId, what: Call<'_, G::Message>) {
    if !ctx.game.scene().has::<T>(id) {
        return;
    }
    let outer = ctx.current.replace(id);
    match what {
        Call::Start => T::start(ctx, id),
        Call::Fixed => T::fixed_update(ctx, id),
        Call::Late => T::late_update(ctx, id),
        Call::Message(msg) => T::on_message(ctx, id, msg),
        Call::Destroy => T::on_destroy(ctx, id),
    }
    ctx.current = outer;
}

/// Every Behaviour type a game uses, in execution order. Configuration,
/// not state: build it once at startup.
pub struct Behaviours<G: Game> {
    entries: Vec<Entry<G>>,
}

impl<G: Game> Default for Behaviours<G> {
    fn default() -> Self {
        Behaviours { entries: Vec::new() }
    }
}

impl<G: Game> Behaviours<G> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `T`. Registering a type twice does nothing.
    pub fn behaviour<T: Behaviour<G>>(&mut self) -> &mut Self {
        if self.entries.iter().any(|e| e.name == T::NAME) {
            return self;
        }
        let entry = Entry {
            name: T::NAME,
            order: T::ORDER,
            ids: |scene| scene.iter::<T>().map(|(id, _)| id).collect(),
            has: |scene, id| scene.has::<T>(id),
            call: call::<G, T>,
        };
        // Stable: after every type with an order <= this one's.
        let at = self.entries.iter().position(|e| e.order > T::ORDER).unwrap_or(self.entries.len());
        self.entries.insert(at, entry);
        self
    }

    /// Registered type names, in execution order.
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|e| e.name).collect()
    }

    /// Every (type, object) a stage would visit, in execution order.
    pub fn execution_order(&self, scene: &Scene) -> Vec<(&'static str, ObjectId)> {
        self.entries
            .iter()
            .flat_map(|e| (e.ids)(scene).into_iter().map(move |id| (e.name, id)))
            .collect()
    }

    fn each(&self, ctx: &mut Ctx<'_, G>, what: fn(&Entry<G>) -> Call<'static, G::Message>) {
        for entry in &self.entries {
            for id in (entry.ids)(ctx.game.scene()) {
                (entry.call)(ctx, id, what(entry));
            }
        }
    }

    /// Calls `start` on every Behaviour that hasn't had it, in execution
    /// order, and forgets objects that are gone.
    pub fn start(&self, ctx: &mut Ctx<'_, G>) {
        let revision = ctx.game.scene().revision();
        if ctx.game.behaviours().checked == Some(revision) {
            return;
        }
        let scene = ctx.game.scene();
        let all: Vec<(usize, ObjectId)> = self
            .entries
            .iter()
            .enumerate()
            .flat_map(|(i, e)| (e.ids)(scene).into_iter().map(move |id| (i, id)))
            .collect();
        let state = ctx.game.behaviours();
        let pending: Vec<(usize, ObjectId)> = all
            .iter()
            .copied()
            .filter(|&(i, id)| !state.is_started(self.entries[i].name, id))
            .collect();
        // Forget objects that lost the component or are gone (inactive
        // ones are kept: start runs once per object, as in Unity).
        let mut started = std::mem::take(&mut state.started);
        let scene = ctx.game.scene();
        started.retain(|(name, id)| {
            self.entries.iter().any(|e| e.name == *name && (e.has)(scene, *id))
        });
        ctx.game.behaviours().started = started;
        for (i, id) in pending {
            let entry = &self.entries[i];
            ctx.game.behaviours().mark(entry.name, id);
            (entry.call)(ctx, id, Call::Start);
        }
        let revision = ctx.game.scene().revision();
        ctx.game.behaviours().checked = Some(revision);
    }

    /// `fixed_update` on every Behaviour, in execution order.
    pub fn fixed_update(&self, ctx: &mut Ctx<'_, G>) {
        self.each(ctx, |_| Call::Fixed);
    }

    /// `late_update` on every Behaviour, in execution order.
    pub fn late_update(&self, ctx: &mut Ctx<'_, G>) {
        self.each(ctx, |_| Call::Late);
    }

    /// Delivers queued messages in send order, including ones sent while
    /// delivering, until none are left.
    pub fn deliver(&self, ctx: &mut Ctx<'_, G>) {
        // A cap so two objects answering each other forever fail loudly.
        let mut budget = 1 << 16;
        while !ctx.outbox.is_empty() {
            let batch = std::mem::take(ctx.outbox);
            for msg in &batch {
                budget -= 1;
                assert!(budget > 0, "messages kept sending messages");
                match msg.to {
                    To::Object(id) => {
                        for entry in &self.entries {
                            if (entry.has)(ctx.game.scene(), id) {
                                (entry.call)(ctx, id, Call::Message(msg));
                            }
                        }
                    }
                    To::All => {
                        for entry in &self.entries {
                            for id in (entry.ids)(ctx.game.scene()) {
                                (entry.call)(ctx, id, Call::Message(msg));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Calls `on_destroy` for every object the command buffer despawns
    /// (and everything under it), then applies the buffer, repeating for
    /// commands queued by `on_destroy`.
    pub fn apply_commands(&self, ctx: &mut Ctx<'_, G>) -> bool {
        let applied = !ctx.commands.is_empty();
        while !ctx.commands.is_empty() {
            let mut doomed = Vec::new();
            for id in ctx.commands.despawned() {
                let scene = ctx.game.scene();
                if scene.contains(id) {
                    let mut stack = vec![id];
                    while let Some(at) = stack.pop() {
                        doomed.push(at);
                        stack.extend(scene.children(at).iter().rev());
                    }
                }
            }
            let mut batch = std::mem::take(ctx.commands);
            for id in doomed {
                for entry in &self.entries {
                    if (entry.has)(ctx.game.scene(), id) {
                        (entry.call)(ctx, id, Call::Destroy);
                    }
                }
            }
            // Commands queued by on_destroy run after the ones that caused it.
            batch.append(ctx.commands);
            ctx.game.scene_mut().apply(&mut batch);
        }
        applied
    }
}

/// Whether the rest of the tick runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// Skip to the end of the tick: messages are still delivered, commands
    /// applied and new objects started. For hitstop and pauses.
    EndTick,
}

/// A game system: code that isn't one object's, such as moving platforms
/// or resolving hits across everyone.
pub type SystemFn<G> = fn(&mut Ctx<'_, G>) -> Flow;

/// Stateless shared rule registered into a named stage. Lower order runs
/// first; ties preserve registration order. All mutable state belongs to G.
pub struct System<G: Game> {
    pub name: &'static str,
    pub stage: &'static str,
    pub order: i32,
    run: SystemFn<G>,
}

impl<G: Game> System<G> {
    pub const fn new(
        name: &'static str,
        stage: &'static str,
        order: i32,
        run: SystemFn<G>,
    ) -> Self {
        Self { name, stage, order, run }
    }
}

/// One step of the tick.
pub enum Stage {
    /// `start` for Behaviours that haven't had it.
    Start,
    /// All systems registered for this stage, by order then registration.
    Systems(&'static str),
    /// `fixed_update` on every Behaviour.
    FixedUpdate,
    /// Deliver queued messages.
    Messages,
    /// `late_update` on every Behaviour.
    LateUpdate,
    /// `on_destroy`, then apply the command buffer, then `start` what it
    /// made.
    Commands,
}

/// The stages of a tick, in order. The standard one is
///
/// ```text
/// Start → input → FixedUpdate → physics → combat → Messages → LateUpdate
///       → Messages → Commands
/// ```
///
/// where `input`, `physics` and `combat` are the game's systems. A game may
/// build its own order with [`Schedule::then`]; whatever the order, a tick
/// always ends by delivering messages, applying commands and starting new
/// objects.
pub struct Schedule<G: Game> {
    stages: Vec<Stage>,
    systems: Vec<System<G>>,
}

impl<G: Game> Default for Schedule<G> {
    fn default() -> Self {
        Schedule { stages: Vec::new(), systems: Vec::new() }
    }
}

impl<G: Game> Schedule<G> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The standard order, with the game's input, physics and combat.
    pub fn standard(input: SystemFn<G>, physics: SystemFn<G>, combat: SystemFn<G>) -> Self {
        Self::new()
            .then(Stage::Start)
            .then(Stage::Systems("input"))
            .then(Stage::FixedUpdate)
            .then(Stage::Systems("physics"))
            .then(Stage::Systems("combat"))
            .then(Stage::Messages)
            .then(Stage::LateUpdate)
            .then(Stage::Messages)
            .then(Stage::Commands)
            .system(System::new("input", "input", 0, input))
            .system(System::new("physics", "physics", 0, physics))
            .system(System::new("combat", "combat", 0, combat))
    }

    /// Registers shared work. Stage names must appear in the schedule when
    /// it runs, and names must be unique within a stage.
    pub fn system(mut self, system: System<G>) -> Self {
        assert!(
            !self.systems.iter().any(|s| s.stage == system.stage && s.name == system.name),
            "duplicate system in stage"
        );
        let at =
            self.systems.iter().position(|s| s.order > system.order).unwrap_or(self.systems.len());
        self.systems.insert(at, system);
        self
    }

    /// Registered names and order values for one stage, in execution order.
    pub fn systems(&self, stage: &str) -> impl Iterator<Item = (&'static str, i32)> + '_ {
        let stage = stage.to_owned();
        self.systems.iter().filter(move |s| s.stage == stage).map(|s| (s.name, s.order))
    }

    pub fn then(mut self, stage: Stage) -> Self {
        self.stages.push(stage);
        self
    }

    /// Runs one tick. Returns the presentation events it emitted.
    pub fn run(
        &self,
        behaviours: &Behaviours<G>,
        game: &mut G,
        assets: Option<&AssetDb>,
    ) -> Vec<G::Event> {
        for system in &self.systems {
            assert!(
                self.stages
                    .iter()
                    .any(|s| matches!(s, Stage::Systems(name) if *name == system.stage)),
                "system {:?} references missing stage {:?}",
                system.name,
                system.stage
            );
        }
        let mut commands = Commands::new();
        let mut outbox = Vec::new();
        let mut events = Vec::new();
        let mut ctx = Ctx {
            game,
            commands: &mut commands,
            outbox: &mut outbox,
            events: &mut events,
            assets,
            current: None,
        };
        for stage in &self.stages {
            let flow = match stage {
                Stage::Start => {
                    behaviours.start(&mut ctx);
                    Flow::Continue
                }
                Stage::Systems(name) => {
                    let mut flow = Flow::Continue;
                    for system in self.systems.iter().filter(|s| s.stage == *name) {
                        flow = (system.run)(&mut ctx);
                        if flow == Flow::EndTick {
                            break;
                        }
                    }
                    flow
                }
                Stage::FixedUpdate => {
                    behaviours.fixed_update(&mut ctx);
                    Flow::Continue
                }
                Stage::Messages => {
                    behaviours.deliver(&mut ctx);
                    Flow::Continue
                }
                Stage::LateUpdate => {
                    behaviours.late_update(&mut ctx);
                    Flow::Continue
                }
                Stage::Commands => {
                    if behaviours.apply_commands(&mut ctx) {
                        behaviours.start(&mut ctx);
                    }
                    Flow::Continue
                }
            };
            if flow == Flow::EndTick {
                break;
            }
        }
        // Nothing waits between ticks outside the hashed world: deliver
        // what's left, apply what's queued and start what that made.
        behaviours.deliver(&mut ctx);
        if behaviours.apply_commands(&mut ctx) {
            behaviours.start(&mut ctx);
            behaviours.deliver(&mut ctx);
            behaviours.apply_commands(&mut ctx);
        }
        events
    }

    /// Runs `start` (and what it queues) outside a tick: after building a
    /// world, so placed objects are ready before the first tick.
    pub fn start(&self, behaviours: &Behaviours<G>, game: &mut G, assets: Option<&AssetDb>) {
        for system in &self.systems {
            assert!(
                self.stages
                    .iter()
                    .any(|s| matches!(s, Stage::Systems(name) if *name == system.stage)),
                "system {:?} references missing stage {:?}",
                system.name,
                system.stage
            );
        }
        let mut commands = Commands::new();
        let mut outbox = Vec::new();
        let mut events = Vec::new();
        let mut ctx = Ctx {
            game,
            commands: &mut commands,
            outbox: &mut outbox,
            events: &mut events,
            assets,
            current: None,
        };
        behaviours.start(&mut ctx);
        behaviours.deliver(&mut ctx);
        behaviours.apply_commands(&mut ctx);
    }
}

#[cfg(test)]
mod tests;
