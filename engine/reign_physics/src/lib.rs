//! Tile collision and pixel-stepped movement for platformer bodies.
//!
//! Bodies sit on whole pixels and carry a subpixel remainder. Each move steps
//! one pixel at a time and stops at the first collision, so nothing tunnels
//! through thin geometry and results are exact on every machine.

mod aabb;
mod body;
mod platform;
mod tilemap;

pub use aabb::Aabb;
pub use body::Body;
pub use platform::MovingPlatform;
pub use tilemap::{Surface, TILE, Tile, TileMap};
