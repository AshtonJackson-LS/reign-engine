# Contributing to Reign Engine

## Building and testing

Reign builds with stable Rust (`rust-toolchain.toml`). The editor (`app`),
the editor shell (`engine/reign_editor`) and the hub need Rust 1.95 or
newer. On Linux, install `pkg-config`, `libudev-dev` and `libasound2-dev`
(gamepads and audio).

```bash
cargo build --workspace            # everything; plain `cargo build` builds only app and hub
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features
cargo deny check                   # licenses and advisories, see deny.toml
```

Format with `cargo fmt` (settings in `rustfmt.toml`). CI
(`.github/workflows/ci.yml`) runs clippy, the tests and a release build on
macOS, Linux and Windows, and `cargo deny`.

## Running

```bash
# The editor, on a project folder (default: ~/ReignProjects/Default)
cargo run --release -p reign_engine -- ~/ReignProjects/MyGame

# Reign Hub, the launcher. It starts the `reign-engine` binary next to it,
# or the one $REIGN_EDITOR names.
cargo build --release
target/release/reign-hub
```

`--play` starts the editor in Play mode and `--exit-after SECONDS` quits on
its own. `scripts/bundle-macos.sh` builds `dist/Reign Hub.app` with both.

## Layout

| Path | What it is |
| --- | --- |
| `engine/reign` | Facade crate re-exporting every engine crate. |
| `engine/reign_core` | Fixed-point math, input, clock, RNG. |
| `engine/reign_scene`, `engine/reign_derive` | Scenes, game objects, components, reflection, scene files. |
| `engine/reign_behaviour` | Per-object Behaviours and Systems run in fixed stages. |
| `engine/reign_assets` | GUIDs, `.meta` files, handles, prefabs, hot reload, packing. |
| `engine/reign_render`, `reign_app`, `reign_audio` | Renderer, window and loop, audio. |
| `engine/reign_physics`, `reign_combat`, `reign_net` | Platformer physics, frame-data combat, rollback netcode. |
| `engine/reign_editor` | The editor shell (egui). |
| `engine/reign_build` | Project tooling and component templates. |
| `app` | `reign-engine`, the standalone editor. |
| `hub` | `reign-hub`, the project launcher. |

[docs/ENGINE.md](docs/ENGINE.md) explains the scene, component, Behaviour,
prefab and asset model.

## Determinism rules

The simulation must give the same result on every machine, every replay and
every rollback. Anything in a component, the game's hashed world, or a
sim-affecting asset follows these rules:

- **No floats.** Use `Fx`/`FxVec2` for positions and speeds, `Rgba8` for
  colours and integers for percentages. Floats are for presentation only
  (rendering, audio).
- **No `HashMap`/`HashSet` with random state.** Use `Vec`, `BTreeMap` or
  sorted vectors, and order anything that matters by id or by data, never
  by memory address.
- **Everything is `Clone + Eq + Hash`.** `Component` requires it, so state
  is part of snapshots and checksums.
- **All state lives in components or the game's world:** no statics,
  `Rc`/`RefCell`, closures or trait objects holding state between ticks.
- **Randomness only from `ctx.rng()`** (the world's `Rng`).
- **Structural changes go through `Commands`** (`ctx.commands`), applied at
  a fixed point in the tick.
- **`ctx.emit` output is presentation only** and never feeds back into the
  simulation.
- Mark runtime-only component fields `#[serde(skip)]` so saves don't bake
  in one run's state, and don't rename a registered component: its name is
  what scene files store.

## License

By contributing you agree that your contributions are licensed under the
Mozilla Public License 2.0, the engine's license (see `LICENSE`).
