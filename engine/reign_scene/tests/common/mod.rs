//! Test components shared by the integration tests.

use reign_core::{Fx, FxVec2};
use reign_scene::{Component, ObjectId, Reflect, Rgba8};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub enum Mood {
    #[default]
    Calm,
    Angry,
    Sleepy,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct Inner {
    pub depth: u8,
    pub label: String,
}

/// Every kind of field the inspector handles.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[component(name = "Everything", bounds(-8, -16, 16, 16))]
pub struct Everything {
    pub small: i8,
    #[reflect(range(0, 100), tooltip = "Hit points.")]
    pub health: u16,
    pub big: i64,
    /// Whether it's lit.
    pub lit: bool,
    pub speed: Fx,
    pub offset: FxVec2,
    pub tint: Rgba8,
    pub name: String,
    pub mood: Mood,
    pub target: Option<ObjectId>,
    pub cooldown: Option<Fx>,
    pub path: Vec<FxVec2>,
    pub slots: [u8; 3],
    pub inner: Inner,
    #[reflect(read_only)]
    pub ticks: u32,
    #[reflect(skip)]
    pub cache: u64,
}

impl Everything {
    /// A value with every field away from its default.
    pub fn sample() -> Self {
        Everything {
            small: -7,
            health: 42,
            big: -1 << 40,
            lit: true,
            speed: Fx::from_raw(-3),
            offset: FxVec2::from_px(1, -2),
            tint: Rgba8::rgba(1, 2, 3, 4),
            name: "lantern \"post\"".into(),
            mood: Mood::Sleepy,
            target: Some(ObjectId::new(4, 1)),
            cooldown: Some(Fx::from_raw(129)),
            path: vec![FxVec2::from_px(3, 4), FxVec2 { x: Fx::from_raw(-1), y: Fx::ONE }],
            slots: [1, 2, 255],
            inner: Inner { depth: 9, label: "deep".into() },
            ticks: 77,
            cache: 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
pub struct Marker {
    pub tag: u8,
}

#[allow(dead_code)]
pub fn registry() -> reign_scene::ComponentRegistry {
    let mut registry = reign_scene::ComponentRegistry::new();
    registry.register::<Everything>().register::<Marker>();
    registry
}
