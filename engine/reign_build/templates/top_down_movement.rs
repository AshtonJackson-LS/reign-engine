use reign_engine::prelude::*;

/// Top-down movement in eight directions with WASD or the arrow keys, as in a
/// Zelda-like or a twin-stick game.
///
/// With a **Rigidbody** on the object it sets the body's velocity, so walls
/// with a **BoxCollider** stop it; set the Rigidbody's Gravity Scale to 0 so it
/// doesn't fall. Without a Rigidbody it moves the object directly and passes
/// through everything.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct TopDownMovement {
    /// Top speed, in pixels per second.
    pub speed: Fx,
    /// How fast it speeds up and slows down, in pixels per second per second.
    pub acceleration: Fx,
    /// The current velocity when there's no Rigidbody. State, not a setting.
    #[reflect(read_only)]
    pub velocity: FxVec2,
}

impl Default for TopDownMovement {
    fn default() -> Self {
        TopDownMovement {
            speed: Fx::from_px(100),
            acceleration: Fx::from_px(900),
            velocity: FxVec2::ZERO,
        }
    }
}

impl Behaviour<World> for TopDownMovement {
    fn fixed_update(ctx: &mut Ctx, me: ObjectId) {
        let Some(mut m) = ctx.get::<TopDownMovement>(me).cloned() else { return };
        let (x, y) = (ctx.axis_x(), ctx.axis_y());

        // Diagonals at full speed on both axes would be about 41% faster, so
        // scale them by 181/256 (about 1/√2).
        let target_speed = if x != 0 && y != 0 { m.speed * 181 / 256 } else { m.speed };
        let target = FxVec2::new(target_speed * x, target_speed * y);

        let step = m.acceleration / TICKS_PER_SECOND;
        let body = ctx.get::<Rigidbody>(me).map(|b| b.velocity);
        let current = body.unwrap_or(m.velocity);
        let velocity = FxVec2::new(current.x.approach(target.x, step), current.y.approach(target.y, step));

        match ctx.get_mut::<Rigidbody>(me) {
            // Physics moves it, and stops it at walls.
            Some(body) => body.velocity = velocity,
            // No Rigidbody: move it ourselves.
            None => {
                m.velocity = velocity;
                ctx.translate(me, FxVec2::new(velocity.x / TICKS_PER_SECOND, velocity.y / TICKS_PER_SECOND));
                if let Some(state) = ctx.get_mut::<TopDownMovement>(me) {
                    *state = m;
                }
            }
        }
    }
}
