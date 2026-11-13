# Reign Engine

A deterministic 2D engine in Rust (wgpu + winit), with a Unity-style editor:
scenes of game objects and components, reflection-driven Inspector, Project
browser with hot reload, Tilemap painter, Sprite slicer and Animation editor.

## Layout

| Path | What it is |
| --- | --- |
| `engine/reign` | The facade crate: one dependency that re-exports every engine crate. |
| `engine/reign_core` | Fixed-point math, input, clock, RNG: the deterministic simulation primitives. |
| `engine/reign_scene`, `reign_derive` | Scenes, game objects, components, reflection, scene files. |
| `engine/reign_behaviour` | Per-object scripts run in fixed stages, with messages. |
| `engine/reign_assets` | GUIDs, `.meta` files, handles, prefabs, hot reload, packing. |
| `engine/reign_render` | wgpu renderer: 640x360 pixel target, integer upscale, post effects. |
| `engine/reign_app` | Window, event loop, fixed-tick loop, gamepads. |
| `engine/reign_physics`, `reign_combat`, `reign_audio`, `reign_net` | Platformer physics, frame-data combat, audio mixing, rollback netcode. |
| `engine/reign_editor` | The editor shell (egui). |
| `app` | `reign-engine`: the editor around a project folder, with no game attached. |
| `hub` | `reign-hub`: Reign Hub, the launcher that creates, lists and opens projects. |
| `engine/reign_build` | Project tooling with no dependencies: registers `src/components/` from a project's `build.rs`, writes project and component templates, finds code editors. |

See [docs/ENGINE.md](docs/ENGINE.md) for the scene/component/asset model.

## Reign Hub

`scripts/bundle-macos.sh` builds `dist/Reign Hub.app`, which holds both the
hub and the editor. Open it to make a new project, add an existing folder, or
open a project (double-click or Open) in the editor. The project list is kept
in `~/Library/Application Support/Reign Engine/hub.ron`. A project folder is
marked by `ReignProject.ron` (name and engine version).

For UI checks without a screen, `REIGN_HUB_SNAPSHOTS=<dir> reign-hub` plays a
fixed script (intro frames, each page, the New project dialog), saves PNGs of
the hub's window into `<dir>` and quits. Fonts are Bebas Neue and Inter, both
under the SIL Open Font License (`hub/assets/fonts/`).

### Updates

The hub checks the GitHub repository it was built from for a newer release
and, when there is one, shows a green arrow beside the engine version. On
Windows the arrow downloads `reign-windows-x64.zip` from that release (it must
hold `reign-hub.exe` and `reign-engine.exe` at its root), swaps the programs
in place and restarts the hub; elsewhere it opens the release page. Release
tags are versions like `v0.2.0`. A fork follows its own releases by building
with `REIGN_UPDATE_REPO=owner/name` (or changing `DEFAULT_REPO` in
`hub/src/update.rs`). `REIGN_NO_UPDATE_CHECK=1` turns the check off, and
`REIGN_UPDATE_FAKE=9.9.9` shows the arrow without a release, for UI checks.

To ship an update, bump `version` in `Cargo.toml`, commit, then push a tag
that matches it (`git tag v0.2.0 && git push origin v0.2.0`).
`.github/workflows/release.yml` builds the Windows zip and publishes the
release; it refuses a tag that doesn't match the crate version.

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
`AudioSource`, `TilemapRenderer`, and the physics components below. Every
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
`fixed_update` (120 Hz), `late_update` and `on_message`. Files start with
`use reign_engine::prelude::*;`, which brings in the engine types plus
shortcuts such as `ctx.get::<T>(me)`, `ctx.translate(me, delta)`,
`ctx.axis_x()` and `ctx.pressed(Button::Jump)` (arrows/WASD, Space, J/K/L).

**Input** is a list of named actions in `assets/input.ron` (up to 10),
each bound to keys and gamepad buttons; edit them in **Scripts → Input
Actions…** (press "+ key" and then the key). Scripts read
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

**Scenes:** `ctx.load_scene("scenes/level2.scene.ron")` moves to another
scene when the tick ends. Input, the tick count and the random numbers
carry on; the new scene's Behaviours start fresh.

When code changes and the editor is in front, it rebuilds in the
background (output in the Console), saves the scene, and reopens on it.
Ctrl/Cmd+R does it by hand. Reign Hub builds a project with code before
opening it; the first build compiles the engine and takes a few minutes.
Pick the code editor (VS Code, RustRover, Cursor, Zed, Sublime Text) in the
hub's About page or Scripts → Code editor. Projects build against this
checkout (`REIGN_ENGINE_SOURCE` overrides it).

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
panel to paint it in the Tilemap tab. Slopes collide as solid blocks for now.

Create objects from the toolbar's **GameObject** menu or by right-clicking
the Hierarchy (on an object to make a child). Shortcuts: Ctrl/Cmd+Shift+N
(or Ctrl/Cmd+N) for an empty object, Alt+Shift+N for an empty child of the
selection, Ctrl/Cmd+I (or F2, or double-click) to rename the selection in
the Hierarchy (Enter keeps it, Esc cancels). New objects appear in the middle of the view. Dragging a PNG or
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
into `assets.rpak` and writes `game.ron` (title and start scene) beside
it, inside `Contents/Resources` in the app. A game needs the project's
own crate, so a project without code is set up as one first, and a
project made before export existed gets `src/game.rs` and its `[[bin]]`
added. The first release build compiles the engine and takes a few
minutes.

The game plays `scenes/main.scene.ron` (else the first scene) with the
same simulation and drawing as Play mode. F11 toggles fullscreen.
Flags: `--scene PATH`, `--fullscreen`, `--exit-after SECONDS`, and a
project folder to play its loose assets instead of a pack (with no pack
beside it, `cargo run --bin <crate>-game` plays the project it was built
in). Without the editor, `reign-engine PROJECT_DIR --export OUT_DIR` does
the same export from the command line.

## Without the window: tools and AI agents

Three commands open no window and print one JSON document on stdout, so
scripts, CI and AI coding agents can work on a project:

```bash
reign-engine PROJECT_DIR --check                 # do all scenes and prefabs load? exit 1 if not
reign-engine PROJECT_DIR --describe              # components, fields, defaults, inputs, scenes, assets
reign-engine PROJECT_DIR --simulate 240 --input "Right,Jump@30-45" --watch Player
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
