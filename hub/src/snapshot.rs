//! UI checks without a person at the screen: with `REIGN_HUB_SNAPSHOTS=<dir>`
//! the hub plays a fixed script (intro frames, each page, the New project
//! dialog), saves PNGs of its own window into `<dir>`, then quits.

use std::path::PathBuf;

use eframe::egui;

pub enum Step {
    Shot(&'static str),
    About,
    Projects,
    NewProject,
    /// Closes any dialog.
    CloseDialogs,
    /// The Engine version dialog, for the first project listed.
    EngineVersion,
    /// What opening a project made with an engine that isn't here asks.
    OpenChoice,
    Quit,
}

const SCRIPT: &[(f64, Step)] = &[
    (0.30, Step::Shot("intro-1-seam")),
    (0.85, Step::Shot("intro-2-wipe")),
    (1.30, Step::Shot("intro-3-caption")),
    (1.65, Step::Shot("intro-4-flood")),
    (2.12, Step::Shot("intro-5-split")),
    (3.20, Step::Shot("projects")),
    (3.40, Step::About),
    (3.90, Step::Shot("about")),
    (4.10, Step::Projects),
    (4.20, Step::NewProject),
    (4.80, Step::Shot("new-project")),
    (5.00, Step::CloseDialogs),
    (5.10, Step::EngineVersion),
    (7.00, Step::Shot("engine-version")),
    (7.20, Step::CloseDialogs),
    (7.30, Step::OpenChoice),
    (7.80, Step::Shot("open-choice")),
    (8.20, Step::Quit),
];

pub struct Snapshots {
    dir: PathBuf,
    start: Option<f64>,
    next: usize,
    /// A screenshot asked for and not yet back.
    waiting: Option<&'static str>,
}

impl Snapshots {
    pub fn from_env() -> Option<Self> {
        let dir = PathBuf::from(std::env::var_os("REIGN_HUB_SNAPSHOTS")?);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Snapshots { dir, start: None, next: 0, waiting: None })
    }

    /// Saves any screenshot that came back, and returns the step due now.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<&'static Step> {
        ctx.request_repaint();
        let images: Vec<_> = ctx.input(|i| {
            i.raw
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
                .collect()
        });
        for image in images {
            if let Some(name) = self.waiting.take() {
                self.save(name, &image);
            }
        }
        if self.waiting.is_some() {
            return None;
        }
        let now = ctx.input(|i| i.time);
        let start = *self.start.get_or_insert(now);
        let (at, step) = SCRIPT.get(self.next)?;
        if now - start < *at {
            return None;
        }
        self.next += 1;
        if let Step::Shot(name) = step {
            self.waiting = Some(name);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        Some(step)
    }

    fn save(&self, name: &str, image: &egui::ColorImage) {
        let path = self.dir.join(format!("{name}.png"));
        let [w, h] = image.size;
        let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        let result = std::fs::File::create(&path).map_err(|e| e.to_string()).and_then(|file| {
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
            writer.write_image_data(&rgba).map_err(|e| e.to_string())
        });
        match result {
            Ok(()) => log::info!("saved {}", path.display()),
            Err(e) => log::error!("{}: {e}", path.display()),
        }
    }
}
