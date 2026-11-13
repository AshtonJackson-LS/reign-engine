use reign_engine::prelude::*;

/// MyData: say what it holds here. Public fields show in the Inspector.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Component)]
#[serde(default)]
pub struct MyData {
    pub value: i32,
}
