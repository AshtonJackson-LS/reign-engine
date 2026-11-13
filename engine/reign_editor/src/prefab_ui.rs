//! Prefabs in the editor: Prefab Mode, the instance header and overrides
//! in the Inspector, making prefabs from objects, and keeping instances in
//! step when a prefab changes.
//!
//! **Prefab Mode** edits a prefab on its own: the open scene is set aside
//! (with its undo history and selection) and the prefab's objects take its
//! place in the Hierarchy and Inspector. The Game view shows the prefab's
//! objects on a canvas. Ctrl+S saves the prefab; **Back to scene** brings
//! the scene back and re-stamps its instances.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
use reign_assets::prefab::{self, PrefabInstance, PrefabLookup};
use reign_assets::{AssetDb, Guid, Handle, Prefab};
use reign_scene::{ObjectId, Scene};

use crate::history::History;

/// What the open scene was before Prefab Mode took its place.
pub(crate) struct PrefabMode {
    pub guid: Guid,
    pub name: String,
    pub scene: Scene,
    pub history: History,
    pub saved: Option<u64>,
    pub selected: Option<ObjectId>,
}

/// The prefab instance an object belongs to (itself or the nearest
/// ancestor that is one and made it).
pub(crate) fn instance_root(scene: &Scene, id: ObjectId) -> Option<ObjectId> {
    let mut at = Some(id);
    while let Some(a) = at {
        if let Some(inst) = scene.get::<PrefabInstance>(a)
            && (a == id || inst.path_of(id).is_some())
        {
            return Some(a);
        }
        at = scene.parent(a);
    }
    None
}

/// What the Inspector shows about an object's prefab.
pub(crate) struct PrefabInfo {
    pub root: ObjectId,
    pub guid: Guid,
    pub name: String,
    /// Overridden properties of the selected object (`Component.field`,
    /// `GameObject.name`, ...).
    pub overridden: Vec<String>,
    /// Whether the instance overrides anything at all.
    pub any: bool,
}

pub(crate) fn info(
    scene: &mut Scene,
    id: ObjectId,
    lookup: &dyn PrefabLookup,
    db: &AssetDb,
) -> Option<PrefabInfo> {
    let root = instance_root(scene, id)?;
    let inst = scene.get::<PrefabInstance>(root)?.clone();
    let path = inst.path_of(id).cloned().unwrap_or_default();
    let fresh = prefab::overrides(scene, root, lookup).ok()?;
    let mut overridden: Vec<String> =
        fresh.overrides.iter().filter(|o| o.object == path).map(|o| o.path.clone()).collect();
    overridden.extend(fresh.added.iter().filter(|a| a.object == path).map(|a| a.component.clone()));
    let name = db.path(inst.prefab.guid).unwrap_or_else(|| "missing prefab".into());
    Some(PrefabInfo { root, guid: inst.prefab.guid, name, overridden, any: fresh.has_overrides() })
}

/// Stores every saved instance's overrides on it (after an edit), so a
/// later change to its prefab keeps them.
pub(crate) fn sync_all(scene: &mut Scene, lookup: &dyn PrefabLookup) {
    let roots: Vec<ObjectId> = scene
        .walk()
        .into_iter()
        .filter(|id| {
            scene.object(*id).is_some_and(|o| !o.dont_save) && scene.has::<PrefabInstance>(*id)
        })
        .collect();
    for root in roots {
        let _ = prefab::sync_overrides(scene, root, lookup);
    }
}

/// Re-stamps every saved instance of `guid` (every prefab, for `None`) in
/// `scene` from the current prefab, keeping its stored overrides. Objects placed
/// by the game at runtime are left alone. Returns how many changed.
pub(crate) fn refresh_instances(
    scene: &mut Scene,
    guid: Option<Guid>,
    lookup: &dyn PrefabLookup,
) -> usize {
    let roots: Vec<ObjectId> = scene
        .walk()
        .into_iter()
        .filter(|id| {
            scene.object(*id).is_some_and(|o| !o.dont_save)
                && scene
                    .get::<PrefabInstance>(*id)
                    .is_some_and(|i| guid.is_none_or(|g| i.prefab.guid == g))
        })
        .collect();
    let mut changed = 0;
    for root in roots {
        match prefab::refresh(scene, root, lookup) {
            Ok(()) => changed += 1,
            Err(e) => log::warn!("couldn't update a prefab instance: {e}"),
        }
    }
    changed
}

/// Saves the tree at `id` as a new prefab in `folder`, and makes the
/// object an instance of it. Returns the new prefab's path.
pub(crate) fn create(
    scene: &mut Scene,
    id: ObjectId,
    folder: &str,
    db: &AssetDb,
    lookup: &dyn PrefabLookup,
) -> Result<String, String> {
    let text = prefab::prefab_from(scene, id, lookup)?;
    let stem: String = scene
        .name(id)
        .unwrap_or("Prefab")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect();
    let base = if folder.is_empty() { String::new() } else { format!("{folder}/") };
    let mut path = format!("{base}{stem}.prefab.ron");
    let mut n = 2;
    while db.guid(&path).is_some() {
        path = format!("{base}{stem}_{n}.prefab.ron");
        n += 1;
    }
    let guid = db.create(&path, text.as_bytes()).map_err(|e| e.to_string())?;
    prefab::make_instance(scene, id, Handle::<Prefab>::new(guid), lookup)?;
    Ok(path)
}

/// The prefab's objects, ready to edit on their own: the root is a plain
/// object, nested prefabs stay instances.
pub(crate) fn open(guid: Guid, lookup: &dyn PrefabLookup) -> Result<Scene, String> {
    let template = lookup.template(Handle::new(guid))?;
    let mut scene = template.scene.clone();
    prefab::unpack(&mut scene, template.root);
    Ok(scene)
}

/// The prefab file for a scene edited in Prefab Mode.
pub(crate) fn save_text(scene: &Scene, lookup: &dyn PrefabLookup) -> Result<String, String> {
    let root = *scene.roots().first().ok_or("the prefab has no objects")?;
    prefab::prefab_from(scene, root, lookup)
}

/// The Game view in Prefab Mode: the prefab's objects drawn as their click
/// boxes around the root, names beside them. Click to select.
pub(crate) fn canvas(ui: &mut Ui, scene: &Scene, name: &str, selected: &mut Option<ObjectId>) {
    let space = ui.available_rect_before_wrap();
    let (rect, response) = ui.allocate_exact_size(space.size(), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(38, 42, 52));
    let scale = 4.0;
    let origin = Pos2::new(rect.center().x, rect.center().y + rect.height() * 0.2);
    // A ground line at the root's origin and a grid every 16 px.
    for i in -12..=12 {
        let x = origin.x + i as f32 * 16.0 * scale;
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0, Color32::from_rgb(48, 52, 64)),
        );
    }
    painter.line_segment(
        [Pos2::new(rect.left(), origin.y), Pos2::new(rect.right(), origin.y)],
        Stroke::new(1.0, Color32::from_rgb(90, 100, 120)),
    );
    let mut hit = None;
    for id in scene.walk() {
        let at = scene.world_position(id);
        let b = scene.bounds(id).unwrap_or(reign_scene::Bounds::new(-8, -8, 16, 16));
        let min =
            origin + Vec2::new(at.x.to_f32() + b.x as f32, at.y.to_f32() + b.y as f32) * scale;
        let r = Rect::from_min_size(min, Vec2::new(b.w as f32, b.h as f32) * scale);
        let chosen = *selected == Some(id);
        let colour = if chosen { Color32::from_rgb(255, 200, 80) } else { Color32::LIGHT_BLUE };
        painter.rect_stroke(
            r,
            1.0,
            Stroke::new(if chosen { 2.0 } else { 1.0 }, colour),
            egui::StrokeKind::Inside,
        );
        painter.text(
            r.left_top() + Vec2::new(2.0, -2.0),
            egui::Align2::LEFT_BOTTOM,
            scene.name(id).unwrap_or("?"),
            egui::FontId::proportional(12.0),
            colour,
        );
        if response.clicked() && response.interact_pointer_pos().is_some_and(|p| r.contains(p)) {
            hit = Some(id);
        }
    }
    if response.clicked() {
        *selected = hit;
    }
    painter.text(
        rect.left_top() + Vec2::new(8.0, 8.0),
        egui::Align2::LEFT_TOP,
        format!("Prefab Mode: {name}"),
        egui::FontId::proportional(14.0),
        Color32::WHITE,
    );
}

/// The breadcrumb over the Hierarchy in Prefab Mode. True when asked to go
/// back to the scene.
pub(crate) fn breadcrumb(ui: &mut Ui, name: &str, dirty: bool) -> (bool, bool) {
    let (mut back, mut save) = (false, false);
    ui.horizontal(|ui| {
        back = ui.button("< Scene").on_hover_text("Back to the scene").clicked();
        ui.label(RichText::new(format!("Prefab: {name}{}", if dirty { "*" } else { "" })).strong());
        save = ui.button("Save").on_hover_text("Save the prefab (Ctrl+S)").clicked();
    });
    ui.separator();
    (back, save)
}

#[cfg(test)]
mod tests {
    use reign_assets::prefab::MemoryPrefabs;
    use reign_scene::{Component, ComponentRegistry};
    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
    struct Hp {
        max: i32,
    }

    fn lookup(max: i32) -> MemoryPrefabs {
        let mut registry = ComponentRegistry::new();
        registry.register::<Hp>();
        let mut p = MemoryPrefabs::new(registry);
        let mut s = Scene::new();
        let root = s.spawn("Thing");
        s.insert(root, Hp { max });
        let child = s.spawn_child(root, "Part");
        s.insert(child, Hp { max: 1 });
        p.texts.insert(Guid(7), s.to_ron().unwrap());
        p
    }

    #[test]
    fn objects_find_their_instance_and_instances_follow_their_prefab() {
        let old = lookup(5);
        let mut scene = Scene::new();
        let root = scene.spawn("Placed");
        scene.insert(root, PrefabInstance::new(Handle::new(Guid(7))));
        prefab::expand(&mut scene, root, &old).unwrap();
        let part = scene.children(root)[0];
        assert_eq!(instance_root(&scene, part), Some(root));
        assert_eq!(instance_root(&scene, root), Some(root));
        // An override on the part, then the prefab changes underneath.
        scene.get_mut::<Hp>(part).unwrap().max = 9;
        let new = lookup(50);
        sync_all(&mut scene, &old);
        assert_eq!(refresh_instances(&mut scene, Some(Guid(7)), &new), 1);
        assert_eq!(scene.get::<Hp>(root).unwrap().max, 50, "follows the prefab");
        let part = scene.children(root)[0];
        assert_eq!(scene.get::<Hp>(part).unwrap().max, 9, "keeps the override");
        // Prefab Mode opens the prefab with a plain root, and saves it back.
        let opened = open(Guid(7), &new).unwrap();
        let r = opened.roots()[0];
        assert!(!opened.has::<PrefabInstance>(r));
        assert!(save_text(&opened, &new).unwrap().contains("max: 50"));
    }
}
