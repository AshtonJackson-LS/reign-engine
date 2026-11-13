//! Attacks as frame data: each move is startup, active and recovery ticks,
//! a hitbox, what a hit does, and when the move can be cancelled.
//!
//! Moves are plain data so the editor and hot reload can author them later.
//! The game decides what a move id means and what happens on a hit; this
//! crate keeps the timing rules the same for every attacker.

mod health;
mod moveset;
mod shake;

pub use health::Health;
pub use moveset::{FrameBoxes, MoveSetFile, MoveSpec};
pub use shake::{ScreenShake, Shake};

use reign_core::FxVec2;
use reign_physics::Aabb;

/// A box placed relative to its owner, authored facing right: `x` from the
/// owner's horizontal centre, `y` from its top. Mirrored when facing left.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    reign_scene::Reflect,
)]
pub struct HitShape {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl HitShape {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        HitShape { x, y, w, h }
    }

    /// Where the shape lands in the world for an owner box and facing.
    pub fn place(&self, owner: Aabb, facing: i32) -> Aabb {
        let centre = owner.x + owner.w / 2;
        let x = if facing >= 0 { centre + self.x } else { centre - self.x - self.w };
        Aabb::new(x, owner.y + self.y, self.w, self.h)
    }
}

/// A hitbox that replaces a move's own for some of its ticks: authored
/// per animation frame, turned into ticks when the move set loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TickBox {
    /// First tick (from the move's first) it applies to.
    pub from: u16,
    /// The tick after its last.
    pub to: u16,
    pub hitbox: HitShape,
}

/// One attack's frame data. `M` is the game's move id type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveData<M> {
    pub startup: u16,
    pub active: u16,
    pub recovery: u16,
    pub hitbox: HitShape,
    pub damage: i32,
    /// Velocity given to whatever is hit, authored facing right.
    pub knockback: FxVec2,
    /// Ticks both sides freeze when the hit lands.
    pub hitstop: u16,
    pub shake: Shake,
    /// Velocity given to the attacker when the hit lands, authored facing
    /// right. Zero leaves the attacker's speed alone.
    pub recoil: FxVec2,
    /// Tick (counted from the move's first) from which jump and dash end
    /// the move. `None` commits to the whole move, as heavy attacks do.
    pub cancel_from: Option<u16>,
    /// The move that Attack starts inside the cancel window. `None` means
    /// the move can only be cancelled into movement, not another attack.
    pub chain: Option<M>,
    /// Keeps running control during the move. Rooted moves stop the
    /// attacker in place.
    pub mobile: bool,
    /// For moves that strike more than once: ticks after a hit lands before
    /// the hitbox can connect again, while still active. 0 hits once.
    pub rehit: u16,
    /// Per-frame hitboxes that replace `hitbox` on their ticks.
    pub frames: &'static [TickBox],
}

impl<M> MoveData<M> {
    pub fn total(&self) -> u16 {
        self.startup + self.active + self.recovery
    }

    pub fn is_active(&self, tick: u16) -> bool {
        (self.startup..self.startup + self.active).contains(&tick)
    }

    pub fn cancellable(&self, tick: u16) -> bool {
        self.cancel_from.is_some_and(|from| tick >= from)
    }

    /// The hitbox on `tick`: a per-frame box if one covers it, else the
    /// move's own.
    pub fn shape_at(&self, tick: u16) -> HitShape {
        self.frames
            .iter()
            .find(|b| (b.from..b.to).contains(&tick))
            .map_or(self.hitbox, |b| b.hitbox)
    }
}

/// Anything that can look up a move's data from its id.
pub trait MoveSet: Copy + 'static {
    fn data(self) -> &'static MoveData<Self>;
}

/// A move in progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Attack<M> {
    pub id: M,
    /// Ticks since the move started.
    pub tick: u16,
    /// Set once the hitbox connects, so one swing hits a target once.
    pub landed: bool,
    /// The tick from which a multi-hit move may connect again.
    pub rehit_at: u16,
}

impl<M: MoveSet> Attack<M> {
    pub fn new(id: M) -> Self {
        Attack { id, tick: 0, landed: false, rehit_at: 0 }
    }

    pub fn data(&self) -> &'static MoveData<M> {
        self.id.data()
    }

    /// Advances one tick. Returns false once the move has finished.
    pub fn advance(&mut self) -> bool {
        self.tick += 1;
        self.tick < self.data().total()
    }

    /// Jump and dash may end the move now.
    pub fn movement_cancellable(&self) -> bool {
        self.data().cancellable(self.tick)
    }

    /// The next attack this one can chain into right now, if any.
    pub fn chain(&self) -> Option<M> {
        self.data().chain.filter(|_| self.movement_cancellable())
    }

    /// The hitbox connected: it stays dead for the rest of the swing, or
    /// for `rehit` ticks on a multi-hit move.
    pub fn land(&mut self) {
        self.landed = true;
        self.rehit_at = self.tick + self.data().rehit;
    }

    /// The live hitbox for an owner box and facing, if the move is active
    /// and ready to connect.
    pub fn hitbox(&self, owner: Aabb, facing: i32) -> Option<Aabb> {
        let data = self.data();
        let ready = !self.landed || (data.rehit > 0 && self.tick >= self.rehit_at);
        (data.is_active(self.tick) && ready).then(|| data.shape_at(self.tick).place(owner, facing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Test {
        Light,
        Heavy,
        Flurry,
    }

    const LIGHT: MoveData<Test> = MoveData {
        startup: 2,
        active: 2,
        recovery: 4,
        hitbox: HitShape::new(4, 0, 10, 6),
        damage: 1,
        knockback: FxVec2::ZERO,
        hitstop: 3,
        shake: Shake::NONE,
        recoil: FxVec2::ZERO,
        cancel_from: Some(4),
        chain: Some(Test::Light),
        mobile: true,
        rehit: 0,
        frames: &[],
    };
    const HEAVY: MoveData<Test> =
        MoveData { cancel_from: None, chain: None, mobile: false, ..LIGHT };
    const FLURRY: MoveData<Test> = MoveData { active: 10, rehit: 3, ..LIGHT };

    impl MoveSet for Test {
        fn data(self) -> &'static MoveData<Test> {
            match self {
                Test::Light => &LIGHT,
                Test::Heavy => &HEAVY,
                Test::Flurry => &FLURRY,
            }
        }
    }

    const OWNER: Aabb = Aabb::new(100, 50, 14, 30);

    #[test]
    fn shapes_mirror_with_facing() {
        let shape = HitShape::new(4, 2, 10, 6);
        assert_eq!(shape.place(OWNER, 1), Aabb::new(111, 52, 10, 6));
        assert_eq!(shape.place(OWNER, -1), Aabb::new(93, 52, 10, 6));
    }

    #[test]
    fn hitbox_is_live_only_while_active_and_unlanded() {
        let mut a = Attack::new(Test::Light);
        let live: Vec<bool> = (0..8)
            .map(|_| {
                let live = a.hitbox(OWNER, 1).is_some();
                a.advance();
                live
            })
            .collect();
        assert_eq!(live, [false, false, true, true, false, false, false, false]);
        let mut a = Attack::new(Test::Light);
        a.advance();
        a.advance();
        a.land();
        assert!(a.hitbox(OWNER, 1).is_none());
    }

    #[test]
    fn multi_hit_moves_connect_again_after_rehit() {
        let mut a = Attack::new(Test::Flurry);
        let mut hits = Vec::new();
        loop {
            if a.hitbox(OWNER, 1).is_some() {
                hits.push(a.tick);
                a.land();
            }
            if !a.advance() {
                break;
            }
        }
        assert_eq!(hits, [2, 5, 8, 11], "active 2..12, again every 3 ticks");
    }

    #[test]
    fn lights_cancel_and_chain_heavies_commit() {
        let mut light = Attack::new(Test::Light);
        let mut heavy = Attack::new(Test::Heavy);
        assert!(light.chain().is_none(), "not before the window");
        for _ in 0..4 {
            light.advance();
            heavy.advance();
        }
        assert_eq!(light.chain(), Some(Test::Light));
        assert!(light.movement_cancellable());
        assert!(heavy.chain().is_none() && !heavy.movement_cancellable());
    }

    #[test]
    fn moves_finish_after_their_total() {
        let mut a = Attack::new(Test::Light);
        let ticks = std::iter::from_fn(|| a.advance().then_some(())).count();
        assert_eq!(ticks + 1, LIGHT.total() as usize);
    }
}
