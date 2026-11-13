//! The Create menu: an empty object, plus whatever the app offers
//! ([`crate::EditorApp::create_items`]) as nested submenus. The same menu
//! shows in the toolbar's GameObject menu and on right-click in the
//! Hierarchy; the shell makes the object once the panels are drawn.

use egui::Ui;
use reign_scene::ObjectId;

/// An object a menu or shortcut asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreateRequest {
    /// The app's item path, or `None` for an empty object.
    pub item: Option<String>,
    /// Made as a child of this object, or at the top level.
    pub parent: Option<ObjectId>,
}

/// Fills a menu with Create Empty and the app's items, all made under
/// `parent`.
pub fn menu(
    ui: &mut Ui,
    items: &[String],
    parent: Option<ObjectId>,
    out: &mut Option<CreateRequest>,
) {
    let empty =
        if parent.is_some() { "Create Empty Child" } else { "Create Empty  (Ctrl+Shift+N)" };
    if ui.button(empty).clicked() {
        *out = Some(CreateRequest { item: None, parent });
        ui.close();
    }
    if items.is_empty() {
        return;
    }
    ui.separator();
    let paths: Vec<Vec<&str>> = items.iter().map(|p| p.split('/').collect()).collect();
    level(ui, items, &paths, 0, &[], parent, out);
}

/// One level of the tree: items whose path continues past `prefix`.
fn level(
    ui: &mut Ui,
    items: &[String],
    paths: &[Vec<&str>],
    depth: usize,
    prefix: &[&str],
    parent: Option<ObjectId>,
    out: &mut Option<CreateRequest>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        if path.len() <= depth || path[..depth] != *prefix {
            continue;
        }
        let name = path[depth];
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        if path.len() == depth + 1 {
            if ui.button(name).clicked() {
                *out = Some(CreateRequest { item: Some(items[i].clone()), parent });
                ui.close();
            }
        } else {
            let mut next = prefix.to_vec();
            next.push(name);
            ui.menu_button(name, |ui| level(ui, items, paths, depth + 1, &next, parent, out));
        }
    }
}
