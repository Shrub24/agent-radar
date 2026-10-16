//! Explicit DTOs whose shapes are part of Radar's public wire contract.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Status of the daemon's volatile task-publication lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TaskLaneStatus {
    pub available: bool,
    #[serde(default)]
    pub error: Option<String>,
}

/// Managed owner/parent metadata accepted with a spawn intent. Domain validation
/// and parent-binding checks remain in the daemon handler.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedLaunchMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_subject_id: Option<String>,
}

/// A non-secret Pi-session breadcrumb issued by Radar. This is correlation
/// evidence only and cannot establish lifecycle authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchIdentityMarker {
    pub version: u32,
    pub subject_id: String,
    pub launch_id: String,
    #[schemars(range(min = 1u32, max = 4294967295u32))]
    pub execution_generation: u32,
}

/// `daemon.ping` result. Contract names remain strings so older clients can
/// preserve and ignore names introduced by newer daemon builds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PingResult {
    pub protocol: u32,
    pub revision: u32,
    pub backend: String,
    #[serde(default)]
    pub instance: Option<String>,
    pub contracts: Vec<String>,
    pub tasks: TaskLaneStatus,
}

#[cfg(test)]
mod tests {
    use super::PingResult;

    #[test]
    fn ping_defaults_are_input_only_and_serialized_fields_are_present() {
        let input = serde_json::json!({
            "protocol": 2,
            "revision": 1,
            "backend": "tmux",
            "contracts": [],
            "tasks": { "available": false }
        });
        let parsed: PingResult = serde_json::from_value(input).expect("deserialize ping");
        assert_eq!(parsed.instance, None);
        assert_eq!(parsed.tasks.error, None);
        assert_eq!(
            serde_json::to_value(parsed).expect("serialize ping"),
            serde_json::json!({
                "protocol": 2,
                "revision": 1,
                "backend": "tmux",
                "instance": null,
                "contracts": [],
                "tasks": { "available": false, "error": null }
            })
        );
    }
}
