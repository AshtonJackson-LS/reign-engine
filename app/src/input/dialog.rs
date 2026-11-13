//! Scripts → Input Actions: rename actions and rebind keys and pad buttons,
//! saved to `assets/input.ron` (which then hot-reloads).

use std::path::PathBuf;

use reign::editor::egui;

use super::{Action, InputMap, MAX_ACTIONS, key_code};

struct Row {
    name: String,
    keys: String,
    pad: String,
}

pub struct InputDialog {
    file: PathBuf,
    rows: Vec<Row>,
    /// The row waiting for a key press to add.
    capturing: Option<usize>,
    error: Option<String>,
}

fn join(list: &[String]) -> String {
    list.join(", ")
}

fn split(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

impl InputDialog {
    /// Opens the project's map (`assets/input.ron`), or the defaults.
    pub fn open(file: PathBuf) -> Self {
        let map = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| InputMap::parse(&t).ok())
            .unwrap_or_default();
        let mut dialog = InputDialog { file, rows: Vec::new(), capturing: None, error: None };
        dialog.load(&map);
        dialog
    }

    fn load(&mut self, map: &InputMap) {
        self.rows = map
            .actions
            .iter()
            .map(|a| Row { name: a.name.clone(), keys: join(&a.keys), pad: join(&a.pad) })
            .collect();
    }

    fn map(&self) -> InputMap {
        InputMap {
            actions: self
                .rows
                .iter()
                .map(|r| Action {
                    name: r.name.trim().to_owned(),
                    keys: split(&r.keys),
                    pad: split(&r.pad),
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

    /// Draws the dialog. Returns false once it should close.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        // A key pressed while capturing joins that row's keys.
        if let Some(row) = self.capturing {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, .. } => Some(*key),
                    _ => None,
                })
            });
            if let Some(key) = pressed {
                let name = key.name();
                if key_code(name).is_some()
                    && let Some(r) = self.rows.get_mut(row)
                {
                    let mut keys = split(&r.keys);
                    if !keys.iter().any(|k| key_code(k) == key_code(name)) {
                        keys.push(name.to_owned());
                    }
                    r.keys = join(&keys);
                }
                self.capturing = None;
            }
        }

        let mut open = true;
        let mut save = false;
        let modal = egui::Modal::new(egui::Id::new("input actions")).show(ctx, |ui| {
            ui.set_width(640.0);
            ui.heading("Input Actions");
            ui.weak(
                "Scripts read these with ctx.action(\"Jump\") and ctx.action_pressed(\"Jump\"). \
                 The first eight are also Button::Left … Button::Parry.",
            );
            ui.add_space(6.0);
            let mut remove = None;
            egui::Grid::new("actions").num_columns(5).striped(true).spacing([8.0, 4.0]).show(
                ui,
                |ui| {
                    ui.strong("Action");
                    ui.strong("Keys");
                    ui.label("");
                    ui.strong("Gamepad");
                    ui.label("");
                    ui.end_row();
                    for (i, row) in self.rows.iter_mut().enumerate() {
                        ui.add(egui::TextEdit::singleline(&mut row.name).desired_width(110.0));
                        ui.add(egui::TextEdit::singleline(&mut row.keys).desired_width(190.0));
                        let capturing = self.capturing == Some(i);
                        let label = if capturing { "press a key…" } else { "+ key" };
                        if ui.selectable_label(capturing, label).clicked() {
                            self.capturing = if capturing { None } else { Some(i) };
                        }
                        ui.add(egui::TextEdit::singleline(&mut row.pad).desired_width(170.0))
                            .on_hover_text(
                                "South, East, West, North, LeftTrigger, RightTrigger, \
                                 LeftTrigger2, RightTrigger2, Start, Select, DPadUp…, \
                                 LeftStickLeft/Right/Up/Down",
                            );
                        if ui.small_button("✕").on_hover_text("Remove this action").clicked() {
                            remove = Some(i);
                        }
                        ui.end_row();
                    }
                },
            );
            if let Some(i) = remove {
                self.rows.remove(i);
                self.capturing = None;
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(self.rows.len() < MAX_ACTIONS, egui::Button::new("+ Action"))
                    .on_hover_text(format!("Up to {MAX_ACTIONS} actions"))
                    .clicked()
                {
                    self.rows.push(Row {
                        name: String::new(),
                        keys: String::new(),
                        pad: String::new(),
                    });
                }
                if ui.button("Reset to defaults").clicked() {
                    self.load(&InputMap::default());
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
        if modal.should_close() && self.capturing.is_none() {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("reign-input-{name}-{}.ron", std::process::id()))
    }

    #[test]
    fn opens_defaults_and_validates_edits() {
        let file = temp("dialog");
        let _ = std::fs::remove_file(&file);
        let mut d = InputDialog::open(file.clone());
        assert_eq!(d.map(), InputMap::default(), "no file: the defaults");

        d.rows[8].name = "Use".into();
        d.rows[8].keys = " E , F ,".into();
        let map = d.validate().unwrap();
        assert_eq!(map.actions[8].name, "Use");
        assert_eq!(map.actions[8].keys, ["E", "F"]);

        d.rows[9].name = "Use".into();
        assert!(d.validate().unwrap_err().contains("two actions"));
        d.rows[9].name = "Pause".into();
        d.rows[9].pad = "Start, Banana".into();
        assert!(d.validate().unwrap_err().contains("Banana"));

        // What it saves, the game loads.
        d.rows[9].pad = "Start".into();
        std::fs::write(&file, d.validate().unwrap().to_text()).unwrap();
        let reopened = InputDialog::open(file.clone());
        assert_eq!(reopened.map().actions[8].keys, ["E", "F"]);
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn egui_key_names_are_known_keys() {
        use egui::Key;
        for key in [
            Key::A,
            Key::Z,
            Key::Num1,
            Key::Space,
            Key::Escape,
            Key::Enter,
            Key::ArrowLeft,
            Key::F5,
        ] {
            assert!(key_code(key.name()).is_some(), "{:?} → {:?}", key, key.name());
        }
    }
}
