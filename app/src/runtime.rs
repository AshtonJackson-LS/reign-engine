//! What runs in Play: the [`World`] (scene, input, random numbers), each
//! project's Behaviours on the fixed schedule, physics, and the keyboard.

use reign::app::winit::event::{ElementState, WindowEvent};
use reign::app::winit::keyboard::{KeyCode, PhysicalKey};
use reign::assets::{AssetDb, AssetKind, Guid};
use reign::behaviour::{
    Behaviour, BehaviourState, Behaviours, Flow, Game, Schedule, Stage, System,
};
use reign::core::{Button, FxVec2, InputBuffer, InputFrame, Rng};
use reign::scene::{Component, ComponentRegistry, ObjectId, Scene};

use crate::components::{Animator, AudioSource, Text};
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

/// What a tick asks the outside world to do: sounds and music. Never read
/// back by the simulation, so replaying a tick only replays the noise.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Play a sound once. `volume` is a percentage.
    Sound {
        clip: Guid,
        volume: u8,
    },
    /// Switch the music to this, looping.
    Music {
        clip: Guid,
        volume: u8,
    },
    StopMusic,
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
    /// The scene a script asked for with `ctx.load_scene`, entered when
    /// the tick ends.
    next_scene: Option<String>,
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
            next_scene: None,
        }
    }

    /// Replaces the scene, as `ctx.load_scene` does once its scene is
    /// loaded: Behaviours start again, nothing is touching yet. Input, the
    /// tick count and the random numbers carry on.
    pub fn enter(&mut self, scene: Scene) {
        self.scene = scene;
        self.behaviours = BehaviourState::default();
        self.contacts.clear();
    }

    /// The scene a script asked for this tick, if any; asking clears it.
    pub fn take_scene_request(&mut self) -> Option<String> {
        self.next_scene.take()
    }

    /// What `id` is touching right now: solid contacts and trigger overlaps.
    pub fn touching(&self, id: ObjectId) -> Vec<ObjectId> {
        self.contacts.iter().filter_map(|c| c.other(id)).collect()
    }

    /// One fixed tick: input, then the schedule. Returns the sounds it
    /// made. A scene change a script asked for waits in
    /// [`World::take_scene_request`]; [`tick`] does both.
    pub fn step(
        &mut self,
        input: InputFrame,
        behaviours: &Behaviours<World>,
        schedule: &Schedule<World>,
    ) -> Vec<Event> {
        self.step_with(input, behaviours, schedule, crate::DB.get())
    }

    /// [`World::step`] with the assets to use, instead of the open
    /// project's.
    pub fn step_with(
        &mut self,
        input: InputFrame,
        behaviours: &Behaviours<World>,
        schedule: &Schedule<World>,
        assets: Option<&AssetDb>,
    ) -> Vec<Event> {
        self.input.push(input);
        let events = schedule.run(behaviours, self, assets);
        crate::components::update_cameras(&mut self.scene);
        self.tick += 1;
        events
    }
}

impl Game for World {
    type Message = Message;
    type Event = Event;

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
/// `late_update`, animation, then spawns and despawns.
pub fn schedule() -> Schedule<World> {
    Schedule::new()
        .then(Stage::Start)
        .then(Stage::FixedUpdate)
        .then(Stage::Systems("physics"))
        .then(Stage::Messages)
        .then(Stage::LateUpdate)
        .then(Stage::Systems("animation"))
        .then(Stage::Messages)
        .then(Stage::Commands)
        .system(System::new("physics", "physics", 0, physics_system))
        .system(System::new("animation", "animation", 0, animation_system))
}

/// One tick of a game as every host plays it (the editor's Play, the
/// exported game, `--simulate`): [`World::step`], then the scene a script
/// asked for with `ctx.load_scene`, loaded from `assets`. Returns the
/// tick's sounds.
pub fn tick(
    world: &mut World,
    input: InputFrame,
    behaviours: &Behaviours<World>,
    schedule: &Schedule<World>,
    registry: &ComponentRegistry,
    assets: Option<&AssetDb>,
) -> Vec<Event> {
    let events = world.step_with(input, behaviours, schedule, assets);
    if let Some(path) = world.take_scene_request() {
        let loaded = match assets {
            Some(db) => crate::player::load_scene(db, registry, &path),
            None => Err("the project's assets aren't loaded".to_owned()),
        };
        match loaded {
            Ok(scene) => {
                log::info!("entering {path}");
                world.enter(scene);
            }
            Err(e) => log::error!("load_scene({path:?}): {e}"),
        }
    }
    events
}

/// Says something once, however many ticks ask.
fn warn_once(message: String) {
    static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut said = SAID.lock().expect("warnings");
    if !said.contains(&message) {
        log::warn!("{message}");
        said.push(message);
    }
}

/// Advances every playing [`Animator`], and tells its object about the
/// frame events it reaches and the end of a clip that doesn't loop.
fn animation_system(ctx: &mut Ctx) -> Flow {
    let playing: Vec<(ObjectId, Animator)> = ctx
        .game
        .scene
        .iter::<Animator>()
        .filter(|(_, a)| a.playing && a.speed > 0 && !a.clip.is_none())
        .map(|(id, a)| (id, a.clone()))
        .collect();
    for (id, animator) in playing {
        let before = animator.time;
        let mut after = before.saturating_add(u32::from(animator.speed));
        let clip = ctx.assets().and_then(|db| db.load(animator.clip).ok());
        if let Some(clip) = clip.filter(|c| !c.frames.is_empty()) {
            let frame = |time: u32| clip.frame_at_tick(time / 100);
            let event = |f: u32| clip.frames.get(f as usize).and_then(|fr| fr.event.clone());
            if before == 0
                && let Some(name) = event(frame(0))
            {
                ctx.send(id, Message::with(name, 0));
            }
            let end = u32::from(clip.ticks());
            if !clip.looping() {
                // Holds the last frame; the clock stops there too.
                after = after.min(end * 100);
            }
            if frame(after) != frame(before)
                && let Some(name) = event(frame(after))
            {
                ctx.send(id, Message::with(name, i64::from(frame(after))));
            }
            if !clip.looping() && before / 100 < end && after / 100 >= end {
                ctx.send(id, Message::new("AnimationFinished"));
            }
        }
        if let Some(a) = ctx.game.scene.get_mut::<Animator>(id) {
            a.time = after;
        }
    }
    Flow::Continue
}

/// An [`AudioSource`] with `play_on_start` plays when its object appears.
impl Behaviour<World> for AudioSource {
    fn start(ctx: &mut Ctx, me: ObjectId) {
        if ctx.get::<AudioSource>(me).is_some_and(|s| s.play_on_start) {
            ctx.play(me);
        }
    }
}

/// The built-in Behaviours, before a project's own.
pub fn register_builtin_behaviours(behaviours: &mut Behaviours<World>) {
    behaviours.behaviour::<AudioSource>();
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
    /// Switches an object's [`Animator`] to the clip it lists under `name`,
    /// from the start. Asking for the clip that's already playing leaves it
    /// running, so it's safe to call every tick. False if the object has no
    /// Animator or no clip by that name.
    fn play_animation(&mut self, id: ObjectId, name: &str) -> bool;
    /// Whether an object's animation has played to its end. Always false
    /// for a looping clip.
    fn animation_finished(&self, id: ObjectId) -> bool;
    /// Changes what an object's [`Text`] says.
    fn set_text(&mut self, id: ObjectId, text: impl Into<String>);
    /// Plays a sound once, by its path in `assets/`:
    /// `ctx.play_sound("sounds/jump.wav")`.
    fn play_sound(&mut self, path: &str);
    /// Plays an object's [`AudioSource`].
    fn play(&mut self, id: ObjectId);
    /// Switches the music to a `.wav` by its path in `assets/`, looping.
    fn play_music(&mut self, path: &str);
    fn stop_music(&mut self);
    /// Leaves this scene for another when the tick ends, by its path in
    /// `assets/`: `ctx.load_scene("scenes/level2.scene.ron")`.
    fn load_scene(&mut self, path: &str);
}

/// The sound file at `path`, if the project has one.
fn sound(ctx: &Ctx, path: &str) -> Option<Guid> {
    let db = ctx.assets()?;
    let found = db
        .guid(path)
        .filter(|g| matches!(db.kind(*g), Some(AssetKind::AudioClip | AssetKind::Music)));
    if found.is_none() {
        warn_once(format!("no sound at {path:?}: paths start in assets/, such as sounds/jump.wav"));
    }
    found
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
        self.game.touching(id)
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

    fn play_animation(&mut self, id: ObjectId, name: &str) -> bool {
        let Some(animator) = self.game.scene.get::<Animator>(id) else { return false };
        let Some(clip) = animator.named(name) else {
            warn_once(format!("play_animation: no clip called {name:?} in the Animator's clips"));
            return false;
        };
        // Only touch the component when something changes, so asking every
        // tick doesn't copy the scene's animators.
        if (animator.clip != clip || !animator.playing)
            && let Some(animator) = self.game.scene.get_mut::<Animator>(id)
        {
            animator.play(clip);
        }
        true
    }

    fn animation_finished(&self, id: ObjectId) -> bool {
        let Some(animator) = self.game.scene.get::<Animator>(id) else { return false };
        self.assets()
            .and_then(|db| db.load(animator.clip).ok())
            .is_some_and(|clip| animator.finished(&clip))
    }

    fn set_text(&mut self, id: ObjectId, text: impl Into<String>) {
        let text = text.into();
        if self.game.scene.get::<Text>(id).is_some_and(|t| t.text != text)
            && let Some(t) = self.game.scene.get_mut::<Text>(id)
        {
            t.text = text;
        }
    }

    fn play_sound(&mut self, path: &str) {
        if let Some(clip) = sound(self, path) {
            self.emit(Event::Sound { clip, volume: 100 });
        }
    }

    fn play(&mut self, id: ObjectId) {
        if let Some(source) = self.game.scene.get::<AudioSource>(id)
            && !source.clip.is_none()
        {
            let event = Event::Sound { clip: source.clip.guid, volume: source.volume };
            self.emit(event);
        }
    }

    fn play_music(&mut self, path: &str) {
        if let Some(clip) = sound(self, path) {
            self.emit(Event::Music { clip, volume: 100 });
        }
    }

    fn stop_music(&mut self) {
        self.emit(Event::StopMusic);
    }

    fn load_scene(&mut self, path: &str) {
        self.game.next_scene = Some(path.to_owned());
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
    world.touching(id)
}
