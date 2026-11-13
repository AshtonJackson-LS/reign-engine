//! Reign, a small 2D engine built for deterministic, rollback-friendly
//! action games.
//!
//! Games depend on this crate alone and reach the engine through its
//! modules, so the crates behind them can be split, merged or renamed
//! without touching game code:
//!
//! | Module | Crate | What it holds |
//! | --- | --- | --- |
//! | [`core`] | `reign_core` | Fixed-point math, the 120 Hz clock, input, RNG. Deterministic. |
//! | [`physics`] | `reign_physics` | Collision tiles, bodies and pixel-stepped movement. |
//! | [`combat`] | `reign_combat` | Frame data, hitboxes, hitstop and screen shake. |
//! | [`render`] | `reign_render` | The wgpu renderer and its post-effect chain. |
//! | [`audio`] | `reign_audio` | PCM playback, spatial mixing and buses. |
//! | [`net`] | `reign_net` | Rollback netcode over the deterministic simulation. |
//! | [`assets`] | `reign_assets` | Asset decoding (PNG). |
//! | [`scene`] | `reign_scene` | Game objects, components, reflection and scene files. |
//! | [`behaviour`] | `reign_behaviour` | Behaviours: per-object scripts run in fixed stages, with messages. |
//! | [`app`] | `reign_app` | Window, event loop, fixed-tick loop and gamepads. Re-exports `winit` and `gilrs`. |
//! | `editor` | `reign_editor` | The editor shell (egui), with the `editor` feature. Re-exports `egui`. |

pub use reign_app as app;
pub use reign_assets as assets;
pub use reign_audio as audio;
pub use reign_behaviour as behaviour;
pub use reign_combat as combat;
pub use reign_core as core;
#[cfg(feature = "editor")]
pub use reign_editor as editor;
pub use reign_net as net;
pub use reign_physics as physics;
pub use reign_render as render;
pub use reign_scene as scene;
