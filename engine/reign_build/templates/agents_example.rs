use reign_engine::prelude::*;

/// Spins a coin and gives a point when the player touches it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct Coin {
    /// Points it's worth. Public fields show in the Inspector and scene files.
    pub value: i32,
}

impl Default for Coin {
    fn default() -> Self {
        Coin { value: 1 }
    }
}

impl Behaviour<World> for Coin {
    fn start(_ctx: &mut Ctx, _me: ObjectId) {}

    /// 120 times a second.
    fn fixed_update(_ctx: &mut Ctx, _me: ObjectId) {}

    fn on_message(ctx: &mut Ctx, me: ObjectId, msg: &Envelope<Message>) {
        if let Message::TriggerEnter(other) = msg.body {
            let value = ctx.get::<Coin>(me).map_or(0, |c| c.value);
            ctx.send(other, Message::with("Score", value as i64));
            ctx.commands.despawn(me);
        }
    }
}
