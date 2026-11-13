//! The project without a window, for tools and AI agents: each command
//! prints one JSON document on stdout (logs go to stderr) and sets the exit
//! code.
//!
//! ```text
//! <editor> PROJECT_DIR --check
//! <editor> PROJECT_DIR --describe
//! <editor> PROJECT_DIR --simulate TICKS [--scene PATH] [--input SPEC]
//!                      [--every TICKS] [--watch NAMES]
//! ```
//!
//! - `--check` loads every scene and prefab as the game would and lists what's
//!   wrong. Exit code 1 if there are errors.
//! - `--describe` lists what the project is made of: components with their
//!   fields, defaults and ranges, Behaviours, input actions, scenes, assets.
//! - `--simulate` plays a scene for a number of ticks (120 a second) with
//!   scripted input, and prints where every object ended up.
//!
//! Run them with the project's own editor (`cargo run --bin <crate>-editor
//! -- . --check`) so its components are known; `reign-engine` only knows the
//! built-in ones.

use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use reign::assets::{AssetDb, AssetKind, Handle, PrefabInstance, Prefabs, SceneAsset, prefab};
use reign::behaviour::Game;
use reign::core::{Button, InputFrame};
use reign::scene::{ComponentRegistry, Mut, ObjectId, Reflect, Scene};

use crate::input::{INPUT_FILE, InputMap};
use crate::player;
use crate::runtime::{self, Event, Scripts, TICKS_PER_SECOND, World, schedule};

/// A JSON value, written by hand: the engine has no JSON dependency.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i128),
    /// A number already in its decimal form (fixed-point values).
    Number(String),
    Str(String),
    List(Vec<Json>),
    Map(Vec<(&'static str, Json)>),
}

impl Json {
    fn str(s: impl Into<String>) -> Json {
        Json::Str(s.into())
    }

    /// The document, indented two spaces a level. Lists of plain values stay
    /// on one line.
    pub fn pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, 0);
        out
    }

    fn is_plain(&self) -> bool {
        !matches!(self, Json::List(_) | Json::Map(_))
    }

    fn write(&self, out: &mut String, depth: usize) {
        let pad = |out: &mut String, d: usize| out.push_str(&"  ".repeat(d));
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            Json::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Json::Number(n) => out.push_str(n),
            Json::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            let _ = write!(out, "\\u{:04x}", c as u32);
                        }
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            Json::List(items) if items.iter().all(Json::is_plain) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.write(out, depth);
                }
                out.push(']');
            }
            Json::List(items) => {
                out.push_str("[\n");
                for (i, item) in items.iter().enumerate() {
                    pad(out, depth + 1);
                    item.write(out, depth + 1);
                    out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                pad(out, depth);
                out.push(']');
            }
            Json::Map(fields) if fields.is_empty() => out.push_str("{}"),
            Json::Map(fields) => {
                out.push_str("{\n");
                for (i, (key, value)) in fields.iter().enumerate() {
                    pad(out, depth + 1);
                    let _ = write!(out, "\"{key}\": ");
                    value.write(out, depth + 1);
                    out.push_str(if i + 1 < fields.len() { ",\n" } else { "\n" });
                }
                pad(out, depth);
                out.push('}');
            }
        }
    }
}

/// What kind of value a reflected field is, as `--describe` names it.
fn kind(value: &mut dyn Reflect) -> &'static str {
    match value.reflect_mut() {
        Mut::Bool(_) => "bool",
        Mut::Int(_) => "int",
        Mut::Fx(_) => "fx",
        Mut::FxVec2(_) => "fxvec2",
        Mut::Color(_) => "color",
        Mut::String(_) => "string",
        Mut::Object(_) => "object",
        Mut::Asset(_) => "asset",
        Mut::Enum(_) => "enum",
        Mut::Option(_) => "option",
        Mut::List(_) => "list",
        Mut::Struct(_) => "struct",
    }
}

/// A reflected value as JSON. Fixed-point numbers are decimal strings, as in
/// scene files; an object id is `[index, generation]`; an asset is its GUID.
pub fn value(v: &mut dyn Reflect) -> Json {
    match v.reflect_mut() {
        Mut::Bool(b) => Json::Bool(*b),
        Mut::Int(i) => Json::Int(i.get()),
        Mut::Fx(f) => Json::str(f.to_string()),
        Mut::FxVec2(p) => {
            Json::Map(vec![("x", Json::str(p.x.to_string())), ("y", Json::str(p.y.to_string()))])
        }
        Mut::Color(c) => Json::Map(vec![
            ("r", Json::Int(c.r.into())),
            ("g", Json::Int(c.g.into())),
            ("b", Json::Int(c.b.into())),
            ("a", Json::Int(c.a.into())),
        ]),
        Mut::String(s) => Json::str(s.as_str()),
        Mut::Object(id) => id_json(*id),
        Mut::Asset(a) => match a.guid() {
            0 => Json::Null,
            guid => Json::str(format!("{guid:032x}")),
        },
        Mut::Enum(e) => Json::str(e.variants().get(e.variant_index()).copied().unwrap_or("")),
        Mut::Option(o) => o.get_mut().map_or(Json::Null, value),
        Mut::List(list) => {
            Json::List((0..list.len()).filter_map(|i| list.get_mut(i).map(value)).collect())
        }
        Mut::Struct(s) => {
            let fields = s.fields();
            Json::Map(
                (0..fields.len())
                    .filter_map(|i| Some((fields[i].name, value(s.field_mut(i)?))))
                    .collect(),
            )
        }
    }
}

fn id_json(id: ObjectId) -> Json {
    Json::List(vec![Json::Int(id.index.into()), Json::Int(id.generation.into())])
}

/// One component type for `--describe`: its fields with their kinds,
/// defaults and editor hints.
fn describe_component(registry: &ComponentRegistry, name: &'static str) -> Json {
    let mut scene = Scene::new();
    let id = scene.spawn("probe");
    registry.insert_default(&mut scene, id, name);
    let mut fields = Vec::new();
    if let Some(component) = scene.component_mut(id, name)
        && let Mut::Struct(s) = component.reflect_mut()
    {
        let infos = s.fields();
        for (i, info) in infos.iter().enumerate() {
            let Some(field) = s.field_mut(i) else { continue };
            let mut entry = vec![
                ("name", Json::str(info.name)),
                ("type", Json::str(info.type_name)),
                ("kind", Json::str(kind(field))),
            ];
            match field.reflect_mut() {
                Mut::Enum(e) => {
                    let variants = e.variants().iter().map(|v| Json::str(*v)).collect();
                    entry.push(("variants", Json::List(variants)));
                }
                Mut::Asset(a) => entry.push(("asset_kind", Json::str(a.asset_kind()))),
                _ => {}
            }
            entry.push(("default", value(field)));
            if info.read_only {
                entry.push(("read_only", Json::Bool(true)));
            }
            if let Some((min, max)) = info.range {
                let number = |v: f64| Json::Number(format!("{v}"));
                entry.push(("range", Json::List(vec![number(min), number(max)])));
            }
            if let Some(tip) = info.tooltip {
                entry.push(("doc", Json::str(tip.trim())));
            }
            fields.push(Json::Map(entry));
        }
    }
    Json::Map(vec![("name", Json::str(name)), ("fields", Json::List(fields))])
}

/// `--describe`: what the project is made of.
pub fn describe(db: &AssetDb, scripts: &Scripts, title: &str) -> Json {
    let registry = player::registry(scripts);
    let behaviours = player::behaviours(scripts);
    let components = registry.names().map(|n| describe_component(&registry, n)).collect();
    let input = input_map(db).unwrap_or_default();
    let actions = input
        .actions
        .iter()
        .map(|a| {
            let names = |v: &[String]| Json::List(v.iter().map(Json::str).collect());
            Json::Map(vec![
                ("name", Json::str(a.name.as_str())),
                ("keys", names(&a.keys)),
                ("pad", names(&a.pad)),
            ])
        })
        .collect();
    let assets = db
        .assets()
        .into_iter()
        .map(|a| {
            Json::Map(vec![
                ("path", Json::str(a.path)),
                ("kind", Json::str(a.kind.name())),
                ("guid", Json::str(a.guid.to_string())),
            ])
        })
        .collect();
    Json::Map(vec![
        ("engine_version", Json::str(env!("CARGO_PKG_VERSION"))),
        ("project", Json::str(title)),
        ("crate", scripts.project.map_or(Json::Null, Json::str)),
        ("ticks_per_second", Json::Int(TICKS_PER_SECOND.into())),
        ("components", Json::List(components)),
        ("behaviours", Json::List(behaviours.names().into_iter().map(Json::str).collect())),
        ("input_actions", Json::List(actions)),
        ("scenes", Json::List(player::scene_paths(db).into_iter().map(Json::str).collect())),
        ("assets", Json::List(assets)),
    ])
}

/// The project's `input.ron`: `None` if it has none, an error if it's wrong.
fn input_map(db: &AssetDb) -> Result<InputMap, String> {
    match db.bytes_at(INPUT_FILE) {
        Ok(bytes) => InputMap::parse(&String::from_utf8_lossy(&bytes)),
        Err(_) => Ok(InputMap::default()),
    }
}

/// Something `--check` found, and the asset it's in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub file: String,
    pub message: String,
}

impl Problem {
    fn new(file: impl Into<String>, message: impl Into<String>) -> Self {
        Problem { file: file.into(), message: message.into() }
    }

    fn json(&self) -> Json {
        Json::Map(vec![
            ("file", Json::str(self.file.as_str())),
            ("message", Json::str(self.message.as_str())),
        ])
    }
}

/// What `--check` found: errors stop the game from loading something;
/// warnings are probably mistakes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub errors: Vec<Problem>,
    pub warnings: Vec<Problem>,
}

impl Report {
    pub fn json(&self) -> Json {
        Json::Map(vec![
            ("ok", Json::Bool(self.errors.is_empty())),
            ("errors", Json::List(self.errors.iter().map(Problem::json).collect())),
            ("warnings", Json::List(self.warnings.iter().map(Problem::json).collect())),
        ])
    }
}

/// `--check`: loads every scene and prefab as the game would, and looks for
/// references to assets that don't exist.
pub fn check(db: &AssetDb, scripts: &Scripts) -> Report {
    let mut report = Report::default();
    let registry = player::registry(scripts);
    let prefabs = Prefabs::new(registry.clone());

    match input_map(db) {
        Ok(map) => {
            for name in map.unknown_bindings() {
                let message = format!("unknown key or gamepad button {name:?}");
                report.warnings.push(Problem::new(INPUT_FILE, message));
            }
        }
        Err(e) => report.errors.push(Problem::new(INPUT_FILE, e)),
    }

    let mut scenes = 0;
    for asset in db.assets() {
        match asset.kind {
            AssetKind::Scene => {
                scenes += 1;
                let text = match db.load_path::<SceneAsset>(&asset.path) {
                    Ok(a) => a.text.clone(),
                    Err(e) => {
                        report.errors.push(Problem::new(&asset.path, e.to_string()));
                        continue;
                    }
                };
                let mut scene = match Scene::from_ron(&text, prefabs.registry()) {
                    Ok(s) => s,
                    Err(e) => {
                        report.errors.push(Problem::new(&asset.path, e.to_string()));
                        continue;
                    }
                };
                for e in prefab::expand_all(&mut scene, &prefabs.with(db)) {
                    report.errors.push(Problem::new(&asset.path, e));
                }
                if scene.count::<crate::Camera>() == 0 {
                    let message = "no Camera: the view stays at the world origin";
                    report.warnings.push(Problem::new(&asset.path, message));
                }
            }
            AssetKind::Prefab => {
                // A scene with one instance of it loads it the same way the
                // game does.
                let mut scene = Scene::new();
                let id = scene.spawn("probe");
                scene.insert(id, PrefabInstance::new(Handle::new(asset.guid)));
                for e in prefab::expand_all(&mut scene, &prefabs.with(db)) {
                    report.errors.push(Problem::new(&asset.path, e));
                }
            }
            _ => {}
        }
    }
    if scenes == 0 {
        report.errors.push(Problem::new("scenes/", "the project has no scenes"));
    }

    for (path, guid) in db.missing_references() {
        let message = format!("refers to asset {guid}, which doesn't exist");
        report.errors.push(Problem::new(path, message));
    }
    report
}

/// An action held from tick `from` to tick `to`, both included.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hold {
    pub button: Button,
    pub from: u64,
    pub to: u64,
}

/// Reads `--input`: actions separated by commas, each `Name` (held the
/// whole time), `Name@TICK` (pressed on that tick) or `Name@FROM-TO`.
/// `Right,Jump@30-45` runs right and holds Jump for ticks 30 to 45.
pub fn parse_input(spec: &str, map: &InputMap) -> Result<Vec<Hold>, String> {
    let mut holds = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, when) = match part.split_once('@') {
            Some((n, w)) => (n.trim(), Some(w.trim())),
            None => (part, None),
        };
        let Some(index) = map.index(name) else {
            let known: Vec<&str> = map.actions.iter().map(|a| a.name.as_str()).collect();
            return Err(format!("no input action {name:?}: there's {}", known.join(", ")));
        };
        let tick = |s: &str| {
            s.trim().parse::<u64>().map_err(|_| format!("{part:?}: {s:?} isn't a tick number"))
        };
        let (from, to) = match when {
            None => (0, u64::MAX),
            Some(w) => match w.split_once('-') {
                Some((a, b)) => (tick(a)?, tick(b)?),
                None => (tick(w)?, tick(w)?),
            },
        };
        if to < from {
            return Err(format!("{part:?}: ends before it starts"));
        }
        holds.push(Hold { button: Button::ALL[index], from, to });
    }
    Ok(holds)
}

/// What `--simulate` plays.
#[derive(Clone, Debug, Default)]
pub struct Simulation {
    /// The scene's asset path; `None` picks the start scene.
    pub scene: Option<String>,
    pub ticks: u64,
    pub input: Vec<Hold>,
    /// Also record the objects every this many ticks.
    pub every: Option<u64>,
    /// Only these objects, by name. Empty records all of them.
    pub watch: Vec<String>,
}

/// Every object (or just the watched ones) as JSON: where it is in the
/// world, and its components' values.
fn snapshot(world: &mut World, watch: &[String]) -> Json {
    let mut objects = Vec::new();
    for id in world.scene.walk() {
        let Some(object) = world.scene.object(id) else { continue };
        if !watch.is_empty() && !watch.contains(&object.name) {
            continue;
        }
        let (name, active) = (object.name.clone(), object.active);
        let at = world.scene.world_position(id);
        let mut components = Vec::new();
        for component in world.scene.component_names(id) {
            if let Some(c) = world.scene.component_mut(id, component) {
                components.push((component, value(c)));
            }
        }
        let mut entry = vec![
            ("id", id_json(id)),
            ("name", Json::str(name)),
            ("x", Json::Number(at.x.to_string())),
            ("y", Json::Number(at.y.to_string())),
        ];
        if !active {
            entry.push(("active", Json::Bool(false)));
        }
        if let Some(parent) = world.scene.parent(id) {
            entry.push(("parent", id_json(parent)));
        }
        let touching = world.touching(id);
        if !touching.is_empty() {
            entry.push(("touching", Json::List(touching.into_iter().map(id_json).collect())));
        }
        entry.push(("components", Json::Map(components)));
        objects.push(Json::Map(entry));
    }
    Json::Map(vec![("tick", Json::Int(world.tick().into())), ("objects", Json::List(objects))])
}

/// `--simulate`: plays `scene` for `sim.ticks` ticks, exactly as Play does,
/// and reports the objects at the end (and along the way, with `every`).
/// `state_hash` is the same for the same scene, code and input, every run.
pub fn simulate(
    db: &AssetDb,
    scene: Scene,
    scene_path: &str,
    scripts: &Scripts,
    sim: &Simulation,
) -> Json {
    let registry = player::registry(scripts);
    let behaviours = player::behaviours(scripts);
    let schedule = schedule();
    let mut world = World::new(scene);
    let mut steps = Vec::new();
    let mut sounds = Vec::new();
    // A sound's path in assets/, or its GUID without the project's assets.
    let name =
        |guid: reign::assets::Guid| Json::str(db.path(guid).unwrap_or_else(|| guid.to_string()));
    for tick in 0..sim.ticks {
        let mut frame = InputFrame::default();
        for hold in sim.input.iter().filter(|h| h.from <= tick && tick <= h.to) {
            frame.set(hold.button, true);
        }
        for event in runtime::tick(&mut world, frame, &behaviours, &schedule, &registry, Some(db)) {
            let heard = match event {
                Event::Sound { clip, .. } => ("sound", name(clip)),
                Event::Music { clip, .. } => ("music", name(clip)),
                Event::StopMusic => ("music", Json::Null),
            };
            sounds.push(Json::Map(vec![("tick", Json::Int(tick.into())), heard]));
        }
        if let Some(every) = sim.every.filter(|e| *e > 0)
            && (tick + 1) % every == 0
            && tick + 1 < sim.ticks
        {
            steps.push(snapshot(&mut world, &sim.watch));
        }
    }
    let mut hasher = DefaultHasher::new();
    world.hash(&mut hasher);
    let mut out = vec![
        ("scene", Json::str(scene_path)),
        ("ticks", Json::Int(sim.ticks.into())),
        ("seconds", Json::Number(format!("{:.3}", sim.ticks as f64 / TICKS_PER_SECOND as f64))),
        ("state_hash", Json::str(format!("{:016x}", hasher.finish()))),
    ];
    if !sounds.is_empty() {
        out.push(("sounds", Json::List(sounds)));
    }
    if !steps.is_empty() {
        out.push(("steps", Json::List(steps)));
    }
    out.push(("end", snapshot(&mut world, &sim.watch)));
    Json::Map(out)
}

/// Which headless command to run, from the command line.
pub enum Command {
    Check,
    Describe,
    Simulate {
        ticks: u64,
        scene: Option<String>,
        input: Option<String>,
        every: Option<u64>,
        watch: Vec<String>,
    },
}

/// Runs a command on the project's assets and prints its JSON. Returns the
/// process's exit code.
pub fn run(command: Command, db: &'static AssetDb, scripts: &Scripts, title: &str) -> i32 {
    match command {
        Command::Check => {
            let report = check(db, scripts);
            println!("{}", report.json().pretty());
            if report.errors.is_empty() { 0 } else { 1 }
        }
        Command::Describe => {
            println!("{}", describe(db, scripts, title).pretty());
            0
        }
        Command::Simulate { ticks, scene, input, every, watch } => {
            let fail = |message: String| {
                let error =
                    Json::Map(vec![("ok", Json::Bool(false)), ("error", Json::Str(message))]);
                println!("{}", error.pretty());
                1
            };
            let map = match input_map(db) {
                Ok(m) => m,
                Err(e) => return fail(format!("{INPUT_FILE}: {e}")),
            };
            let holds = match parse_input(input.as_deref().unwrap_or(""), &map) {
                Ok(h) => h,
                Err(e) => return fail(e),
            };
            let Some(path) = player::choose_scene(&player::scene_paths(db), scene.as_deref())
            else {
                return fail("the project has no scenes".into());
            };
            if let Some(wanted) = scene.filter(|w| *w != path) {
                return fail(format!("no scene {wanted:?}: try {path:?}"));
            }
            let registry = player::registry(scripts);
            let loaded = match player::load_scene(db, &registry, &path) {
                Ok(s) => s,
                Err(e) => return fail(format!("{path}: {e}")),
            };
            let sim = Simulation { scene: Some(path.clone()), ticks, input: holds, every, watch };
            println!("{}", simulate(db, loaded, &path, scripts, &sim).pretty());
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Project;

    const SCENE: &str = r#"(
    version: 1,
    objects: [
        (id: (0, 0), name: "Main Camera", components: {"Camera": ()}),
        (id: (1, 0), name: "Box", components: {"BoxCollider": (size: (16, 16)), "Rigidbody": ()}),
        (
            id: (2, 0),
            name: "Floor",
            transform: (position: (x: "0", y: "100")),
            components: {"BoxCollider": (size: (200, 16))},
        ),
    ],
)"#;

    fn temp_project(name: &str, scene: &str) -> (Project, AssetDb) {
        let dir =
            std::env::temp_dir().join(format!("reign-headless-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = Project::open(dir).expect("project opens");
        std::fs::write(project.scenes().join("main.scene.ron"), scene).unwrap();
        let db = AssetDb::loose(project.assets()).expect("assets load");
        (project, db)
    }

    #[test]
    fn json_escapes_strings_and_nests() {
        let doc = Json::Map(vec![
            ("name", Json::str("a \"b\"\n\\")),
            ("list", Json::List(vec![Json::Int(1), Json::Null, Json::Bool(true)])),
            ("empty", Json::Map(vec![])),
        ]);
        assert_eq!(
            doc.pretty(),
            "{\n  \"name\": \"a \\\"b\\\"\\n\\\\\",\n  \"list\": [1, null, true],\n  \"empty\": {}\n}"
        );
    }

    #[test]
    fn input_specs_name_actions_and_ticks() {
        let map = InputMap::default();
        let holds = parse_input("Right, Jump@30-45,Attack@7", &map).unwrap();
        assert_eq!(
            holds,
            vec![
                Hold { button: Button::Right, from: 0, to: u64::MAX },
                Hold { button: Button::Jump, from: 30, to: 45 },
                Hold { button: Button::Attack, from: 7, to: 7 },
            ]
        );
        assert_eq!(parse_input("", &map), Ok(vec![]));
        assert!(parse_input("Fly", &map).unwrap_err().contains("Jump"));
        assert!(parse_input("Jump@9-3", &map).is_err());
        assert!(parse_input("Jump@soon", &map).is_err());
    }

    #[test]
    fn check_passes_a_good_project_and_names_what_is_wrong() {
        let (project, db) = temp_project("check", SCENE);
        assert_eq!(check(&db, &Scripts::none()), Report::default());
        let _ = std::fs::remove_dir_all(&project.root);

        let broken =
            SCENE.replace("\"Rigidbody\": ()", "\"Jetpack\": ()").replace("Main Camera", "X");
        let broken = broken.replace("components: {\"Camera\": ()}", "");
        let (project, db) = temp_project("check-broken", &broken);
        let report = check(&db, &Scripts::none());
        assert_eq!(report.errors.len(), 1, "{report:?}");
        assert_eq!(report.errors[0].file, "scenes/main.scene.ron");
        assert!(report.errors[0].message.contains("Jetpack"), "{report:?}");
        assert!(report.json().pretty().contains("\"ok\": false"));
        let _ = std::fs::remove_dir_all(&project.root);
    }

    #[test]
    fn simulate_drops_the_box_onto_the_floor_the_same_way_every_time() {
        let (project, db) = temp_project("simulate", SCENE);
        let scripts = Scripts::none();
        let registry = player::registry(&scripts);
        let run = || {
            let scene = player::load_scene(&db, &registry, "scenes/main.scene.ron").unwrap();
            let sim = Simulation { ticks: 240, watch: vec!["Box".into()], ..Simulation::default() };
            simulate(&db, scene, "scenes/main.scene.ron", &scripts, &sim).pretty()
        };
        let out = run();
        // 16 px box centred on its position, on a floor whose top is at 92.
        assert!(out.contains("\"y\": 84"), "{out}");
        assert!(out.contains("\"grounded\": true"), "{out}");
        assert!(out.contains("\"touching\""), "{out}");
        assert!(!out.contains("\"Floor\""), "only the watched object: {out}");
        assert_eq!(out, run());
        let _ = std::fs::remove_dir_all(&project.root);
    }

    #[test]
    fn describe_lists_fields_with_defaults_and_hints() {
        let (project, db) = temp_project("describe", SCENE);
        let out = describe(&db, &Scripts::none(), "Test").pretty();
        for expected in [
            "\"name\": \"Rigidbody\"",
            "\"variants\": [\"Dynamic\", \"Kinematic\", \"Static\"]",
            "\"asset_kind\": \"Texture\"",
            "\"range\": [0, 31]",
            "\"name\": \"Jump\"",
            "\"scenes\": [\"scenes/main.scene.ron\"]",
        ] {
            assert!(out.contains(expected), "missing {expected}: {out}");
        }
        let _ = std::fs::remove_dir_all(&project.root);
    }
}
