//! Per-tick input snapshots and press buffering.
//!
//! The simulation only ever sees an [`InputFrame`] per tick. Devices are
//! mapped to frames outside the simulation, which is also what a replay file
//! or a rollback peer sends.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Button {
    Left = 1 << 0,
    Right = 1 << 1,
    Up = 1 << 2,
    Down = 1 << 3,
    Jump = 1 << 4,
    Attack = 1 << 5,
    Dash = 1 << 6,
    Parry = 1 << 7,
    Ex = 1 << 8,
    Relics = 1 << 9,
}

impl Button {
    pub const ALL: [Button; 10] = [
        Button::Left,
        Button::Right,
        Button::Up,
        Button::Down,
        Button::Jump,
        Button::Attack,
        Button::Dash,
        Button::Parry,
        Button::Ex,
        Button::Relics,
    ];
}

/// Buttons held during one simulation tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InputFrame(u16);

impl InputFrame {
    pub const fn from_bits(bits: u16) -> Self {
        InputFrame(bits)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }

    pub fn held(self, button: Button) -> bool {
        self.0 & button as u16 != 0
    }

    pub fn set(&mut self, button: Button, down: bool) {
        if down {
            self.0 |= button as u16;
        } else {
            self.0 &= !(button as u16);
        }
    }

    /// -1, 0 or 1 from Left/Right; pressing both cancels out.
    pub fn axis_x(self) -> i32 {
        self.held(Button::Right) as i32 - self.held(Button::Left) as i32
    }
}

/// Remembers recent presses so an input slightly before it is legal still
/// lands (jump buffering, combo buffering into cancel windows).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InputBuffer {
    current: InputFrame,
    previous: InputFrame,
    /// Ticks since each button was last pressed, indexed like `Button::ALL`.
    since_press: [u16; 10],
}

impl Default for InputBuffer {
    fn default() -> Self {
        // Nothing has been pressed yet, so nothing is buffered.
        InputBuffer {
            current: InputFrame::default(),
            previous: InputFrame::default(),
            since_press: [u16::MAX; 10],
        }
    }
}

impl InputBuffer {
    pub fn push(&mut self, frame: InputFrame) {
        self.previous = self.current;
        self.current = frame;
        for (i, button) in Button::ALL.iter().enumerate() {
            if self.just_pressed(*button) {
                self.since_press[i] = 0;
            } else {
                self.since_press[i] = self.since_press[i].saturating_add(1);
            }
        }
    }

    pub fn frame(&self) -> InputFrame {
        self.current
    }

    pub fn just_pressed(&self, button: Button) -> bool {
        self.current.held(button) && !self.previous.held(button)
    }

    /// True if `button` was pressed within the last `window` ticks and not yet
    /// consumed. A window of 0 means this tick only.
    pub fn buffered(&self, button: Button, window: u16) -> bool {
        self.since_press[Self::index(button)] <= window
    }

    /// Marks a buffered press as used so one press triggers one action.
    pub fn consume(&mut self, button: Button) {
        self.since_press[Self::index(button)] = u16::MAX;
    }

    fn index(button: Button) -> usize {
        (button as u16).trailing_zeros() as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(buttons: &[Button]) -> InputFrame {
        let mut f = InputFrame::default();
        for b in buttons {
            f.set(*b, true);
        }
        f
    }

    #[test]
    fn early_press_is_buffered_then_consumed() {
        let mut buf = InputBuffer::default();
        buf.push(InputFrame::default());
        buf.push(frame(&[Button::Jump]));
        buf.push(frame(&[Button::Jump]));
        buf.push(InputFrame::default());
        assert!(buf.buffered(Button::Jump, 2));
        assert!(!buf.buffered(Button::Jump, 1));
        buf.consume(Button::Jump);
        assert!(!buf.buffered(Button::Jump, 10));
    }

    #[test]
    fn fresh_buffer_holds_no_presses() {
        let mut buf = InputBuffer::default();
        buf.push(InputFrame::default());
        assert!(!buf.buffered(Button::Jump, 10));
    }

    #[test]
    fn opposite_directions_cancel() {
        assert_eq!(frame(&[Button::Left, Button::Right]).axis_x(), 0);
        assert_eq!(frame(&[Button::Left]).axis_x(), -1);
    }
}
