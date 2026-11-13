# {project}: a Reign Engine project (for AI agents)

This folder is a game made with Reign Engine, a deterministic 2D engine in
Rust with a Unity-style editor. Everything in it is text you can read and
edit; nothing needs the editor's window.

The engine's source is at `{engine}`. Its `README.md` and `docs/ENGINE.md`
are the full reference.

## Layout

```text
ReignProject.ron        marks the folder as a project (name, engine version)
assets/                 everything the game loads
  scenes/*.scene.ron    scenes; main.scene.ron is where the game starts
  input.ron             input actions and their keys (optional; has defaults)
  *.prefab.ron, *.tilemap.ron, *.sheet.ron, *.anim.ron, *.png, *.wav
  *.meta                one beside every asset: its GUID. Never delete or
                        hand-write these; they're made when the project opens.
Cargo.toml, build.rs    only once the project has code
src/components/*.rs     one component per file, registered automatically
src/main.rs, game.rs    the project's editor and its game; don't edit
```

## Commands

Each prints one JSON document on stdout (logs go to stderr).

With code (`Cargo.toml` exists), run them through the project's own editor,
so your components are known. `<crate>` is the package name in `Cargo.toml`:

```bash
cargo build --bin <crate>-editor                      # compile; errors name file and line
cargo run --bin <crate>-editor -- . --check           # do all scenes and prefabs load?
cargo run --bin <crate>-editor -- . --describe        # components, fields, defaults, inputs, assets
cargo run --bin <crate>-editor -- . --simulate 240    # play 2 seconds, print every object
```

Without code, use the engine's editor instead:

```bash
cargo run --manifest-path "{engine}/Cargo.toml" -p reign_engine -- . --check
```

`--simulate TICKS` runs at 120 ticks a second. Options:

- `--scene scenes/level2.scene.ron`: a scene other than the start scene.
- `--input "Right,Jump@30-45"`: actions to hold. `Name` holds it the whole
  time, `Name@30` presses it on tick 30, `Name@30-45` holds it from 30 to 45.
- `--every 60`: also print the objects every 60 ticks.
- The output's `sounds` lists each sound with the tick it played on, so you
  can check a jump made its noise without hearing it.
- `--watch Player,Coin`: only these objects, by name.

The first build compiles the engine and takes a few minutes. After that
it's seconds.

**The loop to work in:** edit, `--check`, `--simulate` with the input that
exercises what you changed, read the positions and component values, repeat.
Run `--describe` first: it tells you every component and field that exists,
so you never have to guess a name.

## Scenes

A scene is RON. Objects are listed parents before children.

```ron
(
    version: 1,
    objects: [
        (id: (0, 0), name: "Main Camera", components: {"Camera": ()}),
        (
            id: (1, 0),
            name: "Player",
            transform: (position: (x: "0", y: "-40"), z: 1),
            components: {
                "SpriteRenderer": (sheet: "524549474e0053484150455300000002", sprite: "square", size: (16, 24)),
                "BoxCollider": (size: (16, 24)),
                "Rigidbody": (),
                "PlatformerMovement": (),
            },
        ),
        (
            id: (2, 0),
            name: "Ground",
            transform: (position: (x: "0", y: "100")),
            components: {
                "SpriteRenderer": (sheet: "524549474e0053484150455300000002", sprite: "square", size: (320, 16)),
                "BoxCollider": (size: (320, 16)),
            },
        ),
    ],
)
```

- `id` is `(index, generation)`. Give each object the next unused index and
  generation 0. A child names its parent: `parent: Some((1, 0))`.
- **Fixed-point numbers are quoted decimal strings**: `x: "1.5"`. Unquoted
  `1.5` is an error. Integers such as sizes are plain numbers.
- Leave out any field to get its default; `"Rigidbody": ()` is all defaults.
- Positions are in pixels. The view is 640 by 360. **y grows downwards.**
- Higher `z` draws on top.
- Assets are named by GUID (32 hex digits), from `--describe`'s `assets`
  list or the asset's `.meta` file. `524549474e0053484150455300000002` is the
  built-in shapes sheet: sprites `square`, `circle`, `triangle`, `diamond`,
  `hexagon`, `capsule`, `rounded_square`. Tint with
  `color: (r: 255, g: 80, b: 80, a: 255)`.
- Text: `"Text": (text: "Score: 0", scale: 2, screen: true)` draws in the
  built-in font; `screen: true` pins it to the view, counting from its
  top-left corner. `align` is `Left`, `Center` or `Right`.
- Animation: `"Animator": (clip: "<anim GUID>", clips: [(name: "run", clip:
  "<anim GUID>")])` beside a `SpriteRenderer`. Clips are `*.anim.ron`:
  `(sheet: "<sheet GUID>", row: 0, mode: Loop, frames: [(cell: 0, ms: 100),
  (cell: 1, ms: 100)])`. A frame's `cell` is a grid cell in that row, or the
  sheet's nth sprite if the sheet has no grid.
- Sound: `"AudioSource": (clip: "<wav GUID>", volume: 100, play_on_start: true)`.
  Sounds must be 16-bit PCM `.wav`.
- A `BoxCollider` is centred on the object. With a `Rigidbody` the object
  moves (Dynamic falls, Kinematic moves by its velocity, Static never
  moves); without one it's solid scenery.

## Components (scripts)

One file per component in `src/components/`. Adding a file is enough: it's
registered on the next build. If the project has no `Cargo.toml` yet, make
the first component from the editor (Scripts → New Component), which sets
the crate up.

```rust
{example}```

What a script can call (`ctx` is the `&mut Ctx`):

| Call | Does |
| --- | --- |
| `ctx.get::<T>(id)`, `ctx.get_mut::<T>(id)` | A component on an object |
| `ctx.position(id)`, `ctx.set_position(id, at)`, `ctx.translate(id, delta)` | World position, in pixels |
| `ctx.axis_x()`, `ctx.axis_y()` | -1, 0 or 1 from the Left/Right and Up/Down actions |
| `ctx.action("Jump")`, `ctx.action_pressed("Jump")` | Held; went down this tick |
| `ctx.action_buffered("Jump", 12)`, `ctx.consume_action("Jump")` | Pressed in the last 12 ticks; use it up |
| `ctx.touching(id)` | Objects it's in contact with |
| `ctx.send(id, Message::with("Hit", 3))`, `ctx.broadcast(Message::new("Reset"))` | Messages, received in `on_message` |
| `msg.body.is("Hit")`, `msg.body.value()`, `msg.from` | Reading a message |
| `ctx.commands.spawn("Name")`, `.insert(id, component)`, `.despawn(id)` | Changes applied at the end of the tick |
| `ctx.play_animation(id, "run")`, `ctx.animation_finished(id)` | Switch an `Animator` to a named clip (safe every tick) |
| `ctx.set_text(id, format!("Score: {n}"))` | Change what a `Text` says |
| `ctx.play_sound("sounds/jump.wav")`, `ctx.play(id)` | A sound by path in `assets/`; an object's `AudioSource` |
| `ctx.play_music("music/theme.wav")`, `ctx.stop_music()` | Looping music |
| `ctx.load_scene("scenes/level2.scene.ron")` | Go to another scene when this tick ends |
| `ctx.rng()` | The only allowed source of randomness |
| `ctx.tick()` | Ticks since the game started |

Animation events arrive in `on_message` as messages named after the
frame's event (`msg.body.is("step")`), and `"AnimationFinished"` when a clip
that doesn't loop ends.

Physics events arrive in `on_message` as `Message::CollisionEnter(other)`,
`CollisionExit`, `TriggerEnter` and `TriggerExit`. One of the two objects
needs a `Rigidbody`; a collider with `is_trigger: true` overlaps instead of
blocking.

### Rules

- **No floats** in components: use `Fx` (`Fx::from_px(120)`) and `FxVec2`.
  No `HashMap` or `HashSet`: use `Vec` or `BTreeMap`. Everything a script
  remembers lives in its component's fields.
- **Speeds are per second.** One tick's worth is `speed / TICKS_PER_SECOND`.
- **Up is negative y.** A jump is `velocity.y = -jump_speed`.
- Derive exactly `Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize,
  Component`, add `#[serde(default)]` and implement `Default`.
- Renaming a component or a field breaks scenes that use it. Update the
  scene files in the same change.
- To move something with physics, set its `Rigidbody`'s `velocity`; don't
  `translate` it, or it will pass through walls.

## Input

`assets/input.ron` holds up to 10 named actions with keys and gamepad
buttons. Without the file, the defaults are Left, Right, Up, Down, Jump,
Attack, Dash, Parry, Interact and Pause. `--describe` prints the ones in use.

## Before you say it's done

1. `--check` reports `"ok": true`.
2. `--simulate` shows the behaviour you meant (the player moved, landed,
   picked the thing up), not only that nothing crashed.
3. You say what you could not check without the window: how it looks and
   how it feels to play.

## Camera follow

Set a Camera's `follow` to an object's exact name. It follows the first active
match in object ID order. A `smoothing` of 1 snaps to the target, while 8
closes one eighth of the gap each tick. Enable `bounded` and set `min` and
`max` to limit the camera centre in world coordinates. The highest active
`priority` supplies the Game view. Ties use the lowest object ID, and other
active cameras keep following their targets.
