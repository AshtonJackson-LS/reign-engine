//! Undo and redo for the asset tools, which edit a working copy of one
//! asset rather than the scene.

/// Snapshots of a value being edited. Each frame the tool calls
/// [`Edits::track`] after its UI: a change starts an edit, and the edit is
/// recorded once the pointer lets go, so a drag or a paint stroke is one
/// undo step.
pub struct Edits<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    /// The value before the edit in progress.
    base: Option<T>,
    /// The value as of the last frame.
    last: T,
}

/// Undo steps each tool keeps.
pub const TOOL_HISTORY: usize = 100;

impl<T: Clone + PartialEq> Edits<T> {
    pub fn new(value: &T) -> Self {
        Edits { undo: Vec::new(), redo: Vec::new(), base: None, last: value.clone() }
    }

    /// Notes this frame's value. `holding` is true while the pointer is
    /// down, so the edit goes on.
    pub fn track(&mut self, value: &T, holding: bool) {
        if *value != self.last {
            if self.base.is_none() {
                self.base = Some(self.last.clone());
            }
            self.last = value.clone();
        }
        if !holding && let Some(base) = self.base.take() {
            self.undo.push(base);
            if self.undo.len() > TOOL_HISTORY {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    pub fn undo(&mut self, value: &mut T) -> bool {
        self.finish();
        let Some(before) = self.undo.pop() else { return false };
        self.redo.push(std::mem::replace(value, before));
        self.last = value.clone();
        true
    }

    pub fn redo(&mut self, value: &mut T) -> bool {
        self.finish();
        let Some(after) = self.redo.pop() else { return false };
        self.undo.push(std::mem::replace(value, after));
        self.last = value.clone();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.base.is_some()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Recorded undo steps (not counting an edit still in progress).
    pub fn steps(&self) -> usize {
        self.undo.len()
    }

    fn finish(&mut self) {
        if let Some(base) = self.base.take() {
            self.undo.push(base);
            self.redo.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_edit_is_one_step() {
        let mut v = 0;
        let mut edits = Edits::new(&v);
        for _ in 0..5 {
            v += 1;
            edits.track(&v, true);
        }
        edits.track(&v, false);
        v = 9;
        edits.track(&v, false);
        assert!(edits.undo(&mut v));
        assert_eq!(v, 5);
        assert!(edits.undo(&mut v));
        assert_eq!(v, 0);
        assert!(!edits.undo(&mut v));
        assert!(edits.redo(&mut v));
        assert_eq!(v, 5);
        assert!(edits.redo(&mut v));
        assert_eq!(v, 9);
    }
}
