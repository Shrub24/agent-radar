//! Explicit DTOs whose shapes are part of Radar's public wire contract.

pub mod observation;

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

/// The current managed Pi execution that published or received a handoff.
/// `subject_id` is stable across verified successors; `execution_id` and
/// generation identify the exact execution that took the action.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffIdentity {
    pub agent_id: String,
    pub session_id: String,
    pub subject_id: String,
    pub execution_id: String,
    #[schemars(range(min = 1u32, max = 4294967295u32))]
    pub execution_generation: u32,
}

/// Inputs for publishing an immutable handoff. The wire request ID carries the
/// stable communication identity; the daemon derives the sender execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffPublishParams {
    pub sender_agent_id: String,
    pub recipient_subject_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub summary: String,
    pub content: String,
}

/// A bounded inbox page by stable handoff ID. Payloads are fetched one at a
/// time with `communication.get`, keeping each response within the wire bound.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffInbox {
    pub handoff_ids: Vec<String>,
}

/// Immutable handoff content. The request id is the stable communication id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    pub version: u32,
    pub id: String,
    pub sender: HandoffIdentity,
    pub recipient_subject_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub summary: String,
    pub content: String,
    #[schemars(range(min = 0u64, max = 9007199254740991u64))]
    pub published_at_unix_ms: u64,
}

/// Durable recipient-consumer receipt, stored separately from immutable content.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HandoffReceipt {
    pub version: u32,
    pub handoff_id: String,
    pub recipient: HandoffIdentity,
    #[schemars(range(min = 0u64, max = 9007199254740991u64))]
    pub received_at_unix_ms: u64,
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
