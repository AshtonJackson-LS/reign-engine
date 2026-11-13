//! The Inspector: the selected object's name, transform and components,
//! edited through reflection. Nothing here knows any component type.

use egui::{Color32, RichText, Ui};
use reign_assets::{AssetDb, Guid};
use reign_core::{Fx, FxVec2};
use reign_scene::reflect::{FieldInfo, Mut, Reflect, ReflectStruct};
use reign_scene::{ComponentRegistry, ObjectId, Scene};

use crate::assets_ui::AssetDrag;

/// What pickers offer: objects for `ObjectId` fields, assets for handles.
pub(crate) struct Pick<'a> {
    pub objects: Vec<(ObjectId, String)>,
    pub assets: Option<&'a AssetDb>,
    /// Properties this prefab instance overrides (`Component.field.sub`):
    /// shown bold, with Revert on right-click.
    pub overridden: Vec<String>,
    /// A property whose Revert was clicked.
    pub revert: std::cell::RefCell<Option<String>>,
}

impl Pick<'_> {
    fn is_overridden(&self, path: &str) -> bool {
        self.overridden
            .iter()
            .any(|o| o == path || o.strip_prefix(path).is_some_and(|rest| rest.starts_with('.')))
    }
}

fn join(prefix: &str, name: &str) -> String {
    if prefix.is_empty() { name.to_owned() } else { format!("{prefix}.{name}") }
}

/// What the Inspector asked the editor to do, besides editing in place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// A label for the undo step the next change makes.
    Edit(String),
    /// Something to do with the selected object's prefab.
    Prefab(PrefabAction),
}

/// The prefab instance buttons.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrefabAction {
    SelectAsset(Guid),
    Open(Guid),
    ApplyAll(ObjectId),
    RevertAll(ObjectId),
    Unpack(ObjectId),
    /// Revert one property of an object to the prefab's value.
    Revert(ObjectId, ObjectId, String),
}

/// Draws the Inspector for `id`. Field edits go straight into the scene;
/// the editor sees the change and records it for undo.
pub fn inspector(
    ui: &mut Ui,
    scene: &mut Scene,
    registry: Option<&ComponentRegistry>,
    id: ObjectId,
    editable: bool,
    assets: Option<&AssetDb>,
    prefab: Option<&crate::prefab_ui::PrefabInfo>,
) -> Option<Action> {
    let Some(object) = scene.object(id) else {
        ui.weak("The selected object is gone.");
        return None;
    };
    let runtime = object.dont_save;
    let mut action = None;
    // Objects to offer wherever a field holds an ObjectId.
    let objects: Vec<(ObjectId, String)> =
        scene.walk().into_iter().map(|o| (o, label(scene, o))).collect();
    let overridden = prefab.map_or_else(Vec::new, |p| p.overridden.clone());
    let pick = Pick { objects, assets, overridden, revert: Default::default() };

    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            let object = scene.object_mut(id).expect("checked");
            ui.checkbox(&mut object.active, "").on_hover_text("Active");
            ui.add(egui::TextEdit::singleline(&mut object.name).desired_width(f32::INFINITY));
        });
        ui.horizontal(|ui| {
            ui.weak(format!("{id}"));
            if runtime {
                ui.weak("· made at runtime, not saved").on_hover_text(
                    "A bridge to state outside the scene (players, enemies, dummies). \
                     Moving it moves the real thing.",
                );
            }
        });
        if let Some(p) = prefab {
            let instance = RichText::new(format!("Prefab: {}", file_label(&p.name)))
                .color(Color32::from_rgb(110, 170, 255));
            ui.horizontal_wrapped(|ui| {
                ui.label(instance).on_hover_text(&p.name);
                if ui.small_button("Select").on_hover_text("Select the prefab asset").clicked() {
                    action = Some(Action::Prefab(PrefabAction::SelectAsset(p.guid)));
                }
                if ui.small_button("Open").on_hover_text("Edit the prefab in Prefab Mode").clicked()
                {
                    action = Some(Action::Prefab(PrefabAction::Open(p.guid)));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(p.any, |ui| {
                    if ui
                        .small_button("Apply All")
                        .on_hover_text("Write this instance's overrides into the prefab")
                        .clicked()
                    {
                        action = Some(Action::Prefab(PrefabAction::ApplyAll(p.root)));
                    }
                    if ui.small_button("Revert All").on_hover_text("Drop every override").clicked()
                    {
                        action = Some(Action::Prefab(PrefabAction::RevertAll(p.root)));
                    }
                });
                if ui
                    .small_button("Unpack")
                    .on_hover_text("Make it plain objects, no longer linked to the prefab")
                    .clicked()
                {
                    action = Some(Action::Prefab(PrefabAction::Unpack(p.root)));
                }
                if p.any {
                    ui.weak("overrides in bold");
                }
            });
        }
        ui.separator();

        egui::CollapsingHeader::new(RichText::new("Transform").strong())
            .id_salt(("transform", id))
            .default_open(true)
            .show(ui, |ui| {
                let transform = &mut scene.object_mut(id).expect("checked").transform;
                fields_at(ui, transform, &pick, ("transform", id), "GameObject.transform");
            });

        let names = scene.component_names(id);
        for name in names {
            let mut title = RichText::new(name).strong();
            if pick.is_overridden(name) {
                title = title.color(Color32::from_rgb(110, 170, 255));
            }
            let open = egui::CollapsingHeader::new(title)
                .id_salt(("component", id, name))
                .default_open(true)
                .show(ui, |ui| {
                    if let Some(value) = scene.component_mut(id, name) {
                        fields_at(ui, value, &pick, (name, id), name);
                    }
                });
            open.header_response.context_menu(|ui| {
                if ui.button("Reset").clicked() {
                    if let Some(registry) = registry {
                        registry.insert_default(scene, id, name);
                        action = Some(Action::Edit(format!("Reset {name}")));
                    }
                    ui.close();
                }
                if ui.button("Remove component").clicked() {
                    scene.remove_component(id, name);
                    action = Some(Action::Edit(format!("Remove {name}")));
                    ui.close();
                }
            });
            open.header_response.on_hover_text("Right-click to reset or remove");
        }

        if let (Some(path), Some(p)) = (pick.revert.borrow_mut().take(), prefab) {
            action = Some(Action::Prefab(PrefabAction::Revert(p.root, id, path)));
        }
        if let Some(registry) = registry {
            ui.add_space(8.0);
            let present = scene.component_names(id);
            ui.menu_button("Add Component", |ui| {
                for name in registry.names().filter(|n| !present.contains(n)) {
                    if ui.button(name).clicked() {
                        registry.insert_default(scene, id, name);
                        action = Some(Action::Edit(format!("Add {name}")));
                        ui.close();
                    }
                }
            });
        }
    });
    action
}

/// An object's label in pickers and the Hierarchy.
pub fn label(scene: &Scene, id: ObjectId) -> String {
    format!("{} {id}", scene.name(id).unwrap_or("?"))
}

/// A value's fields, one row each, or the value alone if it isn't a struct.
pub(crate) fn fields(
    ui: &mut Ui,
    value: &mut dyn Reflect,
    objects: &Pick,
    salt: impl std::hash::Hash + std::fmt::Debug,
) {
    fields_at(ui, value, objects, salt, "");
}

/// [`fields`], for a value at property path `prefix` (for overrides).
pub(crate) fn fields_at(
    ui: &mut Ui,
    value: &mut dyn Reflect,
    objects: &Pick,
    salt: impl std::hash::Hash + std::fmt::Debug,
    prefix: &str,
) {
    match value.reflect_mut() {
        Mut::Struct(s) => struct_ui(ui, s, objects, egui::Id::new(salt), prefix),
        _ => value_ui(ui, value, None, objects, prefix),
    }
}

fn struct_ui(ui: &mut Ui, s: &mut dyn ReflectStruct, objects: &Pick, id: egui::Id, prefix: &str) {
    egui::Grid::new(id).num_columns(2).striped(true).show(ui, |ui| {
        for (index, info) in s.fields().iter().enumerate() {
            let Some(field) = s.field_mut(index) else { continue };
            let path = join(prefix, info.name);
            let overridden = objects.is_overridden(&path);
            let text = if overridden {
                RichText::new(info.name).strong().color(Color32::WHITE)
            } else {
                RichText::new(info.name)
            };
            let mut label = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
            if let Some(tip) = info.tooltip {
                label = label.on_hover_text(tip);
            }
            if overridden {
                label = label.on_hover_text("Overrides the prefab. Right-click to revert.");
                label.context_menu(|ui| {
                    if ui.button("Revert to prefab").clicked() {
                        *objects.revert.borrow_mut() = Some(path.clone());
                        ui.close();
                    }
                });
            }
            ui.add_enabled_ui(!info.read_only, |ui| {
                value_ui(ui, field, Some(info), objects, &path)
            });
            ui.end_row();
        }
    });
}

/// One value's editor, by what kind of value it is.
fn value_ui(
    ui: &mut Ui,
    value: &mut dyn Reflect,
    info: Option<&FieldInfo>,
    objects: &Pick,
    path: &str,
) {
    let range = info.and_then(|i| i.range);
    match value.reflect_mut() {
        Mut::Bool(b) => {
            ui.checkbox(b, "");
        }
        Mut::Int(mut int) => {
            let (lo, hi) = int.bounds();
            let (lo, hi) = match range {
                Some((a, b)) => ((a as i128).max(lo), (b as i128).min(hi)),
                None => (lo, hi),
            };
            let mut v = int.get().clamp(lo, hi) as f64;
            let response = ui.add(
                egui::DragValue::new(&mut v)
                    .range(lo as f64..=hi as f64)
                    .speed(0.25)
                    .fixed_decimals(0),
            );
            if response.changed() {
                int.set(v.round() as i128);
            }
        }
        Mut::Fx(fx) => {
            fx_ui(ui, fx, range);
        }
        Mut::FxVec2(v) => {
            vec_ui(ui, v, range);
        }
        Mut::Color(c) => {
            let mut rgba = [c.r, c.g, c.b, c.a];
            if ui.color_edit_button_srgba_unmultiplied(&mut rgba).changed() {
                (c.r, c.g, c.b, c.a) = (rgba[0], rgba[1], rgba[2], rgba[3]);
            }
        }
        Mut::String(s) => {
            ui.add(egui::TextEdit::singleline(s).desired_width(160.0));
        }
        Mut::Object(id) => {
            let shown = objects
                .objects
                .iter()
                .find(|(o, _)| o == id)
                .map_or_else(|| format!("(missing) {id}"), |(_, name)| name.clone());
            egui::ComboBox::from_id_salt(("object", ui.next_auto_id()))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for (o, name) in &objects.objects {
                        ui.selectable_value(id, *o, name);
                    }
                });
        }
        Mut::Asset(a) => asset_picker(ui, a, objects.assets),
        Mut::Enum(e) => {
            let variants = e.variants();
            let mut index = e.variant_index();
            egui::ComboBox::from_id_salt(("enum", ui.next_auto_id()))
                .selected_text(variants.get(index).copied().unwrap_or("?"))
                .show_ui(ui, |ui| {
                    for (i, name) in variants.iter().enumerate() {
                        ui.selectable_value(&mut index, i, *name);
                    }
                });
            if index != e.variant_index() {
                e.set_variant_index(index);
            }
        }
        Mut::Option(o) => {
            ui.horizontal(|ui| {
                let mut some = o.get_mut().is_some();
                if ui.checkbox(&mut some, "").changed() {
                    if some {
                        o.set_some_default();
                    } else {
                        o.set_none();
                    }
                }
                match o.get_mut() {
                    Some(inner) => value_ui(ui, inner, info, objects, &join(path, "?")),
                    None => {
                        ui.weak("None");
                    }
                }
            });
        }
        Mut::List(list) => {
            ui.vertical(|ui| {
                let mut remove = None;
                for i in 0..list.len() {
                    ui.horizontal(|ui| {
                        ui.weak(format!("{i}"));
                        if let Some(item) = list.get_mut(i) {
                            value_ui(ui, item, info, objects, &join(path, &i.to_string()));
                        }
                        if list.resizable() && ui.small_button("✕").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    list.remove(i);
                }
                if list.resizable() && ui.small_button("+ Add").clicked() {
                    list.push_default();
                }
            });
        }
        Mut::Struct(s) => {
            let id = ui.next_auto_id();
            ui.vertical(|ui| struct_ui(ui, s, objects, id, path));
        }
    }
}

/// A fixed-point number, edited in whole 1/256ths and shown exactly.
fn fx_ui(ui: &mut Ui, fx: &mut Fx, range: Option<(f64, f64)>) {
    let mut raw = fx.raw() as f64;
    let (lo, hi) =
        range.map_or((i32::MIN as f64, i32::MAX as f64), |(a, b)| (a * 256.0, b * 256.0));
    let response = ui.add(
        egui::DragValue::new(&mut raw)
            .range(lo..=hi)
            // Dragging moves a quarter pixel per point; typing takes any
            // exact decimal.
            .speed(64.0)
            .custom_formatter(|v, _| Fx::from_raw(v as i32).to_string())
            .custom_parser(|s| Fx::parse_decimal(s.trim()).ok().map(|f| f.raw() as f64)),
    );
    if response.changed() {
        *fx = Fx::from_raw(raw.round() as i32);
    }
}

fn vec_ui(ui: &mut Ui, v: &mut FxVec2, range: Option<(f64, f64)>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("x").color(Color32::from_rgb(230, 110, 110)));
        fx_ui(ui, &mut v.x, range);
        ui.label(RichText::new("y").color(Color32::from_rgb(120, 200, 120)));
        fx_ui(ui, &mut v.y, range);
    });
}

/// A handle field: a menu of the assets of its kind, and a drop target for
/// assets dragged from the Project panel.
fn asset_picker(ui: &mut Ui, a: &mut dyn reign_scene::reflect::ReflectAsset, db: Option<&AssetDb>) {
    let kind = a.asset_kind();
    let current = Guid(a.guid());
    let shown = if current == Guid::NIL {
        "None".to_owned()
    } else {
        db.and_then(|db| db.path(current))
            .map_or_else(|| format!("(missing) {current}"), |p| file_label(&p))
    };
    let response = ui
        .horizontal(|ui| {
            egui::ComboBox::from_id_salt(("asset", ui.next_auto_id()))
                .selected_text(shown)
                .width(180.0)
                .show_ui(ui, |ui| {
                    let mut chosen = None;
                    if ui.selectable_label(current == Guid::NIL, "None").clicked() {
                        chosen = Some(Guid::NIL);
                    }
                    let Some(db) = db else { return };
                    for info in db.assets().into_iter().filter(|i| i.kind.name() == kind) {
                        let selected = info.guid == current;
                        if ui.selectable_label(selected, &info.path).clicked() {
                            chosen = Some(info.guid);
                        }
                    }
                    if let Some(guid) = chosen {
                        a.set_guid(guid.0);
                    }
                })
                .response
        })
        .inner;
    let response = response.on_hover_text(format!("{kind}: pick one, or drop it from Project"));
    if let Some(drag) = response.dnd_hover_payload::<AssetDrag>() {
        let ok = drag.kind.name() == kind;
        let color =
            if ok { Color32::from_rgb(120, 200, 120) } else { Color32::from_rgb(220, 90, 90) };
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            2.0,
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Outside,
        );
    }
    if let Some(drag) = response.dnd_release_payload::<AssetDrag>()
        && drag.kind.name() == kind
    {
        a.set_guid(drag.guid.0);
    }
}

/// An asset path shown by its file name.
pub(crate) fn file_label(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}
