//! Move sets as data: what a game's `*.moves.ron` asset holds, and how it
//! becomes [`MoveData`].
//!
//! A [`MoveSetFile`] names each move and gives its frame data. Chains name
//! the next move. A move may name its animation clip (by GUID) and give
//! hitboxes and hurtboxes per animation frame: frame hitboxes replace the
//! move's own box on the ticks that frame covers ([`MoveData::frames`]);
//! hurtboxes are kept for tools and drawn by the editor, while the
//! simulation still takes hurtboxes from bodies.

use std::collections::BTreeMap;

use reign_core::FxVec2;
use serde::{Deserialize, Serialize};

use crate::{HitShape, MoveData, Shake, TickBox};

/// Boxes for one animation frame of a move.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FrameBoxes {
    /// The clip frame, counted from 0.
    pub frame: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hitboxes: Vec<HitShape>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hurtboxes: Vec<HitShape>,
}

/// One move, as authored.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MoveSpec {
    /// The animation clip this move plays, as a GUID (32 hex digits).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<String>,
    pub startup: u16,
    pub active: u16,
    pub recovery: u16,
    pub hitbox: HitShape,
    pub damage: i32,
    pub knockback: FxVec2,
    pub hitstop: u16,
    pub shake: Shake,
    pub recoil: FxVec2,
    pub cancel_from: Option<u16>,
    /// The next move's name.
    pub chain: Option<String>,
    pub mobile: bool,
    pub rehit: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<FrameBoxes>,
}

/// A named set of moves: a `*.moves.ron` file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MoveSetFile {
    pub moves: BTreeMap<String, MoveSpec>,
}

impl MoveSpec {
    /// The spec of `data`, naming chained moves with `name`.
    pub fn from_data<M: Copy>(data: &MoveData<M>, name: impl Fn(M) -> String) -> Self {
        MoveSpec {
            clip: None,
            startup: data.startup,
            active: data.active,
            recovery: data.recovery,
            hitbox: data.hitbox,
            damage: data.damage,
            knockback: data.knockback,
            hitstop: data.hitstop,
            shake: data.shake,
            recoil: data.recoil,
            cancel_from: data.cancel_from,
            chain: data.chain.map(name),
            mobile: data.mobile,
            rehit: data.rehit,
            frames: Vec::new(),
        }
    }

    /// The move's frame data. `chain` turns a move name into the game's
    /// id; `ticks` gives the ticks an animation frame covers (from the
    /// move's clip), for per-frame hitboxes. Per-frame boxes are leaked
    /// into `'static` memory: move sets load once (and again on a hot
    /// reload).
    pub fn to_data<M>(
        &self,
        chain: impl Fn(&str) -> Option<M>,
        ticks: impl Fn(u16) -> Option<(u16, u16)>,
    ) -> MoveData<M> {
        let boxes: Vec<TickBox> = self
            .frames
            .iter()
            .filter_map(|f| {
                let hitbox = *f.hitboxes.first()?;
                let (from, to) = ticks(f.frame)?;
                Some(TickBox { from, to, hitbox })
            })
            .collect();
        let frames: &'static [TickBox] =
            if boxes.is_empty() { &[] } else { Box::leak(boxes.into_boxed_slice()) };
        MoveData {
            startup: self.startup,
            active: self.active,
            recovery: self.recovery,
            hitbox: self.hitbox,
            damage: self.damage,
            knockback: self.knockback,
            hitstop: self.hitstop,
            shake: self.shake,
            recoil: self.recoil,
            cancel_from: self.cancel_from,
            chain: self.chain.as_deref().and_then(chain),
            mobile: self.mobile,
            rehit: self.rehit,
            frames,
        }
    }
}

impl MoveSetFile {
    pub fn get(&self, name: &str) -> Option<&MoveSpec> {
        self.moves.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_round_trip_and_frame_boxes_replace_the_hitbox_on_their_ticks() {
        let mut spec = MoveSpec {
            startup: 2,
            active: 6,
            recovery: 3,
            hitbox: HitShape::new(1, 2, 3, 4),
            chain: Some("Next".into()),
            ..MoveSpec::default()
        };
        spec.frames.push(FrameBoxes {
            frame: 1,
            hitboxes: vec![HitShape::new(9, 9, 9, 9)],
            hurtboxes: vec![HitShape::new(0, 0, 4, 4)],
        });
        let text = ron::to_string(&spec).unwrap();
        assert_eq!(ron::from_str::<MoveSpec>(&text).unwrap(), spec);
        let data: MoveData<u8> =
            spec.to_data(|n| (n == "Next").then_some(7), |f| (f == 1).then_some((3, 5)));
        assert_eq!(data.chain, Some(7));
        assert_eq!(data.shape_at(2), HitShape::new(1, 2, 3, 4));
        assert_eq!(data.shape_at(3), HitShape::new(9, 9, 9, 9));
        assert_eq!(data.shape_at(5), HitShape::new(1, 2, 3, 4));
        let back = MoveSpec::from_data(&data, |m| if m == 7 { "Next".into() } else { "?".into() });
        assert_eq!(back.frames, [], "tick boxes don't go back to frames");
        assert_eq!((back.startup, back.chain.as_deref()), (2, Some("Next")));
    }
}
