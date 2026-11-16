//! Scripts → Input Actions: rename actions and bind them by listening:
//! press **+** (or click a binding to change it), then press a key, a pad
//! button or push the stick, or click a mouse button in the box. Saved to
//! `assets/input.ron` (which then hot-reloads).
//!
//! An input belongs to one action. Binding it to another moves it there;
//! changing a binding to one another action has swaps the two.

use std::path::PathBuf;

use reign::editor::{Heard, egui};

use super::{
    Action, InputMap, MAX_ACTIONS, MOUSE_NAMES, key_code, key_name, pad_input, pad_name, pad_names,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Device {
    Key,
    Mouse,
    Pad,
}

/// One binding, by its name in `input.ron`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Bind {
    device: Device,
    name: String,
}

impl Bind {
    fn new(device: Device, name: impl Into<String>) -> Self {
        Bind { device, name: name.into() }
    }

    fn label(&self) -> String {
        match self.device {
            Device::Key => self.name.clone(),
            Device::Mouse => format!("Mouse {}", self.name),
            Device::Pad => format!("Pad {}", self.name),
        }
    }

    /// The name everything else would write for this input, so `"A"` and
    /// `"KeyA"` are the same key.
    fn canonical(&self) -> String {
        let known = match self.device {
            Device::Key => key_code(&self.name).map(|k| format!("{k:?}")),
            Device::Mouse => None,
            Device::Pad => pad_input(&self.name).and_then(pad_name).map(str::to_owned),
        };
        known.unwrap_or_else(|| self.name.clone())
    }

    fn same(&self, other: &Bind) -> bool {
        self.device == other.device && self.canonical() == other.canonical()
    }
}

struct Row {
    name: String,
    binds: Vec<Bind>,
}

/// Waiting for an input for this row: a new one, or in place of one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Listen {
    row: usize,
    replace: Option<usize>,
}

pub struct InputDialog {
    file: PathBuf,
    rows: Vec<Row>,
    listen: Option<Listen>,
    /// What the last binding did to other actions, or why it didn't bind.
    note: Option<String>,
    error: Option<String>,
}

impl InputDialog {
    /// Opens the project's map (`assets/input.ron`), or the defaults.
    pub fn open(file: PathBuf) -> Self {
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| InputMap::parse(&t).ok())
            .unwrap_or_default();
        let mut dialog =
            InputDialog { file, rows: Vec::new(), listen: None, note: None, error: None };
        dialog.load(&map);
        dialog
    }

    fn load(&mut self, map: &InputMap) {
        let binds = |device, names: &[String]| {
            names.iter().map(move |n| Bind::new(device, n.as_str())).collect::<Vec<_>>()
        };
        self.rows = map
            .actions
            .iter()
            .map(|a| {
                let mut all = binds(Device::Key, &a.keys);
                all.extend(binds(Device::Mouse, &a.mouse));
                all.extend(binds(Device::Pad, &a.pad));
                Row { name: a.name.clone(), binds: all }
            })
            .collect();
        self.listen = None;
    }

    fn map(&self) -> InputMap {
        let names = |row: &Row, device| {
            row.binds.iter().filter(|b| b.device == device).map(|b| b.name.clone()).collect()
        };
        InputMap {
            actions: self
                .rows
                .iter()
                .map(|r| Action {
                    name: r.name.trim().to_owned(),
                    keys: names(r, Device::Key),
                    mouse: names(r, Device::Mouse),
                    pad: names(r, Device::Pad),
                })
                .collect(),
        }
    }

    fn validate(&self) -> Result<InputMap, String> {
        let map = self.map();
        for (i, a) in map.actions.iter().enumerate() {
            if a.name.is_empty() {
                return Err(format!("action {} needs a name", i + 1));
            }
            if map.actions[..i].iter().any(|b| b.name == a.name) {
                return Err(format!("two actions are called {:?}", a.name));
            }
        }
        let unknown = map.unknown_bindings();
        if !unknown.is_empty() {
            return Err(format!("unknown keys or buttons: {}", unknown.join(", ")));
        }
        Ok(map)
    }

    /// Keys and pad inputs from the editor, before this frame's UI. While
    /// listening, the first one binds and the keys are taken (returns
    /// true), so the key bound doesn't also press a button or close the
    /// dialog.
    pub fn hear(&mut self, heard: &[Heard]) -> bool {
        if self.listen.is_none() {
            return false;
        }
        for h in heard {
            let bind = match *h {
                Heard::Key(key) => match key_name(key) {
                    Some(name) => Bind::new(Device::Key, name),
                    None => {
                        self.note = Some(format!("{key:?} can't be bound"));
                        continue;
                    }
                },
                // Buttons with no name (the pad's own extras) are skipped.
                Heard::Pad(input) => match pad_name(input) {
                    Some(name) => Bind::new(Device::Pad, name),
                    None => continue,
                },
            };
            self.bind(bind);
            break;
        }
        true
    }

    /// Binds what was heard to the row being listened for, moving it from
    /// (or swapping it with) any other action that had it.
    fn bind(&mut self, bind: Bind) {
        let Some(Listen { row, replace }) = self.listen.take() else { return };
        if row >= self.rows.len() {
            return;
        }
        let old = replace.and_then(|i| self.rows[row].binds.get(i).cloned());
        let this = self.rows[row].name.clone();
        self.note = None;
        for (j, other) in self.rows.iter_mut().enumerate() {
            if j == row {
                continue;
            }
            let Some(k) = other.binds.iter().position(|b| b.same(&bind)) else { continue };
            match &old {
                Some(old) if !old.same(&bind) && !other.binds.iter().any(|b| b.same(old)) => {
                    self.note = Some(format!(
                        "Swapped: {} is now {}, {} is now {}",
                        other.name,
                        old.label(),
                        this,
                        bind.label()
                    ));
                    other.binds[k] = old.clone();
                }
                _ => {
                    self.note = Some(format!("{} moved here from {}", bind.label(), other.name));
                    other.binds.remove(k);
                }
            }
        }
        let binds = &mut self.rows[row].binds;
        let at = match replace {
            Some(i) if i < binds.len() => {
                binds[i] = bind.clone();
                i
            }
            _ => {
                binds.push(bind.clone());
                binds.len() - 1
            }
        };
        // Once per action.
        let mut i = 0;
        binds.retain(|b| {
            let keep = i == at || !b.same(&bind);
            i += 1;
            keep
        });
    }

    /// Draws the dialog. Returns false once it should close.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut open = true;
        let mut save = false;
        let modal = egui::Modal::new(egui::Id::new("input actions")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading("Input Actions");
            ui.weak(
                "Scripts read these with ctx.action(\"Jump\") and ctx.action_pressed(\"Jump\"). \
                 The first eight are also Button::Left … Button::Parry. Press + to listen for \
                 a key, mouse button or pad input; click a binding to change it, right-click \
                 to remove it.",
            );
            ui.add_space(6.0);
            let mut remove_row = None;
            let mut remove_bind = None;
            egui::Grid::new("actions").num_columns(3).striped(true).spacing([8.0, 4.0]).show(
                ui,
                |ui| {
                    ui.strong("Action");
                    ui.strong("Bindings");
                    ui.label("");
                    ui.end_row();
                    for i in 0..self.rows.len() {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.rows[i].name).desired_width(110.0),
                        );
                        ui.horizontal_wrapped(|ui| {
                            ui.set_max_width(460.0);
                            for (k, bind) in self.rows[i].binds.iter().enumerate() {
                                let here = self.listen == Some(Listen { row: i, replace: Some(k) });
                                let label = if here { "press…".to_owned() } else { bind.label() };
                                let chip = ui
                                    .add(egui::Button::new(label).small().selected(here))
                                    .on_hover_text("Click to change, right-click to remove");
                                if chip.clicked() {
                                    self.listen =
                                        (!here).then_some(Listen { row: i, replace: Some(k) });
                                } else if chip.secondary_clicked() {
                                    remove_bind = Some((i, k));
                                }
                            }
                            let adding = self.listen == Some(Listen { row: i, replace: None });
                            let add = ui
                                .add(
                                    egui::Button::new(if adding { "press…" } else { "+" })
                                        .small()
                                        .selected(adding),
                                )
                                .on_hover_text("Listen for a key, mouse button or pad input");
                            if add.clicked() {
                                self.listen = (!adding).then_some(Listen { row: i, replace: None });
                            }
                        });
                        if ui.small_button("✕").on_hover_text("Remove this action").clicked() {
                            remove_row = Some(i);
                        }
                        ui.end_row();
                    }
                },
            );
            if let Some((i, k)) = remove_bind {
                self.rows[i].binds.remove(k);
                self.listen = None;
            }
            if let Some(i) = remove_row {
                self.rows.remove(i);
                self.listen = None;
            }
            if let Some(listen) = self.listen {
                self.listen_ui(ui, listen);
            } else if let Some(note) = &self.note {
                ui.weak(note);
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.rows.len() < MAX_ACTIONS, egui::Button::new("+ Action"))
                    .on_hover_text(format!("Up to {MAX_ACTIONS} actions"))
                    .clicked()
                {
                    self.rows.push(Row { name: String::new(), binds: Vec::new() });
                }
                if ui.button("Reset to defaults").clicked() {
                    self.load(&InputMap::default());
                    self.note = None;
                }
            });
            // Bad names show as you type.
            let check = self.validate();
            if let Err(e) = &check {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            } else if let Some(e) = &self.error {
                ui.colored_label(egui::Color32::LIGHT_RED, e);
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(check.is_ok(), egui::Button::new("Save")).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    open = false;
                }
            });
        });
        if modal.should_close() && self.listen.is_none() {
            open = false;
        }
        if save && let Ok(map) = self.validate() {
            match std::fs::write(&self.file, map.to_text()) {
                Ok(()) => {
                    log::info!("saved {}", self.file.display());
                    open = false;
                }
                Err(e) => self.error = Some(format!("couldn't save: {e}")),
            }
        }
        open
    }

    /// While listening: where to click a mouse button, menus for inputs
    /// with nothing to press (no pad plugged in), and Stop.
    fn listen_ui(&mut self, ui: &mut egui::Ui, listen: Listen) {
        let action = self.rows.get(listen.row).map_or("", |r| r.name.as_str()).to_owned();
        ui.group(|ui| {
            ui.label(format!(
                "Listening for {}: press a key, press a pad button or push the stick, or \
                 click in the box with a mouse button.",
                if action.is_empty() { "this action" } else { &action }
            ));
            let (rect, _) = ui
                .allocate_exact_size(egui::vec2(ui.available_width(), 36.0), egui::Sense::click());
            let visuals = ui.visuals();
            ui.painter().rect_stroke(
                rect,
                4.0,
                visuals.widgets.active.bg_stroke,
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Click here with any mouse button",
                egui::FontId::proportional(13.0),
                visuals.weak_text_color(),
            );
            let clicked = ui.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::PointerButton { pos, button, pressed: true, .. }
                        if rect.contains(*pos) =>
                    {
                        mouse_name(*button)
                    }
                    _ => None,
                })
            });
            let mut picked = clicked.map(|name| Bind::new(Device::Mouse, name));
            ui.horizontal(|ui| {
                ui.menu_button("Pad input ▾", |ui| {
                    for name in pad_names() {
                        if ui.button(name).clicked() {
                            picked = Some(Bind::new(Device::Pad, name));
                        }
                    }
                });
                ui.menu_button("Mouse button ▾", |ui| {
                    for name in MOUSE_NAMES {
                        if ui.button(name).clicked() {
                            picked = Some(Bind::new(Device::Mouse, name));
                        }
                    }
                });
                if ui.button("Stop listening").clicked() {
                    self.listen = None;
                }
            });
            if let Some(bind) = picked {
                self.bind(bind);
            }
            if let Some(note) = &self.note {
                ui.weak(note);
            }
        });
    }
}

/// The name `input.ron` uses for an egui mouse button.
fn mouse_name(button: egui::PointerButton) -> Option<&'static str> {
    use egui::PointerButton::*;
    Some(match button {
        Primary => "Left",
        Secondary => "Right",
        Middle => "Middle",
        Extra1 => "Back",
        Extra2 => "Forward",
    })
}

#[cfg(test)]
mod tests {
    use reign::app::PadInput;
    use reign::app::gilrs::Button as Pad;
    use reign::app::winit::keyboard::KeyCode;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("reign-input-{name}-{}.ron", std::process::id()))
    }

    fn dialog() -> InputDialog {
        InputDialog::open(temp("nothing-here"))
    }

    fn labels(d: &InputDialog, row: usize) -> Vec<String> {
        d.rows[row].binds.iter().map(Bind::label).collect()
    }

    #[test]
    fn opens_defaults_and_validates_edits() {
        let file = temp("dialog");
        let _ = std::fs::remove_file(&file);
        let mut d = InputDialog::open(file.clone());
        assert_eq!(d.map(), InputMap::default(), "no file: the defaults");

        d.rows[8].name = "Use".into();
        d.rows[9].name = "Use".into();
        assert!(d.validate().unwrap_err().contains("two actions"));
        d.rows[9].name = "Pause".into();
        d.rows[9].binds.push(Bind::new(Device::Pad, "Banana"));
        assert!(d.validate().unwrap_err().contains("Banana"));
        d.rows[9].binds.pop();

        // What it saves, the game loads.
        d.listen = Some(Listen { row: 8, replace: None });
        d.bind(Bind::new(Device::Mouse, "Right"));
        std::fs::write(&file, d.validate().unwrap().to_text()).unwrap();
        let reopened = InputDialog::open(file.clone()).map();
        assert_eq!(reopened.actions[8].name, "Use");
        assert_eq!(reopened.actions[8].mouse, ["Right"]);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn listening_binds_the_next_key_or_pad_input() {
        let mut d = dialog();
        assert!(!d.hear(&[Heard::Key(KeyCode::KeyQ)]), "not listening: keys aren't taken");
        assert_eq!(d.map(), InputMap::default());

        d.listen = Some(Listen { row: 4, replace: None });
        assert!(d.hear(&[]), "listening takes the keys even with none pressed");
        assert!(d.hear(&[Heard::Key(KeyCode::KeyQ), Heard::Key(KeyCode::KeyR)]));
        assert_eq!(labels(&d, 4), ["Space", "Pad South", "KeyQ"], "the first one only");
        assert!(d.listen.is_none());

        d.listen = Some(Listen { row: 4, replace: None });
        d.hear(&[Heard::Pad(PadInput::Button(Pad::RightTrigger2))]);
        assert_eq!(d.map().actions[4].pad, ["South", "RightTrigger2"]);

        d.listen = Some(Listen { row: 0, replace: None });
        d.hear(&[Heard::Key(KeyCode::F24)]);
        assert!(d.listen.is_some(), "a key with no name keeps listening");
        assert!(d.note.as_deref().unwrap().contains("F24"));
    }

    #[test]
    fn an_input_moves_from_the_action_that_had_it() {
        let mut d = dialog();
        // Space is Jump's; give it to Attack.
        d.listen = Some(Listen { row: 5, replace: None });
        d.hear(&[Heard::Key(KeyCode::Space)]);
        assert_eq!(d.map().actions[4].keys, Vec::<String>::new());
        assert!(d.map().actions[5].keys.contains(&"Space".into()));
        assert!(d.note.as_deref().unwrap().contains("from Jump"));
    }

    #[test]
    fn changing_a_binding_to_another_actions_swaps_them() {
        let mut d = dialog();
        // Jump's Space becomes KeyJ, which was Attack's: Attack gets Space.
        let space = d.rows[4].binds.iter().position(|b| b.name == "Space").unwrap();
        d.listen = Some(Listen { row: 4, replace: Some(space) });
        d.hear(&[Heard::Key(KeyCode::KeyJ)]);
        let map = d.map();
        assert_eq!(map.actions[4].keys, ["KeyJ"]);
        assert_eq!(map.actions[5].keys, ["Space", "KeyZ"], "in KeyJ's place");
        assert!(d.note.as_deref().unwrap().starts_with("Swapped"));
    }

    #[test]
    fn an_action_has_each_input_once() {
        let mut d = dialog();
        // "A" in the file is the same key as the KeyA a listener hears.
        d.rows[0].binds = vec![Bind::new(Device::Key, "A"), Bind::new(Device::Key, "ArrowLeft")];
        d.listen = Some(Listen { row: 0, replace: None });
        d.hear(&[Heard::Key(KeyCode::KeyA)]);
        assert_eq!(labels(&d, 0), ["ArrowLeft", "KeyA"]);
        // Changing ArrowLeft to KeyA leaves one KeyA where ArrowLeft was.
        d.listen = Some(Listen { row: 0, replace: Some(0) });
        d.hear(&[Heard::Key(KeyCode::KeyA)]);
        assert_eq!(labels(&d, 0), ["KeyA"]);
    }

    #[test]
    fn every_mouse_button_has_a_name_the_map_knows() {
        use egui::PointerButton::*;
        for b in [Primary, Secondary, Middle, Extra1, Extra2] {
            let name = mouse_name(b).unwrap();
            assert!(super::super::mouse_button(name).is_some(), "{b:?}");
        }
    }
}
