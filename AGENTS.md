# Working on Reign Engine (for AI agents)

Reign is a deterministic 2D engine in Rust with a Unity-style editor. This
file is for agents changing **the engine**. A game project made with Reign
has its own `AGENTS.md` (written from
`engine/reign_build/templates/AGENTS.md`) about making a game.

Read [README.md](README.md) for what the engine does,
[docs/ENGINE.md](docs/ENGINE.md) for the scene/component/asset model and
[docs/ROADMAP.md](docs/ROADMAP.md) for what's done and what's next.

## Commands

```bash
cargo build                                   # the editor (app) and the hub
cargo test --workspace --all-features         # everything CI runs
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all
cargo test -p reign_engine --lib headless     # one module's tests
```

CI fails on clippy warnings and on unformatted code. Run both before you
finish.

## Seeing what you changed without a window

The editor is a GUI, so use the headless commands to check your work. Each
prints one JSON document on stdout; logs go to stderr.

```bash
cargo run -p reign_engine -- PROJECT_DIR --check
cargo run -p reign_engine -- PROJECT_DIR --describe
cargo run -p reign_engine -- PROJECT_DIR --simulate 240 --input "Right,Jump@30-45" --watch Player
```

- `--check` loads every scene and prefab as the game would. Exit code 1
  and an `errors` list if anything fails.
- `--describe` lists every component with its fields, kinds, defaults,
  ranges and doc comments, plus input actions, scenes and assets.
- `--simulate TICKS` plays the start scene (or `--scene ASSET_PATH`) at 120
  ticks a second and prints every object's position and component values at
  the end. `--every N` adds a snapshot every N ticks; `--watch A,B` keeps
  only those objects. `state_hash` must be identical between two runs with
  the same input: if it isn't, something broke determinism.

`PROJECT_DIR` can be an empty folder: opening it makes a project there. Use
your scratch directory, not the repository.

A project with its own components must be run through its own editor
binary (`cargo run --bin <crate>-editor -- . --check` inside the project),
because `reign-engine` only knows the built-in components.

The hub can save PNGs of its own UI: `REIGN_HUB_SNAPSHOTS=<dir> reign-hub`.
The editor can't yet (see the roadmap).

## Rules that aren't obvious from the code

- **Determinism.** Simulation state (anything in a component or `World`) has
  no floats, no `HashMap`/`HashSet`, and is `Clone + Eq + Hash`. Use `Fx` and
  `FxVec2` for positions and speeds, `Vec`/`BTreeMap` for collections.
  Floats are fine in drawing and editor code.
- **World y grows downwards.** Negative y velocity is up.
- **Presentation never feeds the simulation.** Sounds leave a tick as
  `Event`s (`ctx.emit`); the mixer is outside rollback. Animation state is
  the opposite: an `Animator`'s clock is in the component, so frames and
  their events replay exactly.
- **Every host ticks through `runtime::tick`.** The editor, the exported
  game and `--simulate` all call it, so a feature added there (like
  `ctx.load_scene`) works the same everywhere.
- **Speeds are per second.** Divide by `TICKS_PER_SECOND` (120) for one tick.
- **Component names are a file format.** Renaming a component or a field
  breaks saved scenes; add `#[serde(default)]` to structs so new fields load
  from old files.
- **Scene files are RON** with `Fx` values as quoted decimal strings
  (`"1.5"`, never `1.5`). See ENGINE.md.
- **Windows, macOS and Linux all matter.** Paths from `canonicalize()` carry
  a `\\?\` prefix on Windows that Cargo can't read; a running `.exe` can't
  be overwritten, only renamed. Both have bitten this project.
- **Templates are compiled by the tests.** `engine/reign_build/templates/*.rs`
  are what users get from Scripts → New Component; change them with the
  API, and keep them readable for beginners.
- **Docs are part of the change.** A new script call goes in the README's
  "Writing components" section and in the project `AGENTS.md` template.

## Where things are

| You want to change | Look in |
| --- | --- |
| What scripts can call (`ctx.…`) | `app/src/runtime.rs` (`CtxExt`), `app/src/lib.rs` (`prelude`) |
| Built-in components | `app/src/components.rs`, `app/src/physics.rs` |
| Animation playback, scene changes, sound events | `app/src/runtime.rs` (`animation_system`, `tick`, `Event`) |
| Sound output | `app/src/audio.rs`, `engine/reign_audio` |
| The built-in font | `app/src/font.rs` (installed by `app/src/starter.rs`) |
| How a frame is drawn | `app/src/view.rs`, `engine/reign_render` |
| The tick's order | `app/src/runtime.rs` (`schedule`), `engine/reign_behaviour` |
| Headless commands | `app/src/headless.rs`, flags in `app/src/lib.rs` |
| New projects and component templates | `engine/reign_build/src/project.rs`, `engine/reign_build/templates/` |
| Script reload and Export Game | `app/src/code.rs`, `app/src/export.rs` |
| Editor panels | `engine/reign_editor/src/` |
| The project launcher | `hub/src/main.rs` |

## Before you say it's done

1. `cargo test --workspace --all-features` passes.
2. `cargo clippy --workspace --all-targets --all-features -- -D warnings` is clean.
3. You ran the feature, with `--simulate` or `--check` if it has no window.
4. You say plainly what you could not check (anything that needs clicking
   in the editor).
