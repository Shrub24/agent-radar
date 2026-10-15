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
