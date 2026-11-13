//! The editor's view of a project: the [`ReignEngine`] app it hosts.

use std::path::{Path, PathBuf};

use reign::app::winit::event::WindowEvent;
use reign::app::{App, Ctx, TickCtx};
use reign::assets::{AssetDb, AssetKind, Guid, Handle};
use reign::behaviour::{Behaviours, Schedule};
use reign::core::FxVec2;
use reign::editor::{EditorApp, EditorRequests, PlayMode, SceneView, egui};
use reign::render::{Color, Renderer};
use reign::scene::{ComponentRegistry, ObjectId, Rgba8, Scene, Transform};
use serde::{Deserialize, Serialize};

use crate::audio::Audio;
use crate::code::Code;
use crate::components::{Camera, Image, SpriteRenderer, TilemapRenderer, sprite_size};
use crate::runtime::{self, Keyboard, Scripts, World, schedule};
use crate::view::{CLEAR, Sprites};
use crate::{DB, physics, player, starter};

/// The file that marks a folder as a Reign project (Reign Hub reads it).
pub const PROJECT_FILE: &str = "ReignProject.ron";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct ProjectFile {
    name: String,
    engine_version: String,
}

pub struct Project {
    pub root: PathBuf,
}

impl Project {
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }

    pub fn scenes(&self) -> PathBuf {
        self.assets().join("scenes")
    }

    pub fn open(root: PathBuf) -> std::io::Result<Project> {
        let project = Project { root };
        std::fs::create_dir_all(project.scenes())?;
        // Marks the folder as a project for Reign Hub.
        let marker = project.root.join(PROJECT_FILE);
        if !marker.exists() {
            let file = ProjectFile {
                name: project
                    .root
                    .file_name()
                    .map_or_else(|| "Project".to_owned(), |n| n.to_string_lossy().into_owned()),
                engine_version: env!("CARGO_PKG_VERSION").to_owned(),
            };
            let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default())
                .map_err(std::io::Error::other)?;
            std::fs::write(&marker, text)?;
        }
        if let Err(e) = starter::install(&project.assets()) {
            log::warn!("couldn't add the built-in assets: {e}");
        }
        // How to work on the project without the window, for AI agents.
        if let Err(e) = reign_build::project::write_agent_guide(&project.root, &project.title()) {
            log::warn!("couldn't write {}: {e}", reign_build::project::AGENT_GUIDE);
        }
        let main = project.scenes().join("main.scene.ron");
        if !main.exists() {
            let mut scene = Scene::new();
            let camera = scene.spawn("Main Camera");
            scene.insert(camera, Camera::default());
            let text = scene.to_ron().map_err(|e| std::io::Error::other(e.to_string()))?;
            std::fs::write(&main, text)?;
        }
        Ok(project)
    }

    /// The name in `ReignProject.ron`, else the folder's name.
    pub fn title(&self) -> String {
        std::fs::read_to_string(self.root.join(PROJECT_FILE))
            .ok()
            .and_then(|text| ron::from_str::<ProjectFile>(&text).ok())
            .map(|p| p.name)
            .filter(|n| !n.is_empty())
            .or_else(|| self.root.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "Untitled".into())
    }
}

pub struct ReignEngine {
    pub(crate) project: Project,
    pub(crate) registry: ComponentRegistry,
    pub(crate) world: World,
    behaviours: Behaviours<World>,
    schedule: Schedule<World>,
    keyboard: Keyboard,
    audio: Audio,
    /// Whether the last frame was in Play, to hear it stop.
    was_playing: bool,
    code: Code,
    scene_file: Option<PathBuf>,
    sprites: Sprites,
    view_origin: (f32, f32),
    show_origin: bool,
}

impl ReignEngine {
    pub fn new(project: Project, scripts: Scripts, scene: Option<PathBuf>) -> Self {
        let registry = player::registry(&scripts);
        let behaviours = player::behaviours(&scripts);
        let code = Code::new(project.root.clone(), project.title(), scripts.project);
        let mut app = ReignEngine {
            project,
            registry,
            world: World::new(Scene::new()),
            behaviours,
            schedule: schedule(),
            keyboard: Keyboard::default(),
            audio: Audio::default(),
            was_playing: false,
            code,
            scene_file: None,
            sprites: Sprites::default(),
            view_origin: (0.0, 0.0),
            show_origin: true,
        };
        let path = scene.unwrap_or_else(|| app.project.scenes().join("main.scene.ron"));
        if let Err(e) = app.open_scene(&path) {
            log::error!("{}: {e}", path.display());
        }
        app
    }

    pub(crate) fn scene(&self) -> &Scene {
        &self.world.scene
    }

    fn db(&self) -> Option<&'static AssetDb> {
        DB.get()
    }

    /// Green outlines on every collider, in the edit view: whole boxes,
    /// and the outer edges of solid tiles (one-way tops in yellow).
    fn draw_colliders(&self, r: &mut Renderer, view: SceneView) {
        let line = Color::rgba8(120, 230, 140, 220);
        let one_way = Color::rgba8(230, 220, 110, 220);
        let (w, h) = view.size();
        let (x, y) = (view.origin.0.floor() as i32, view.origin.1.floor() as i32);
        let seen = physics::Rect { x, y, w: w.ceil() as i32 + 1, h: h.ceil() as i32 + 1 };
        for (e, top) in physics::tile_outlines(self.scene(), seen) {
            let color = if top { one_way } else { line };
            r.rect(e.x as f32, e.y as f32, e.w as f32, e.h as f32, color);
        }
        for (_, b) in physics::colliders(self.scene(), &sprite_size) {
            let (x, y, w, h) = (b.x as f32, b.y as f32, b.w as f32, b.h as f32);
            r.rect(x, y, w, 1.0, line);
            r.rect(x, y + h - 1.0, w, 1.0, line);
            r.rect(x, y + 1.0, 1.0, h - 2.0, line);
            r.rect(x + w - 1.0, y + 1.0, 1.0, h - 2.0, line);
        }
    }

    /// A Tilemap object, its top-left at `at`, drawing (and colliding
    /// with) a new starter map in `assets/tilemaps/`. Without a writable
    /// project the map is left empty for the Inspector.
    fn create_tilemap(&mut self, parent: Option<ObjectId>, at: (i32, i32)) -> ObjectId {
        let tilemap = match self.db().map(crate::tilemap_assets::new_tilemap) {
            Some(Ok(handle)) => handle,
            Some(Err(e)) => {
                log::warn!("couldn't make a tilemap asset: {e}");
                Handle::NONE
            }
            None => Handle::NONE,
        };
        let scene = &mut self.world.scene;
        let id = match parent {
            Some(p) => scene.spawn_child(p, "Tilemap"),
            None => scene.spawn("Tilemap"),
        };
        scene.set_world_position(id, FxVec2::from_px(at.0, at.1));
        scene.insert(id, TilemapRenderer { tilemap, ..TilemapRenderer::default() });
        scene.insert(id, physics::TilemapCollider::default());
        id
    }

    fn draw_origin(&self, r: &mut Renderer) {
        if self.show_origin {
            let axis = Color::rgba8(255, 255, 255, 60);
            r.rect(-4000.0, 0.0, 8000.0, 1.0, axis);
            r.rect(0.0, -4000.0, 1.0, 8000.0, axis);
        }
    }
}

impl App for ReignEngine {
    fn tick(&mut self, ctx: &mut TickCtx) {
        let input = self.keyboard.frame(Some(ctx.pads()));
        let events = runtime::tick(
            &mut self.world,
            input,
            &self.behaviours,
            &self.schedule,
            &self.registry,
            DB.get(),
        );
        self.audio.play(events, DB.get());
    }

    fn draw(&mut self, r: &mut Renderer, _alpha: f32) {
        self.view_origin = self.sprites.draw_game(r, &self.world.scene, DB.get());
    }

    fn event(&mut self, _ctx: &mut Ctx, event: &WindowEvent) {
        self.keyboard.event(event);
    }
}

impl EditorApp for ReignEngine {
    type Snapshot = World;

    fn snapshot(&self) -> World {
        self.world.clone()
    }

    fn restore(&mut self, snapshot: World) {
        self.world = snapshot;
    }

    fn set_play_mode(&mut self, mode: PlayMode) {
        if mode != PlayMode::Playing {
            self.keyboard.clear();
        }
        // Called every frame: silence only when Play ends (pausing lets
        // sounds finish).
        let playing = mode != PlayMode::Edit;
        if self.was_playing && !playing {
            self.audio.stop();
        }
        self.was_playing = playing;
        self.code.set_mode(mode);
    }

    fn scene(&self) -> Option<&Scene> {
        Some(&self.world.scene)
    }

    fn scene_mut(&mut self) -> Option<&mut Scene> {
        Some(&mut self.world.scene)
    }

    fn registry(&self) -> Option<&ComponentRegistry> {
        Some(&self.registry)
    }

    fn scene_path(&self) -> Option<PathBuf> {
        self.scene_file.clone()
    }

    fn scene_files(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(self.project.scenes())
            .map(|dir| {
                dir.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.to_string_lossy().ends_with(".scene.ron"))
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        files
    }

    fn open_scene(&mut self, path: &Path) -> Result<(), String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let scene = Scene::from_ron(&text, &self.registry).map_err(|e| e.to_string())?;
        self.world = World::new(scene);
        self.scene_file = Some(path.to_owned());
        Ok(())
    }

    fn view_origin(&self) -> Option<(f32, f32)> {
        Some(self.view_origin)
    }

    fn draw_scene(&mut self, r: &mut Renderer, view: SceneView) -> bool {
        r.set_draw_scale(view.zoom);
        r.set_offset(-view.origin.0, -view.origin.1);
        self.view_origin = view.origin;
        self.draw_origin(r);
        if let Some(db) = DB.get() {
            let (w, h) = view.size();
            let seen = [view.origin.0, view.origin.1, w, h];
            self.sprites.draw_objects(r, &self.world.scene, db, seen);
        }
        self.draw_colliders(r, view);
        r.present(CLEAR);
        true
    }

    fn toolbar_ui(&mut self, ui: &mut egui::Ui) -> EditorRequests {
        let scene_file = self.scene_file.clone();
        self.code.toolbar_ui(ui, scene_file.as_deref())
    }

    fn debug_ui(&mut self, _ctx: &mut Ctx, ui: &mut egui::Ui) {
        ui.label(format!("Project: {}", self.project.root.display()));
        ui.label(format!("Objects: {}", self.world.scene.len()));
        ui.checkbox(&mut self.show_origin, "Show world origin");
    }

    fn assets(&self) -> Option<&'static AssetDb> {
        self.db()
    }

    fn create_items(&self) -> Vec<String> {
        let mut items = vec!["Camera".to_owned(), "2D Object/Sprite".to_owned()];
        items.extend(starter::SHAPES.iter().map(|s| format!("2D Object/Sprites/{}", s.name)));
        items.push("2D Object/Physics/Dynamic Box".to_owned());
        items.push("2D Object/Physics/Static Platform".to_owned());
        items.push("2D Object/Tilemap".to_owned());
        items.push("2D Object/Text".to_owned());
        items
    }

    fn create(&mut self, item: &str, parent: Option<ObjectId>, at: (i32, i32)) -> Option<ObjectId> {
        if item == "2D Object/Tilemap" {
            return Some(self.create_tilemap(parent, at));
        }
        if item == "2D Object/Text" {
            let scene = &mut self.world.scene;
            let id = match parent {
                Some(p) => scene.spawn_child(p, "Text"),
                None => scene.spawn("Text"),
            };
            scene.set_world_position(id, FxVec2::from_px(at.0, at.1));
            scene.insert(id, crate::components::Text::default());
            return Some(id);
        }
        let shape_renderer =
            |shape: &starter::Shape, size: [u16; 2], color: Rgba8| SpriteRenderer {
                sheet: starter::shapes_sheet(),
                sprite: shape.sprite.to_owned(),
                size,
                color,
            };
        let square = starter::shape("Square")?;
        let (renderer, collider, body) = match item {
            "Camera" => (None, false, false),
            "2D Object/Sprite" => (Some(SpriteRenderer::default()), false, false),
            "2D Object/Physics/Dynamic Box" => {
                (Some(shape_renderer(square, [24, 24], Rgba8::rgb(228, 46, 52))), true, true)
            }
            "2D Object/Physics/Static Platform" => {
                (Some(shape_renderer(square, [192, 16], Rgba8::rgb(120, 110, 112))), true, false)
            }
            _ => {
                let shape = item.strip_prefix("2D Object/Sprites/").and_then(starter::shape)?;
                (
                    Some(shape_renderer(shape, shape.size.map(|v| v as u16), Rgba8::WHITE)),
                    false,
                    false,
                )
            }
        };
        let name = item.rsplit('/').next().unwrap_or(item);
        let scene = &mut self.world.scene;
        let id = match parent {
            Some(p) => scene.spawn_child(p, name),
            None => scene.spawn(name),
        };
        scene.set_world_position(id, FxVec2::from_px(at.0, at.1));
        match renderer {
            Some(sr) => {
                scene.insert(id, sr);
            }
            None => {
                scene.insert(id, Camera::default());
            }
        }
        if collider {
            scene.insert(id, physics::BoxCollider::default());
        }
        if body {
            scene.insert(id, physics::Rigidbody::default());
        }
        Some(id)
    }

    fn create_from_asset(
        &mut self,
        asset: Guid,
        sprite: Option<&str>,
        at: (i32, i32),
    ) -> Option<ObjectId> {
        let db = self.db()?;
        let path = db.path(asset)?;
        let name = Path::new(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.split('.').next().unwrap_or(n).to_owned())
            .unwrap_or_else(|| "Object".into());
        let component = match db.kind(asset)? {
            AssetKind::Texture => None,
            AssetKind::SpriteSheet => Some(sprite.unwrap_or_default().to_owned()),
            _ => return None,
        };
        let scene = &mut self.world.scene;
        let id = scene.spawn(sprite.map(str::to_owned).unwrap_or(name));
        *scene.transform_mut(id)? = Transform::at_px(at.0, at.1);
        match component {
            None => {
                scene.insert(id, Image { texture: Handle::new(asset) });
            }
            Some(sprite) => {
                scene.insert(
                    id,
                    SpriteRenderer { sheet: Handle::new(asset), sprite, ..Default::default() },
                );
            }
        }
        Some(id)
    }
}
