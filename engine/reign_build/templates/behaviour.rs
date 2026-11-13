use reign_engine::prelude::*;

/// MyBehaviour: say what it does here. Public fields show in the Inspector, and
/// everything it remembers lives in them, so Play/Stop and rollback just work.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct MyBehaviour {
    /// World pixels per second.
    pub speed: Fx,
}

impl Default for MyBehaviour {
    fn default() -> Self {
        MyBehaviour { speed: Fx::from_px(120) }
    }
}

impl Behaviour<World> for MyBehaviour {
    /// Once, before the object's first update.
    fn start(_ctx: &mut Ctx, _me: ObjectId) {}

    /// Every fixed tick (120 per second) while playing.
    fn fixed_update(ctx: &mut Ctx, me: ObjectId) {
        let Some(speed) = ctx.get::<MyBehaviour>(me).map(|c| c.speed) else { return };
        // Arrow keys or WASD move it.
        let step = speed / TICKS_PER_SECOND;
        ctx.translate(me, FxVec2::new(step * ctx.axis_x(), step * ctx.axis_y()));
    }

    /// Physics events, and messages from other Behaviours. Collisions need a
    /// BoxCollider here and a Rigidbody on one of the two objects; tick
    /// "is_trigger" on a collider for overlap events instead of a solid hit.
    fn on_message(_ctx: &mut Ctx, _me: ObjectId, msg: &Envelope<Message>) {
        match msg.body {
            // Bumped into something solid.
            Message::CollisionEnter(_other) => {}
            // Overlapping a trigger (or a trigger overlapping this). To pick
            // it up, for example: `_ctx.commands.despawn(_other);`
            Message::TriggerEnter(_other) => {}
            _ => {}
        }
    }
}
