//! Project Settings: the dialog behind the toolbar's **Project** menu. For
//! now it holds the project's app icon (see `icon.rs` for the rules): pick a
//! PNG to use as `icon.png` in the project, or go back to the Reign default.
//!
//! What the dialog does is plain functions ([`choose_icon`], [`reset_icon`])
//! so it can be tested without a window.

use std::path::Path;

use reign::assets::RgbaImage;
use reign::editor::egui;

use crate::icon::{self, IconError};

/// Takes the image at `from` as the project's icon: it must pass the icon
/// rules (see [`icon::check`]), and is then copied to `icon.png` in the
/// project folder, replacing the one there. Returns the image. A file that
/// is plainly too big is refused without reading it.
pub fn choose_icon(root: &Path, from: &Path) -> Result<RgbaImage, String> {
    let size =
        std::fs::metadata(from).map_err(|e| format!("Can't read {}: {e}", from.display()))?.len();
    if size > icon::MAX_BYTES as u64 {
        return Err(IconError::TooHeavy { bytes: size as usize }.to_string());
    }
    let bytes = std::fs::read(from).map_err(|e| format!("Can't read {}: {e}", from.display()))?;
    let image = icon::check(&bytes).map_err(|e| e.to_string())?;
    // Written beside the old one first, so a failure leaves the old icon.
    let target = root.join(icon::FILE);
    let temp = root.join(format!("{}.new", icon::FILE));
    std::fs::write(&temp, &bytes).and_then(|()| std::fs::rename(&temp, &target)).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("Can't save {}: {e}", target.display())
    })?;
    Ok(image)
}

/// Goes back to the default icon by removing the project's `icon.png`.
/// Returns whether there was one to remove.
pub fn reset_icon(root: &Path) -> std::io::Result<bool> {
    match std::fs::remove_file(root.join(icon::FILE)) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// What the project's icon is right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconState {
    /// No icon.png: the Reign default.
    Default,
    /// The project's own, with its size in pixels.
    Own { side: u32 },
    /// An icon.png that can't be used, and why: the default is used.
    Unusable(String),
}

/// Looks at the project's `icon.png`.
pub fn icon_state(root: &Path) -> IconState {
    match std::fs::read(root.join(icon::FILE)) {
        Ok(bytes) => match icon::check(&bytes) {
            Ok(image) => IconState::Own { side: image.width },
            Err(e) => IconState::Unusable(e.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => IconState::Default,
        Err(e) => IconState::Unusable(format!("It can't be read: {e}")),
    }
}

/// The side of the icon preview in the dialog, in points.
const PREVIEW: f32 = 96.0;

/// The dialog's state.
#[derive(Default)]
pub struct ProjectSettings {
    open: bool,
    /// What the icon is, and how it looks, refreshed when the dialog opens
    /// and after each change.
    state: Option<(IconState, egui::TextureHandle)>,
    /// What to tell the user after the last action, and whether it's an error.
    message: Option<(String, bool)>,
    /// "Remove icon.png?" is being asked.
    confirm_reset: bool,
    /// A new window icon, for the editor to show once.
    new_icon: Option<RgbaImage>,
}

impl ProjectSettings {
    /// The toolbar's **Project** menu.
    pub fn menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Project", |ui| {
            if ui.button("Project Settings…").clicked() {
                self.open = true;
                self.state = None;
                self.message = None;
                self.confirm_reset = false;
                ui.close();
            }
        });
    }

    /// A new window icon, once, for `EditorApp::take_window_icon`.
    pub fn take_new_icon(&mut self) -> Option<RgbaImage> {
        self.new_icon.take().map(|image| icon::window_icon(&image))
    }

    /// Draws the dialog while it is open.
    pub fn dialog(&mut self, ctx: &egui::Context, root: &Path) {
        if !self.open {
            return;
        }
        if self.state.is_none() {
            self.refresh(ctx, root);
        }
        let mut choose = false;
        let mut ask_reset = false;
        let mut do_reset = false;
        let mut cancel_reset = false;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("project settings")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.heading("Project Settings");
            ui.add_space(8.0);
            ui.strong("App icon");
            ui.weak("Shown on this project's windows and on the game you export.");
            ui.add_space(6.0);
            let Some((state, texture)) = &self.state else { return };
            ui.horizontal(|ui| {
                ui.add(egui::Image::new(texture).fit_to_exact_size(egui::Vec2::splat(PREVIEW)));
                ui.vertical(|ui| {
                    match state {
                        IconState::Default => {
                            ui.label("The Reign default icon.");
                        }
                        IconState::Own { side } => {
                            ui.label(format!(
                                "Your icon: {icon} ({side} × {side} pixels).",
                                icon = icon::FILE
                            ));
                        }
                        IconState::Unusable(why) => {
                            ui.colored_label(
                                egui::Color32::LIGHT_RED,
                                format!(
                                    "{} in the project can't be used, so the default is shown.",
                                    icon::FILE
                                ),
                            );
                            ui.weak(why);
                        }
                    }
                    ui.add_space(6.0);
                    if self.confirm_reset {
                        ui.label(format!("Remove {} from this project?", icon::FILE));
                        ui.horizontal(|ui| {
                            if ui.button("Remove it").clicked() {
                                do_reset = true;
                            }
                            if ui.button("Keep it").clicked() {
                                cancel_reset = true;
                            }
                        });
                    } else {
                        ui.horizontal(|ui| {
                            if ui.button("Choose image…").clicked() {
                                choose = true;
                            }
                            let has_file = !matches!(state, IconState::Default);
                            if ui
                                .add_enabled(has_file, egui::Button::new("Use the default"))
                                .on_hover_text(format!("Removes {} from the project", icon::FILE))
                                .clicked()
                            {
                                ask_reset = true;
                            }
                        });
                    }
                });
            });
            ui.add_space(8.0);
            ui.weak(format!(
                "A PNG, square, from {min} to {max} pixels a side (256 or 512 is a good choice), \
                 at most {mb} MB. It is copied into the project as {file}.",
                min = icon::MIN_SIZE,
                max = icon::MAX_SIZE,
                mb = icon::MAX_BYTES / (1024 * 1024),
                file = icon::FILE,
            ));
            if let Some((message, is_error)) = &self.message {
                ui.add_space(6.0);
                if *is_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, message);
                } else {
                    ui.label(message);
                }
            }
            ui.add_space(10.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
        if modal.should_close() {
            close = true;
        }

        if choose
            && let Some(from) = rfd::FileDialog::new()
                .set_title("Choose the project's icon")
                .add_filter("PNG image", &["png"])
                .pick_file()
        {
            self.apply_choice(ctx, root, &from);
        }
        if ask_reset {
            self.confirm_reset = true;
        }
        if cancel_reset {
            self.confirm_reset = false;
        }
        if do_reset {
            self.confirm_reset = false;
            match reset_icon(root) {
                Ok(_) => {
                    self.new_icon = Some(icon::default_icon().clone());
                    self.message = Some(("Back to the default icon.".into(), false));
                }
                Err(e) => self.message = Some((format!("Can't remove {}: {e}", icon::FILE), true)),
            }
            self.refresh(ctx, root);
        }
        if close {
            self.open = false;
        }
    }

    /// Takes `from` as the icon and updates what the dialog shows.
    fn apply_choice(&mut self, ctx: &egui::Context, root: &Path, from: &Path) {
        match choose_icon(root, from) {
            Ok(image) => {
                self.message = Some((
                    format!(
                        "Saved as {} ({} × {} pixels). The editor's window shows it now; \
                         the game and exports use it from now on.",
                        icon::FILE,
                        image.width,
                        image.height
                    ),
                    false,
                ));
                self.new_icon = Some(image);
            }
            Err(e) => self.message = Some((e, true)),
        }
        self.refresh(ctx, root);
    }

    /// Reads the project's icon again, for the preview and the words.
    fn refresh(&mut self, ctx: &egui::Context, root: &Path) {
        let state = icon_state(root);
        let image = icon::resize(&icon::project_icon(root), (PREVIEW * 2.0) as u32);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width as usize, image.height as usize],
            &image.rgba,
        );
        let texture = ctx.load_texture("project icon", color, egui::TextureOptions::LINEAR);
        self.state = Some((state, texture));
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("reign-settings-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png(side: u32, rgba: [u8; 4]) -> Vec<u8> {
        let image = RgbaImage {
            rgba: rgba.iter().copied().cycle().take((side * side * 4) as usize).collect(),
            width: side,
            height: side,
        };
        icon::encode_png(&image)
    }

    #[test]
    fn choosing_a_good_image_copies_it_into_the_project() {
        let root = temp("choose");
        let from = root.join("art.png");
        std::fs::write(&from, png(256, [10, 20, 30, 255])).unwrap();
        let image = choose_icon(&root, &from).unwrap();
        assert_eq!((image.width, &image.rgba[..4]), (256, &[10, 20, 30, 255][..]));
        // The file is the same bytes, named icon.png, and nothing else is left.
        assert_eq!(std::fs::read(root.join(icon::FILE)).unwrap(), std::fs::read(&from).unwrap());
        assert!(!root.join("icon.png.new").exists());
        assert_eq!(icon_state(&root), IconState::Own { side: 256 });
        // The project now uses it.
        assert_eq!(icon::project_icon(&root), image);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn choosing_again_replaces_the_icon() {
        let root = temp("again");
        let first = root.join("one.png");
        let second = root.join("two.png");
        std::fs::write(&first, png(128, [1, 1, 1, 255])).unwrap();
        std::fs::write(&second, png(512, [9, 9, 9, 255])).unwrap();
        choose_icon(&root, &first).unwrap();
        choose_icon(&root, &second).unwrap();
        assert_eq!(icon_state(&root), IconState::Own { side: 512 });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_bad_choice_says_why_and_changes_nothing() {
        let root = temp("bad");
        let good = root.join("good.png");
        std::fs::write(&good, png(128, [5, 5, 5, 255])).unwrap();
        choose_icon(&root, &good).unwrap();
        let before = std::fs::read(root.join(icon::FILE)).unwrap();

        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            ("tiny.png", png(32, [1, 2, 3, 255]), "at least 64"),
            ("huge.png", png(2048, [1, 2, 3, 255]), "at most 1024"),
            ("photo.jpg", b"\xff\xd8\xff\xe0 jpeg bytes".to_vec(), "must be a PNG"),
            ("clear.png", png(64, [1, 2, 3, 0]), "transparent"),
        ];
        for (name, bytes, words) in cases {
            let path = root.join(name);
            std::fs::write(&path, bytes).unwrap();
            let error = choose_icon(&root, &path).unwrap_err();
            assert!(error.contains(words), "{name}: {error}");
            assert_eq!(
                std::fs::read(root.join(icon::FILE)).unwrap(),
                before,
                "{name} changed the icon"
            );
        }
        // Not square.
        let wide = root.join("wide.png");
        let mut encoder_out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut encoder_out, 128, 64);
            e.set_color(png::ColorType::Rgba);
            e.set_depth(png::BitDepth::Eight);
            e.write_header().unwrap().write_image_data(&vec![7; 128 * 64 * 4]).unwrap();
        }
        std::fs::write(&wide, encoder_out).unwrap();
        assert!(choose_icon(&root, &wide).unwrap_err().contains("must be square"));
        // A file that isn't there.
        assert!(choose_icon(&root, &root.join("nope.png")).unwrap_err().starts_with("Can't read"));
        assert!(!root.join("icon.png.new").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_over_the_limit_is_refused_without_being_read() {
        let root = temp("heavy");
        let big = root.join("big.png");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(icon::MAX_BYTES as u64 + 1).unwrap(); // sparse: not actually written
        let error = choose_icon(&root, &big).unwrap_err();
        assert!(error.contains("at most 8 MB"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn resetting_removes_the_icon_and_is_safe_to_repeat() {
        let root = temp("reset");
        assert_eq!(icon_state(&root), IconState::Default);
        assert!(!reset_icon(&root).unwrap(), "nothing to remove");
        let from = root.join("a.png");
        std::fs::write(&from, png(64, [3, 3, 3, 255])).unwrap();
        choose_icon(&root, &from).unwrap();
        assert!(reset_icon(&root).unwrap());
        assert_eq!(icon_state(&root), IconState::Default);
        assert!(from.is_file(), "the picked image itself is never touched");
        assert_eq!(icon::project_icon(&root), *icon::default_icon());
        assert!(!reset_icon(&root).unwrap());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_unusable_icon_file_is_reported_not_hidden() {
        let root = temp("unusable");
        std::fs::write(root.join(icon::FILE), b"GIF89a not a png").unwrap();
        match icon_state(&root) {
            IconState::Unusable(why) => assert!(why.contains("PNG"), "{why}"),
            other => panic!("{other:?}"),
        }
        // The default is what the project actually uses meanwhile.
        assert_eq!(icon::project_icon(&root), *icon::default_icon());
        let _ = std::fs::remove_dir_all(root);
    }
}
