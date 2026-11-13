//! The launch intro, about 2.4 seconds:
//!
//! 1. A red seam grows out of the dark at the wordmark's left edge.
//! 2. The seam sweeps right and wipes `REIGN` in behind it, with a hairline
//!    underneath; `ENGINE HUB` and the version rise in below.
//! 3. Red floods out from the seam until it fills the window, turning the
//!    wordmark dark.
//! 4. The red splits along the middle and the two halves slide away,
//!    opening onto the hub.
//!
//! Any click or key jumps straight to the split.

use eframe::egui::{self, Align2, Color32, Id, LayerId, Order, Pos2, Rect, Sense, Stroke, Vec2};

use crate::theme::{self, Face, INK, MUTED, RED, TEXT, ease_in_out, ease_out, span};

/// When the red starts to split open.
const SPLIT: f32 = 1.85;
pub const LENGTH: f32 = 2.45;

pub struct Intro {
    start: Option<f64>,
    /// Added to the clock after a skip.
    skipped_to: Option<f32>,
}

impl Intro {
    pub fn new() -> Self {
        Intro { start: None, skipped_to: None }
    }

    pub fn finished() -> Self {
        Intro { start: Some(f64::NEG_INFINITY), skipped_to: None }
    }

    fn time(&mut self, ctx: &egui::Context) -> f32 {
        let now = ctx.input(|i| i.time);
        let start = *self.start.get_or_insert(now);
        let t = (now - start) as f32;
        match self.skipped_to {
            Some(offset) => t + offset,
            None => t,
        }
    }

    /// Draws the intro over everything. Call after the hub's own UI.
    pub fn show(&mut self, ctx: &egui::Context, grain: &egui::TextureHandle, version: &str) {
        let t = self.time(ctx);
        if t >= LENGTH {
            return;
        }
        ctx.request_repaint();

        // Swallow input under the intro; a click or key skips ahead.
        let screen = ctx.content_rect();
        let skip = egui::Area::new(Id::new("intro-blocker"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| ui.allocate_rect(screen, Sense::click()))
            .inner
            .clicked()
            || ctx.input(|i| {
                i.events.iter().any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            });
        if skip && t < SPLIT {
            let raw = t - self.skipped_to.unwrap_or(0.0);
            self.skipped_to = Some(SPLIT - raw);
        }

        let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("intro")));
        let c = screen.center();

        // The wordmark and its geometry.
        let word = painter.layout_job(theme::text("REIGN", Face::Display, 168.0, TEXT, 4.0));
        let size = word.size();
        let word_pos = Pos2::new(c.x - size.x / 2.0, c.y - size.y / 2.0 - 14.0);
        let word_rect = Rect::from_min_size(word_pos, size);
        let (x0, x1) = (word_rect.left() - 18.0, word_rect.right() + 18.0);
        let seam_h = size.y * 0.78;
        let seam_mid = word_rect.center().y + 6.0;

        if t < SPLIT {
            // 1-3: dark ground, seam, wipe, flood.
            painter.rect_filled(screen, 0.0, INK);
            theme::grain(&painter, screen, grain, 0.5);

            let grow = ease_out(span(t, 0.05, 0.5));
            let sweep = ease_in_out(span(t, 0.5, 1.15));
            let seam_x = x0 + (x1 - x0) * sweep;
            let flood = ease_in_out(span(t, 1.35, SPLIT));

            // Wordmark, revealed left of the seam.
            let reveal =
                Rect::from_min_max(Pos2::new(x0, screen.top()), Pos2::new(seam_x, screen.bottom()));
            let rise = (1.0 - sweep) * 10.0;
            painter.with_clip_rect(reveal).galley_with_override_text_color(
                word_pos + Vec2::new(0.0, rise),
                word.clone(),
                TEXT,
            );

            // Hairline under the wordmark, drawn along with the seam.
            let rule_y = word_rect.bottom() + 4.0;
            if sweep > 0.0 {
                painter.hline(x0..=seam_x, rule_y, Stroke::new(1.0, MUTED.gamma_multiply(0.6)));
            }

            // Caption under the rule.
            let cap = ease_out(span(t, 1.0, 1.4));
            if cap > 0.0 {
                let y = rule_y + 14.0 + (1.0 - cap) * 8.0;
                let a = cap;
                painter.text(
                    Pos2::new(x0, y),
                    Align2::LEFT_TOP,
                    "ENGINE HUB",
                    Face::SemiBold.id(11.0),
                    MUTED.gamma_multiply(a),
                );
                painter.text(
                    Pos2::new(x1, y),
                    Align2::RIGHT_TOP,
                    format!("v{version}"),
                    Face::SemiBold.id(11.0),
                    MUTED.gamma_multiply(a),
                );
            }

            // The seam itself.
            let h = seam_h * grow;
            if flood <= 0.0 {
                let seam = Rect::from_center_size(Pos2::new(seam_x, seam_mid), Vec2::new(3.0, h));
                painter.rect_filled(seam, 0.0, RED);
                // A soft glow either side.
                painter.rect_filled(
                    seam.expand2(Vec2::new(5.0, 0.0)),
                    0.0,
                    RED.gamma_multiply(0.12),
                );
            } else {
                // The red floods out from the seam to fill the window.
                let lerp = |a: f32, b: f32| a + (b - a) * flood;
                let red = Rect::from_min_max(
                    Pos2::new(
                        lerp(seam_x - 1.5, screen.left()),
                        lerp(seam_mid - h / 2.0, screen.top()),
                    ),
                    Pos2::new(
                        lerp(seam_x + 1.5, screen.right()),
                        lerp(seam_mid + h / 2.0, screen.bottom()),
                    ),
                );
                painter.rect_filled(red, 0.0, RED);
                theme::grain(&painter, red, grain, 0.35);
                painter.with_clip_rect(red).galley_with_override_text_color(
                    word_pos,
                    word.clone(),
                    INK,
                );
            }
        } else {
            // 4: the red splits and slides apart, carrying the wordmark.
            let open = ease_in_out(span(t, SPLIT, LENGTH));
            let half = screen.height() / 2.0;
            let shift = half * open;
            for (dir, clip) in [
                (-1.0, Rect::from_min_max(screen.min, Pos2::new(screen.right(), c.y))),
                (1.0, Rect::from_min_max(Pos2::new(screen.left(), c.y), screen.max)),
            ] {
                let moved = clip.translate(Vec2::new(0.0, dir * shift));
                let p = painter.with_clip_rect(moved);
                p.rect_filled(moved, 0.0, RED);
                theme::grain(&p, moved, grain, 0.35);
                p.galley_with_override_text_color(
                    word_pos + Vec2::new(0.0, dir * shift),
                    word.clone(),
                    INK,
                );
                // A dark edge where the halves part.
                let edge_y = if dir < 0.0 { moved.bottom() } else { moved.top() };
                p.hline(moved.x_range(), edge_y, Stroke::new(2.0, Color32::from_black_alpha(90)));
            }
        }
    }
}
