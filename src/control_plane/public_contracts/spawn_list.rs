use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::control_plane::registry::PublicSpawnTopology;

/// Input accepted by `spawn.list`.
///
/// A caller presents itself; the parent whose launches it reads is derived from
/// the claim it holds. There is deliberately no parent parameter: naming another
/// subject would let any client read its launches.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnListParams {
    #[serde(default)]
    pub registration_id: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
}

/// One page of spawn edges. `next` names the request id the page ended on.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnListPage {
    pub spawns: Vec<PublicSpawnTopology>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
