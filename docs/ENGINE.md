# Reign: scenes, components, Behaviours, prefabs and assets

This is how Reign models a game's content, and how the editor and scene files
work with it. It covers `engine/reign_scene` (scenes, components, reflection,
scene files) and `engine/reign_derive` (the derives), both reached through the
facade as `reign::scene`; `engine/reign_behaviour` (per-object scripts run in
fixed stages, with messages), reached as `reign::behaviour`; and
`engine/reign_assets` (GUIDs, `.meta` files, handles, asset types, prefabs,
hot reload and packing), reached as `reign::assets`. See
[Behaviours](#behaviours), [Prefabs](#prefabs), [Combat data](#combat-data)
and [Assets](#assets).

## The model: game objects with components

Reign follows Unity's mental model, not an ECS: **a scene holds game objects,
and game objects hold components**. Game logic is ordinary Rust, called from
the fixed tick, that reads and writes components through simple queries.

- **`Scene`**: game objects in a generational arena. It is simulation state:
  `Clone + Hash + Eq`, so it rides along in rollback, replays, checksums and
  the editor's Play/Stop snapshot like everything else. Cloning is cheap: the
  object table and each component type's storage are shared copy-on-write, so
  a clone copies only what changes after it.
- **`ObjectId { index, generation }`**: a handle to an object. Despawning
  bumps the slot's generation, so an old handle never finds the object that
  reuses the slot.
- **`GameObject`**: a name, an `active` flag, a `Transform`, a parent and
  ordered children, and `dont_save` (made at runtime, never written to scene
  files).
- **`Transform`**: built into every object. A local `position: FxVec2`, a
  `z` draw order and `flip_x`/`flip_y`. World positions add up the parent
  chain (`Scene::world_position`). There is no rotation or scale.
- **Components**: plain structs, stored per type and sorted by object id, so
  iteration order never depends on the order of spawns and despawns.

### Queries

```rust
scene.get::<Rigidbody>(id);                      // Option<&Rigidbody>
scene.get_mut::<Rigidbody>(id);
scene.iter::<Rigidbody>();                       // (ObjectId, &Rigidbody), by id
scene.iter_mut::<SpriteRenderer>();              // (ObjectId, &mut SpriteRenderer)
scene.iter2_mut::<Rigidbody, BoxCollider>();     // objects with both
let (mut bodies, colliders, sprites) =
    scene.query_mut::<(Rigidbody, BoxCollider, SpriteRenderer)>();
scene.iter_with_transform::<Camera>();           // (ObjectId, &Transform, &Camera)
```

Queries skip inactive objects and everything under them. Direct `get` works
on any live object. `query_mut` splits one to eight distinct type stores into
views with `iter`, `iter_mut`, `get` and `get_mut`. Each view iterates by id;
cross-object systems can hold several component references simultaneously.
Duplicate types/names panic before borrowing. Safe `get_disjoint_mut` borrows
and per-store copy-on-write isolate snapshots; no `unsafe` or actor copies
are needed. Missing stores yield empty views and don't affect the scene hash.
Queue structural changes until the views are released.

### Spawning during a tick

Spawns, despawns and component changes made while a query is running go
through a `Commands` buffer, applied at a fixed point with `Scene::apply`:

```rust
let mut commands = Commands::new();
let bolt = commands.spawn("Bolt");
commands.insert(bolt, Projectile { speed: Fx::from_px(3) });
commands.despawn(old);
scene.apply(&mut commands); // in order, at a deterministic point
```

Free slots are reused lowest index first, so objects spawned one after
another get ascending ids, and iterate in the order they were made, even
after despawns elsewhere.

### Determinism rules

- **No floats in components.** Positions and speeds are `Fx`/`FxVec2`;
  colours are `Rgba8`; percentages are integers. (A light's `intensity: 80`
  becomes `0.8` only when drawn.)
- **No `HashMap`/`HashSet` with random state.** Use `Vec`, `BTreeMap` or
  sorted vectors. Component storages and the object table iterate by id.
- **Everything is `Clone + Eq + Hash`.** `Component` requires it.

## Components and the derives

```rust
use reign::scene::{Component, Reflect, Rgba8};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[component(bounds(-6, -6, 12, 12))]
pub struct PointLight {
    /// In world pixels.
    #[reflect(range(1, 512))]
    pub radius: u16,
    pub color: Rgba8,
    #[reflect(range(0, 400), tooltip = "Brightness in percent")]
    pub intensity: u16,
    #[serde(skip)]
    #[reflect(read_only)]
    pub flicker_phase: u16,
}
```

A component is `Reflect + Clone + Eq + Hash + Default + Serialize +
Deserialize + Send + Sync + 'static`. `Default` is what Add Component and
Reset use. `#[derive(Component)]` writes the reflection and the `Component`
impl. `#[derive(Reflect)]` does the same for nested structs and fieldless enums.

| Attribute | Meaning |
| --- | --- |
| `#[component(name = "...")]` | The stable name in scene files and the editor. Defaults to the type name. Renaming it breaks saved scenes. |
| `#[component(bounds(x, y, w, h))]` | The editor's click box, relative to the object's position. |
| `#[component(bounds = "path")]` | A `fn(&Self) -> Option<Bounds>`, for boxes that depend on the data. |
| `#[reflect(skip)]` | Not reflected: the editor never sees it. |
| `#[reflect(read_only)]` | Shown, not editable. |
| `#[reflect(range(min, max))]` | Editors clamp to this range. |
| `#[reflect(tooltip = "...")]` | Hover text. A doc comment works too. |
| `#[serde(...)]` | Left to serde, for example `#[serde(skip)]` for runtime-only state. |

Reflected field kinds: all integer types, `bool`, `Fx`, `FxVec2`, `Rgba8`,
`String`, `ObjectId` (a picker in the editor), fieldless enums (a combo box),
`Option<T>`, `Vec<T>`, `[T; N]` and nested `Reflect` structs.

### Registering

Registration is explicit, once at startup. There is no link-time magic:

```rust
let mut registry = ComponentRegistry::new();
registry.register::<Camera>().register::<SpriteRenderer>().register::<Rigidbody>();
```

Scene files can only load registered types, and Add Component offers exactly
these.

### Runtime state in a component

Authored data and runtime state can share a component: mark the runtime
fields `#[serde(skip)]`, so a save never bakes one run's state into the file.
The `PointLight` above saves `radius`, `color` and `intensity` but not
`flicker_phase`; the `Blinker` in [Write your first
Behaviour](#write-your-first-behaviour) keeps its countdown the same way.

## Scene files

Scenes are RON. A project keeps them in `assets/scenes/*.scene.ron`, where
they are assets like any other (kind `Scene`). `Scene::to_ron` writes one;
`Scene::from_ron(text, &registry)` reads one.

```ron
// A Reign scene. Objects are listed parents first; ids are (index, generation).
// Fixed-point numbers are exact decimal strings. Edit by hand or in the editor.
(
    version: 1,
    objects: [
        (
            id: (0, 0),
            name: "Main Camera",
            components: {"Camera": ()},
        ),
        (
            id: (1, 0),
            name: "Platform",
            transform: (position: (x: "0", y: "96")),
            components: {
                "BoxCollider": (size: (160, 16), offset: (0, 0), is_trigger: false, layer: 0, mask: 4294967295),
                "Rigidbody": (body_type: Static, gravity_scale: "1", velocity: (x: "0", y: "0"), bounciness: 0, grounded: false),
            },
        ),
        (
            id: (2, 0),
            name: "Sprite",
            parent: Some((1, 0)),
            transform: (position: (x: "0", y: "-0.5")),
            components: {"SpriteRenderer": (sheet: "524549474e0053484150455300000002", sprite: "square", color: (r: 255, g: 174, b: 66, a: 255), size: (160, 16))},
        ),
    ],
)
```

- Objects are listed **parents first**, in hierarchy order. A child's place
  among its siblings is its place in the list.
- `id` is `(index, generation)`. Ids stored inside components (an `ObjectId`
  field) use the same form, so references survive a save and load.
- Defaults are left out: `parent: None`, `active: true`, a default transform,
  no components.
- **`Fx` values are exact decimal strings.** `"0.00390625"` is exactly 1/256.
  Every 24.8 value has a finite decimal expansion, so a save writes the exact
  value. Loading parses the decimal with integer arithmetic only, never
  through `f32`/`f64`. Decimals between two representable values round to
  the nearest, ties away from zero. Plain integers are accepted; floats like
  `1.5` (unquoted) are refused.
- Components are keyed by their registered name. Their bodies are their
  serde form (`()`, `Static`, `Some((4, 0))`).
- Objects marked `dont_save` are left out. A scene with despawn churn also
  lists its free slots (`free: [(4, 1)]`), so loading it back gives exactly
  the same scene, hash included.
- Errors say what's wrong: unknown components, parents not listed first,
  reused ids, floats in fixed-point fields, a wrong version.

Scene files are sim-affecting assets: they are part of
`AssetDb::content_hash` (see [Simulation assets and online
play](#simulation-assets-and-online-play)), so a game that compares that hash
between peers refuses to play with different scene data instead of
desyncing.

## The editor

See the README's "Running the editor directly" section for running it. On
the scene:

- **Hierarchy**: the scene as a tree. Select, toggle active, drag onto
  another object to reparent (keeping world position), drop on empty space
  for the top level. Right-click to create an empty child, duplicate or
  delete.
- **Inspector**: name and active flag, Transform, then each component through
  reflection. Right-click a component's header to Reset or Remove; Add
  Component lists the registry.
- **Game view**, while not playing: click selects the topmost object under the
  cursor (highest `z`, then drawn last), using its component bounds or a
  default 16 px box. Drag moves it, snapped to pixels; hold Ctrl to snap to
  the 16 px grid.
- **Undo/redo**: Ctrl+Z, Ctrl+Y or Ctrl+Shift+Z. Each step is a whole-scene
  snapshot (cheap, thanks to copy-on-write), so every kind of edit restores
  exactly.
- **Save/open**: Ctrl+S, and the Scene menu. A `*` marks unsaved changes.
- **Play** snapshots everything; **Stop** restores it, so edits made while
  playing are discarded, as in Unity.

An app plugs in through `EditorApp`. It exposes its scene and registry, hears
about edits through `scene_edited` (to rebuild anything it derives from the
scene), and can mirror state that lives outside the scene into it each frame
through `sync_scene`.

## Assets

`reign::assets` (`engine/reign_assets`) gives every asset a stable identity,
loads it as a typed value, hot reloads it in development and packs it into
the executable for release.

### GUIDs and `.meta` files

Every asset file has a sidecar next to it: `hero.png` has `hero.png.meta`.

```ron
(
    guid: "824886a022e677b8625c2fea04f7d5be",
    importer: Texture((density: 2)),
)
```

- `guid` is a random 128-bit id, written as 32 hex digits. It is made once,
  when the asset is first seen (by the editor, `AssetDb::loose` or
  `reign-assets metas`), and never changes. Renaming or moving a file in the
  Project panel takes its `.meta` along, so the GUID, and every reference
  to it, survives. Moving files by hand works too, as long as the `.meta`
  goes with them.
- `importer` says how to read the file, with its import settings. It is
  picked by extension when the `.meta` is made (`Importer::for_path`):

  | File | Importer | Loads as | Settings |
  | --- | --- | --- | --- |
  | `*.png` | `Texture` | `Texture` | `density` (texels per world pixel) |
  | `*.wav` | `Audio` | `AudioClip`, or `Music` when `stream` | `looping`, `stream` (files under `music/` stream) |
  | `*.sheet.ron` | `SpriteSheet` | `SpriteSheet` | |
  | `*.anim.ron` | `AnimationClip` | `AnimationClip` | |
  | `*.tileset.ron` | `Tileset` | `Tileset` | |
  | `*.tilemap.ron` | `Tilemap` | `Tilemap` | |
  | `*.font.ron` | `Font` | `Font` | |
  | `*.scene.ron` | `Scene` | `SceneAsset` | |
  | `*.prefab.ron` | `Prefab` | `Prefab` | |
  | `*.moves.ron` | `MoveSet` | `MoveSet` | |
  | anything else | `Data` | `DataAsset` (bytes) | |

- A copy of a file with its `.meta` would share a GUID; the database gives
  the copy a new one and logs it.
- `.reignignore` (in the asset root) lists globs that aren't assets
  (sources, previews, docs). `reign-assets check DIR` fails on an asset
  without a `.meta`, a broken `.meta` or a handle pointing at nothing, so a
  project's CI can run it.

### Handles

`Handle<T>` is a typed GUID: `Copy`, `Eq`, `Ord`, `Hash` and `Default`
(`Handle::NONE`). It serializes as the GUID string, so scenes and assets
refer to each other by GUID:

```rust
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
pub struct Image {
    pub texture: Handle<Texture>,
}
```

A handle hashes as its GUID, so it can sit in simulation state. It reflects
as an asset field: the Inspector shows a menu of the assets of its kind and
accepts an asset dragged from the Project panel. `db.load(handle)` gives an
`Arc<T>`, cached until the asset reloads.

Code that owns an asset may name it by path (`db.load_path::<T>("ui/font.png")`,
`db.handle::<T>(path)`), as Unity's `Resources.Load` does. Data refers to
assets by handle.

### Types

| Type | What it holds |
| --- | --- |
| `Texture` | Decoded RGBA8 pixels and their density. |
| `SpriteSheet` | A texture (and an optional mirrored twin), named sprites (rect, pivot), an optional grid of cells for animation rows, and a density. Made in the Sprite slicer. |
| `AnimationClip` | A sheet, a row, a loop mode and frames: cell, duration in ms, a hit flag, an event and presentation cues (shake, flash, dim). Sampled on the 120 Hz tick exactly as `reign_core::animation::Clip` samples (`frame_at_tick`, `to_clip`). |
| `Tileset` | A texture cut into square tiles, numbered row by row. |
| `Tilemap` | A tileset, a `collision` layer (`#` solid, `-` one-way, `/` `\` slopes, `.` empty), a `markers` layer (one glyph per cell that the game spawns from) and visual tile layers. `collision_map()` gives the physics `TileMap`. |
| `AudioClip`, `Music` | PCM16 WAV bytes and whether they loop. |
| `Font` | A bitmap font: texture, cell size, character range. |
| `SceneAsset` | A scene file's text, read with `Scene::from_ron`. |
| `Prefab` | A prefab file's text; see [Prefabs](#prefabs). |
| `MoveSet` | Combat frame data: named moves; see [Combat data](#combat-data). |
| `DataAsset` | Bytes. |

### The database and its two backends

`AssetDb` answers the same calls over either backend:

- **Loose** (`AssetDb::loose(dir)`): the files under a folder. The editor
  opens a project's `assets/` folder this way. It can write (`create`,
  `write`, `write_meta`, `rename`, `delete`, folders), which is how the
  Project panel and the asset tools save.
- **Packed** (`AssetDb::packed(blob)`): one archive (`&'static [u8]`), read
  in place without copying. A game can pack its asset folder with
  `pack_dir` (or `reign-assets pack DIR OUT`), for example from its
  `build.rs` into `OUT_DIR`, and embed the result with `include_bytes!`, so
  a release build is a single executable. The pack format is `RPAK`:
  entries sorted by path, each with its `.meta` text and bytes, so the same
  folder always packs to the same bytes.

### Hot reload

`db.watch()` (feature `watch`, on in the editor) starts a `notify` watcher
that only **queues** changed paths. Nothing is reloaded until
`db.apply_changes()`, which the editor calls once at the start of each frame,
before any tick: it drops cached values, bumps versions and returns
`AssetChange`s. A change waits until the file has been left alone for
150 ms, so a file still being written isn't read half done. The editor then:

- logs `reloaded <kind> <path>` (or `simulation data changed: ...` for sim
  assets) and flashes the asset in the Project panel;
- calls `EditorApp::assets_changed`, where the app refreshes whatever it
  built from those assets (re-uploading textures, rebuilding collision from
  a changed tilemap, and so on);
- re-stamps the open scene's instances of any prefab that changed.

Saving from an editor tool reloads that asset at once and reports it at the
next frame boundary the same way.

### Simulation assets and online play

`AssetKind::sim_affecting` is true for `Tilemap`, `AnimationClip`,
`Scene`, `Prefab` and `MoveSet`: they decide collision, timing, placement,
what actors are made of and how hard they hit. `db.content_hash()`
hashes every sim asset's GUID, import settings and bytes (textures and
sounds are left out). A networked game should compare it between peers
before playing, so peers with different simulation data refuse to play
instead of desyncing, and should ignore hot reloads of sim assets while
online; presentation assets can still reload. The Inspector notes both on a
sim-affecting asset.

### The editor's asset tools

- **Project**: folder tree, thumbnail grid, search and type filter. Create,
  rename, move (drag onto a folder) and delete, always with the `.meta`;
  deleting something that other files refer to lists them first. Drag an
  asset onto a handle field, or a sprite into the Game view to place it
  (`EditorApp::create_from_asset`; the standalone editor makes an object
  with a `SpriteRenderer` for a sprite or an `Image` for a texture). Selecting an
  asset shows it in the Inspector, with import settings, Apply and Revert.
- **Tilemap**: brush (B), eraser (E), rectangle (R), flood fill (G),
  eyedropper (I, or right-click) on the collision, marker and visual
  layers; layer visibility; a 16 px grid; pan (middle-drag or Space-drag)
  and zoom; undo/redo; Save.
- **Sprites**: grid slicing (cell, offset, padding, pivot, names; empty
  cells skipped), sprites dragged out by hand, names, rects, pivots and
  density. Saves `name.sheet.ron` next to the texture.
- **Animation**: the frame timeline (add, reorder, remove), durations with
  the ticks they cover, hit flags, events and cues, row, loop mode and
  cancel frame; a 120 Hz preview with play, pause, step and scrub. The
  preview draws the hitboxes and hurtboxes of every move that plays the
  clip (see [Combat data](#combat-data)): pick the move and the body
  height, add boxes on the current frame, drag them or type their numbers;
  Save boxes writes the move sets. Undo covers frames and boxes.

Each tool has its own undo history; Ctrl+Z and Ctrl+Y go to the tool under
the pointer, otherwise to the scene.

### Adding an asset type

1. Write the type in `engine/reign_assets/src/types.rs` (usually
   `Serialize + Deserialize + Reflect`, read with `ron_asset`) and implement
   `Asset` for it: its `KIND` and `load(bytes, meta)`.
2. Add a variant to `AssetKind` (and `ALL`, `name`; say whether it is
   `sim_affecting`) and to `Importer`, with its extension in
   `Importer::for_path` and its kind in `Importer::kind`. Import settings go
   in the variant as a `Reflect` struct, and the Inspector edits them.
3. Re-export it from `lib.rs`. A game can now hold `Handle<NewType>` fields;
   the Inspector picks them by `AssetKind::name`.
4. If the editor should preview it, add it to `Thumbs::preview` in
   `engine/reign_editor/src/assets_ui.rs`, and handle its changes in the
   game's `assets_changed`.

## Behaviours

`reign::behaviour` is Unity's MonoBehaviour made deterministic. A Behaviour
is a component (so all of its state is in the scene, cloned and hashed with
everything else) with the `Behaviour` trait on it. The engine runs every
Behaviour through the tick's stages; the game never calls an object's update
by hand.

```rust
pub trait Behaviour<G: Game>: Component {
    const ORDER: i32 = 0;
    fn start(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
    fn fixed_update(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
    fn late_update(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
    fn on_message(ctx: &mut Ctx<'_, G>, me: ObjectId, msg: &Envelope<G::Message>) {}
    fn on_destroy(ctx: &mut Ctx<'_, G>, me: ObjectId) {}
}
```

`G` is any `Clone + Hash` game world implementing `Game`: it exposes its
scene, RNG, tick, message/event types and `BehaviourState` (which Behaviours
have started). Input is optional (`Game::input` defaults to `None`). Physics,
a tile map and other resources are ordinary typed fields on `G`, chosen by
the game; the runtime has no `reign_physics` dependency or tile-map interface.
Access resources through `ctx.game`, splitting field borrows as needed.
The engine test game has neither physics nor a tile map nor player inputs;
it exercises lifecycle, messages, Systems and snapshot/replay anyway.

### Lifecycle

- **`start`** runs once per Behaviour, before its first `fixed_update`:
  for objects spawned during a tick, at the end of that tick (in the
  `Commands` stage); for objects placed when the game builds its world,
  when the game calls `Schedule::start`. Deactivating and reactivating an
  object doesn't start it again.
- **`fixed_update`** every tick, before physics and combat.
- **`late_update`** every tick, after combat and messages.
- **`on_message`** for each message sent to the object, or broadcast.
- **`on_destroy`** just before a despawn through the command buffer
  removes the object (and for everything under it).

### Stages and order

A `Schedule` is the tick's stages in order. The standard one is

```text
Start → input → FixedUpdate → physics → combat → Messages → LateUpdate
      → Messages → Commands
```

where `input`, `physics` and `combat` are named **system stages**.
Register `System::new(name, stage, order, function)` on a schedule for
`fn(&mut Ctx<G>) -> Flow` for work that belongs to no single object (moving
platforms, resolving hits across everyone). A game lists its own stages
with `Schedule::new().then(...)`. A system may return `Flow::EndTick` (for
hitstop or a pause): the tick then skips to its end, which **always**
delivers remaining messages, applies queued commands and starts what they
made, so nothing waits between ticks outside the hashed world.

A **Behaviour** owns per-object lifecycle and messages (a door, a moving
character, a blinking light). A **System** owns shared rules across objects
(collision resolution, timers, stage progress). Both keep all state in the
world/components. Systems are plain functions with names for inspection;
lower `order` runs first within their stage, ties preserve registration order.
`Flow::EndTick` skips remaining systems in that stage and all later stages,
but still flushes messages/commands. Duplicate names within one stage and
registrations for missing stages fail loudly.

```rust
let schedule = Schedule::<World>::new()
    .then(Stage::Start)
    .then(Stage::Systems("simulation"))
    .then(Stage::FixedUpdate)
    .then(Stage::Commands)
    .system(System::new("platform motion", "simulation", 0, move_platforms))
    .system(System::new("support planning", "simulation", 10, plan_support));
// Inspect the configured execution order:
let systems: Vec<_> = schedule.systems("simulation").collect();
```

Within a stage, Behaviours run **by type, in `ORDER`** (ties in
registration order), **then by `ObjectId`**. Register types once at
startup:

```rust
let mut behaviours = Behaviours::<World>::new();
behaviours.behaviour::<Player>().behaviour::<Door>().behaviour::<Blinker>();
```

### Borrowing and structural changes

Callbacks receive `ctx` and `me` and borrow their components explicitly:
`ctx.scene_mut().get_mut::<Self>(me)`, or split stores through `query_mut`.
Components remain in place; no temporary default values are installed.
Borrow world resources and scene fields separately when both are needed;
release component borrows before sending messages through `ctx`. Spawns, despawns
and adding or removing components go through `ctx.commands`, applied in
the `Commands` stage, so the set of objects never changes under a running
stage. `ctx.commands.instantiate(template, at)` places a prefab.

### Messages

`ctx.send(object, msg)` and `ctx.broadcast(msg)` queue plain
`Clone + Eq + Hash` data, with the sender in `Envelope::from`. The
`Messages` stage delivers the queue in send order; messages sent while
delivering are delivered in the same stage, after those already queued. A
broadcast reaches every Behaviour in execution order. A system can deliver
early with `behaviours.deliver(ctx)` when an answer must land before it
goes on.

### Rules for staying rollback-safe

- All state lives in components or the game's hashed world: no statics,
  `Rc`/`RefCell`, closures or trait objects holding state between ticks.
- Randomness only from `ctx.rng()`. No floats; no `HashMap`/`HashSet`.
- Physics is optional. A game may expose collision read-only in its own
  helpers and schedule systems that edit it; the runtime imposes no map type.
- `ctx.emit(event)` is presentation output (sounds, particles). It never
  feeds back into the simulation: a rollback replays it.
- Order things that matter by id or by data, never by memory address or
  insertion into a container that isn't hashed.

### Write your first Behaviour

A lamp that blinks every half second and tells its neighbours when it
goes out:

```rust
use reign::behaviour::{Behaviour, Ctx, Envelope};
use reign::scene::{Component, ObjectId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
pub struct Blinker {
    /// Ticks between blinks (authored, saved in scenes and prefabs).
    #[reflect(range(1, 600))]
    pub period: u16,
    /// Runtime state: in the component, so rollback restores it.
    #[serde(skip)]
    #[reflect(read_only)]
    pub left: u16,
    #[serde(skip)]
    pub lit: bool,
}

impl Behaviour<World> for Blinker {
    const ORDER: i32 = 50; // after the actors

    fn start(ctx: &mut Ctx<World>, me: ObjectId) {
        let lamp = ctx.scene_mut().get_mut::<Self>(me).expect("registered");
        lamp.left = lamp.period;
        lamp.lit = true;
    }

    fn fixed_update(ctx: &mut Ctx<World>, me: ObjectId) {
        let lamp = ctx.scene_mut().get_mut::<Self>(me).expect("registered");
        lamp.left = lamp.left.saturating_sub(1);
        if lamp.left == 0 {
            lamp.left = lamp.period;
            lamp.lit = !lamp.lit;
            if !lamp.lit {
                ctx.broadcast(Msg::LampOut(me));
            }
        }
    }

    fn on_message(ctx: &mut Ctx<World>, me: ObjectId, msg: &Envelope<Msg>) {
        if let Msg::LampOut(other) = msg.body
            && other != me
        {
            let lamp = ctx.scene_mut().get_mut::<Self>(me).expect("registered");
            lamp.left = lamp.left.min(10); // catch up with the others
        }
    }
}
```

Register it in the component registry (scene files, Add Component) and in
the game's `Behaviours`, put it on an object in the editor (or a prefab),
and it runs. The engine's own tests (`engine/reign_behaviour/src/tests.rs`)
cover order under spawn and despawn churn, message order, `on_destroy`,
start-once and snapshot and restore with live Behaviours.

## Prefabs

A **prefab** (`*.prefab.ron`, kind `Prefab`, sim-affecting) is a scene
file whose first top-level object is the prefab's root. Ids in it are local
to the file; references out of it are handles. `reign::assets::prefab`
does the rest:

- **Instances.** A scene places a prefab with an object that has a
  `PrefabInstance { prefab: Handle<Prefab>, .. }`. Loading
  (`prefab::expand_all`) or instantiating **stamps** the prefab onto it: the
  root's components go on the object, the prefab's children are made under
  it (marked `dont_save`, since they come from the prefab), and
  `ObjectId` fields inside are remapped to the new objects. The instance's
  name and transform stay its own.
- **Overrides.** An instance stores what it changes: property overrides
  `(object path, "Component.field.sub", value)`, added components (as RON)
  and removed ones. Property paths (`reign::scene::path`) walk reflection:
  `health.max`, list items `slots.1`, a list's length `slots.#len`, an
  option's inside `attack.?`; `GameObject.name`, `.active` and
  `.transform...` cover the object itself. `prefab::overrides` works them
  out by comparing the instance with its prefab; `saved_form` writes each
  instance back as just its `PrefabInstance`, so a scene file follows its
  prefabs as they change. `revert_property`, `revert_all`, `apply_all`,
  `refresh` and `unpack` are the editor's buttons.
- **Nesting.** An object inside a prefab may itself be an instance. Object
  paths name objects through the nesting: `[]` is the root, `[3]` the
  object with local index 3, `[3, 1]` the object with local index 1 in the
  prefab placed at 3. An outer instance can override objects of an inner
  one. A prefab containing itself is an error, not a hang.
- **Variants** (a prefab whose root is an instance of another) are not
  supported yet; such a file fails to load with a clear error.
- **Runtime.** `prefab::instantiate(scene, &template, at, parent)`, or
  `commands.instantiate(template, at)` from a Behaviour, stamps a prefab
  during a tick. Objects are allocated in the prefab's order through the
  scene's allocator, so every peer, and every replay after a rollback,
  gets the same ids. `Prefabs` parses and caches templates from an
  `AssetDb` with the game's component registry.

In the editor:

- **Make a prefab:** drag an object from the Hierarchy onto a Project
  folder (or the empty space of the grid). The object becomes an instance.
- **Prefab Mode:** double-click a prefab (or press Open on an instance).
  The scene is set aside with its undo history; the prefab's objects take
  its place in the Hierarchy and Inspector, drawn on a canvas in the Game
  view. Ctrl+S saves the prefab; `< Scene` goes back. Play waits for the
  scene.
- **Instances in the Inspector** show their prefab, with Select (the
  asset), Open, Apply All, Revert All and Unpack. Overridden fields are
  bold; right-click one to revert it.
- **Updates:** saved instances in the open scene are re-stamped when their
  prefab is saved or hot reloaded, keeping their overrides (kept up to date
  on every edit). Objects a game instantiates at runtime pick up prefab
  changes the next time the game instantiates them.

## Combat data

`reign::combat::MoveSetFile` is a game's moves as data, saved as a
`*.moves.ron` asset (kind `MoveSet`, sim-affecting):

```ron
(
    moves: {
        "Slash": (
            clip: Some("<the clip's GUID: 32 hex digits>"),
            startup: 24, active: 7, recovery: 70,
            hitbox: (x: 3, y: -2, w: 19, h: 26),
            damage: 1,
            knockback: (x: "1.5", y: "-2"),
            hitstop: 7,
            shake: (amplitude: 2, ticks: 12),
            recoil: (x: "0", y: "0"),
            cancel_from: None,
            chain: None,
            mobile: false,
            rehit: 0,
            frames: [(frame: 2, hitboxes: [(x: 3, y: -2, w: 19, h: 26)], hurtboxes: [(x: -6, y: 0, w: 12, h: 24)])],
        ),
    },
)
```

- Timing is in ticks; boxes are in world pixels, authored facing right
  (`x` from the body's centre, `y` from its top) and mirrored for facing
  left. `chain` names the next move.
- `clip` names the animation clip the move plays (by GUID). **Per-frame
  hitboxes** replace the move's own on the ticks that frame covers in the
  clip: `MoveSpec::to_data` turns them into `MoveData::frames`
  (`TickBox` ranges) and `Attack::hitbox` uses `MoveData::shape_at`.
- **Hurtboxes** per frame are stored and drawn in the editor; the
  simulation still takes hurtboxes from bodies (see [Known
  limitations](#known-limitations)).
- Hitstop, health and hit shapes are generic building blocks in
  `reign_combat`; which moves exist and what they mean is the game's.

The editor's Animation tool edits the boxes on its preview.

## Known limitations

- **Hurtboxes** in move sets are editor data only; the simulation takes
  hurtboxes from bodies. Wiring per-frame hurtboxes into `Attack`
  resolution is not done.
- **Prefab variants** (a prefab whose root is an instance of another) are
  not supported; such a file fails to load with a clear error.
- The Animation tool asks for the **body height** to place boxes; a move
  set (or prefab) does not carry it yet.
- The editor works on **one open scene at a time** (`EditorApp::scene`);
  there is no multi-scene editing.

## Camera follow

Set a Camera's `follow` to an object's exact name. It follows the first active
match in object ID order. A `smoothing` of 1 snaps to the target, while 8
closes one eighth of the gap each tick. Enable `bounded` and set `min` and
`max` to limit the camera centre in world coordinates. The highest active
`priority` supplies the Game view. Ties use the lowest object ID, and other
active cameras keep following their targets.
