use reign_engine::prelude::*;

/// Side-on platformer movement: run with A/D or the arrow keys, jump with Space
/// (the "Jump" action in Scripts → Input Actions).
///
/// Put it on an object with a **Rigidbody** (Dynamic) and a **BoxCollider**
/// (GameObject → 2D Object → Physics → Dynamic Box has both). This script only
/// sets the body's velocity; physics does the falling and the colliding.
///
/// It has the touches that make jumping feel good:
/// - acceleration, so running starts and stops smoothly;
/// - coyote time: you can still jump just after running off a ledge;
/// - jump buffering: a jump pressed just before landing still happens;
/// - variable height: let go of Space early for a short hop.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct PlatformerMovement {
    /// Top running speed, in pixels per second.
    pub run_speed: Fx,
    /// How fast it speeds up and slows down, in pixels per second per second.
    pub acceleration: Fx,
    /// Upward speed a jump starts with, in pixels per second.
    pub jump_speed: Fx,
    /// Ticks (120 per second) after leaving the ground that a jump still works.
    pub coyote_ticks: u8,
    /// Ticks a jump pressed before landing is remembered.
    pub jump_buffer_ticks: u8,
    /// Ticks since it last stood on something. State, not a setting.
    #[reflect(read_only)]
    pub airborne_ticks: u8,
}

impl Default for PlatformerMovement {
    fn default() -> Self {
        PlatformerMovement {
            run_speed: Fx::from_px(140),
            acceleration: Fx::from_px(1200),
            jump_speed: Fx::from_px(300),
            coyote_ticks: 10,
            jump_buffer_ticks: 12,
            airborne_ticks: 0,
        }
    }
}

impl Behaviour<World> for PlatformerMovement {
    fn fixed_update(ctx: &mut Ctx, me: ObjectId) {
        let Some(mut m) = ctx.get::<PlatformerMovement>(me).cloned() else { return };
        let Some(body) = ctx.get::<Rigidbody>(me).cloned() else {
            return; // Needs a Rigidbody to move.
        };
        let mut velocity = body.velocity;

        // Run: ease the sideways speed toward where the keys point.
        let target = m.run_speed * ctx.axis_x();
        velocity.x = velocity.x.approach(target, m.acceleration / TICKS_PER_SECOND);

        // Count the time in the air, for coyote time.
        m.airborne_ticks = if body.grounded { 0 } else { m.airborne_ticks.saturating_add(1) };

        // Jump: on the ground (or only just off it), within a moment of the press.
        let can_jump = m.airborne_ticks <= m.coyote_ticks;
        if can_jump && ctx.action_buffered("Jump", m.jump_buffer_ticks as u16) {
            ctx.consume_action("Jump");
            velocity.y = -m.jump_speed; // World y grows downwards: negative is up.
            m.airborne_ticks = m.coyote_ticks.saturating_add(1); // No second jump mid-air.
        }

        // Letting go of Space while still rising cuts the jump short.
        let short_hop = -m.jump_speed / 3;
        if !ctx.action("Jump") && velocity.y < short_hop {
            velocity.y = short_hop;
        }

        if let Some(body) = ctx.get_mut::<Rigidbody>(me) {
            body.velocity = velocity;
        }
        if let Some(state) = ctx.get_mut::<PlatformerMovement>(me) {
            *state = m;
        }
    }
}
