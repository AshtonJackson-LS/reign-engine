# Reign Engine

A deterministic 2D engine in Rust (wgpu + winit), with a Unity-style editor:
scenes of game objects and components, reflection-driven Inspector, Project
browser with hot reload, Tilemap painter, Sprite slicer, Animation editor and
Particle editor.

## Download

- **Windows:** [ReignSetup.exe](https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/ReignSetup.exe)
  ([ARM64](https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/ReignSetup-arm64.exe))
  installs Reign Hub for your user (Start menu, Desktop, and Uninstall in
  Settings → Apps; no administrator rights). Windows may say "Windows
  protected your PC": click **More info → Run anyway**.
- **macOS** (Apple Silicon and Intel): [reign-macos.zip](https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/reign-macos.zip).
  Move **Reign Hub** to Applications, then right-click it → **Open** the first time.
- **Linux:** [x64](https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/reign-linux-x64.tar.gz)
  or [ARM64](https://github.com/AshtonJackson-LS/reign-engine/releases/latest/download/reign-linux-arm64.tar.gz).
  Unpack and run `./reign-hub`.

Reign Hub keeps itself up to date: it installs new releases in the
background and shows a green arrow to restart into them. Turn on **Beta
updates** in its Settings to get each change as soon as it's merged. Writing your own
components needs [Rust](https://rustup.rs).

Making releases: see [RELEASING.md](RELEASING.md).

## Layout

| Path | What it is |
| --- | --- |
| `engine/reign` | The facade crate: one dependency that re-exports every engine crate. |
| `engine/reign_core` | Fixed-point math, input, clock, RNG: the deterministic simulation primitives. |
| `engine/reign_scene`, `reign_derive` | Scenes, game objects, components, reflection, scene files. |
| `engine/reign_behaviour` | Per-object scripts run in fixed stages, with messages. |
| `engine/reign_assets` | GUIDs, `.meta` files, handles, prefabs, hot reload, packing. |
| `engine/reign_render` | wgpu renderer: 640x360 pixel target, integer upscale, post effects. |
| `engine/reign_particles` | Particle systems: plays and draws `*.particles.ron` effects. |
| `engine/reign_app` | Window, event loop, fixed-tick loop, gamepads. |
| `engine/reign_physics`, `reign_combat`, `reign_audio`, `reign_net` | Platformer physics, frame-data combat, audio mixing, rollback netcode. |
| `engine/reign_editor` | The editor shell (egui). |
| `app` | `reign-engine`: the editor around a project folder, with no game attached. |
| `hub` | `reign-hub`: Reign Hub, the launcher that creates, lists and opens projects. Also `reign-release`, which signs releases. |
| `installer` | `ReignSetup.exe`, the Windows installer, and the install/uninstall steps the hub shares. |
| `engine/reign_build` | Project tooling with no dependencies: registers `src/components/` from a project's `build.rs`, writes project and component templates, finds code editors. |

See [docs/ENGINE.md](docs/ENGINE.md) for the scene/component/asset model.

## Reign Hub

`scripts/bundle-macos.sh` builds `dist/Reign Hub.app` (universal: Apple
Silicon and Intel), which holds both the hub and the editor. Open it to make a new project, add an existing folder, or
open a project (double-click or Open) in the editor. The project list is kept
in `~/Library/Application Support/Reign Engine/hub.ron`. A project folder is
marked by `ReignProject.ron` (name and engine version).

For UI checks without a screen, `REIGN_HUB_SNAPSHOTS=<dir> reign-hub` plays a
fixed script (intro frames, each page, the New project and Engine version
dialogs), saves PNGs of the hub's window into `<dir>` and quits. Fonts are
Bebas Neue and Inter, both under the SIL Open Font License
(`hub/assets/fonts/`).

### Installing, updates and engine versions

On Windows, `ReignSetup.exe` (the `installer` crate) installs the hub and
editor to `%LOCALAPPDATA%\Programs\Reign Engine`, with shortcuts and a
Settings → Apps entry whose Uninstall runs `reign-hub.exe --uninstall`.
`update-reign.bat` builds it from your checkout and installs it.

When the hub starts it asks its repository's GitHub Releases for a newer
version. If there is one, it downloads this platform's archive
(`PACKAGE` in `hub/src/update.rs`), checks its signature against the public
key built into the hub (`hub/src/signing.rs`), swaps the new programs in
beside itself and shows a green arrow that restarts into them. Settings →
**Install updates automatically** off makes it wait for a click on the arrow
instead. **Beta updates** on also takes betas (`v0.3.0-beta.4`), which are
built from every merge to `main`. Debug builds and builds inside a Cargo
`target` folder never update on their own. `REIGN_NO_UPDATE_CHECK=1` turns
the check off; `REIGN_UPDATE_FAKE=9.9.9` shows the arrow, for UI checks.
While an update (or an engine version from **Engine version…**) downloads, a
window shows a bar, how much has arrived of the whole, the speed and the time
left, with **Cancel**; closing it doesn't stop the download, and the green
arrow brings it back. When an update has installed, a window offers **Restart
now** or **Later**. `REIGN_DOWNLOAD_BASE=http://127.0.0.1:8765` takes release
files from `<base>/<tag>/<file>` instead of GitHub, to try the window against
a slow local server (the signature check still applies, so an unsigned file is
never installed).
How releases are made, and forks: [RELEASING.md](RELEASING.md).

Each project records its engine version (••• → **Engine version…** changes
it, from the repository's releases). A project without code opens in that
version's editor, downloaded into `versions/` beside the hub when it isn't
the hub's own. A project with code builds its editor against what its
Cargo.toml names: a release's git tag, or a local engine checkout.

The hub starts a project's editor with the project folder as its only
argument. A game that writes its own editor binary around
`reign::editor::run_editor` and reads its own flags should read them from
`reign::editor::game_args()`, which leaves that folder out; otherwise the
editor exits on an argument it doesn't know.
If an editor the hub started exits with an error, the hub's status bar
says so with the most telling line of its output (it has no console of its
own); the rest goes to the hub's log.

The hub (and the editor's Reload and Export Game) builds a project with
plain `cargo build --bin <crate>-editor` / `--bin <crate>-game`. A project
that keeps its editor behind a Cargo feature names the features in its
`Cargo.toml`, and Cargo is asked where the build lands, so a project inside
a larger workspace or with its own `target-dir` works too:

```toml
[package.metadata.reign]
editor-features = ["editor"]   # what the hub and Reload build the editor with
game-features = []             # what Export Game builds the game with
```

## Running the editor directly

```bash
cargo run --release -- ~/ReignProjects/MyGame
```

or build the macOS app into `dist/`:

```bash
scripts/bundle-macos.sh
```

A project is a folder with an `assets/` directory; scenes are
`assets/scenes/*.scene.ron`, and `main.scene.ron` opens at startup (an empty
one with a camera is made if missing). With no folder given, the editor uses
`~/ReignProjects/Default`.

The standalone editor's built-in components: `Camera` (centres the Game
view), `SpriteRenderer` (one sprite of a sprite sheet, with a tint colour and
an optional stretched size), `Image` (a whole texture), `Animator`, `Text`,
`AudioSource`, `ParticleSystem`, `TrailRenderer`, `TilemapRenderer`, and the physics
components below. Every
project gets `assets/Reign/Shapes` (Square, Circle, Triangle, Diamond,
Hexagon, Capsule, Rounded Square) and `assets/Reign/Font`, a 5 by 7 pixel
font, with fixed GUIDs, so they work the same in any project.

## Writing components

Components are Rust, like Unity's C# scripts. In the editor, **Scripts → New
Component…** asks for a name and writes `src/components/<name>.rs` from a
template, opens it in your code editor, and rebuilds. The first time, it
also turns the project into a Rust crate:

```text
MyGame/
  Cargo.toml        the crate; its binary is MyGame's own editor
  build.rs          reign_build::components(): registers src/components/
  src/main.rs       reign_engine::run(my_game::scripts())
  src/lib.rs        includes the generated registration
  src/components/   one file per component: add files, nothing else to list
```

The dialog starts from a template: a blank **Behaviour**, plain **Data**,
**Platformer Movement** (run and jump on a Rigidbody, with acceleration,
coyote time, jump buffering and short hops) or **Top-down Movement**
(eight-way, even on diagonals, stopped by walls when it has a Rigidbody).
The templates live in `engine/reign_build/templates/` and the engine's
tests compile and play them as-is. A Rigidbody's `grounded` flag says
whether it's standing on something.

A component is a `#[derive(Component)]` struct (its public fields show in
the Inspector); `impl Behaviour<World> for It` gives it `start`,
`fixed_update` (120 Hz), `late_update` and `on_message`, and `gizmos` to draw
helper lines in the Scene view (below). Files start with
`use reign_engine::prelude::*;`, which brings in the engine types plus
shortcuts such as `ctx.get::<T>(me)`, `ctx.translate(me, delta)`,
`ctx.axis_x()` and `ctx.pressed(Button::Jump)` (arrows/WASD, Space, J/K/L).

**Gizmos** (Unity's `OnDrawGizmos`) are lines a component draws in the editor's
Scene view while the game is stopped, to show what it does: a range, a patrol
path, an aim direction. Implement `fn gizmos(scene: &Scene, me: ObjectId, g: &mut Gizmos)`
in the component's `impl Behaviour<World>` and draw with `g.set_color(Rgba8::rgb(..))`,
`g.line(a, b)`, `g.polyline(&points, closed)`, `g.rect(x, y, w, h)`, `g.circle(center, radius)`,
`g.cross(center, half)` and `g.arrow(from, to)`, all in world pixels. Gizmos are one
screen pixel wide at any zoom, are never drawn in Play or an exported game, and can't change
the scene. The editor also shows each camera's view and a marker at each particle emitter;
the editor's debug panel has "Show gizmos" and "Show tile grid" checkboxes.

**Input** is a list of named actions in `assets/input.ron` (up to 10),
each bound to keys, mouse buttons and gamepad buttons and sticks; edit them
in **Scripts → Input Actions…** (or **Edit bindings…** in the Input section
of **Project → Project Settings…**, which lists them): press **+** and then the key, pad button
or stick direction (or click a mouse button in the box), or click a binding
to change it. An input belongs to one action: binding it elsewhere moves it,
and changing a binding to another action's swaps the two. Scripts read
`ctx.action("Jump")`, `ctx.action_pressed("Interact")`,
`ctx.action_buffered(..)` and `ctx.axis_x()`. The simulation only ever sees
which actions are held each tick, so rebinding never affects replays.

**Collisions** reach a Behaviour's `on_message` as
`Message::CollisionEnter(other)` / `CollisionExit` (solid colliders touching)
and `TriggerEnter` / `TriggerExit` (a collider with `is_trigger` overlapping
something); `ctx.touching(me)` lists current contacts. BoxCollider `layer`
(0-31) and `mask` choose what interacts with what. One of each pair needs a
Rigidbody.

**Animation:** an `Animator` on an object with a `SpriteRenderer` plays an
animation clip (`*.anim.ron`, made in the Animation tab): each frame replaces
the sprite that's drawn. List clips by name in its `clips` and switch with
`ctx.play_animation(me, "run")`, which is safe to call every tick.
`speed` is a percentage. A frame's event arrives in `on_message` as a
message with that name (`msg.body.is("step")`), and a clip that doesn't
loop sends `"AnimationFinished"`; `ctx.animation_finished(me)` asks the
same.

**Text:** GameObject → 2D Object → Text. A `Text` draws with the built-in
font (or any `*.font.ron`), with a colour, a whole-pixel `scale`, left,
centre or right alignment, and `screen` to pin it to the view (a score)
instead of the world. `ctx.set_text(me, format!("Score: {score}"))`
changes it.

**Sound:** `ctx.play_sound("sounds/jump.wav")` plays a 16-bit PCM `.wav`
from `assets/`; `ctx.play_music("music/theme.wav")` loops one and
`ctx.stop_music()` fades it out. An `AudioSource` gives an object a sound
that plays when it appears (`play_on_start`) or on `ctx.play(me)`. Sounds
are presentation: they never feed back into the simulation, and Stop
silences them.

**Particles:** make an effect with **+ Create > Particle Effect** in the
Project panel (a `*.particles.ron`) and edit it in the **Particles** tab,
which plays it live as you change it, Unity style: a main module (duration,
looping, start lifetime, speed, size and colour) and modules you switch on
for emission (a rate and timed bursts), shape (point, cone, circle, box or
line), forces (gravity, drag, wind), velocity, colour and size over each
particle's life, collision (bounce off, stick to or die on a floor and the
level's solid tilemaps), sub-emitters (another effect where a particle is
born, hits something or dies), lights, and drawing (squares, streaks along
the way they move, soft glows or sprites from a sheet, blended normally or
added as light, in a draw order). A `ParticleSystem` component plays one at its object,
following it and mirroring with it: with `play_on_start` when the object
appears, or on `ctx.play_particles(me)`; `ctx.stop_particles(me)` stops it
emitting. Particles are drawn over the scene and are presentation only,
like sounds; Play, Stop and a new scene start them afresh.

A `TrailRenderer` (Unity's Trail Renderer) draws a ribbon behind a moving object: every tick the
object's position is added, and points older than `time` seconds fade away. The ribbon goes from
`start_width` and `start_color` at the object to `end_width` and `end_color` at the oldest point
(`additive` adds its colour like light), a new point comes every `min_vertex_distance` pixels, and
`max_points` caps its length. It draws at the object's `z`, behind the object. Unlike particles the
points are game state (whole subpixels), so a trail replays exactly and is in `state_hash`.
`ctx.clear_trail(me)` empties it after a teleport.

**Finding and spawning:** `ctx.find("Player")` is the first object with
that name (look it up in `start` and keep the id). `ctx.spawn("prefabs/bullet.prefab.ron", at)`
makes a copy of a prefab at a world position and returns its id straight
away, so you can set it up on the same tick; its Behaviours `start` before
their first update. A wrong path gives `None` and a warning in the Console.

**Tags:** every object can have one tag, picked under its name in the
Inspector (`tag: "Enemy"` in a scene file). The menu lists the project's
tags (a new project has Player and Enemy). To make one, choose **New tag…**,
type it and press Enter; it joins the list for every object. Project
Settings can rename or remove them. `ctx.find_with_tag("Player")`
is the first object with it, `ctx.find_all_with_tag("Enemy")` all of them,
`ctx.has_tag(other, "Enemy")` asks about one (handy in `on_message` when
something hits you) and `ctx.set_tag(id, "Dead")` changes it. Copies of a
prefab get its root's tag.

**Scenes:** `ctx.load_scene("scenes/level2.scene.ron")` moves to another
scene when the tick ends. Input, the tick count and the random numbers
carry on; the new scene's Behaviours start fresh.

When code changes and the editor is in front, it rebuilds in the
background (output in the Console), saves the scene, and reopens on it.
Ctrl/Cmd+R does it by hand. Reign Hub builds a project with code before
opening it; the first build compiles the engine and takes a few minutes.
While it builds, the hub's status bar shows a bar and `141 / 324` (cargo's
own count of finished build steps), and the editor's toolbar shows the same
as `Compiling 141 of 324` while it rebuilds or exports.
The hub's status bar has a **Cancel** button for a build in progress; it stops
cargo and the compilers it started, and the project can be opened again later.
Pick the code editor (VS Code, RustRover, Cursor, Zed, Sublime Text) in the
hub's About page or Scripts → Code editor. Projects build against this
checkout (`REIGN_ENGINE_SOURCE` overrides it). Where there's no checkout (a
downloaded release), new projects fetch the engine from GitHub at the
release's tag instead (`reign = { git = "…", tag = "v0.2.0" }`).

Physics: add a `Rigidbody` (Dynamic falls and collides, Kinematic moves by
its velocity through everything, Static never moves; gravity scale, velocity
in px/s, bounciness %) and a `BoxCollider` (size 0 fits the sprite; offset).
A collider with no rigidbody is solid scenery. Bodies move a pixel at a time
on the 120 Hz tick in fixed-point, so runs are deterministic. Colliders show
as green outlines in the edit view. GameObject → 2D Object → Physics has a
ready-made Dynamic Box and Static Platform.

**Tilemaps:** GameObject → 2D Object → Tilemap makes a level piece that's
already solid: a `TilemapRenderer` (draws the map's layers) plus a
`TilemapCollider` (its collision layer joins scene physics: `#` solid, `-`
one-way platform, with the same layers, triggers and collision events as
BoxColliders). It writes a starter map to `assets/tilemaps/` and a small
built-in tileset to `assets/Reign/`; double-click the map in the Project
panel to paint it in the Tilemap tab. Switching to the Tilemap tab opens the
selected object's map, or else the scene's first one. Slopes collide as solid
blocks for now.

**Scene → New Scene…** (or **+ Create → Scene** in the Project panel) makes
a scene with a Main Camera and opens it; double-click a scene in the Project
panel to open it. When the open scene has unsaved changes, the editor asks
whether to save them first. A game with its own `EditorApp` gets an empty
scene unless it overrides `new_scene_text` with its own starting point.

Drag files or folders from Explorer, Finder or a Linux file manager onto the
editor window to copy them into the folder the Project panel shows. Taken
names get a number (`hero 1.png`), and each copy gets a fresh `.meta`.
(Linux under Wayland doesn't pass drops to the window yet; X11 does.)

Create objects from the toolbar's **GameObject** menu or by right-clicking
the Hierarchy (on an object to make a child). Shortcuts: Ctrl/Cmd+Shift+N
(or Ctrl/Cmd+N) for an empty object, Alt+Shift+N for an empty child of the
selection, Ctrl/Cmd+I (or F2, or double-click) to rename the selection in
the Hierarchy (Enter keeps it, Esc cancels). The Hierarchy's search box
lists only the objects whose names match. New objects appear in the middle of the view. Dragging a PNG or
sprite sheet from the Project panel into the view also places it. Gameplay
comes from a game crate that depends on `reign` and implements
`reign::editor::EditorApp` (`create_items` / `create` fill the Create menus).

Flags: `--play` starts in Play mode; `--exit-after SECONDS` quits on its own.

## Exporting a game

**Scripts → Export Game…** builds the game on its own (no editor) and
puts it in a folder you pick (`Builds/` in the project by default): on
macOS `<Project>.app`, elsewhere a `<Project>/` folder with the
executable. It saves the scene, builds the project's second binary,
`<crate>-game` (`src/game.rs`, which calls `reign_engine::play`), with
`--release` in the background (output in the Console), packs `assets/`
into `assets.rpak` and writes `game.ron` (title, start scene and window) beside
it, inside `Contents/Resources` in the app. A game needs the project's
own crate, so a project without code is set up as one first, and a
project made before export existed gets `src/game.rs` and its `[[bin]]`
added. The first release build compiles the engine and takes a few
minutes.

## Project Settings

**Project → Project Settings…** in the editor holds what describes the game
rather than a scene. Edit the fields, then **Save** (the icon is saved as soon
as it's chosen):

- **Name**: the game's window title and the name of what Export Game makes.
  It is `name` in `ReignProject.ron`, and the hub lists the project by it; the
  project's folder and crate keep their names.
- **Version** (`1.0.0`; one to four numbers) and **Author** (optional): put in
  the exported game's file details on Windows (Properties → Details) and its
  `Info.plist` on macOS.
- **Start scene**: what the exported game and `--simulate` play first and what
  the editor opens on. Automatic is `scenes/main.scene.ron`, else the first
  scene. Export Game stops if the chosen scene has been deleted.
- **Game window**: its size when the game starts (at least the 640 × 360
  view) and whether it starts fullscreen. F11 still switches while it runs.
- **Background**: the colour behind everything, where nothing is drawn, in
  the game and in the editor's views.
- **Collision layers**: names for the layers (0 to 31) colliders are on. The
  Inspector shows a collider's `layer` by name and its `mask` as a list of
  named layers to tick, with Everything and Nothing. Names are only for
  people: scenes keep the numbers, so renaming a layer changes nothing in the
  game. Your own components can show a field this way with
  `#[reflect(layer)]` (a `u8`) or `#[reflect(layer_mask)]` (a `u32`).
- **Tags**: the tags the Inspector's tag menu offers. Taking one off the
  list leaves objects that have it alone; `--check` then warns about them.
- **App icon**: see below.

All but the name and icon are kept in `ProjectSettings.ron` beside
`ReignProject.ron`, a RON file you can also edit by hand (a missing field takes
its default). `--describe` prints the settings and `--check` reports any that
can't be used.

## Project icons

A project's windows (its editor and its game) and its exported game show an
icon. With none of its own, a project gets the Reign project icon (the hub's
"R", inverted: dark on crimson). To use your own, open **Project → Project
Settings…** in the editor and choose an image (it is copied into the project
as **`icon.png`**, next to `ReignProject.ron`; you can also put the file there
yourself). **Use the default** removes it again. The editor's window shows the
change at once. The image must be:

- a **PNG** (JPEG, GIF and `.ico` are not accepted),
- **square**,
- **64 to 1024 pixels** on a side (256 or 512 is a good choice; transparency
  is kept), and
- no larger than **8 MB**.

An image that breaks a rule is refused with the reason in plain words (and one
that is already in the project is skipped and logged). Export Game puts the
icon in the game: `icon.png` beside the game (its window icon), the icon of the `.exe`
file on Windows, and `AppIcon.icns` in the macOS app. The project's own icon
always wins; the default only fills a gap, so a game that already embeds an
icon of its own in its program keeps it.

The game plays its start scene (Project Settings; by default
`scenes/main.scene.ron`, else the first scene) with the same simulation and
drawing as Play mode, in the window Project Settings describes. F11 toggles fullscreen.
Flags: `--scene PATH`, `--fullscreen`, `--exit-after SECONDS`, and a
project folder to play its loose assets instead of a pack (with no pack
beside it, `cargo run --bin <crate>-game` plays the project it was built
in). Without the editor, `reign-engine PROJECT_DIR --export OUT_DIR` does
the same export from the command line.

## Without the window: tools and AI agents

These commands open no window and print one JSON document on stdout, so
scripts, CI and AI coding agents can work on a project:

```bash
reign-engine PROJECT_DIR --check                 # do all scenes and prefabs load? exit 1 if not
reign-engine PROJECT_DIR --describe              # components, fields, defaults, inputs, scenes, tags, assets
reign-engine PROJECT_DIR --simulate 240 --input "Right,Jump@30-45" --watch Player
reign-engine PROJECT_DIR --new-scene scenes/level2.scene.ron   # a scene with a Main Camera
```

`--simulate TICKS` plays the start scene (or `--scene scenes/x.scene.ron`)
at 120 ticks a second, exactly as Play does, and prints every object's
position and component values at the end. `--input` holds actions: `Name`
for the whole run, `Name@TICK` for one tick, `Name@FROM-TO` for a range.
`--every N` adds a snapshot every N ticks, `--watch A,B` keeps only those
objects, `sounds` lists every sound played and on which tick, and
`state_hash` is the same for the same scene, code and input every time.
Animation frames and text show up as the `Animator`'s `time` and the
`Text`'s `text` in each object's components.

A project with code runs them through its own editor, which knows its
components: `cargo run --bin <crate>-editor -- . --check`.

Every project gets an `AGENTS.md` explaining its layout, these commands,
the scene format and how to write a component.
[AGENTS.md](AGENTS.md) here is the same for working on the engine, and
[docs/ROADMAP.md](docs/ROADMAP.md) lists what's done and what's next.

## License

Reign Engine is licensed under the [Mozilla Public License 2.0](LICENSE)
(MPL-2.0). MPL is copyleft per file: if you change the engine's files and
ship them, share those changes under MPL-2.0. Your game's own code and assets
stay yours, under any license you like. The hub's fonts (Bebas Neue, Inter)
are under the SIL Open Font License; see `hub/assets/fonts/`.

## Camera follow

Set a Camera's `follow` to an object's exact name. It follows the first active
match in object ID order. A `smoothing` of 1 snaps to the target, while 8
closes one eighth of the gap each tick. Enable `bounded` and set `min` and
`max` to limit the camera centre in world coordinates. The highest active
`priority` supplies the Game view. Ties use the lowest object ID, and other
active cameras keep following their targets.
