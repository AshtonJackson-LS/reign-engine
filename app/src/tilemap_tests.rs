//! Tilemaps in scenes: tilemap colliders in scene physics, their outlines,
//! scene files, and the starter tilemap the GameObject menu makes.

use reign::assets::{AssetDb, Guid, Tileset};
use reign::scene::{ComponentRegistry, Rgba8};

use super::*;
use crate::components::register_builtins;
use crate::tilemap_assets;

const MAP: Guid = Guid(0x7469_6c65_6d61_7000_0000_0000_0000_0001);

/// 24 x 8 cells: a floor (top at y 112), a one-way plank over columns
/// 6-11 (top at y 48) and a wall in column 18 (left side at x 288).
const LEVEL: &[&str] = &[
    "........................",
    "........................",
    "........................",
    "......------............",
    "..................#.....",
    "..................#.....",
    "..................#.....",
    "########################",
];

fn level(rows: &[&str]) -> Arc<Tilemap> {
    Arc::new(Tilemap {
        collision: rows.iter().map(|r| (*r).to_owned()).collect(),
        ..Tilemap::default()
    })
}

fn fit16(_: &Scene, _: ObjectId) -> Option<[i32; 2]> {
    Some([16, 16])
}

fn tilemap_at(scene: &mut Scene, x: i32, y: i32) -> ObjectId {
    let id = scene.spawn("Tilemap");
    scene.set_world_position(id, FxVec2::from_px(x, y));
    scene.insert(id, TilemapRenderer { tilemap: Handle::new(MAP), ..TilemapRenderer::default() });
    scene.insert(id, TilemapCollider::default());
    id
}

fn crate_at(scene: &mut Scene, x: i32, y: i32) -> ObjectId {
    let id = scene.spawn("Box");
    scene.set_world_position(id, FxVec2::from_px(x, y));
    scene.insert(id, BoxCollider::default());
    scene.insert(id, Rigidbody::default());
    id
}

/// One tick with `map` as the only tilemap asset.
fn tick(scene: &mut Scene, map: &Arc<Tilemap>) -> Vec<Contact> {
    step_with(scene, &fit16, &|h: Handle<Tilemap>| (h.guid == MAP).then(|| map.clone()))
}

fn run(scene: &mut Scene, map: &Arc<Tilemap>, ticks: u32) -> Vec<Vec<Contact>> {
    (0..ticks).map(|_| tick(scene, map)).collect()
}

fn body(scene: &mut Scene, id: ObjectId) -> &mut Rigidbody {
    scene.get_mut::<Rigidbody>(id).expect("rigidbody")
}

#[test]
fn a_box_lands_on_solid_tiles_at_the_exact_pixel() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    tilemap_at(&mut scene, 0, 0);
    let b = crate_at(&mut scene, 40, 0);
    run(&mut scene, &map, 240);
    // Floor top 112; the box's centre rests 8 above.
    assert_eq!(scene.world_position(b), FxVec2::from_px(40, 104));
    let rb = scene.get::<Rigidbody>(b).unwrap();
    assert_eq!(rb.velocity.y, Fx::ZERO);
    assert!(rb.grounded, "standing on the tiles");
}

#[test]
fn the_grid_starts_at_the_objects_position() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let parent = scene.spawn("Level");
    scene.set_world_position(parent, FxVec2::from_px(-100, 30));
    let tm = scene.spawn_child(parent, "Tilemap");
    scene.set_world_position(tm, FxVec2::from_px(-100, 50));
    scene.insert(tm, TilemapRenderer { tilemap: Handle::new(MAP), ..TilemapRenderer::default() });
    scene.insert(tm, TilemapCollider::default());
    let b = crate_at(&mut scene, -60, 0);
    run(&mut scene, &map, 240);
    assert_eq!(scene.world_position(b), FxVec2::from_px(-60, 50 + 112 - 8));
}

#[test]
fn walls_of_tiles_stop_sideways_motion() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    tilemap_at(&mut scene, 0, 0);
    // Floating, in front of the wall.
    let floating = crate_at(&mut scene, 200, 88);
    *body(&mut scene, floating) = Rigidbody {
        gravity_scale: Fx::ZERO,
        velocity: FxVec2::from_px(600, 0),
        ..Rigidbody::default()
    };
    // Walking along the floor into it.
    let walking = crate_at(&mut scene, 220, 104);
    body(&mut scene, walking).velocity = FxVec2::from_px(300, 0);
    run(&mut scene, &map, 120);
    // The wall's left side is 288: centres stop 8 short.
    assert_eq!(scene.world_position(floating), FxVec2::from_px(280, 88));
    assert_eq!(scene.world_position(walking), FxVec2::from_px(280, 104));
    assert_eq!(scene.get::<Rigidbody>(floating).unwrap().velocity.x, Fx::ZERO);

    // And from the other side.
    let left = crate_at(&mut scene, 360, 88);
    *body(&mut scene, left) = Rigidbody {
        gravity_scale: Fx::ZERO,
        velocity: FxVec2::from_px(-600, 0),
        ..Rigidbody::default()
    };
    run(&mut scene, &map, 120);
    assert_eq!(scene.world_position(left), FxVec2::from_px(312, 88));
}

#[test]
fn one_way_tiles_let_bodies_up_and_catch_them_falling() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    tilemap_at(&mut scene, 0, 0);
    // On the floor under the plank, jumping.
    let b = crate_at(&mut scene, 144, 104);
    body(&mut scene, b).velocity = FxVec2::from_px(0, -400);
    let mut highest = 104;
    for _ in 0..240 {
        tick(&mut scene, &map);
        highest = highest.min(scene.world_position(b).y.round_px());
    }
    assert!(highest < 40, "went up through the plank (to {highest})");
    // The plank's top is 48: it lands there.
    assert_eq!(scene.world_position(b), FxVec2::from_px(144, 40));
    assert!(scene.get::<Rigidbody>(b).unwrap().grounded);

    // Sideways through it, too.
    let side = crate_at(&mut scene, 60, 56);
    *body(&mut scene, side) = Rigidbody {
        gravity_scale: Fx::ZERO,
        velocity: FxVec2::from_px(240, 0),
        ..Rigidbody::default()
    };
    run(&mut scene, &map, 60);
    assert_eq!(scene.world_position(side), FxVec2::from_px(180, 56));
}

#[test]
fn layer_masks_can_ignore_the_tilemap() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let tm = tilemap_at(&mut scene, 0, 0);
    scene.get_mut::<TilemapCollider>(tm).unwrap().layer = 3;
    // This box doesn't collide with layer 3.
    let ghost = crate_at(&mut scene, 40, 0);
    scene.get_mut::<BoxCollider>(ghost).unwrap().mask = !(1 << 3);
    // The tilemap doesn't collide with layer 5.
    let unseen = crate_at(&mut scene, 72, 0);
    scene.get_mut::<BoxCollider>(unseen).unwrap().layer = 5;
    scene.get_mut::<TilemapCollider>(tm).unwrap().mask = !(1 << 5);
    // An ordinary box still lands.
    let solid = crate_at(&mut scene, 232, 0);
    let contacts = run(&mut scene, &map, 120);
    assert!(scene.world_position(ghost).y > Fx::from_px(200), "fell through");
    assert!(scene.world_position(unseen).y > Fx::from_px(200), "fell through");
    assert_eq!(scene.world_position(solid).y, Fx::from_px(104));
    let touched_ghosts =
        contacts.iter().flatten().any(|c| c.other(tm).is_some_and(|o| o == ghost || o == unseen));
    assert!(!touched_ghosts, "no contacts across masked layers");
}

#[test]
fn contacts_with_the_tilemap_come_and_go() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let tm = tilemap_at(&mut scene, 0, 0);
    let b = crate_at(&mut scene, 40, 60);
    let landed = Contact { a: tm, b, trigger: false };
    let mut first = None;
    for t in 0..240 {
        let now = tick(&mut scene, &map);
        if first.is_none() && !now.is_empty() {
            first = Some(t);
            // Touching exactly when it reaches the floor, not before.
            assert_eq!(now, vec![landed]);
            assert_eq!(scene.world_position(b).y.round_px(), 104);
        }
    }
    assert!(first.is_some_and(|t| t > 0), "touched on landing");
    assert!(scene.get::<Rigidbody>(b).unwrap().grounded);
    assert_eq!(tick(&mut scene, &map), vec![landed], "still touching while resting");

    // Jumping off ends it.
    body(&mut scene, b).velocity = FxVec2::from_px(0, -300);
    assert_eq!(tick(&mut scene, &map), vec![]);
    assert!(!scene.get::<Rigidbody>(b).unwrap().grounded);

    // Standing on a one-way plank is touching it too.
    let on_plank = crate_at(&mut scene, 144, 20);
    run(&mut scene, &map, 120);
    let now = tick(&mut scene, &map);
    assert!(now.contains(&Contact { a: tm, b: on_plank, trigger: false }), "{now:?}");
}

#[test]
fn walls_touch_from_the_side() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let tm = tilemap_at(&mut scene, 0, 0);
    let b = crate_at(&mut scene, 200, 88);
    *body(&mut scene, b) = Rigidbody {
        gravity_scale: Fx::ZERO,
        velocity: FxVec2::from_px(600, 0),
        ..Rigidbody::default()
    };
    let contacts = run(&mut scene, &map, 60);
    assert_eq!(contacts.last().unwrap(), &vec![Contact { a: tm, b, trigger: false }]);
    assert!(!scene.get::<Rigidbody>(b).unwrap().grounded, "a wall isn't ground");
}

#[test]
fn trigger_tilemaps_report_overlaps_and_block_nothing() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let tm = tilemap_at(&mut scene, 0, 0);
    scene.get_mut::<TilemapCollider>(tm).unwrap().is_trigger = true;
    let b = crate_at(&mut scene, 40, 0);
    let contacts = run(&mut scene, &map, 240);
    assert!(scene.world_position(b).y > Fx::from_px(200), "fell through");
    let overlapped = contacts.iter().filter(|c| !c.is_empty()).count();
    assert!(overlapped > 0);
    for c in contacts.iter().flatten() {
        assert_eq!(*c, Contact { a: tm, b, trigger: true });
    }
    assert!(contacts.last().unwrap().is_empty(), "and stopped once it left");
}

#[test]
fn bouncy_boxes_bounce_on_tiles_then_settle() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    tilemap_at(&mut scene, 0, 0);
    let b = crate_at(&mut scene, 40, 0);
    body(&mut scene, b).bounciness = 70;
    let mut rose = false;
    for _ in 0..600 {
        let before = scene.world_position(b).y;
        tick(&mut scene, &map);
        rose |= scene.world_position(b).y < before;
    }
    assert!(rose, "it bounced");
    assert_eq!(scene.world_position(b).y, Fx::from_px(104), "and came to rest");
}

#[test]
fn missing_tilemaps_and_renderers_collide_with_nothing() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let empty = tilemap_at(&mut scene, 0, 0);
    scene.get_mut::<TilemapRenderer>(empty).unwrap().tilemap = Handle::NONE;
    let bare = scene.spawn("Collider only");
    scene.insert(bare, TilemapCollider::default());
    let b = crate_at(&mut scene, 40, 0);
    let contacts = run(&mut scene, &map, 120);
    assert!(scene.world_position(b).y > Fx::from_px(200));
    assert!(contacts.iter().all(Vec::is_empty));
}

#[test]
fn tilemaps_without_a_collider_are_only_drawn() {
    let map = level(LEVEL);
    let mut scene = Scene::new();
    let tm = tilemap_at(&mut scene, 0, 0);
    scene.remove::<TilemapCollider>(tm);
    let b = crate_at(&mut scene, 40, 0);
    run(&mut scene, &map, 120);
    assert!(scene.world_position(b).y > Fx::from_px(200));
}

#[test]
fn tilemap_runs_are_deterministic() {
    let map = level(LEVEL);
    let build = || {
        let mut scene = Scene::new();
        tilemap_at(&mut scene, 0, 0);
        for i in 0..8 {
            let b = crate_at(&mut scene, 24 + i * 37, -i * 23);
            let rb = body(&mut scene, b);
            rb.bounciness = (i * 12) as u8;
            rb.velocity = FxVec2::from_px((i % 3 - 1) * 170, -(i % 2) * 260);
        }
        let contacts = run(&mut scene, &map, 600);
        (scene, contacts)
    };
    let (a, ca) = build();
    let (b, cb) = build();
    assert!(a == b, "same scene");
    assert_eq!(ca, cb, "same contacts every tick");
}

#[test]
fn outlines_trace_solid_edges_and_one_way_tops() {
    let map = level(&["....", ".##.", "-..."]);
    let mut scene = Scene::new();
    tilemap_at(&mut scene, 10, 20);
    let tiles = |h: Handle<Tilemap>| (h.guid == MAP).then(|| map.clone());
    let everything = Rect { x: -100, y: -100, w: 400, h: 400 };
    let edges = outlines(&scene, everything, &tiles);
    let solid: Vec<Rect> = edges.iter().filter(|e| !e.1).map(|e| e.0).collect();
    let tops: Vec<Rect> = edges.iter().filter(|e| e.1).map(|e| e.0).collect();
    // Two cells side by side: two tops, two bottoms, the outer sides.
    assert_eq!(solid.len(), 6);
    assert!(solid.contains(&Rect { x: 26, y: 36, w: 1, h: 16 }), "left side");
    assert!(solid.contains(&Rect { x: 57, y: 36, w: 1, h: 16 }), "right side");
    assert!(!solid.iter().any(|r| r.x == 41 && r.w == 1), "no inner edge");
    assert_eq!(tops, vec![Rect { x: 10, y: 52, w: 16, h: 1 }]);
    // Only cells in view.
    let corner = Rect { x: 10, y: 52, w: 16, h: 16 };
    assert_eq!(outlines(&scene, corner, &tiles).len(), 1);
}

#[test]
fn scenes_with_tilemaps_save_and_load() {
    let mut registry = ComponentRegistry::new();
    register_builtins(&mut registry);
    let mut scene = Scene::new();
    let tm = tilemap_at(&mut scene, 32, -48);
    scene.get_mut::<TilemapRenderer>(tm).unwrap().color = Rgba8::rgb(200, 180, 255);
    *scene.get_mut::<TilemapCollider>(tm).unwrap() =
        TilemapCollider { is_trigger: true, layer: 4, mask: 0b1011 };
    let text = scene.to_ron().expect("saves");
    assert!(text.contains("TilemapRenderer") && text.contains("TilemapCollider"), "{text}");
    let back = Scene::from_ron(&text, &registry).expect("loads");
    assert_eq!(back.get::<TilemapRenderer>(tm), scene.get::<TilemapRenderer>(tm));
    assert_eq!(back.get::<TilemapCollider>(tm), scene.get::<TilemapCollider>(tm));
    assert_eq!(back.world_position(tm), FxVec2::from_px(32, -48));
    assert_eq!(back.get::<TilemapRenderer>(tm).unwrap().tilemap.guid, MAP);
}

#[test]
fn the_menu_makes_a_paintable_starter_tilemap() {
    let dir = std::env::temp_dir()
        .join(format!("reign-engine-test-tilemap-assets-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = AssetDb::loose(&dir).expect("assets");

    let first = tilemap_assets::new_tilemap(&db).expect("made");
    let second = tilemap_assets::new_tilemap(&db).expect("made another");
    assert_eq!(db.path(first.guid).as_deref(), Some("tilemaps/Tilemap.tilemap.ron"));
    assert_eq!(db.path(second.guid).as_deref(), Some("tilemaps/Tilemap 2.tilemap.ron"));
    assert!(dir.join("tilemaps/Tilemap.tilemap.ron.meta").exists());

    let map = db.load(first).expect("loads");
    let (w, h) = tilemap_assets::STARTER_SIZE;
    assert_eq!((map.width(), map.height()), (w, h));
    assert_eq!(map.markers.len(), h, "markers can be painted");
    assert_eq!(map.layers.len(), 1);
    // Every collision tile has art, and nothing else does.
    for y in 0..h {
        for x in 0..w {
            let solid = Tilemap::glyph(&map.collision, x, y) != '.';
            assert_eq!(map.layers[0].get(x, y).is_some(), solid, "({x}, {y})");
        }
    }
    // Both maps share the built-in tileset, which has its texture.
    assert_eq!(db.load(second).unwrap().tileset, map.tileset);
    let tileset: Arc<Tileset> = db.load(map.tileset).expect("tileset");
    assert_eq!(db.path(map.tileset.guid).as_deref(), Some(tilemap_assets::TILES_TILESET));
    let texture = db.load(tileset.texture).expect("texture");
    assert_eq!(texture.image.width, tileset.tile_size * tileset.columns);
    assert_eq!(texture.image.height, tileset.tile_size);

    // And a box dropped on it lands on the floor.
    let mut scene = Scene::new();
    let tm = scene.spawn("Tilemap");
    scene.insert(tm, TilemapRenderer { tilemap: first, ..TilemapRenderer::default() });
    scene.insert(tm, TilemapCollider::default());
    let b = crate_at(&mut scene, 40, 0);
    for _ in 0..240 {
        step_with(&mut scene, &fit16, &|h| db.load(h).ok());
    }
    let floor = (h as i32 - 1) * TILE;
    assert_eq!(scene.world_position(b).y, Fx::from_px(floor - 8));
    let _ = std::fs::remove_dir_all(&dir);
}
