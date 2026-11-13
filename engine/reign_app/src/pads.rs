//! Gamepads, through gilrs. Every connected pad counts: the left stick and
//! d-pad are one shared direction, and button presses are reported per
//! frame, with a held direction repeating like a held key for menus.
//!
//! What the buttons mean is up to the game; [`Pads::any_pressed`] and
//! [`Pads::direction`] are enough to map them onto its own bindings.

use gilrs::{Axis, Button, EventType, Gilrs};

/// How far the stick must lean to count as a direction.
pub const DEADZONE: f32 = 0.45;
/// A held direction repeats after this long, then this often.
pub const REPEAT_DELAY: f32 = 0.38;
pub const REPEAT_EVERY: f32 = 0.09;

pub struct Pads {
    gilrs: Option<Gilrs>,
    /// The direction held last poll, and when it next repeats.
    held_dir: (i32, i32),
    repeat_in: f32,
    /// Off while a host has given input to something else.
    enabled: bool,
}

/// What the pads did since the last poll.
#[derive(Clone, Debug, Default)]
pub struct PadFrame {
    /// Buttons pressed, d-pad excluded.
    pub pressed: Vec<Button>,
    /// Direction presses, including repeats while held, as -1, 0 or 1 on
    /// each axis; y is down.
    pub nav: Vec<(i32, i32)>,
    /// Anything at all happened, for showing pad prompts.
    pub active: bool,
}

impl Pads {
    /// Opens gilrs. Without gamepad support the pads simply never report
    /// anything.
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => {
                for (_, pad) in g.gamepads() {
                    log::info!("gamepad: {}", pad.name());
                }
                Some(g)
            }
            Err(e) => {
                log::warn!("gamepads unavailable: {e}");
                None
            }
        };
        Pads { gilrs, held_dir: (0, 0), repeat_in: 0.0, enabled: true }
    }

    /// Turns the pads off or on for the app. While off, polls still drain
    /// events but report nothing, and nothing reads as held: a host such as
    /// an editor turns them off while its own UI has the input.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Drains pad events; `dt` is the seconds since the last poll, for
    /// direction repeat.
    pub fn poll(&mut self, dt: f32) -> PadFrame {
        let mut frame = PadFrame::default();
        let Some(gilrs) = &mut self.gilrs else { return frame };
        while let Some(event) = gilrs.next_event() {
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    frame.active = true;
                    if !is_dpad(button) {
                        frame.pressed.push(button);
                    }
                }
                EventType::AxisChanged(..) | EventType::ButtonChanged(..) => {}
                EventType::Connected => {
                    log::info!("gamepad connected: {}", gilrs.gamepad(event.id).name());
                }
                EventType::Disconnected => log::info!("gamepad disconnected"),
                _ => {}
            }
        }
        if !self.enabled {
            self.held_dir = (0, 0);
            return PadFrame::default();
        }
        let dir = self.direction();
        if dir != (0, 0) {
            frame.active = true;
        }
        if dir != self.held_dir {
            self.held_dir = dir;
            self.repeat_in = REPEAT_DELAY;
            if dir != (0, 0) {
                frame.nav.push(dir);
            }
        } else if dir != (0, 0) {
            self.repeat_in -= dt;
            if self.repeat_in <= 0.0 {
                self.repeat_in += REPEAT_EVERY;
                frame.nav.push(dir);
            }
        }
        frame
    }

    /// The direction held on any pad's d-pad or left stick, as -1, 0 or 1
    /// on each axis; y is down.
    pub fn direction(&self) -> (i32, i32) {
        let Some(gilrs) = self.gilrs.as_ref().filter(|_| self.enabled) else { return (0, 0) };
        let (mut x, mut y) = (0, 0);
        for (_, pad) in gilrs.gamepads() {
            let sx = pad.value(Axis::LeftStickX);
            let sy = pad.value(Axis::LeftStickY);
            if pad.is_pressed(Button::DPadLeft) || sx < -DEADZONE {
                x = -1;
            }
            if pad.is_pressed(Button::DPadRight) || sx > DEADZONE {
                x = 1;
            }
            if pad.is_pressed(Button::DPadUp) || sy > DEADZONE {
                y = -1;
            }
            if pad.is_pressed(Button::DPadDown) || sy < -DEADZONE {
                y = 1;
            }
        }
        (x, y)
    }

    /// Whether `button` is held on any connected pad.
    pub fn any_pressed(&self, button: Button) -> bool {
        let Some(gilrs) = self.gilrs.as_ref().filter(|_| self.enabled) else { return false };
        gilrs.gamepads().any(|(_, pad)| pad.is_pressed(button))
    }
}

impl Default for Pads {
    fn default() -> Self {
        Self::new()
    }
}

fn is_dpad(b: Button) -> bool {
    matches!(b, Button::DPadUp | Button::DPadDown | Button::DPadLeft | Button::DPadRight)
}
