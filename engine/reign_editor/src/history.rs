//! Undo and redo, as whole-scene snapshots.
//!
//! Scenes clone cheaply (their storages are shared copy-on-write), so each
//! undo step is simply the scene as it was before the edit. Restoring one
//! gives back exactly that scene, hash for hash, whatever the edit was:
//! a field, a move, a create, delete, duplicate or reparent.

use reign_scene::Scene;

/// Undo steps kept; older ones are dropped.
pub const HISTORY_LIMIT: usize = 200;

struct Step {
    label: String,
    scene: Scene,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// Records an edit: `before` is the scene as it was before it.
    /// Clears the redo steps.
    pub fn record(&mut self, label: impl Into<String>, before: Scene) {
        self.undo.push(Step { label: label.into(), scene: before });
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Puts back the scene before the last edit, keeping the current one
    /// for redo. Returns what was undone.
    pub fn undo(&mut self, current: &mut Scene) -> Option<String> {
        let step = self.undo.pop()?;
        let now = std::mem::replace(current, step.scene);
        self.redo.push(Step { label: step.label.clone(), scene: now });
        Some(step.label)
    }

    /// Re-applies the last undone edit. Returns what was redone.
    pub fn redo(&mut self, current: &mut Scene) -> Option<String> {
        let step = self.redo.pop()?;
        let before = std::mem::replace(current, step.scene);
        self.undo.push(Step { label: step.label.clone(), scene: before });
        Some(step.label)
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn can_redo(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reign_core::{FxVec2, checksum};

    #[test]
    fn undo_and_redo_restore_identical_scenes() {
        let mut scene = Scene::new();
        let a = scene.spawn("a");
        let mut history = History::default();
        let mut hashes = vec![checksum(&scene)];

        // Move.
        history.record("Move", scene.clone());
        scene.transform_mut(a).unwrap().position = FxVec2::from_px(5, 6);
        hashes.push(checksum(&scene));
        // Create and reparent.
        history.record("Create", scene.clone());
        let b = scene.spawn("b");
        scene.set_parent_keep_world(b, Some(a), None);
        hashes.push(checksum(&scene));
        // Duplicate.
        history.record("Duplicate", scene.clone());
        scene.duplicate(a);
        hashes.push(checksum(&scene));
        // Delete.
        history.record("Delete", scene.clone());
        scene.despawn(a);
        hashes.push(checksum(&scene));

        for expected in hashes.iter().rev().skip(1) {
            assert!(history.undo(&mut scene).is_some());
            assert_eq!(checksum(&scene), *expected);
        }
        assert!(history.undo(&mut scene).is_none());
        for expected in hashes.iter().skip(1) {
            assert!(history.redo(&mut scene).is_some());
            assert_eq!(checksum(&scene), *expected);
        }
        assert!(history.redo(&mut scene).is_none());

        // A new edit after undoing drops the redo steps.
        history.undo(&mut scene);
        history.record("Rename", scene.clone());
        scene.object_mut(scene.roots()[0]).unwrap().name = "renamed".into();
        assert!(history.can_redo().is_none());
        assert_eq!(history.can_undo(), Some("Rename"));
    }
}
