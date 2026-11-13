//! The editor window: the event loop, input routing between egui and the
//! game, the docked panels, and drawing egui over the window.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use egui::{Color32, Rect, Sense, Stroke, Vec2};
use egui_dock::{DockArea, DockState, NodeIndex, Style, TabViewer};
use reign_app::winit::application::ApplicationHandler;
use reign_app::winit::event::{ElementState, WindowEvent};
use reign_app::winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use reign_app::winit::keyboard::{KeyCode, PhysicalKey};
use reign_app::winit::window::{Window, WindowId};
use reign_app::{Ctx, Frame, Pads, TickCtx};
use reign_core::checksum;
use reign_render::{Renderer, VIEW_HEIGHT, VIEW_WIDTH, view_scale, wgpu};
use reign_scene::{ComponentRegistry, ObjectId, Scene};

use crate::anim_editor::AnimEditor;
use crate::assets_ui::{self, AssetDrag, Open, ProjectState};
use crate::console::{ConsoleBuffer, init_logger};
use crate::create::{self, CreateRequest};
use crate::gizmo::{self, Drag, ViewMap};
use crate::hierarchy::Rename;
use crate::history::History;
use crate::play::{PlayControl, PlayMode, TIME_SCALES};
use crate::prefab_ui;
use crate::slicer::Slicer;
use crate::tilemap_painter::TilemapPainter;
use crate::{EditorApp, EditorConfig, SceneView, hierarchy, inspector};

/// The tint the editor takes on during a run, like Unity's play-mode tint.
const PLAY_TINT: Color32 = Color32::from_rgb(70, 110, 170);

pub(crate) fn run<A, F>(config: EditorConfig, make: F)
where
    A: EditorApp,
    F: FnOnce(&mut Ctx) -> A,
{
    let event_loop = EventLoop::new().expect("create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut editor = Editor { config, make: Some(make), session: None };
    event_loop.run_app(&mut editor).expect("run event loop");
}

struct Editor<A: EditorApp, F> {
    config: EditorConfig,
    make: Option<F>,
    session: Option<Session<A>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Tab {
    Game,
    Hierarchy,
    Inspector,
    Debug,
    Console,
    Project,
    Tilemap,
    Sprites,
    Animation,
}

impl Tab {
    /// The asset tools, which keep their own undo history.
    fn is_tool(self) -> bool {
        matches!(self, Tab::Tilemap | Tab::Sprites | Tab::Animation)
    }
}

/// Editor state the panels read and write, apart from the app.
struct Panels {
    selected: Option<ObjectId>,
    /// A Game-view move in progress.
    drag: Option<Drag>,
    /// How the last frame mapped onto the Game view.
    view_map: Option<ViewMap>,
    scene_camera: Option<SceneView>,
    drawn_scene: Option<SceneView>,
    /// What the edit this frame was, for its undo step.
    edit_label: Option<String>,
    /// The app's Create menu items, read each frame.
    create_items: Vec<String>,
    /// An object a Create menu or shortcut asked for this frame.
    create: Option<CreateRequest>,
    /// An object being renamed in the Hierarchy.
    rename: Option<Rename>,
    /// The last thing the editor did, shown in the toolbar.
    status: Option<String>,
    /// Ctrl (Cmd on macOS) is held.
    command: bool,
    /// The space the Game tab had last frame, in points; sizes the frame.
    game_space: Option<Rect>,
    /// Where the frame was shown last frame, in points.
    game_image: Option<Rect>,
    /// The game has the keyboard: the Game view was clicked last.
    game_focused: bool,
    /// The cursor, in physical window pixels.
    cursor: Option<(f32, f32)>,
    console: ConsoleBuffer,
    /// Shown levels, by `log::Level as usize - 1`.
    levels: [bool; 5],
    project: ProjectState,
    painter: TilemapPainter,
    slicer: Slicer,
    anim: AnimEditor,
    /// The Inspector shows the selected asset rather than the object.
    inspect_asset: bool,
    /// The asset tool under the pointer last frame (it gets Ctrl+Z), and
    /// this frame's, being found.
    tool_hover: Option<Tab>,
    tool_hover_now: Option<Tab>,
    /// What the Inspector's prefab buttons asked for this frame.
    prefab_action: Option<inspector::PrefabAction>,
    /// The Prefab Mode breadcrumb's buttons: back to the scene, save.
    prefab_back: bool,
    prefab_save: bool,
}

impl Panels {
    /// The Game view in physical window pixels: x, y, width, height.
    fn game_rect_px(&self, ppp: f32) -> Option<[f32; 4]> {
        self.game_image.map(|r| [r.min.x * ppp, r.min.y * ppp, r.width() * ppp, r.height() * ppp])
    }

    fn cursor_over_game(&self, ppp: f32) -> bool {
        let (Some((x, y)), Some([gx, gy, gw, gh])) = (self.cursor, self.game_rect_px(ppp)) else {
            return false;
        };
        (gx..gx + gw).contains(&x) && (gy..gy + gh).contains(&y)
    }
}

struct Session<A: EditorApp> {
    app: A,
    window: Arc<Window>,
    renderer: Renderer,
    pads: Pads,
    play: PlayControl<A::Snapshot>,
    started: Instant,
    last_frame: Instant,
    egui: egui::Context,
    egui_winit: egui_winit::State,
    egui_wgpu: egui_wgpu::Renderer,
    /// The Game view's texture in egui and its size in pixels.
    game_texture: Option<(egui::TextureId, (u32, u32))>,
    dock: DockState<Tab>,
    panels: Panels,
    history: History,
    /// The edit-mode history, kept aside during a run.
    stashed_history: Option<History>,
    /// The scene before the edit in progress (a drag), until it ends.
    edit_base: Option<Scene>,
    /// A hash of the scene file's text when last saved or opened; `None`
    /// until the next frame takes it.
    saved: Option<u64>,
    /// The scene hash the dirty flag was last worked out for, and the flag.
    dirty_cache: Option<(u64, bool)>,
    /// Unsaved changes when the run began (the title keeps them).
    dirty_at_play: bool,
    /// The configured title, and the title as last shown.
    base_title: String,
    title: String,
    /// Parsed prefabs, with the app's component types.
    prefabs: Option<reign_assets::Prefabs>,
    /// Editing a prefab on its own, with the scene set aside.
    prefab_mode: Option<prefab_ui::PrefabMode>,
    /// Prefab Mode swapped the scene this frame: not an edit.
    swapped: bool,
}

/// What the toolbar and shortcuts asked for this frame.
#[derive(Default)]
struct Requests {
    undo: bool,
    redo: bool,
    save: bool,
    open: Option<PathBuf>,
    /// Play started or a run ended: the scene changed, but not by an edit.
    run_changed: bool,
    /// The app asked to close the editor.
    quit: bool,
}

impl<A: EditorApp, F: FnOnce(&mut Ctx) -> A> ApplicationHandler for Editor<A, F> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(make) = self.make.take() else { return };
        let console = init_logger();
        let (window, mut renderer) = reign_app::open_window(event_loop, &self.config.app);
        let pads = Pads::new();
        let mut app = make(&mut Ctx::hosted(&window, &mut renderer, &pads, event_loop, None));
        let mut play = PlayControl::new(self.config.app.tick_hz);
        if self.config.start_playing {
            play.play(|| app.snapshot());
        }
        app.set_play_mode(play.mode());

        let egui = egui::Context::default();
        let egui_winit = egui_winit::State::new(
            egui.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(renderer.device().limits().max_texture_dimension_2d as usize),
        );
        let egui_wgpu = egui_wgpu::Renderer::new(
            renderer.device(),
            renderer.surface_format(),
            egui_wgpu::RendererOptions::default(),
        );

        let mut dock = DockState::new(vec![Tab::Game, Tab::Tilemap, Tab::Sprites, Tab::Animation]);
        let surface = dock.main_surface_mut();
        let [game, _] = surface.split_left(NodeIndex::root(), 0.18, vec![Tab::Hierarchy]);
        let [game, _] = surface.split_right(game, 0.76, vec![Tab::Inspector, Tab::Debug]);
        surface.split_below(game, 0.70, vec![Tab::Project, Tab::Console]);

        let now = Instant::now();
        let registry_for_prefabs = app.registry().cloned().map(reign_assets::Prefabs::new);
        log::info!("editor ready: edit mode, press Play to run");
        self.session = Some(Session {
            app,
            window,
            renderer,
            pads,
            play,
            started: now,
            last_frame: now,
            egui,
            egui_winit,
            egui_wgpu,
            game_texture: None,
            dock,
            panels: Panels {
                selected: None,
                drag: None,
                view_map: None,
                scene_camera: None,
                drawn_scene: None,
                edit_label: None,
                create_items: Vec::new(),
                create: None,
                rename: None,
                status: None,
                command: false,
                game_space: None,
                game_image: None,
                game_focused: false,
                cursor: None,
                console,
                levels: [true, true, true, false, false],
                project: ProjectState::default(),
                painter: TilemapPainter::default(),
                slicer: Slicer::default(),
                anim: AnimEditor::default(),
                inspect_asset: false,
                tool_hover: None,
                tool_hover_now: None,
                prefab_action: None,
                prefab_back: false,
                prefab_save: false,
            },
            prefabs: registry_for_prefabs,
            prefab_mode: None,
            swapped: false,
            history: History::default(),
            stashed_history: None,
            edit_base: None,
            saved: None,
            dirty_cache: None,
            dirty_at_play: false,
            base_title: self.config.app.title.clone(),
            title: self.config.app.title.clone(),
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(s) = self.session.as_mut() else { return };
        match &event {
            WindowEvent::RedrawRequested => return s.frame(event_loop),
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => s.renderer.resize(size.width, size.height),
            _ => {}
        }
        let (to_egui, to_game) = s.route(&event);
        if to_egui {
            let _ = s.egui_winit.on_window_event(&s.window, &event);
        }
        if to_game {
            let view = s.panels.game_rect_px(s.egui.pixels_per_point());
            let mut ctx = Ctx::hosted(&s.window, &mut s.renderer, &s.pads, event_loop, view);
            s.app.event(&mut ctx, &event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(s) = &self.session else { return };
        if self.config.app.exit_after.is_some_and(|limit| s.started.elapsed() >= limit) {
            event_loop.exit();
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        s.window.request_redraw();
    }
}

impl<A: EditorApp> Session<A> {
    /// Who gets `event`: egui, the game, or both. The game gets input only
    /// while the Game view is hovered or focused; releases always reach it,
    /// so nothing stays held after the cursor leaves.
    fn route(&mut self, event: &WindowEvent) -> (bool, bool) {
        let ppp = self.egui.pixels_per_point();
        match event {
            WindowEvent::CursorMoved { position, .. } => {
                self.panels.cursor = Some((position.x as f32, position.y as f32));
                (true, true)
            }
            WindowEvent::CursorLeft { .. } => {
                self.panels.cursor = None;
                (true, true)
            }
            WindowEvent::ModifiersChanged(m) => {
                let state = m.state();
                self.panels.command =
                    if cfg!(target_os = "macos") { state.super_key() } else { state.control_key() };
                (true, true)
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, .. } => {
                let over = self.panels.cursor_over_game(ppp);
                self.panels.game_focused = over;
                // While not playing, clicks in the Game view are the
                // editor's: select and move.
                (true, over && self.play.mode() == PlayMode::Playing)
            }
            WindowEvent::MouseInput { state: ElementState::Released, .. } => (true, true),
            WindowEvent::MouseWheel { .. } => {
                let over = self.panels.cursor_over_game(ppp);
                (
                    !over || self.play.mode() != PlayMode::Playing,
                    over && self.play.mode() == PlayMode::Playing,
                )
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Released {
                    return (true, true);
                }
                // The editor's own shortcuts never reach the game.
                let shortcut = matches!(
                    event.physical_key,
                    PhysicalKey::Code(
                        KeyCode::KeyZ | KeyCode::KeyY | KeyCode::KeyS | KeyCode::KeyD
                    )
                );
                if self.panels.command && shortcut {
                    return (true, false);
                }
                let game = self.play.mode() == PlayMode::Playing
                    && (self.panels.game_focused || self.panels.cursor_over_game(ppp))
                    && !self.egui.egui_wants_keyboard_input();
                (!game, game)
            }
            WindowEvent::Ime(_) => (true, false),
            _ => (true, true),
        }
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let elapsed = now - self.last_frame;
        let dt = elapsed.as_secs_f32().min(0.1);
        self.last_frame = now;
        self.fit_game_texture();
        self.reload_assets(event_loop);
        self.app.set_play_mode(self.play.mode());

        // Run the game: as many ticks as the play state allows.
        let ppp = self.egui.pixels_per_point();
        let game_input = self.play.mode() == PlayMode::Playing
            && (self.panels.game_focused || self.panels.cursor_over_game(ppp));
        self.pads.set_enabled(game_input);
        let pads = self.pads.poll(dt);
        if self.prefab_mode.is_some()
            && let Some(snapshot) = self.play.stop()
        {
            // The game can't run on a prefab: Play waits for the scene.
            self.app.restore(snapshot);
            self.panels.status = Some("Leave Prefab Mode to play".into());
        }
        let ticks = self.play.advance(elapsed);
        if self.play.mode() == PlayMode::Playing || ticks > 0 {
            // A step is one tick's worth of time for the game's real-time
            // parts too (menus, fades).
            let dt = if self.play.mode() == PlayMode::Playing {
                dt
            } else {
                self.play.tick_duration().as_secs_f32()
            };
            let frame = Frame { dt, ticks, pads };
            let view = self.panels.game_rect_px(ppp);
            let mut ctx =
                Ctx::hosted(&self.window, &mut self.renderer, &self.pads, event_loop, view);
            self.app.update(&mut ctx, &frame);
            for index in 0..ticks {
                self.app.tick(&mut TickCtx::hosted(&self.pads, event_loop, index, ticks));
            }
            let mut ctx =
                Ctx::hosted(&self.window, &mut self.renderer, &self.pads, event_loop, view);
            self.app.late_update(&mut ctx, &frame);
        }
        // The game presents into the Game view's texture. In Prefab Mode
        // the scene holds a prefab, not the game's content: the game
        // neither draws nor syncs until the scene is back.
        if self.prefab_mode.is_none() {
            self.panels.drawn_scene = None;
            let camera = self.panels.scene_camera;
            if self.play.mode() != PlayMode::Playing
                && let Some(camera) = camera
                && self.app.draw_scene(&mut self.renderer, camera)
            {
                self.panels.drawn_scene = Some(camera);
            } else {
                self.app.draw(&mut self.renderer, self.play.alpha());
                if self.panels.scene_camera.is_none() {
                    self.panels.scene_camera = self.app.view_origin().map(SceneView::new);
                }
            }
            // Bring outside state into the scene, and note it before the UI.
            self.app.sync_scene();
        }
        let base = self.app.scene().cloned();
        let base_hash = base.as_ref().map(checksum);
        if self.saved.is_none() {
            self.saved = base.as_ref().map(saved_text_hash);
            self.dirty_cache = None;
        }
        if let Some(scene) = &base
            && self.panels.selected.is_some_and(|s| !scene.contains(s))
        {
            self.panels.selected = None;
        }
        let dirty =
            if self.play.in_run() { self.dirty_at_play } else { self.dirty(&base, base_hash) };
        self.update_title(dirty);

        // The editor UI, over the whole window.
        self.panels.tool_hover = self.panels.tool_hover_now.take();
        // Set again if the Game tab is shown; a hidden Game view takes no
        // input.
        self.panels.game_image = None;
        let selected_before = (self.panels.selected, self.panels.project.selected);
        let assets = self.app.assets();
        let input = self.egui_winit.take_egui_input(&self.window);
        let registry = self.app.registry().cloned();
        self.panels.create_items = self.app.create_items();
        let mut requests = Requests::default();
        let Session {
            app,
            window,
            renderer,
            pads,
            play,
            egui,
            dock,
            panels,
            game_texture,
            history,
            stashed_history,
            dirty_at_play,
            prefabs,
            prefab_mode,
            ..
        } = self;
        let prefab_name = prefab_mode.as_ref().map(|m| m.name.clone());
        let output = egui.run_ui(input, |ui| {
            shortcuts(ui, app, panels, &mut requests);
            let state = ToolbarState { history, stashed_history, dirty, dirty_at_play };
            toolbar(ui, app, play, panels, state, &mut requests);
            let mut style = Style::from_egui(ui.style());
            style.tab_bar.fill_tab_bar = true;
            let mut viewer = Viewer {
                app: &mut *app,
                play: &*play,
                panels: &mut *panels,
                game_texture: *game_texture,
                window,
                renderer: &mut *renderer,
                pads,
                event_loop,
                registry: registry.as_ref(),
                assets,
                prefabs: prefabs.as_ref(),
                prefab_mode: prefab_name.as_deref(),
                dirty,
            };
            DockArea::new(dock)
                .style(style)
                .show_close_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
        });
        self.app.set_play_mode(self.play.mode());
        self.apply_create();
        self.after_panels(selected_before);
        self.prefab_requests();
        let quit = requests.quit;
        self.track_edits(base, base_hash, requests);
        if quit {
            event_loop.exit();
        }
        self.egui_winit.handle_platform_output(&self.window, output.platform_output);
        let ppp = output.pixels_per_point;
        let jobs = self.egui.tessellate(output.shapes, ppp);
        // Drained, not just read: egui checks every delta was applied.
        let mut textures = output.textures_delta;
        for (id, deltas) in textures.set.drain() {
            for delta in deltas {
                self.egui_wgpu.update_texture(
                    self.renderer.device(),
                    self.renderer.queue(),
                    id,
                    &delta,
                );
            }
        }
        let egui_wgpu = &mut self.egui_wgpu;
        self.renderer.present_with(|device, queue, encoder, view, size| {
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [size.0, size.1],
                pixels_per_point: ppp,
            };
            let buffers = egui_wgpu.update_buffers(device, queue, encoder, &jobs, &screen);
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("editor ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            egui_wgpu.render(&mut pass, &jobs, &screen);
            buffers
        });
        for id in textures.free.drain() {
            self.egui_wgpu.free_texture(&id);
        }
    }

    /// Sizes the offscreen frame to the largest whole-number scale of the
    /// view that fits the Game tab, and keeps egui's texture pointing at it.
    fn fit_game_texture(&mut self) {
        let ppp = self.egui.pixels_per_point();
        let scale = self.panels.game_space.map_or(1, |space| {
            view_scale((space.width() * ppp) as u32, (space.height() * ppp) as u32)
        });
        let size = (VIEW_WIDTH * scale, VIEW_HEIGHT * scale);
        if self.game_texture.is_some_and(|(_, current)| current == size) {
            return;
        }
        let offscreen = self.renderer.create_offscreen(size.0, size.1);
        let device = self.renderer.device();
        let filter = wgpu::FilterMode::Nearest;
        let id = match self.game_texture {
            Some((id, _)) => {
                self.egui_wgpu.update_egui_texture_from_wgpu_texture(
                    device,
                    &offscreen.view,
                    filter,
                    id,
                );
                id
            }
            None => self.egui_wgpu.register_native_texture(device, &offscreen.view, filter),
        };
        self.game_texture = Some((id, size));
        self.renderer.set_offscreen(Some(offscreen));
    }
}

impl<A: EditorApp> Session<A> {
    /// Applies queued asset changes at the frame boundary, logs them, and
    /// lets the app refresh.
    fn reload_assets(&mut self, event_loop: &ActiveEventLoop) {
        let Some(db) = self.app.assets() else { return };
        let changes = db.apply_changes();
        if changes.is_empty() {
            return;
        }
        for change in &changes {
            let what = change.kind.name();
            match (change.removed, change.sim_affecting()) {
                (true, _) => log::warn!("{what} removed: {}", change.path),
                (false, true) => {
                    log::warn!("simulation data changed: reloaded {what} {}", change.path)
                }
                (false, false) => log::info!("reloaded {what} {}", change.path),
            }
        }
        let view = self.panels.game_rect_px(self.egui.pixels_per_point());
        let mut ctx = Ctx::hosted(&self.window, &mut self.renderer, &self.pads, event_loop, view);
        self.app.assets_changed(&mut ctx, &changes);
        let prefab_changes: Vec<reign_assets::Guid> = changes
            .iter()
            .filter(|c| c.kind == reign_assets::AssetKind::Prefab && !c.removed)
            .map(|c| c.guid)
            .collect();
        for guid in prefab_changes {
            self.refresh_instances(Some(guid));
        }
        self.panels.project.changed(&changes);
        for change in changes.iter().filter(|c| !c.removed) {
            self.panels.painter.reloaded(db, change.guid);
            self.panels.anim.reloaded(db, change.guid);
        }
    }

    /// Follows what the panels did this frame: which selection the
    /// Inspector shows, and assets the Project panel asked to open.
    /// Makes the object a Create menu or shortcut asked for, in the middle
    /// of the view, and selects it.
    fn apply_create(&mut self) {
        let Some(request) = self.panels.create.take() else { return };
        let at = match self.panels.drawn_scene.or(self.panels.scene_camera) {
            Some(view) => {
                let (w, h) = view.size();
                (view.origin.0 + w / 2.0, view.origin.1 + h / 2.0)
            }
            None => {
                let (x, y) = self.app.view_origin().unwrap_or((0.0, 0.0));
                (x + VIEW_WIDTH as f32 / 2.0, y + VIEW_HEIGHT as f32 / 2.0)
            }
        };
        let at = (at.0.round() as i32, at.1.round() as i32);
        let parent = request.parent.filter(|p| self.app.scene().is_some_and(|s| s.contains(*p)));
        let (id, label) = match &request.item {
            Some(item) => {
                let name = item.rsplit('/').next().unwrap_or(item);
                (self.app.create(item, parent, at), format!("Create {name}"))
            }
            None => {
                let Some(scene) = self.app.scene_mut() else { return };
                let id = match parent {
                    Some(p) => scene.spawn_child(p, "GameObject"),
                    None => scene.spawn("GameObject"),
                };
                scene.set_world_position(id, reign_core::FxVec2::from_px(at.0, at.1));
                let label = if parent.is_some() { "Create empty child" } else { "Create empty" };
                (Some(id), label.to_owned())
            }
        };
        match id {
            Some(id) => {
                self.panels.selected = Some(id);
                self.panels.status = Some(label.clone());
                self.panels.edit_label = Some(label);
            }
            None => self.panels.status = Some(format!("{label}: the app couldn't make it")),
        }
    }

    fn after_panels(&mut self, (object, asset): (Option<ObjectId>, Option<reign_assets::Guid>)) {
        let panels = &mut self.panels;
        if panels.project.selected != asset && panels.project.selected.is_some() {
            panels.inspect_asset = true;
        }
        if panels.selected != object && panels.selected.is_some() {
            panels.inspect_asset = false;
        }
        if let Some(status) = panels.project.status.take() {
            log::info!("{status}");
            panels.status = Some(status);
        }
        let (Some(open), Some(db)) = (panels.project.open.take(), self.app.assets()) else {
            return;
        };
        let tab = match open {
            Open::Tilemap(guid) => {
                panels.painter.open(db, guid);
                Tab::Tilemap
            }
            Open::Slicer(guid) => {
                panels.slicer.open(db, guid);
                Tab::Sprites
            }
            Open::Animation(guid) => {
                panels.anim.open(db, guid);
                Tab::Animation
            }
            Open::Prefab(guid) => {
                self.enter_prefab_mode(guid);
                Tab::Hierarchy
            }
        };
        if let Some(found) = self.dock.find_tab(&tab) {
            let _ = self.dock.set_active_tab(found);
        }
    }

    /// Notices scene edits made through the UI and records them for undo,
    /// then carries out undo, redo, save and open.
    fn track_edits(&mut self, base: Option<Scene>, base_hash: Option<u64>, requests: Requests) {
        if requests.run_changed || std::mem::take(&mut self.swapped) {
            // Play, Stop or Prefab Mode swapped the scene; that isn't an
            // edit.
            self.edit_base = None;
        } else if self.app.scene().map(checksum) != base_hash {
            if self.edit_base.is_none() {
                self.edit_base = base;
            }
            if self.prefab_mode.is_none() {
                self.app.scene_edited();
                // Instances keep their overrides up to date, so a change
                // to their prefab keeps them.
                if let (Some(p), Some(db), Some(scene)) =
                    (&self.prefabs, self.app.assets(), self.app.scene_mut())
                {
                    prefab_ui::sync_all(scene, &p.with(db));
                }
            }
        }
        // A drag is one edit: record it when the pointer lets go.
        let dragging = self.egui.input(|i| i.pointer.any_down());
        if !dragging || requests.undo || requests.redo {
            self.commit_edit();
        }
        let history = &mut self.history;
        let done = match (requests.undo, requests.redo, self.app.scene_mut()) {
            (true, _, Some(scene)) => history.undo(scene).map(|l| format!("Undid {l}")),
            (_, true, Some(scene)) => history.redo(scene).map(|l| format!("Redid {l}")),
            _ => None,
        };
        if let Some(done) = done {
            if self.prefab_mode.is_none() {
                self.app.scene_edited();
            }
            self.panels.status = Some(done);
        }
        if requests.save {
            self.save();
        }
        if let Some(path) = requests.open {
            self.open(&path);
        }
    }

    /// Sets the scene aside and edits the prefab `guid` on its own.
    fn enter_prefab_mode(&mut self, guid: reign_assets::Guid) {
        let (Some(_), Some(db)) = (&self.prefabs, self.app.assets()) else { return };
        if self.play.in_run() {
            self.panels.status = Some("Stop first: Prefab Mode edits the prefab on its own".into());
            return;
        }
        if self.prefab_mode.is_some() {
            self.exit_prefab_mode();
        }
        let prefabs = self.prefabs.as_ref().expect("checked");
        let opened = prefab_ui::open(guid, &prefabs.with(db));
        let scene = match opened {
            Ok(scene) => scene,
            Err(e) => {
                self.panels.status = Some(format!("Couldn't open the prefab: {e}"));
                return;
            }
        };
        self.commit_edit();
        let Some(current) = self.app.scene_mut() else { return };
        let stashed = std::mem::replace(current, scene);
        let name = db.path(guid).map_or_else(|| "prefab".into(), |p| assets_ui_file_label(&p));
        self.prefab_mode = Some(prefab_ui::PrefabMode {
            guid,
            name: name.clone(),
            scene: stashed,
            history: std::mem::take(&mut self.history),
            saved: self.saved.take(),
            selected: self.panels.selected.take(),
        });
        self.panels.selected = self.app.scene().and_then(|s| s.roots().first().copied());
        self.dirty_cache = None;
        self.swapped = true;
        self.panels.status = Some(format!("Prefab Mode: {name}"));
        log::info!("prefab mode: {name}");
    }

    /// Brings the scene back, with its instances updated from the prefab.
    fn exit_prefab_mode(&mut self) {
        let Some(mode) = self.prefab_mode.take() else { return };
        self.commit_edit();
        if let Some(scene) = self.app.scene_mut() {
            *scene = mode.scene;
        }
        self.history = mode.history;
        self.saved = mode.saved;
        self.dirty_cache = None;
        self.swapped = true;
        self.panels.selected = mode.selected;
        self.refresh_instances(Some(mode.guid));
        self.app.scene_edited();
        self.panels.status = Some("Back to the scene".into());
    }

    /// Writes the prefab being edited in Prefab Mode.
    fn save_prefab(&mut self) {
        let (Some(mode), Some(prefabs), Some(db), Some(scene)) =
            (&self.prefab_mode, &self.prefabs, self.app.assets(), self.app.scene())
        else {
            return;
        };
        let status = match prefab_ui::save_text(scene, &prefabs.with(db)) {
            Ok(text) => match db.write(mode.guid, text.as_bytes()) {
                Ok(()) => {
                    self.saved = Some(saved_text_hash(scene));
                    self.dirty_cache = None;
                    prefabs.clear();
                    format!("Saved {}", mode.name)
                }
                Err(e) => format!("Couldn't write {}: {e}", mode.name),
            },
            Err(e) => format!("Couldn't save the prefab: {e}"),
        };
        log::info!("{status}");
        self.panels.status = Some(status);
    }

    /// Re-stamps the saved instances of a prefab (every prefab, for
    /// `None`) in the open scene, or the scene set aside in Prefab Mode.
    fn refresh_instances(&mut self, guid: Option<reign_assets::Guid>) {
        let (Some(prefabs), Some(db)) = (&self.prefabs, self.app.assets()) else { return };
        prefabs.clear();
        let lookup = prefabs.with(db);
        let scene = match &mut self.prefab_mode {
            Some(mode) => Some(&mut mode.scene),
            None => self.app.scene_mut(),
        };
        if let Some(scene) = scene {
            let n = prefab_ui::refresh_instances(scene, guid, &lookup);
            if n > 0 {
                log::info!("updated {n} prefab instance(s)");
            }
        }
    }

    /// Carries out the prefab buttons, the breadcrumb and objects dropped
    /// on the Project panel.
    fn prefab_requests(&mut self) {
        if std::mem::take(&mut self.panels.prefab_save) {
            self.save_prefab();
        }
        if std::mem::take(&mut self.panels.prefab_back) {
            self.exit_prefab_mode();
        }
        let (Some(prefabs), Some(db)) = (&self.prefabs, self.app.assets()) else { return };
        let lookup = prefabs.with(db);
        if let Some((object, folder)) = self.panels.project.make_prefab.take()
            && let Some(scene) = self.app.scene_mut()
        {
            let status = match prefab_ui::create(scene, object, &folder, db, &lookup) {
                Ok(path) => {
                    self.panels.edit_label = Some("Make prefab".into());
                    format!("Made prefab {path}")
                }
                Err(e) => format!("Couldn't make a prefab: {e}"),
            };
            self.panels.status = Some(status);
        }
        let Some(action) = self.panels.prefab_action.take() else { return };
        use inspector::PrefabAction as P;
        let Some(scene) = self.app.scene_mut() else { return };
        let (label, result): (&str, Result<(), String>) = match action {
            P::SelectAsset(guid) => {
                self.panels.project.selected = Some(guid);
                if let Some(path) = db.path(guid) {
                    self.panels.project.folder =
                        path.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_owned());
                }
                ("", Ok(()))
            }
            P::Open(guid) => {
                drop(lookup);
                self.enter_prefab_mode(guid);
                return;
            }
            P::ApplyAll(root) => {
                let guid = scene.get::<reign_assets::PrefabInstance>(root).map(|i| i.prefab.guid);
                let applied =
                    reign_assets::prefab::apply_all(scene, root, &lookup).and_then(|text| {
                        let guid = guid.ok_or("not an instance")?;
                        db.write(guid, text.as_bytes()).map_err(|e| e.to_string())?;
                        Ok(guid)
                    });
                match applied {
                    Ok(guid) => {
                        drop(lookup);
                        self.refresh_instances(Some(guid));
                        self.panels.edit_label = Some("Apply prefab".into());
                        self.panels.status = Some("Applied the overrides to the prefab".into());
                        return;
                    }
                    Err(e) => ("Apply prefab", Err(e)),
                }
            }
            P::RevertAll(root) => {
                ("Revert prefab", reign_assets::prefab::revert_all(scene, root, &lookup))
            }
            P::Unpack(root) => {
                reign_assets::prefab::unpack(scene, root);
                ("Unpack prefab", Ok(()))
            }
            P::Revert(root, object, path) => (
                "Revert override",
                reign_assets::prefab::revert_property(scene, root, object, &path, &lookup),
            ),
        };
        match result {
            Ok(()) if !label.is_empty() => self.panels.edit_label = Some(label.into()),
            Ok(()) => {}
            Err(e) => self.panels.status = Some(format!("{label}: {e}")),
        }
    }

    fn commit_edit(&mut self) {
        if let Some(before) = self.edit_base.take() {
            let label = self.panels.edit_label.take().unwrap_or_else(|| "Edit".into());
            self.history.record(label, before);
        }
    }

    fn save(&mut self) {
        if self.prefab_mode.is_some() {
            self.save_prefab();
            return;
        }
        let mut wrote = false;
        let status = if self.play.in_run() {
            "Stop first: changes made while playing are discarded".to_owned()
        } else {
            // Prefab instances are saved as their prefab and overrides.
            let saved_form = match (self.app.scene(), &self.prefabs, self.app.assets()) {
                (Some(scene), Some(p), Some(db)) => {
                    reign_assets::prefab::saved_form(scene, &p.with(db)).ok()
                }
                _ => None,
            };
            match (saved_form.as_ref().or(self.app.scene()), self.app.scene_path()) {
                (Some(scene), Some(path)) => match scene.to_ron() {
                    Ok(text) => match std::fs::write(&path, text) {
                        Ok(()) => {
                            wrote = true;
                            self.dirty_cache = None;
                            log::info!("saved the scene to {}", path.display());
                            format!("Saved {}", file_name(&path))
                        }
                        Err(e) => format!("Couldn't write {}: {e}", path.display()),
                    },
                    Err(e) => format!("Couldn't save: {e}"),
                },
                (Some(_), None) => "This scene has no file to save to".to_owned(),
                (None, _) => "No scene".to_owned(),
            }
        };
        if wrote {
            self.saved = self.app.scene().map(saved_text_hash);
        }
        self.panels.status = Some(status);
    }

    fn open(&mut self, path: &std::path::Path) {
        self.exit_prefab_mode();
        if let Some(snapshot) = self.play.stop() {
            self.app.restore(snapshot);
        }
        if let Some(stashed) = self.stashed_history.take() {
            self.history = stashed;
        }
        self.panels.status = Some(match self.app.open_scene(path) {
            Ok(()) => {
                self.history.clear();
                self.edit_base = None;
                self.saved = None;
                self.panels.selected = None;
                self.panels.scene_camera = None;
                log::info!("opened {}", path.display());
                format!("Opened {}", file_name(path))
            }
            Err(e) => {
                log::warn!("couldn't open {}: {e}", path.display());
                format!("Couldn't open {}: {e}", file_name(path))
            }
        });
    }

    /// The window title: the scene's file, with `*` when it has unsaved
    /// changes.
    fn update_title(&mut self, dirty: bool) {
        let file = self.app.scene_path().map(|p| format!(" - {}", file_name(&p)));
        let title = format!(
            "{}{}{}",
            self.base_title,
            file.unwrap_or_default(),
            if dirty { "*" } else { "" }
        );
        if title != self.title {
            self.window.set_title(&title);
            self.title = title;
        }
    }
}

impl<A: EditorApp> Session<A> {
    /// Whether saving would change the file. Worked out from the text that
    /// would be written, so runtime-only objects and fields don't count, and
    /// only again when the scene changed.
    fn dirty(&mut self, scene: &Option<Scene>, hash: Option<u64>) -> bool {
        let (Some(scene), Some(hash)) = (scene, hash) else { return false };
        match self.dirty_cache {
            Some((seen, dirty)) if seen == hash => dirty,
            _ => {
                let dirty = Some(saved_text_hash(scene)) != self.saved;
                self.dirty_cache = Some((hash, dirty));
                dirty
            }
        }
    }
}

/// A hash of what saving `scene` would write.
fn saved_text_hash(scene: &Scene) -> u64 {
    scene.to_ron().map_or(0, |text| checksum(&text))
}

fn assets_ui_file_label(path: &str) -> String {
    crate::inspector::file_label(path)
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into())
}

/// Undo, redo, save, create, duplicate and delete from the keyboard.
fn shortcuts<A: EditorApp>(
    ui: &mut egui::Ui,
    app: &mut A,
    panels: &mut Panels,
    requests: &mut Requests,
) {
    use egui::{Key, KeyboardShortcut, Modifiers};
    let command = |key| KeyboardShortcut::new(Modifiers::COMMAND, key);
    let typing = ui.ctx().egui_wants_keyboard_input();
    // Rename the selection: Ctrl/Cmd+I, or F2. Not over an asset tool,
    // where I is the Tilemap eyedropper.
    let over_tool = panels.tool_hover.is_some_and(|t| t.is_tool());
    let rename = !typing
        && !over_tool
        && ui.input_mut(|i| {
            i.consume_shortcut(&command(Key::I)) || i.consume_key(Modifiers::NONE, Key::F2)
        });
    if rename
        && let Some(id) = panels.selected
        && let Some(scene) = app.scene()
    {
        panels.rename = Rename::start(scene, id);
    }
    let (new_empty, new_child) = ui.input_mut(|i| {
        let shift_n = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::N);
        let child = KeyboardShortcut::new(Modifiers::ALT | Modifiers::SHIFT, Key::N);
        let child = !typing && i.consume_shortcut(&child);
        let empty =
            !typing && (i.consume_shortcut(&shift_n) || i.consume_shortcut(&command(Key::N)));
        (empty, child)
    });
    if new_empty {
        panels.create = Some(CreateRequest { item: None, parent: None });
    } else if new_child && let Some(parent) = panels.selected {
        panels.create = Some(CreateRequest { item: None, parent: Some(parent) });
    }
    let (redo, undo, save, duplicate, delete) = ui.input_mut(|i| {
        let redo = i.consume_shortcut(&KeyboardShortcut::new(
            Modifiers::COMMAND | Modifiers::SHIFT,
            Key::Z,
        )) || i.consume_shortcut(&command(Key::Y));
        let undo = i.consume_shortcut(&command(Key::Z));
        let save = i.consume_shortcut(&command(Key::S));
        let duplicate = i.consume_shortcut(&command(Key::D));
        let delete = !typing && i.key_pressed(Key::Delete);
        (redo, undo, save, duplicate, delete)
    });
    // Text fields keep their own undo while typing; an asset tool under
    // the pointer undoes its own edits.
    let tool = panels.tool_hover.filter(|t| t.is_tool());
    if let Some(tool) = tool.filter(|_| !typing) {
        let done = match (tool, undo, redo) {
            (Tab::Tilemap, true, _) => panels.painter.undo(),
            (Tab::Tilemap, _, true) => panels.painter.redo(),
            (Tab::Sprites, true, _) => panels.slicer.undo(),
            (Tab::Sprites, _, true) => panels.slicer.redo(),
            (Tab::Animation, true, _) => panels.anim.undo(),
            (Tab::Animation, _, true) => panels.anim.redo(),
            _ => false,
        };
        if done {
            panels.status = Some(format!("{} in {tool:?}", if undo { "Undid" } else { "Redid" }));
        }
    } else {
        requests.undo |= undo && !typing;
        requests.redo |= redo && !typing;
    }
    requests.save |= save;
    let Some(scene) = app.scene_mut() else { return };
    if let Some(id) = panels.selected.filter(|id| scene.contains(*id)) {
        if duplicate {
            panels.selected = scene.duplicate(id);
            panels.edit_label = Some("Duplicate".into());
        } else if delete {
            scene.despawn(id);
            panels.selected = None;
            panels.edit_label = Some("Delete".into());
        }
    }
}

/// The toolbar's view of the editor's undo and file state.
struct ToolbarState<'a> {
    history: &'a mut History,
    stashed_history: &'a mut Option<History>,
    dirty: bool,
    dirty_at_play: &'a mut bool,
}

/// Play, Pause, Step, Stop, the time scale, undo and redo, and the Scene
/// menu.
fn toolbar<A: EditorApp>(
    ui: &mut egui::Ui,
    app: &mut A,
    play: &mut PlayControl<A::Snapshot>,
    panels: &mut Panels,
    state: ToolbarState,
    requests: &mut Requests,
) {
    let mode = play.mode();
    let mut frame = egui::Frame::side_top_panel(ui.style());
    if mode != PlayMode::Edit {
        frame = frame.fill(PLAY_TINT.gamma_multiply(0.55));
    }
    egui::Panel::top("toolbar").frame(frame).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("Scene", |ui| {
                let can_save = app.scene_path().is_some() && !play.in_run();
                if ui.add_enabled(can_save, egui::Button::new("Save  (Ctrl+S)")).clicked() {
                    requests.save = true;
                    ui.close();
                }
                ui.separator();
                ui.label(egui::RichText::new("Open").weak());
                for path in app.scene_files() {
                    if ui.button(file_name(&path)).clicked() {
                        requests.open = Some(path);
                        ui.close();
                    }
                }
            });
            ui.menu_button("GameObject", |ui| {
                let items = std::mem::take(&mut panels.create_items);
                create::menu(ui, &items, None, &mut panels.create);
                if let Some(selected) = panels.selected {
                    ui.separator();
                    ui.menu_button("Create as child of selection", |ui| {
                        create::menu(ui, &items, Some(selected), &mut panels.create);
                    });
                }
                panels.create_items = items;
            });
            let asked = app.toolbar_ui(ui);
            requests.save |= asked.save_scene;
            requests.quit |= asked.quit;
            let undo = state.history.can_undo().map(|l| format!("Undo {l}"));
            let redo = state.history.can_redo().map(|l| format!("Redo {l}"));
            if ui
                .add_enabled(undo.is_some(), egui::Button::new("Undo"))
                .on_hover_text(undo.unwrap_or_else(|| "Nothing to undo".into()) + "  (Ctrl+Z)")
                .clicked()
            {
                requests.undo = true;
            }
            if ui
                .add_enabled(redo.is_some(), egui::Button::new("Redo"))
                .on_hover_text(redo.unwrap_or_else(|| "Nothing to redo".into()) + "  (Ctrl+Y)")
                .clicked()
            {
                requests.redo = true;
            }
            ui.separator();
            let playing = mode == PlayMode::Playing;
            let mut enter_run = |play: &mut PlayControl<A::Snapshot>, app: &mut A, step: bool| {
                if !play.in_run() {
                    // Edits made while playing get their own history, and
                    // go with the run when it stops.
                    *state.stashed_history = Some(std::mem::take(state.history));
                    *state.dirty_at_play = state.dirty;
                    requests.run_changed = true;
                }
                if step { play.step(|| app.snapshot()) } else { play.play(|| app.snapshot()) }
            };
            if ui.add_enabled(!playing, egui::Button::new("▶ Play")).clicked() {
                enter_run(play, app, false);
                log::info!("play");
            }
            if ui.add_enabled(playing, egui::Button::new("⏸ Pause")).clicked() {
                play.pause();
            }
            if ui.button("⏭ Step").on_hover_text("Run exactly one fixed tick").clicked() {
                enter_run(play, app, true);
            }
            if ui.add_enabled(play.in_run(), egui::Button::new("⏹ Stop")).clicked()
                && let Some(snapshot) = play.stop()
            {
                app.restore(snapshot);
                if let Some(stashed) = state.stashed_history.take() {
                    *state.history = stashed;
                }
                requests.run_changed = true;
                log::info!("stop: restored the state from before Play");
            }
            ui.separator();
            ui.label("Time scale");
            let mut scale = play.scale();
            egui::ComboBox::from_id_salt("time scale").selected_text(format!("{scale}x")).show_ui(
                ui,
                |ui| {
                    for s in TIME_SCALES {
                        ui.selectable_value(&mut scale, s, format!("{s}x"));
                    }
                },
            );
            play.set_scale(scale);
            ui.separator();
            let (text, color) = match mode {
                PlayMode::Edit => ("EDIT", ui.visuals().weak_text_color()),
                PlayMode::Playing => ("PLAYING", Color32::from_rgb(140, 200, 255)),
                PlayMode::Paused => ("PAUSED", Color32::from_rgb(255, 200, 110)),
            };
            ui.label(egui::RichText::new(text).strong().color(color));
            if let Some(path) = app.scene_path() {
                let dirty = if play.in_run() { *state.dirty_at_play } else { state.dirty };
                let name = format!("{}{}", file_name(&path), if dirty { "*" } else { "" });
                ui.label(egui::RichText::new(name).monospace())
                    .on_hover_text(path.display().to_string());
            }
            if let Some(status) = &panels.status {
                ui.label(egui::RichText::new(status).weak());
            }
        });
    });
}

struct Viewer<'a, A: EditorApp> {
    app: &'a mut A,
    play: &'a PlayControl<A::Snapshot>,
    panels: &'a mut Panels,
    game_texture: Option<(egui::TextureId, (u32, u32))>,
    window: &'a Window,
    renderer: &'a mut Renderer,
    pads: &'a Pads,
    event_loop: &'a ActiveEventLoop,
    registry: Option<&'a ComponentRegistry>,
    assets: Option<&'static reign_assets::AssetDb>,
    prefabs: Option<&'a reign_assets::Prefabs>,
    /// The prefab being edited in Prefab Mode.
    prefab_mode: Option<&'a str>,
    dirty: bool,
}

impl<A: EditorApp> TabViewer for Viewer<'_, A> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let dirty = match tab {
            Tab::Tilemap => self.panels.painter.editing().is_some_and(|(_, d)| d),
            Tab::Sprites => self.panels.slicer.editing().is_some_and(|(_, d)| d),
            Tab::Animation => self.panels.anim.editing().is_some_and(|(_, d)| d),
            _ => false,
        };
        if *tab == Tab::Game && self.play.mode() != PlayMode::Playing {
            return "Scene".into();
        }
        format!("{tab:?}{}", if dirty { "*" } else { "" }).into()
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Game | Tab::Console | Tab::Project => [false, false],
            Tab::Tilemap | Tab::Sprites | Tab::Animation => [false, false],
            _ => [false, true],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        match tab {
            Tab::Game if self.prefab_mode.is_some() => {
                if let Some(scene) = self.app.scene() {
                    let name = self.prefab_mode.unwrap_or("");
                    prefab_ui::canvas(ui, scene, name, &mut self.panels.selected);
                }
            }
            Tab::Game => self.game(ui),
            Tab::Hierarchy => match self.app.scene_mut() {
                Some(scene) => {
                    if let Some(name) = self.prefab_mode {
                        let (back, save) = prefab_ui::breadcrumb(ui, name, self.dirty);
                        self.panels.prefab_back |= back;
                        self.panels.prefab_save |= save;
                    }
                    if let Some(label) = hierarchy::hierarchy(
                        ui,
                        scene,
                        &mut self.panels.selected,
                        true,
                        &self.panels.create_items,
                        &mut self.panels.create,
                        &mut self.panels.rename,
                    ) {
                        self.panels.edit_label = Some(label);
                    }
                }
                None => {
                    ui.weak("This app has no scene.");
                }
            },
            Tab::Inspector
                if self.panels.inspect_asset && self.panels.project.selected.is_some() =>
            {
                if let (Some(db), Some(guid)) = (self.assets, self.panels.project.selected) {
                    assets_ui::inspector(ui, db, &mut self.panels.project, guid);
                }
            }
            Tab::Inspector => match (self.panels.selected, self.app.scene_mut()) {
                (Some(id), Some(scene)) => {
                    if self.play.in_run() {
                        ui.weak("Changes made while playing are discarded on Stop.");
                    }
                    let info = match (self.prefabs, self.assets) {
                        (Some(p), Some(db)) => prefab_ui::info(scene, id, &p.with(db), db),
                        _ => None,
                    };
                    let action = inspector::inspector(
                        ui,
                        scene,
                        self.registry,
                        id,
                        true,
                        self.assets,
                        info.as_ref(),
                    );
                    match action {
                        Some(inspector::Action::Edit(label)) => {
                            self.panels.edit_label = Some(label)
                        }
                        Some(inspector::Action::Prefab(a)) => self.panels.prefab_action = Some(a),
                        None => {}
                    }
                }
                _ => {
                    ui.weak("Select an object in the Hierarchy or the Game view.");
                }
            },
            Tab::Debug => {
                let view = self.panels.game_rect_px(ui.ctx().pixels_per_point());
                let mut ctx =
                    Ctx::hosted(self.window, self.renderer, self.pads, self.event_loop, view);
                self.app.debug_ui(&mut ctx, ui);
            }
            Tab::Console => console(ui, self.panels),
            Tab::Project | Tab::Tilemap | Tab::Sprites | Tab::Animation => {
                let Some(db) = self.assets else {
                    ui.weak("This app has no asset database.");
                    return;
                };
                let panels = &mut *self.panels;
                match tab {
                    Tab::Project => assets_ui::project(ui, db, &mut panels.project),
                    Tab::Tilemap => panels.painter.ui(ui, db, &mut panels.project.thumbs),
                    Tab::Sprites => panels.slicer.ui(ui, db, &mut panels.project.thumbs),
                    _ => panels.anim.ui(ui, db, &mut panels.project.thumbs),
                }
                if tab.is_tool() && ui.ui_contains_pointer() {
                    panels.tool_hover_now = Some(*tab);
                }
            }
        }
    }
}

impl<A: EditorApp> Viewer<'_, A> {
    /// The frame, centred and letterboxed, at whole-number scale where it
    /// fits and scaled down to fit where it doesn't. While not playing, the
    /// scene tools work on it: click to select, drag to move.
    fn game(&mut self, ui: &mut egui::Ui) {
        let playing = self.play.mode() == PlayMode::Playing;
        if !playing && self.panels.drawn_scene.is_some() {
            let mut focus = false;
            ui.horizontal_wrapped(|ui| {
                focus = ui
                    .add_enabled(self.panels.selected.is_some(), egui::Button::new("Focus (F)"))
                    .clicked();
                if ui.button("Reset view").clicked() {
                    self.panels.scene_camera = None;
                }
                if let Some(camera) = self.panels.scene_camera {
                    ui.label(format!("{:.0}%", camera.zoom * 100.0));
                }
                ui.weak("Middle-drag / Space-drag: pan · Scroll: zoom · Drag object: move");
            });
            focus |= (self.panels.game_focused || ui.ui_contains_pointer())
                && !ui.ctx().egui_wants_keyboard_input()
                && ui.input(|i| i.key_pressed(egui::Key::F));
            if focus
                && let (Some(camera), Some(scene), Some(id)) =
                    (&mut self.panels.scene_camera, self.app.scene(), self.panels.selected)
                && scene.contains(id)
            {
                camera.focus(gizmo::world_box(scene, id));
            }
        }
        let space = ui.available_rect_before_wrap();
        ui.painter().rect_filled(space, 0.0, Color32::BLACK);
        self.panels.game_space = Some(space);
        let Some((id, (w, h))) = self.game_texture else { return };
        let ppp = ui.ctx().pixels_per_point();
        let mut size = Vec2::new(w as f32, h as f32) / ppp;
        if size.x > space.width() || size.y > space.height() {
            size *= (space.width() / size.x).min(space.height() / size.y);
        }
        // Land on whole physical pixels so 1:1 stays crisp.
        let min = ((space.center() - size / 2.0) * ppp).round() / ppp;
        let rect = Rect::from_min_size(min, size);
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        ui.painter().image(id, rect, uv, Color32::WHITE);
        if self.play.in_run() {
            ui.painter().rect_stroke(
                rect.expand(1.0),
                0.0,
                Stroke::new(2.0, PLAY_TINT),
                egui::StrokeKind::Outside,
            );
        } else if self.panels.game_focused {
            ui.painter().rect_stroke(
                rect.expand(1.0),
                0.0,
                Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Outside,
            );
        }
        self.panels.game_image = Some(rect);
        let playing = self.play.mode() == PlayMode::Playing;
        let sense = if playing { Sense::hover() } else { Sense::click_and_drag() };
        let response = ui.interact(rect, ui.id().with("game view"), sense);
        ui.allocate_rect(space, Sense::hover());
        let map = self.app.view_origin().map(|origin| ViewMap {
            image: rect,
            origin,
            zoom: self.panels.drawn_scene.map_or(1.0, |v| v.zoom),
        });
        let space_held = ui.input(|i| i.key_down(egui::Key::Space));
        let panning = !playing
            && self.panels.drawn_scene.is_some()
            && (response.dragged_by(egui::PointerButton::Middle)
                || (space_held && response.dragged_by(egui::PointerButton::Primary)));
        if !playing
            && self.panels.drawn_scene.is_some()
            && let Some(camera) = &mut self.panels.scene_camera
        {
            if panning {
                camera.pan(ui.input(|i| i.pointer.delta()), rect);
                self.panels.drag = None;
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if response.hovered() {
                let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                if scroll != 0.0
                    && let Some(p) = response.hover_pos()
                {
                    camera.zoom_at((scroll * 0.003).exp(), (p - rect.min) / rect.size());
                }
            }
        }
        self.panels.view_map = map;
        if !playing
            && let (Some(map), Some(drag)) = (map, response.dnd_release_payload::<AssetDrag>())
            && let Some(p) = response.interact_pointer_pos().or(response.hover_pos())
        {
            let (x, y) = map.to_world(p);
            let at = (x.round() as i32, y.round() as i32);
            match self.app.create_from_asset(drag.guid, drag.sprite.as_deref(), at) {
                Some(id) => {
                    self.panels.selected = Some(id);
                    self.panels.edit_label = Some("Create from asset".into());
                }
                None => {
                    self.panels.status = Some("That asset doesn't make an object here".into());
                }
            }
        }
        if !playing && response.dnd_hover_payload::<AssetDrag>().is_some() {
            ui.painter().rect_stroke(
                rect,
                0.0,
                Stroke::new(2.0, Color32::from_rgb(120, 200, 120)),
                egui::StrokeKind::Inside,
            );
        }
        let (Some(map), Some(scene)) = (map, self.app.scene_mut()) else { return };
        if !playing && !panning && !space_held {
            let snap = ui.input(|i| i.modifiers.command);
            let moved = gizmo::interact(
                &response,
                &map,
                scene,
                &mut self.panels.selected,
                &mut self.panels.drag,
                snap,
            );
            if moved {
                self.panels.edit_label = Some("Move".into());
            }
        }
        let hover = if playing { None } else { response.hover_pos() };
        gizmo::draw(ui.painter(), &map, scene, self.panels.selected, hover);
    }
}

fn console(ui: &mut egui::Ui, panels: &mut Panels) {
    use log::Level;
    const LEVELS: [Level; 5] = [Level::Error, Level::Warn, Level::Info, Level::Debug, Level::Trace];
    ui.horizontal(|ui| {
        for level in LEVELS {
            ui.checkbox(&mut panels.levels[level as usize - 1], level.as_str());
        }
        ui.separator();
        if ui.button("Clear").clicked() {
            panels.console.clear();
        }
        ui.weak(format!("{} lines", panels.console.len()));
    });
    ui.separator();
    let levels = panels.levels;
    let lines = panels.console.lines(|l| levels[l.level as usize - 1]);
    let row = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::both().auto_shrink(false).stick_to_bottom(true).show_rows(
        ui,
        row,
        lines.len(),
        |ui, range| {
            for line in &lines[range] {
                let color = match line.level {
                    Level::Error => Color32::from_rgb(240, 100, 100),
                    Level::Warn => Color32::from_rgb(235, 190, 90),
                    Level::Info => ui.visuals().text_color(),
                    Level::Debug | Level::Trace => ui.visuals().weak_text_color(),
                };
                let text = format!(
                    "{:>8.3} {:<5} {}: {}",
                    line.time, line.level, line.target, line.message
                );
                let text = egui::RichText::new(text).monospace().color(color);
                ui.add(egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend));
            }
        },
    );
}
