//! What runs in Play: the [`World`] (scene, input, random numbers), each
//! project's Behaviours on the fixed schedule, physics, and the keyboard.

use reign::app::winit::event::{ElementState, WindowEvent};
use reign::app::winit::keyboard::{KeyCode, PhysicalKey};
use reign::behaviour::{BehaviourState, Behaviours, Flow, Game, Schedule, Stage, System};
use reign::core::{Button, FxVec2, InputBuffer, InputFrame, Rng};
use reign::scene::{Component, ComponentRegistry, ObjectId, Scene};

use crate::{components, physics};

/// Fixed ticks per second: divide a per-second speed by this for one tick.
pub const TICKS_PER_SECOND: i32 = reign::core::TICK_HZ as i32;

/// A project's code, as the editor sees it. Generated for each project by
/// `reign_build`; [`Scripts::none`] for a project without code.
#[derive(Clone, Copy)]
pub struct Scripts {
    /// The project crate this editor was built for, if any.
    pub project: Option<&'static str>,
    /// Registers the project's components.
    pub register: fn(&mut ComponentRegistry),
    /// Registers the project's Behaviours.
    pub behaviours: fn(&mut Behaviours<World>),
}

impl Scripts {
    pub const fn none() -> Self {
        Scripts { project: None, register: |_| {}, behaviours: |_| {} }
    }
}

/// What a Behaviour's `on_message` receives (as `msg.body`): a message
/// another Behaviour sent with [`Ctx::send`] or [`Ctx::broadcast`], or a
/// physics event about something this object touched.
///
/// ```ignore
/// fn on_message(ctx: &mut Ctx, me: ObjectId, msg: &Envelope<Message>) {
///     if let Some(other) = msg.body.trigger_enter() {
///         // `other` walked into this object's trigger.
///     }
/// }
/// ```
///
/// [`Ctx::send`]: reign::behaviour::Ctx::send
/// [`Ctx::broadcast`]: reign::behaviour::Ctx::broadcast
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Message {
    /// Sent by a Behaviour: a name, and a number for whatever it needs.
    Custom { name: String, value: i64 },
    /// This object's collider started touching `other`'s.
    CollisionEnter(ObjectId),
    /// This object's collider stopped touching `other`'s.
    CollisionExit(ObjectId),
    /// `other` started overlapping this object, and one of them is a trigger.
    TriggerEnter(ObjectId),
    /// `other` stopped overlapping this object, and one of them is a trigger.
    TriggerExit(ObjectId),
}

impl Message {
    /// A named message with no number.
    pub fn new(name: impl Into<String>) -> Self {
        Message::Custom { name: name.into(), value: 0 }
    }

    /// A named message carrying a number.
    pub fn with(name: impl Into<String>, value: i64) -> Self {
        Message::Custom { name: name.into(), value }
    }

    /// Whether it's the custom message called `name`.
    pub fn is(&self, name: &str) -> bool {
        matches!(self, Message::Custom { name: n, .. } if n == name)
    }

    /// The number a custom message carries.
    pub fn value(&self) -> Option<i64> {
        match self {
            Message::Custom { value, .. } => Some(*value),
            _ => None,
        }
    }

    /// What started touching this object, if that's what this is.
    pub fn collision_enter(&self) -> Option<ObjectId> {
        match self {
            Message::CollisionEnter(other) => Some(*other),
            _ => None,
        }
    }

    pub fn collision_exit(&self) -> Option<ObjectId> {
        match self {
            Message::CollisionExit(other) => Some(*other),
            _ => None,
        }
    }

    pub fn trigger_enter(&self) -> Option<ObjectId> {
        match self {
            Message::TriggerEnter(other) => Some(*other),
            _ => None,
        }
    }

    pub fn trigger_exit(&self) -> Option<ObjectId> {
        match self {
            Message::TriggerExit(other) => Some(*other),
            _ => None,
        }
    }
}

/// Everything the simulation is: the scene, player input and the random
/// number stream. Cloned for Play/Stop and compared for determinism.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct World {
    pub scene: Scene,
    input: InputBuffer,
    rng: Rng,
    tick: u64,
    behaviours: BehaviourState,
    /// Who touched whom at the end of the last tick, for enter and exit
    /// events.
    contacts: Vec<physics::Contact>,
}

impl World {
    pub fn new(scene: Scene) -> Self {
        World {
            scene,
            input: InputBuffer::default(),
            rng: Rng::new(0x52_45_49_47_4e), // "REIGN"
            tick: 0,
            behaviours: BehaviourState::default(),
            contacts: Vec::new(),
        }
    }

    /// One fixed tick: input, then the schedule.
    pub fn step(
        &mut self,
        input: InputFrame,
        behaviours: &Behaviours<World>,
        schedule: &Schedule<World>,
    ) {
        self.input.push(input);
        let assets = crate::DB.get();
        schedule.run(behaviours, self, assets);
        self.tick += 1;
    }
}

impl Game for World {
    type Message = Message;
    type Event = ();

    fn scene(&self) -> &Scene {
        &self.scene
    }

    fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    fn input(&mut self, player: usize) -> Option<&mut InputBuffer> {
        (player == 0).then_some(&mut self.input)
    }

    fn rng(&mut self) -> &mut Rng {
        &mut self.rng
    }

    fn tick(&self) -> u64 {
        self.tick
    }

    fn behaviours(&mut self) -> &mut BehaviourState {
        &mut self.behaviours
    }
}

/// What a Behaviour gets each callback.
pub type Ctx<'a> = reign::behaviour::Ctx<'a, World>;

/// The tick: Behaviours' `start` and `fixed_update`, physics, messages,
/// `late_update`, then spawns and despawns.
pub fn schedule() -> Schedule<World> {
    Schedule::new()
        .then(Stage::Start)
        .then(Stage::FixedUpdate)
        .then(Stage::Systems("physics"))
        .then(Stage::Messages)
        .then(Stage::LateUpdate)
        .then(Stage::Messages)
        .then(Stage::Commands)
        .system(System::new("physics", "physics", 0, physics_system))
}

/// Moves the bodies, then tells both objects in each contact that began or
/// ended: [`Message::CollisionEnter`], [`Message::TriggerExit`] and so on,
/// delivered in the Messages stage right after.
fn physics_system(ctx: &mut Ctx) -> Flow {
    let now = physics::step(ctx.game.scene_mut(), &components::sprite_size);
    let before = std::mem::replace(&mut ctx.game.contacts, now.clone());
    // Both lists are sorted, so ends and starts come out in a fixed order.
    for c in before.iter().filter(|c| now.binary_search(c).is_err()) {
        let exit = if c.trigger { Message::TriggerExit } else { Message::CollisionExit };
        notify(ctx, c, exit);
    }
    for c in now.iter().filter(|c| before.binary_search(c).is_err()) {
        let enter = if c.trigger { Message::TriggerEnter } else { Message::CollisionEnter };
        notify(ctx, c, enter);
    }
    Flow::Continue
}

fn notify(ctx: &mut Ctx, contact: &physics::Contact, event: fn(ObjectId) -> Message) {
    for (me, other) in [(contact.a, contact.b), (contact.b, contact.a)] {
        if ctx.game.scene.contains(me) {
            ctx.send(me, event(other));
        }
    }
}

/// Shortcuts for Behaviours.
pub trait CtxExt {
    /// A component on an object.
    fn get<T: Component>(&self, id: ObjectId) -> Option<&T>;
    fn get_mut<T: Component>(&mut self, id: ObjectId) -> Option<&mut T>;
    /// Where an object is in the world, in pixels.
    fn position(&self, id: ObjectId) -> FxVec2;
    fn set_position(&mut self, id: ObjectId, at: FxVec2);
    /// Moves an object by `delta` pixels.
    fn translate(&mut self, id: ObjectId, delta: FxVec2);
    /// What this object is touching right now: solid contacts and trigger
    /// overlaps, by object id.
    fn touching(&self, id: ObjectId) -> Vec<ObjectId>;
    /// Whether a button is held this tick.
    fn held(&self, button: Button) -> bool;
    /// Whether a button went down this tick.
    fn pressed(&self, button: Button) -> bool;
    /// Whether a button was pressed within the last `ticks` ticks and not
    /// yet [`consume`](CtxExt::consume)d: forgiving input such as jump
    /// buffering.
    fn buffered(&self, button: Button, ticks: u16) -> bool;
    /// Uses up a buffered press, so one press does one thing.
    fn consume(&mut self, button: Button);
    /// Whether a named input action (from `assets/input.ron`) is held.
    fn action(&self, name: &str) -> bool;
    /// Whether a named action went down this tick.
    fn action_pressed(&self, name: &str) -> bool;
    /// Whether a named action was pressed within the last `ticks` ticks and
    /// not yet consumed.
    fn action_buffered(&self, name: &str, ticks: u16) -> bool;
    /// Uses up a buffered press of a named action.
    fn consume_action(&mut self, name: &str);
    /// -1 (left), 0 or 1 (right).
    fn axis_x(&self) -> i32;
    /// -1 (up), 0 or 1 (down): world y grows downwards.
    fn axis_y(&self) -> i32;
}

impl CtxExt for Ctx<'_> {
    fn get<T: Component>(&self, id: ObjectId) -> Option<&T> {
        self.game.scene.get::<T>(id)
    }

    fn get_mut<T: Component>(&mut self, id: ObjectId) -> Option<&mut T> {
        self.game.scene.get_mut::<T>(id)
    }

    fn position(&self, id: ObjectId) -> FxVec2 {
        self.game.scene.world_position(id)
    }

    fn set_position(&mut self, id: ObjectId, at: FxVec2) {
        self.game.scene.set_world_position(id, at);
    }

    fn translate(&mut self, id: ObjectId, delta: FxVec2) {
        let at = self.position(id) + delta;
        self.set_position(id, at);
    }

    fn touching(&self, id: ObjectId) -> Vec<ObjectId> {
        self.game.contacts.iter().filter_map(|c| c.other(id)).collect()
    }

    fn held(&self, button: Button) -> bool {
        self.game.input.frame().held(button)
    }

    fn pressed(&self, button: Button) -> bool {
        self.game.input.just_pressed(button)
    }

    fn buffered(&self, button: Button, ticks: u16) -> bool {
        self.game.input.buffered(button, ticks)
    }

    fn consume(&mut self, button: Button) {
        self.game.input.consume(button);
    }

    fn action(&self, name: &str) -> bool {
        crate::input::button(name).is_some_and(|b| self.held(b))
    }

    fn action_pressed(&self, name: &str) -> bool {
        crate::input::button(name).is_some_and(|b| self.pressed(b))
    }

    fn action_buffered(&self, name: &str, ticks: u16) -> bool {
        crate::input::button(name).is_some_and(|b| self.buffered(b, ticks))
    }

    fn consume_action(&mut self, name: &str) {
        if let Some(b) = crate::input::button(name) {
            self.consume(b);
        }
    }

    fn axis_x(&self) -> i32 {
        self.game.input.frame().axis_x()
    }

    fn axis_y(&self) -> i32 {
        let f = self.game.input.frame();
        f.held(Button::Down) as i32 - f.held(Button::Up) as i32
    }
}

/// Keys held on the keyboard, turned into each tick's buttons.
#[derive(Default)]
pub struct Keyboard {
    held: Vec<KeyCode>,
}

impl Keyboard {
    pub fn event(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                match event.state {
                    ElementState::Pressed if !self.held.contains(&code) => self.held.push(code),
                    ElementState::Released => self.held.retain(|k| *k != code),
                    _ => {}
                }
            }
            WindowEvent::Focused(false) => self.held.clear(),
            _ => {}
        }
    }

    pub fn clear(&mut self) {
        self.held.clear();
    }

    #[cfg(test)]
    pub fn press_for_tests(&mut self, keys: &[KeyCode]) {
        self.held.extend_from_slice(keys);
    }

    /// The actions the held keys and the pads trigger this tick, by the
    /// project's `input.ron`.
    pub fn frame(&self, pads: Option<&reign::app::Pads>) -> InputFrame {
        crate::input::frame(&self.held, pads)
    }
}

/// [`CtxExt::touching`] without a running tick.
#[cfg(test)]
pub fn touching_for_tests(world: &World, id: ObjectId) -> Vec<ObjectId> {
    world.contacts.iter().filter_map(|c| c.other(id)).collect()
}
