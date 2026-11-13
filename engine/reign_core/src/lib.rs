//! Deterministic simulation primitives for the Reign engine.
//!
//! Nothing in this crate touches the OS, the GPU or floating point state that
//! feeds back into the simulation. Same inputs in, same frames out, on every
//! platform. That rule is what keeps rollback netcode and replays possible.

pub mod animation;
pub mod checksum;
pub mod clock;
pub mod decimal;
pub mod fixed;
pub mod input;
pub mod rng;
#[cfg(feature = "serde")]
mod serde_impls;

pub use checksum::checksum;
pub use clock::FixedClock;
pub use decimal::ParseFxError;
pub use fixed::{Fx, FxVec2};
pub use input::{Button, InputBuffer, InputFrame};
pub use rng::Rng;

/// Simulation ticks per second. Rendering runs at the display rate and
/// interpolates between ticks.
pub const TICK_HZ: u32 = 120;
