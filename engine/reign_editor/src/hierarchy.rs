//! The Hierarchy: the scene's objects as a tree, built from parents and
//! children. Select, toggle active, drag onto another object to reparent
//! (or onto empty space for the top level), and right-click to create,
//! duplicate or delete.

use egui::{Color32, RichText, Sense, Ui};
use reign_scene::{ObjectId, Scene};

use crate::create::{self, CreateRequest};

/// An object being renamed in place, and the text typed so far.
#[derive(Clone, Debug)]
pub struct Rename {
    pub id: ObjectId,
    pub text: String,
    /// Give the text box the keyboard on its first frame.
    pub focus: bool,
}

impl Rename {
    pub fn start(scene: &Scene, id: ObjectId) -> Option<Rename> {
        Some(Rename { id, text: scene.name(id)?.to_owned(), focus: true })
    }
}

/// Draws the tree and applies what was done to it. Returns a label for the
/// undo step when something changed.
pub fn hierarchy(
    ui: &mut Ui,
    scene: &mut Scene,
    selected: &mut Option<ObjectId>,
    editable: bool,
    items: &[String],
    create: &mut Option<CreateRequest>,
    rename: &mut Option<Rename>,
) -> Option<String> {
    let mut edit = None;
    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            ui.menu_button("+ Create", |ui| create::menu(ui, items, None, create))
                .response
                .on_hover_text("Create an object at the top level (or right-click the tree)");
            ui.weak(format!("{} objects", scene.len()));
        });
        ui.separator();
        let mut ops = Ops::default();
        for &root in scene.roots().to_vec().iter() {
            let mut menus = Menus { items, create: &mut *create, rename: &mut *rename };
            row(ui, scene, root, selected, &mut ops, &mut menus);
        }
        // Dropping below the tree moves an object to the top level, and
        // right-clicking there creates one.
        let space = ui.available_size().max(egui::vec2(0.0, 32.0));
        let (rect, response) = ui.allocate_exact_size(space, Sense::click());
        response.context_menu(|ui| create::menu(ui, items, None, create));
        if response.clicked() {
            *selected = None;
        }
        if response.dnd_hover_payload::<ObjectId>().is_some() {
            let stroke = (1.0, Color32::GRAY);
            ui.painter().rect_stroke(rect.shrink(2.0), 2.0, stroke, egui::StrokeKind::Inside);
        }
        if let Some(dragged) = response.dnd_release_payload::<ObjectId>() {
            ops.reparent = Some((*dragged, None));
        }
        if let Some(label) = apply(scene, selected, ops) {
            edit = Some(label);
        }
    });
    edit
}

/// What the rows asked for, applied once the tree is drawn.
#[derive(Default)]
struct Ops {
    reparent: Option<(ObjectId, Option<ObjectId>)>,
    duplicate: Option<ObjectId>,
    delete: Option<ObjectId>,
    /// An object and its new name.
    rename: Option<(ObjectId, String)>,
    toggled: bool,
}

/// The Create menu's items, and what the menus and rename asked for.
struct Menus<'a> {
    items: &'a [String],
    create: &'a mut Option<CreateRequest>,
    rename: &'a mut Option<Rename>,
}

fn row(
    ui: &mut Ui,
    scene: &mut Scene,
    id: ObjectId,
    selected: &mut Option<ObjectId>,
    ops: &mut Ops,
    menus: &mut Menus,
) {
    let items = menus.items;
    let children = scene.children(id).to_vec();
    let object = scene.object(id).expect("listed");
    let mut active = object.active;
    let name = object.name.clone();
    let mut text = RichText::new(&object.name);
    if !scene.is_active(id) {
        text = text.weak();
    }
    if object.dont_save {
        text = text.italics();
    }
    let header = |ui: &mut Ui,
                  ops: &mut Ops,
                  selected: &mut Option<ObjectId>,
                  active: &mut bool,
                  create: &mut Option<CreateRequest>,
                  rename: &mut Option<Rename>| {
        if ui.checkbox(active, "").on_hover_text("Active").changed() {
            ops.toggled = true;
        }
        if let Some(r) = rename.as_mut().filter(|r| r.id == id) {
            let edit = ui.add(egui::TextEdit::singleline(&mut r.text).desired_width(160.0));
            if r.focus {
                edit.request_focus();
                // Select all, so typing replaces the old name.
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), edit.id) {
                    let all = egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        egui::text::CCursor::new(r.text.chars().count()),
                    );
                    state.cursor.set_char_range(Some(all));
                    state.store(ui.ctx(), edit.id);
                }
                r.focus = false;
            } else if edit.lost_focus() {
                let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let name = r.text.trim().to_owned();
                if !cancelled && !name.is_empty() {
                    ops.rename = Some((id, name));
                }
                *rename = None;
            }
            return;
        }
        let response = ui
            .selectable_label(*selected == Some(id), text.clone())
            .interact(Sense::click_and_drag());
        if response.clicked() || response.drag_started() {
            *selected = Some(id);
        }
        if response.drag_started() {
            response.dnd_set_drag_payload(id);
        }
        if response.dnd_hover_payload::<ObjectId>().is_some_and(|d| *d != id) {
            ui.painter().rect_stroke(
                response.rect,
                2.0,
                (1.0, Color32::LIGHT_BLUE),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(dragged) = response.dnd_release_payload::<ObjectId>()
            && *dragged != id
        {
            ops.reparent = Some((*dragged, Some(id)));
        }
        if response.double_clicked() {
            *rename = Some(Rename { id, text: name.clone(), focus: true });
        }
        response.context_menu(|ui| {
            ui.menu_button("Create", |ui| create::menu(ui, items, Some(id), create));
            ui.separator();
            if ui.button("Rename  (Ctrl+I)").clicked() {
                *rename = Some(Rename { id, text: name.clone(), focus: true });
                ui.close();
            }
            if ui.button("Duplicate  (Ctrl+D)").clicked() {
                ops.duplicate = Some(id);
                ui.close();
            }
            if ui.button("Delete  (Del)").clicked() {
                ops.delete = Some(id);
                ui.close();
            }
        });
    };
    if children.is_empty() {
        ui.horizontal(|ui| {
            // Line up with rows that have a collapse arrow.
            ui.add_space(ui.spacing().icon_width + ui.spacing().item_spacing.x);
            header(ui, ops, selected, &mut active, menus.create, menus.rename);
        });
    } else {
        let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            ui.make_persistent_id(("hierarchy", id)),
            children.len() <= 16,
        );
        // Open the way to an object that's about to be renamed.
        if menus.rename.as_ref().is_some_and(|r| r.focus && scene.is_ancestor(id, r.id)) {
            state.set_open(true);
        }
        state
            .show_header(ui, |ui| {
                header(ui, ops, selected, &mut active, menus.create, menus.rename)
            })
            .body(|ui| {
                for child in children {
                    row(ui, scene, child, selected, ops, menus);
                }
            });
    }
    if let Some(object) = scene.object_mut(id)
        && object.active != active
    {
        object.active = active;
    }
}

fn apply(scene: &mut Scene, selected: &mut Option<ObjectId>, ops: Ops) -> Option<String> {
    if let Some((id, name)) = ops.rename
        && let Some(object) = scene.object_mut(id)
        && object.name != name
    {
        object.name = name;
        return Some("Rename".to_owned());
    }
    if let Some((id, parent)) = ops.reparent {
        // Refused (and nothing changes) if it would make a loop.
        return scene.set_parent_keep_world(id, parent, None).then(|| "Reparent".to_owned());
    }
    if let Some(id) = ops.duplicate {
        *selected = scene.duplicate(id);
        return Some("Duplicate".to_owned());
    }
    if let Some(id) = ops.delete {
        scene.despawn(id);
        if selected.is_some_and(|s| !scene.contains(s)) {
            *selected = None;
        }
        return Some("Delete".to_owned());
    }
    ops.toggled.then(|| "Toggle active".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the Hierarchy for one frame with `events` as input.
    fn frame(
        ctx: &egui::Context,
        scene: &mut Scene,
        selected: &mut Option<ObjectId>,
        rename: &mut Option<Rename>,
        events: Vec<egui::Event>,
    ) -> Option<String> {
        let input = egui::RawInput { events, ..Default::default() };
        let mut edit = None;
        let mut create = None;
        let mut output = ctx.run_ui(input, |ui| {
            edit = hierarchy(ui, scene, selected, true, &[], &mut create, rename);
        });
        // Nothing draws these frames; egui panics if the font texture it
        // made is dropped without being uploaded or cleared.
        output.textures_delta.clear();
        edit
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn renaming_in_place_replaces_the_name() {
        let ctx = egui::Context::default();
        let mut scene = Scene::new();
        let parent = scene.spawn("Parent");
        let id = scene.spawn_child(parent, "GameObject");
        let mut selected = Some(id);
        let mut rename = Rename::start(&scene, id);
        // The box opens with the old name selected, so typing replaces it.
        frame(&ctx, &mut scene, &mut selected, &mut rename, vec![]);
        frame(&ctx, &mut scene, &mut selected, &mut rename, vec![egui::Event::Text("Hero".into())]);
        assert_eq!(rename.as_ref().map(|r| r.text.as_str()), Some("Hero"));
        let edit = frame(&ctx, &mut scene, &mut selected, &mut rename, vec![key(egui::Key::Enter)]);
        assert_eq!(edit.as_deref(), Some("Rename"));
        assert_eq!(scene.name(id), Some("Hero"));
        assert!(rename.is_none());
    }

    #[test]
    fn escape_keeps_the_old_name() {
        let ctx = egui::Context::default();
        let mut scene = Scene::new();
        let id = scene.spawn("Crate");
        let mut selected = Some(id);
        let mut rename = Rename::start(&scene, id);
        frame(&ctx, &mut scene, &mut selected, &mut rename, vec![]);
        frame(&ctx, &mut scene, &mut selected, &mut rename, vec![egui::Event::Text("Oops".into())]);
        let edit =
            frame(&ctx, &mut scene, &mut selected, &mut rename, vec![key(egui::Key::Escape)]);
        assert_eq!(edit, None);
        assert_eq!(scene.name(id), Some("Crate"));
        assert!(rename.is_none());
    }
}
