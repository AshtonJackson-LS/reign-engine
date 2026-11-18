//! The Inspector: the selected object's name, transform and components,
//! edited through reflection. Nothing here knows any component type.

use egui::{Color32, RichText, Ui};
use reign_assets::{AssetDb, Guid};
use reign_core::{Fx, FxVec2};
use reign_scene::reflect::{FieldHint, FieldInfo, IntMut, Mut, Reflect, ReflectStruct};
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
    /// Collision layers' names by number, for layer and layer-mask fields.
    pub layers: &'a [String],
    /// The project's tags, for the tag menu.
    pub tags: &'a [String],
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
    /// Add this tag to the project's tags (the object already has it).
    AddTag(String),
    /// Add Component → Create Component: open the app's New Component
    /// dialog, starting with this name (what was typed in the search box).
    NewComponent(String),
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

/// What the Inspector looks things up in, besides the scene.
pub struct Lookups<'a> {
    pub registry: Option<&'a ComponentRegistry>,
    /// For asset pickers.
    pub assets: Option<&'a AssetDb>,
    /// The selected object's prefab, if it's an instance of one.
    pub prefab: Option<&'a crate::prefab_ui::PrefabInfo>,
    /// Collision layers' names by number (see `EditorApp::layer_names`).
    pub layers: &'a [String],
    /// The project's tags (see `EditorApp::tag_names`).
    pub tags: &'a [String],
    /// Whether Add Component ends with Create Component (see
    /// `EditorApp::can_create_component`).
    pub can_create_component: bool,
}

/// Draws the Inspector for `id`. Field edits go straight into the scene;
/// the editor sees the change and records it for undo.
pub fn inspector(
    ui: &mut Ui,
    scene: &mut Scene,
    id: ObjectId,
    editable: bool,
    lookups: Lookups,
) -> Option<Action> {
    let Lookups { registry, assets, prefab, layers, tags, can_create_component } = lookups;
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
    let pick = Pick { objects, assets, overridden, revert: Default::default(), layers, tags };

    ui.add_enabled_ui(editable, |ui| {
        ui.horizontal(|ui| {
            let object = scene.object_mut(id).expect("checked");
            ui.checkbox(&mut object.active, "").on_hover_text("Active");
            ui.add(egui::TextEdit::singleline(&mut object.name).desired_width(f32::INFINITY));
        });
        if let Some(a) = tag_ui(ui, scene, id, &pick) {
            action = Some(a);
        }
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
            let names: Vec<&'static str> =
                registry.names().filter(|n| !present.contains(n)).collect();
            // The search box's text and whether it still wants the focus
            // live in egui's memory, and start afresh each time the menu opens.
            let key = egui::Id::new(("add component", id));
            let mut query: String = ui.data_mut(|d| d.get_temp(key)).unwrap_or_default();
            let mut focus: bool = ui.data_mut(|d| d.get_temp(key.with("focus"))).unwrap_or(true);
            let menu = ui.menu_button("Add Component", |ui| {
                let choice =
                    add_component_list(ui, &names, &mut query, can_create_component, &mut focus);
                if choice.is_some() {
                    ui.close();
                }
                choice
            });
            match menu.inner {
                None => {
                    query.clear();
                    focus = true;
                }
                Some(Some(Choice::Add(name))) => {
                    registry.insert_default(scene, id, name);
                    action = Some(Action::Edit(format!("Add {name}")));
                    query.clear();
                    focus = true;
                }
                Some(Some(Choice::Create(name))) => {
                    action = Some(Action::NewComponent(name));
                    query.clear();
                    focus = true;
                }
                Some(None) => {}
            }
            ui.data_mut(|d| {
                d.insert_temp(key, query);
                d.insert_temp(key.with("focus"), focus);
            });
        }
    });
    action
}

/// What Add Component's list chose.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Choice<'a> {
    /// Add this component to the object.
    Add(&'a str),
    /// Make a new component type, starting with the typed text as its name.
    Create(String),
}

/// The names that match what was typed, best first: every word of `query`
/// must be somewhere in the name, in any case ("trail rend" finds
/// `TrailRenderer`), and names that start with the first word come before
/// the rest. An empty query matches everything, in the order given.
fn matching<'a>(names: &[&'a str], query: &str) -> Vec<&'a str> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found: Vec<&'a str> = names
        .iter()
        .copied()
        .filter(|name| {
            let lower = name.to_lowercase();
            words.iter().all(|w| lower.contains(w.as_str()))
        })
        .collect();
    if let Some(first) = words.first() {
        // Stable, so each group keeps the order given.
        found.sort_by_key(|name| !name.to_lowercase().starts_with(first.as_str()));
    }
    found
}

/// The inside of the Add Component menu: a search box, the components that
/// match it, then (when the app can) Create Component at the bottom. Enter
/// takes the first match, or starts Create Component when nothing matches.
/// `focus` is true until the box has taken the keyboard.
fn add_component_list<'a>(
    ui: &mut Ui,
    names: &[&'a str],
    query: &mut String,
    can_create: bool,
    focus: &mut bool,
) -> Option<Choice<'a>> {
    ui.set_min_width(220.0);
    let search = ui.add(
        egui::TextEdit::singleline(&mut *query).hint_text("Search…").desired_width(f32::INFINITY),
    );
    if *focus {
        search.request_focus();
        *focus = false;
    }
    let found = matching(names, query);
    let typed = !query.trim().is_empty();
    let mut choice = None;
    if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        choice = match found.first() {
            Some(first) => Some(Choice::Add(first)),
            None if can_create => Some(Choice::Create(query.trim().to_owned())),
            None => None,
        };
    }
    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
        for (i, name) in found.iter().enumerate() {
            // With something typed, Enter takes the first: show which.
            let button = egui::Button::new(*name).selected(typed && i == 0);
            if ui.add(button).clicked() {
                choice = Some(Choice::Add(name));
            }
        }
        if found.is_empty() {
            ui.weak("No matching components");
        }
    });
    if can_create {
        ui.separator();
        if ui
            .button("Create Component…")
            .on_hover_text("Make a new component, as Scripts → New Component does")
            .clicked()
        {
            choice = Some(Choice::Create(query.trim().to_owned()));
        }
    }
    choice
}

/// The object's tag: picked from the project's tags, or typed. Typing a
/// new one and pressing Enter (or `+ Add`) adds it to the project's tags.
/// Bold, with Revert on right-click, when a prefab instance overrides its
/// prefab's tag.
fn tag_ui(ui: &mut Ui, scene: &mut Scene, id: ObjectId, pick: &Pick) -> Option<Action> {
    // Tags in the scene that the project doesn't list: offered too, so
    // they can be picked again or added.
    let mut stray: Vec<String> = scene
        .walk()
        .into_iter()
        .filter_map(|o| scene.object(o).map(|o| o.tag.trim().to_owned()))
        .filter(|t| !t.is_empty() && !pick.tags.contains(t))
        .collect();
    stray.sort();
    stray.dedup();
    let object = scene.object_mut(id)?;
    let mut action = None;
    ui.horizontal(|ui| {
        let overridden = pick.is_overridden("GameObject.tag");
        let text = if overridden {
            RichText::new("Tag").strong().color(Color32::WHITE)
        } else {
            RichText::new("Tag")
        };
        let label = ui
            .add(egui::Label::new(text).sense(egui::Sense::click()))
            .on_hover_text("Scripts find objects by tag: ctx.find_with_tag(\"Enemy\")");
        if overridden {
            label.on_hover_text("Overrides the prefab. Right-click to revert.").context_menu(
                |ui| {
                    if ui.button("Revert to prefab").clicked() {
                        *pick.revert.borrow_mut() = Some("GameObject.tag".to_owned());
                        ui.close();
                    }
                },
            );
        }
        let edit = egui::TextEdit::singleline(&mut object.tag)
            .id(tag_edit_id(id))
            .hint_text("Untagged")
            .desired_width(140.0);
        let typed = ui.add(edit);
        let mut new_tag = false;
        egui::ComboBox::from_id_salt(("tag", id))
            .selected_text("")
            .width(16.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(object.tag.is_empty(), "Untagged").clicked() {
                    object.tag.clear();
                }
                for tag in pick.tags {
                    if ui.selectable_label(object.tag == *tag, tag).clicked() {
                        object.tag.clone_from(tag);
                    }
                }
                for tag in &stray {
                    let label = ui
                        .selectable_label(object.tag == *tag, RichText::new(tag).weak())
                        .on_hover_text("Used in this scene, not in the project's tags");
                    if label.clicked() {
                        object.tag.clone_from(tag);
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "New tag…").clicked() {
                    new_tag = true;
                }
            })
            .response
            .on_hover_text("Pick a tag, or make a new one");
        if new_tag {
            object.tag.clear();
            ui.memory_mut(|m| m.request_focus(tag_edit_id(id)));
        }
        let tag = object.tag.trim().to_owned();
        let unknown = !tag.is_empty() && !pick.tags.contains(&tag);
        let entered = typed.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let add = unknown
            && (entered
                || ui
                    .small_button("+ Add")
                    .on_hover_text(format!(
                        "Add {tag:?} to the project's tags, so every object can pick it"
                    ))
                    .clicked());
        if add {
            object.tag = tag.clone();
            action = Some(Action::AddTag(tag));
        }
    });
    action
}

/// The tag box's id, so tests can type into it.
fn tag_edit_id(id: ObjectId) -> egui::Id {
    egui::Id::new(("tag edit", id))
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
        Mut::Int(int) if info.is_some_and(|i| i.hint == FieldHint::Layer) => {
            layer_ui(ui, int, objects.layers);
        }
        Mut::Int(int) if info.is_some_and(|i| i.hint == FieldHint::LayerMask) => {
            layer_mask_ui(ui, int, objects.layers);
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
/// How many collision layers there are: one bit each in a `u32` mask.
const LAYERS: u32 = 32;

/// A layer as the Inspector shows it: its number, and its name if it has
/// one (`3  Enemies`).
pub(crate) fn layer_label(layer: u32, names: &[String]) -> String {
    match names.get(layer as usize).map(|n| n.trim()).filter(|n| !n.is_empty()) {
        Some(name) => format!("{layer}  {name}"),
        None => format!("{layer}"),
    }
}

/// A layer mask in a few words: Everything, Nothing, the layers in it when
/// there are up to three, else how many.
pub(crate) fn mask_summary(mask: u32, names: &[String]) -> String {
    match mask {
        u32::MAX => "Everything".into(),
        0 => "Nothing".into(),
        _ => {
            let set: Vec<u32> = (0..LAYERS).filter(|l| mask & (1 << l) != 0).collect();
            if set.len() <= 3 {
                let name = |l: u32| {
                    names
                        .get(l as usize)
                        .map(|n| n.trim().to_owned())
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| format!("Layer {l}"))
                };
                set.into_iter().map(name).collect::<Vec<_>>().join(", ")
            } else {
                format!("{} layers", set.len())
            }
        }
    }
}

/// A layer field: a list of the 32 layers by number and name.
fn layer_ui(ui: &mut Ui, mut int: IntMut, names: &[String]) {
    let (lo, hi) = int.bounds();
    let mut layer = int.get().clamp(lo.max(0), hi.min(i128::from(LAYERS) - 1)) as u32;
    let before = layer;
    egui::ComboBox::from_id_salt(("layer", ui.next_auto_id()))
        .selected_text(layer_label(layer, names))
        .show_ui(ui, |ui| {
            for l in 0..LAYERS {
                ui.selectable_value(&mut layer, l, layer_label(l, names));
            }
        })
        .response
        .on_hover_text("Name layers in Project → Project Settings");
    if layer != before {
        int.set(i128::from(layer));
    }
}

/// A layer mask field: Everything, Nothing, and a box for each layer, the
/// named ones first.
fn layer_mask_ui(ui: &mut Ui, mut int: IntMut, names: &[String]) {
    let mut mask = int.get().clamp(0, i128::from(u32::MAX)) as u32;
    let before = mask;
    let named = |l: u32| names.get(l as usize).is_some_and(|n| !n.trim().is_empty());
    egui::ComboBox::from_id_salt(("layer mask", ui.next_auto_id()))
        .selected_text(mask_summary(mask, names))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Everything").clicked() {
                    mask = u32::MAX;
                }
                if ui.button("Nothing").clicked() {
                    mask = 0;
                }
            });
            ui.separator();
            let mut tick = |ui: &mut Ui, l: u32, text: String| {
                let mut on = mask & (1 << l) != 0;
                if ui.checkbox(&mut on, text).changed() {
                    mask ^= 1 << l;
                }
            };
            for l in (0..LAYERS).filter(|&l| named(l)) {
                tick(ui, l, layer_label(l, names));
            }
            let unnamed: Vec<u32> = (0..LAYERS).filter(|&l| !named(l)).collect();
            if !unnamed.is_empty() {
                egui::CollapsingHeader::new(format!("Unnamed layers ({})", unnamed.len()))
                    .id_salt("unnamed layers")
                    .show(ui, |ui| {
                        for l in unnamed {
                            tick(ui, l, format!("Layer {l}"));
                        }
                    });
            }
        })
        .response
        .on_hover_text("The layers this collides with. Name layers in Project → Project Settings");
    if mask != before {
        int.set(i128::from(mask));
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        let mut names = vec![String::new(); 32];
        names[0] = "Default".into();
        names[3] = "Enemies".into();
        names[4] = " ".into();
        names
    }

    const TAGS: &[String] = &[];

    /// Draws the Inspector for `id` for one frame with `events` as input,
    /// with `tags` as the project's tags.
    fn frame(
        ctx: &egui::Context,
        scene: &mut Scene,
        id: ObjectId,
        tags: &[String],
        events: Vec<egui::Event>,
    ) -> Option<Action> {
        let input = egui::RawInput { events, ..Default::default() };
        let mut action = None;
        let mut output = ctx.run_ui(input, |ui| {
            let lookups = Lookups {
                registry: None,
                assets: None,
                prefab: None,
                layers: &[],
                tags,
                can_create_component: false,
            };
            action = inspector(ui, scene, id, true, lookups);
        });
        // Nothing draws these frames; egui panics if the font texture it
        // made is dropped without being uploaded or cleared.
        output.textures_delta.clear();
        action
    }

    fn enter() -> egui::Event {
        egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// Focuses the tag box, types `text` and presses Enter. What the
    /// Inspector asked for on the way.
    fn type_tag(
        ctx: &egui::Context,
        scene: &mut Scene,
        id: ObjectId,
        tags: &[String],
        text: &str,
    ) -> Vec<Action> {
        frame(ctx, scene, id, tags, vec![]);
        ctx.memory_mut(|m| m.request_focus(tag_edit_id(id)));
        let mut actions = Vec::new();
        actions.extend(frame(ctx, scene, id, tags, vec![]));
        actions.extend(frame(ctx, scene, id, tags, vec![egui::Event::Text(text.into())]));
        actions.extend(frame(ctx, scene, id, tags, vec![enter()]));
        actions.extend(frame(ctx, scene, id, tags, vec![]));
        actions
    }

    #[test]
    fn typing_in_the_tag_box_tags_the_object() {
        let ctx = egui::Context::default();
        let mut scene = Scene::new();
        let id = scene.spawn("Goblin");
        frame(&ctx, &mut scene, id, TAGS, vec![]);
        ctx.memory_mut(|m| m.request_focus(tag_edit_id(id)));
        frame(&ctx, &mut scene, id, TAGS, vec![]);
        frame(&ctx, &mut scene, id, TAGS, vec![egui::Event::Text("Enemy".into())]);
        assert_eq!(scene.object(id).unwrap().tag, "Enemy");
    }

    #[test]
    fn enter_on_a_new_tag_adds_it_to_the_project() {
        let ctx = egui::Context::default();
        let tags = vec!["Player".to_owned(), "Enemy".to_owned()];
        let mut scene = Scene::new();
        let id = scene.spawn("Dragon");
        let actions = type_tag(&ctx, &mut scene, id, &tags, " Boss ");
        assert_eq!(actions, [Action::AddTag("Boss".into())]);
        assert_eq!(scene.object(id).unwrap().tag, "Boss", "trimmed");

        // A tag the project has already: nothing to add.
        let other = scene.spawn("Goblin");
        assert_eq!(type_tag(&ctx, &mut scene, other, &tags, "Enemy"), []);
        assert_eq!(scene.object(other).unwrap().tag, "Enemy");
    }

    const COMPONENTS: &[&str] = &["Animator", "ParticleSystem", "Rigidbody", "TrailRenderer"];

    #[test]
    fn searching_components_matches_words_anywhere_and_ignores_case() {
        assert_eq!(matching(COMPONENTS, ""), COMPONENTS, "no search: everything, in order");
        assert_eq!(matching(COMPONENTS, "  "), COMPONENTS);
        assert_eq!(matching(COMPONENTS, "rend"), ["TrailRenderer"]);
        assert_eq!(matching(COMPONENTS, "trail rend"), ["TrailRenderer"], "each word, any order");
        assert_eq!(matching(COMPONENTS, "REND trail"), ["TrailRenderer"]);
        assert!(matching(COMPONENTS, "zzz").is_empty());
        // Names starting with it come first, then the rest in order.
        assert_eq!(
            matching(COMPONENTS, "r"),
            ["Rigidbody", "Animator", "ParticleSystem", "TrailRenderer"]
        );
    }

    /// One frame of the Add Component list, with what it chose as text.
    fn list_frame(
        ctx: &egui::Context,
        query: &mut String,
        can_create: bool,
        focus: &mut bool,
        events: Vec<egui::Event>,
    ) -> Option<String> {
        let input = egui::RawInput { events, ..Default::default() };
        let mut chosen = None;
        let mut output = ctx.run_ui(input, |ui| {
            chosen =
                add_component_list(ui, COMPONENTS, query, can_create, focus).map(|c| match c {
                    Choice::Add(name) => format!("add {name}"),
                    Choice::Create(name) => format!("create {name}"),
                });
        });
        output.textures_delta.clear();
        chosen
    }

    /// Opens the list, types `text` and presses Enter: what it chose.
    fn search_and_enter(text: &str, can_create: bool) -> (Option<String>, String) {
        let ctx = egui::Context::default();
        let (mut query, mut focus) = (String::new(), true);
        let mut chosen = Vec::new();
        for events in [vec![], vec![], vec![egui::Event::Text(text.into())], vec![enter()], vec![]]
        {
            chosen.extend(list_frame(&ctx, &mut query, can_create, &mut focus, events));
        }
        (chosen.pop(), query)
    }

    #[test]
    fn typing_in_add_component_and_pressing_enter_adds_the_first_match() {
        let (chosen, query) = search_and_enter("rend", true);
        assert_eq!(chosen.as_deref(), Some("add TrailRenderer"));
        assert_eq!(query, "rend");
        assert_eq!(
            search_and_enter("r", true).0.as_deref(),
            Some("add Rigidbody"),
            "best match first"
        );
    }

    #[test]
    fn enter_with_no_match_starts_create_component_with_what_was_typed() {
        assert_eq!(search_and_enter(" Fireball ", true).0.as_deref(), Some("create Fireball"));
        assert_eq!(search_and_enter("Fireball", false).0, None, "an app that can't create");
    }

    #[test]
    fn the_list_chooses_nothing_until_asked() {
        let ctx = egui::Context::default();
        let (mut query, mut focus) = (String::new(), true);
        for _ in 0..3 {
            assert_eq!(list_frame(&ctx, &mut query, true, &mut focus, vec![]), None);
        }
        assert!(!focus, "the search box took the keyboard");
    }

    #[test]
    fn layers_show_their_names() {
        let names = names();
        assert_eq!(layer_label(3, &names), "3  Enemies");
        assert_eq!(layer_label(4, &names), "4", "a blank name is no name");
        assert_eq!(layer_label(7, &[]), "7", "no names at all");
    }

    #[test]
    fn masks_say_what_is_in_them() {
        let names = names();
        assert_eq!(mask_summary(u32::MAX, &names), "Everything");
        assert_eq!(mask_summary(0, &names), "Nothing");
        assert_eq!(mask_summary(0b1001, &names), "Default, Enemies");
        assert_eq!(mask_summary(1 << 9, &names), "Layer 9");
        assert_eq!(mask_summary(0b11110, &names), "4 layers");
    }
}
