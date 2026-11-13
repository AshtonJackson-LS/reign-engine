//! Tests for animation, text, sound and changing scenes, played through a
//! real project folder as a game would.

use reign::assets::{AnimationClip, AssetDb, ClipFrame, Handle, LoopMode, to_ron};
use reign::behaviour::{Behaviours, Schedule};
use reign::core::InputFrame;
use reign::scene::ComponentRegistry;

use crate::components::register_builtins;
use crate::editor::Project;
use crate::prelude::*;
use crate::runtime::{self, Event, register_builtin_behaviours, schedule};
use crate::view::current_frame;
use crate::{font, starter};

/// Writes down every message it gets, and does what its fields ask.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
struct Probe {
    heard: Vec<String>,
    /// Played with `ctx.play_sound` on the first tick.
    sound: String,
    /// Switched to with `ctx.play_animation` on tick `at`.
    animation: String,
    /// Entered with `ctx.load_scene` on tick `at`.
    scene: String,
    at: u64,
}

impl Behaviour<World> for Probe {
    fn fixed_update(ctx: &mut Ctx, me: ObjectId) {
        let Some(probe) = ctx.get::<Probe>(me).cloned() else { return };
        if ctx.tick() == 0 && !probe.sound.is_empty() {
            ctx.play_sound(&probe.sound);
        }
        if ctx.tick() == probe.at {
            if !probe.animation.is_empty() {
                ctx.play_animation(me, &probe.animation);
            }
            if !probe.scene.is_empty() {
                ctx.load_scene(&probe.scene);
            }
        }
        let tick = ctx.tick();
        ctx.set_text(me, format!("tick {tick}"));
    }

    fn on_message(ctx: &mut Ctx, me: ObjectId, msg: &Envelope<Message>) {
        if let Message::Custom { name, value } = &msg.body
            && let Some(probe) = ctx.get_mut::<Probe>(me)
        {
            probe.heard.push(format!("{name}:{value}"));
        }
    }
}

/// A project on disk with its assets open, and what plays it.
struct Game {
    project: Project,
    db: AssetDb,
    registry: ComponentRegistry,
    behaviours: Behaviours<World>,
    schedule: Schedule<World>,
}

impl Game {
    /// A new project with `files` (path in `assets/`, contents) added.
    fn new(name: &str, files: &[(&str, Vec<u8>)]) -> Game {
        let dir = std::env::temp_dir().join(format!("reign-feature-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = Project::open(dir).expect("project opens");
        for (path, bytes) in files {
            let file = project.assets().join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        let db = AssetDb::loose(project.assets()).expect("assets load");
        let mut registry = ComponentRegistry::new();
        register_builtins(&mut registry);
        registry.register::<Probe>();
        let mut behaviours = Behaviours::new();
        register_builtin_behaviours(&mut behaviours);
        behaviours.behaviour::<Probe>();
        Game { project, db, registry, behaviours, schedule: schedule() }
    }

    fn run(&self, world: &mut World, ticks: u32) -> Vec<Event> {
        let mut events = Vec::new();
        for _ in 0..ticks {
            events.extend(runtime::tick(
                world,
                InputFrame::default(),
                &self.behaviours,
                &self.schedule,
                &self.registry,
                Some(&self.db),
            ));
        }
        events
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.project.root);
    }
}

/// A clip over the built-in shapes (no grid, so a frame's cell is the
/// sheet's nth sprite): each frame lasts 100 ms, which is 12 ticks.
fn clip(cells: &[u32], mode: LoopMode, event_on: Option<(usize, &str)>) -> Vec<u8> {
    let frames = cells
        .iter()
        .enumerate()
        .map(|(i, &cell)| ClipFrame {
            cell,
            ms: 100,
            event: event_on.filter(|(at, _)| *at == i).map(|(_, name)| name.to_owned()),
            ..ClipFrame::default()
        })
        .collect();
    let clip = AnimationClip { sheet: starter::shapes_sheet(), mode, frames, ..Default::default() };
    ron::ser::to_string(&clip).unwrap().into_bytes()
}

/// A short, silent 16-bit mono WAV.
fn wav() -> Vec<u8> {
    let samples = [0u8; 64];
    let mut out = Vec::new();
    out.extend(b"RIFF");
    out.extend((36 + samples.len() as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(1u16.to_le_bytes()); // PCM
    out.extend(1u16.to_le_bytes()); // mono
    out.extend(22050u32.to_le_bytes());
    out.extend(44100u32.to_le_bytes()); // bytes a second
    out.extend(2u16.to_le_bytes()); // bytes a sample
    out.extend(16u16.to_le_bytes());
    out.extend(b"data");
    out.extend((samples.len() as u32).to_le_bytes());
    out.extend(samples);
    out
}

#[test]
fn projects_get_a_font_that_new_text_uses() {
    let game = Game::new("font", &[]);
    assert_eq!(game.db.path(font::FONT).as_deref(), Some("Reign/Font.font.ron"));
    let loaded = game.db.load(font::font()).expect("the font loads");
    assert_eq!(*loaded, font::asset());
    let texture = game.db.load(loaded.texture).expect("its texture loads");
    assert_eq!((texture.image.width, texture.image.height), (96, 48));

    let text = Text { text: "Score: 10\nHi".into(), scale: 2, ..Text::default() };
    assert_eq!(text.font, font::font());
    assert_eq!(text.size(loaded.cell), [9 * 6 * 2, 2 * 8 * 2]);
    assert_eq!(text.align_offset(108), 0);
    let centred = Text { align: TextAlign::Center, ..text.clone() };
    assert_eq!(centred.align_offset(108), 54);
    // The font file is a plain asset: it round-trips through its text.
    assert_eq!(to_ron(&font::asset()), to_ron(&*loaded));
}

#[test]
fn an_animator_steps_through_frames_and_reports_events_and_the_end() {
    let files = [
        ("anims/once.anim.ron", clip(&[0, 1], LoopMode::Once, Some((1, "step")))),
        ("anims/loop.anim.ron", clip(&[2, 3, 4], LoopMode::Loop, None)),
    ];
    let game = Game::new("animator", &files);
    let once: Handle<AnimationClip> = game.db.handle("anims/once.anim.ron").expect("once");
    let looping: Handle<AnimationClip> = game.db.handle("anims/loop.anim.ron").expect("loop");
    let sheet = game.db.load(starter::shapes_sheet()).unwrap();

    let mut scene = Scene::new();
    let id = scene.spawn("Hero");
    scene.insert(id, SpriteRenderer { sheet: starter::shapes_sheet(), ..Default::default() });
    scene.insert(
        id,
        Animator {
            clip: once,
            clips: vec![NamedClip { name: "spin".into(), clip: looping }],
            ..Animator::default()
        },
    );
    scene.insert(id, Probe { animation: "spin".into(), at: 40, ..Probe::default() });
    let mut world = World::new(scene);
    let shown = |world: &World| {
        let sr = world.scene.get::<SpriteRenderer>(id).unwrap();
        let frame = current_frame(&game.db, sr, world.scene.get::<Animator>(id)).expect("a frame");
        sheet.sprites.iter().position(|s| s.rect == frame.rect).expect("one of the shapes")
    };
    let heard = |world: &World| world.scene.get::<Probe>(id).unwrap().heard.clone();

    // Before Play, the first frame.
    assert_eq!(shown(&world), 0);
    game.run(&mut world, 11);
    assert_eq!((shown(&world), heard(&world).len()), (0, 0));
    // The twelfth tick reaches frame 1, which has an event.
    game.run(&mut world, 1);
    assert_eq!(shown(&world), 1);
    assert_eq!(heard(&world), ["step:1"]);
    // 24 ticks in, the clip ends: it holds its last frame and says so once.
    game.run(&mut world, 20);
    assert_eq!(shown(&world), 1);
    assert_eq!(heard(&world), ["step:1", "AnimationFinished:0"]);
    assert_eq!(world.scene.get::<Animator>(id).unwrap().time, 2400);

    // On tick 40 the script switches to "spin", which loops over three shapes.
    game.run(&mut world, 9);
    assert_eq!(world.scene.get::<Animator>(id).unwrap().clip, looping);
    assert_eq!(shown(&world), 2);
    game.run(&mut world, 12);
    assert_eq!(shown(&world), 3);
    game.run(&mut world, 24);
    assert_eq!(shown(&world), 2, "it wrapped around");
    assert_eq!(heard(&world).len(), 2, "a looping clip never finishes");

    // Half speed takes twice as long to change frame.
    let animator = world.scene.get_mut::<Animator>(id).unwrap();
    (animator.time, animator.speed) = (0, 50);
    game.run(&mut world, 23);
    assert_eq!(shown(&world), 2);
    game.run(&mut world, 1);
    assert_eq!(shown(&world), 3);
}

#[test]
fn scripts_and_audio_sources_make_sounds() {
    let files = [("sounds/blip.wav", wav()), ("music/theme.wav", wav())];
    let game = Game::new("sound", &files);
    let blip = game.db.guid("sounds/blip.wav").expect("blip");

    let mut scene = Scene::new();
    let script = scene.spawn("Script");
    scene.insert(script, Probe { sound: "sounds/blip.wav".into(), at: 999, ..Probe::default() });
    let source = scene.spawn("Source");
    scene.insert(source, AudioSource { clip: Handle::new(blip), volume: 60, play_on_start: true });
    let quiet = scene.spawn("Quiet");
    scene.insert(quiet, AudioSource { play_on_start: false, ..AudioSource::default() });
    let mut world = World::new(scene);

    // The source plays as it starts; the script on its first update.
    let events = game.run(&mut world, 1);
    assert_eq!(
        events,
        [Event::Sound { clip: blip, volume: 60 }, Event::Sound { clip: blip, volume: 100 }]
    );
    assert_eq!(game.run(&mut world, 5), [], "once each");

    // The mixer reads the files the events name.
    let clip = reign::audio::Clip::wav(&game.db.bytes(blip).unwrap()).expect("a readable wav");
    assert!(clip.seconds() > 0.0);

    // A sound that isn't there is a warning, not a crash.
    let mut scene = Scene::new();
    let script = scene.spawn("Script");
    scene.insert(script, Probe { sound: "sounds/nope.wav".into(), at: 999, ..Probe::default() });
    assert_eq!(game.run(&mut World::new(scene), 2), []);
}

#[test]
fn a_script_changes_scene_and_changes_text() {
    let level2 = r#"(version: 1, objects: [
        (id: (0, 0), name: "Main Camera", components: {"Camera": ()}),
        (id: (1, 0), name: "Sign", components: {"Text": (text: "Level 2"), "Probe": (at: 999)}),
    ])"#;
    let game = Game::new("scenes", &[("scenes/level2.scene.ron", level2.as_bytes().to_vec())]);

    let mut scene = Scene::new();
    let door = scene.spawn("Door");
    scene.insert(door, Text::default());
    scene
        .insert(door, Probe { scene: "scenes/level2.scene.ron".into(), at: 3, ..Probe::default() });
    let mut world = World::new(scene);
    game.run(&mut world, 3);
    assert_eq!(world.scene.name(door), Some("Door"));
    assert_eq!(world.scene.get::<Text>(door).unwrap().text, "tick 2");

    // The fourth tick asks for level 2; it's there when the tick ends.
    game.run(&mut world, 1);
    let sign = world.scene.walk().into_iter().find(|id| world.scene.name(*id) == Some("Sign"));
    let sign = sign.expect("level 2 loaded");
    assert_eq!(world.scene.len(), 2);
    assert_eq!(world.scene.get::<Text>(sign).unwrap().text, "Level 2");
    // Its Behaviours start and run; the clock carries on.
    game.run(&mut world, 1);
    assert_eq!(world.scene.get::<Text>(sign).unwrap().text, "tick 4");

    // A scene that doesn't exist leaves the game where it is.
    let mut scene = Scene::new();
    let door = scene.spawn("Door");
    scene.insert(door, Probe { scene: "scenes/nowhere.scene.ron".into(), ..Probe::default() });
    let mut world = World::new(scene);
    game.run(&mut world, 2);
    assert_eq!(world.scene.name(door), Some("Door"));
}
