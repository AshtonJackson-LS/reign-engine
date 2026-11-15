# Roadmap

What's done, what's next and why, most important first. Tick items off here
as they land; add what you find along the way.

## Done

- Export Game: a standalone player, `.app` bundles, Scripts → Export Game.
- Collision events: collision and trigger enter/exit, layers and masks,
  `ctx.touching`.
- Tilemaps in physics: solid tiles, one-way platforms, collision events.
- Input actions and gamepads: `input.ron`, the Input Actions dialog,
  `ctx.action("Jump")`.
- Messages: physics events and name-and-number messages through `on_message`.
- Open-source prep: MPL-2.0, CI, CONTRIBUTING.
- Windows: new projects get engine paths Cargo can read; script reload can
  replace the running editor's `.exe`.
- Headless commands for tools and AI agents: `--check`, `--describe` and
  `--simulate` (see [AGENTS.md](../AGENTS.md)); every project gets its own
  `AGENTS.md`.

## 1. Usable by AI agents

An agent can't click the editor, so everything a person does there needs a
text or command-line way in, and a way to see the result.

- [x] `--check`: validate scenes, prefabs, input and asset references.
- [x] `--describe`: every component with its fields, defaults and ranges,
  the input actions, scenes and assets, as JSON.
- [x] `--simulate`: run a scene for N ticks with scripted input and print
  where everything ended up.
- [x] `AGENTS.md` in the engine and in every project.
- [ ] `--screenshot FILE`: render a frame to a PNG without a window (the hub
  already does this for its own UI with `REIGN_HUB_SNAPSHOTS`).
- [ ] `reign-hub new NAME [--template platformer]`: make a project from the
  command line. Today only the hub's window can.
- [ ] Scene edits from the command line (`--set Player.Rigidbody.gravity_scale=2`,
  `--spawn`), so an agent doesn't have to hand-write ids.
- [ ] Build errors as JSON (`cargo --message-format=json` passed through), with
  the file and line of each one.
- [ ] An MCP server over the same commands, once they've settled.

## 2. Things people reach for early

- [x] **Animator**: plays an `AnimationClip` on a `SpriteRenderer`;
  `ctx.play_animation(me, "run")`, frame events and `"AnimationFinished"`
  as messages, speed in percent.
- [x] **Sound**: `ctx.play_sound`, `ctx.play_music`, `ctx.stop_music`, the
  `AudioSource` component; `--simulate` lists the sounds played.
- [x] **Text**: the `Text` component and a built-in 5x7 pixel font in every
  project; world or screen space; `ctx.set_text`.
- [x] **Change scene** from a script: `ctx.load_scene("scenes/level2.scene.ron")`.
- [ ] An Add Component shortcut for Animator: dragging an `.anim.ron` into
  the view could make an animated sprite in one step.
- [ ] Sound volume settings (master, music, effects) for players, and
  stereo pan by position.
- [ ] Carry objects across a scene change (Unity's `DontDestroyOnLoad`),
  for a player or a score that lives through levels.
- [ ] A drop shadow or outline option on Text, for reading over busy
  backgrounds.
- [x] **Camera follow**: smooth follow with bounds; more than one camera.
- [ ] **Scale and rotation** on `Transform`.
- [ ] **Scripting conveniences**: `ctx.find("Name")`, tags, spawning a prefab
  in one call, friction, circle colliders, colliders on child objects.

## 3. Rough edges

- [ ] Script reload restarts the editor and loses undo history.
- [ ] Slow first build: every project compiles the whole engine into its own
  `target/`. Share one target folder between projects.
- [ ] Projects find the engine by an absolute path. Depend on the GitHub repo
  by git URL (with a local override) so anyone can build them.
- [ ] Tilemaps: slopes, dropping through one-way platforms, resizing maps in
  the painter.
- [ ] Export: an app icon, notarization, other platforms.
- [ ] Recompile with nothing changed still restarts the editor on Windows
  (the moved-aside `.exe` makes cargo relink).

## 4. Polish before people use it

- [ ] A sample project: a small finished platformer.
- [ ] CI's first green run on Linux, Windows and the license check.
- [ ] On-screen testing of everything above.

## Found along the way

- [x] Two Hierarchy tests panicked on every platform (egui's texture
  updates were dropped unhandled), which kept CI red.

- [ ] The README's hub section only describes macOS
  (`scripts/bundle-macos.sh`, `~/Library/...`); Windows and Linux need the
  same.
- [ ] `Camera` has no zoom, no background colour (`CLEAR` is fixed).
- [ ] Only player 0 has input (`World::input`); local multiplayer needs more.
- [ ] `ctx.touching` allocates a `Vec` on every call.
- [ ] Prefab variants and per-frame hurtboxes (see ENGINE.md's known
  limitations).
