//! Explicit DTOs whose shapes are part of Radar's public wire contract.

pub mod observation;
pub mod spawn_list;
pub mod subject;
pub mod ui_session;

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

/// The managed Pi execution that produced or received a message. `subject_id` is
/// the address that survives replacement; the rest is provenance for the
/// execution that acted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentIdentity {
    pub registration_id: String,
    pub session_id: String,
    pub subject_id: String,
    pub claim_id: String,
}

/// Inputs for publishing a message. The wire request ID carries the stable
/// message identity; the daemon derives the producer execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessagePublishParams {
    pub sender_registration_id: String,
    pub recipient_subject_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub summary: String,
    pub content: String,
}

/// One message body. Closed and tagged: a kind is added with its producer,
/// routing policy and consumer, never as an open notification payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PublishBody {
    /// A managed worker's result for its recorded parent.
    Handoff {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        correlation_id: Option<String>,
        summary: String,
        content: String,
    },
}

/// Immutable producer-addressed content. The request id is the message id, and
/// the recipient set is frozen at acceptance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub version: u32,
    pub id: String,
    pub producer: AgentIdentity,
    pub recipients: Vec<String>,
    pub body: PublishBody,
    #[schemars(range(min = 0u64, max = 9007199254740991u64))]
    pub created_at_unix_ms: u64,
}

/// One recipient's durable capture. Stored separately from content, and it
/// means only that: not delivery, not understanding, not settlement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageReceipt {
    pub version: u32,
    pub message_id: String,
    pub recipient: AgentIdentity,
    #[schemars(range(min = 0u64, max = 9007199254740991u64))]
    pub received_at_unix_ms: u64,
}

/// A bounded page of pending message ids, with the acceptance position to
/// continue from. Payloads are fetched one at a time with `communication.get`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MessagePage {
    pub message_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// One recipient's mailbox state. `revision` is opaque and comparable only
/// within one daemon instance; `pending` is what still needs capture.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Attention {
    pub revision: String,
    #[schemars(range(min = 0u64, max = 9007199254740991u64))]
    pub pending: u64,
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
