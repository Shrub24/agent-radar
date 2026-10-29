//! What an effectful operation answers with: its request record, and the public
//! facts the operation itself names beside it.
//!
//! One type serves both ends of the socket. The daemon builds it and renders it
//! once, in [`respond`]; a Rust client reads it back instead of probing keys.

use serde::{Deserialize, Deserializer, Serialize};

use super::containers::PublicContainer;
use super::protocol::{Refusal, Response};
use super::registry::child_close::{CloseIntent, CloseOutcome};
use super::registry::spawn::{PublicSpawnEdge, PublicSpawnExitObservation};
use super::store::PublicRequestRecord;
use crate::model::FleetObservation;
use crate::model::ForegroundEvidence;
use crate::runtime::OutputRead;

/// The `result` of an operation response.
///
/// A fact is present only when the operation names it, and a creation or reload
/// names its container even when there is none, so `container` distinguishes an
/// absent fact from an answered `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Answer {
    pub request: PublicRequestRecord,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub container: Option<Option<PublicContainer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<Vec<PublicContainer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<PublicSpawnEdge>,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl Answer {
    /// The record's public view alone.
    pub fn of(record: impl Into<PublicRequestRecord>) -> Self {
        Self {
            request: record.into(),
            container: None,
            removed: None,
            spawn: None,
        }
    }

    /// Names the container the operation made or acted on, if there was one.
    pub fn with_container(mut self, container: Option<PublicContainer>) -> Self {
        self.container = Some(container);
        self
    }

    /// Names the containers a deletion marked removed.
    pub fn with_removed(mut self, removed: Vec<PublicContainer>) -> Self {
        self.removed = Some(removed);
        self
    }

    /// Names the spawn edge the request authored.
    pub fn with_spawn(mut self, spawn: PublicSpawnEdge) -> Self {
        self.spawn = Some(spawn);
        self
    }
}

/// What a confirmed `spawn.close` reports: the mux pane closure and the caller's
/// intent, with the exit the pane had already made when there was one. No close
/// claims a process or process-group exit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildClosed {
    pub close: PublicClose,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicClose {
    /// `Completed` or `AlreadyExited`; a refused or unknown close is a refusal.
    pub outcome: CloseOutcome,
    pub intent: CloseIntent,
    pub pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_observation: Option<PublicSpawnExitObservation>,
}

/// What `launch.admit` answers: the registration the launch now holds, whose
/// scope the client derives from it rather than receiving the naming rule twice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Admitted {
    pub registration_id: String,
}

/// What `launch.claim` answers a released launch: its registration and the
/// program from the daemon's own record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claimed {
    pub registration_id: String,
    pub executable: String,
    pub argv: Vec<String>,
}

/// `mux.observe`: the whole inventory the backend reports.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inventory {
    pub inventory: FleetObservation,
}

/// `mux.process_info`: what the backend can say about a pane's foreground.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub evidence: ForegroundEvidence,
}

/// `mux.output`: one bounded read of one pane.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Output {
    pub output: OutputRead,
}

/// `mux.request.list`: operation records, public views only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Requests {
    pub requests: Vec<PublicRequestRecord>,
}

/// What serving an operation reaches: its answer, or a refusal before one exists.
pub type Reply = Result<Answer, Refusal>;

/// The one place a reply becomes a response line.
pub fn respond<T: Serialize>(id: &str, reply: Result<T, Refusal>) -> Response {
    match reply {
        Ok(answer) => Response::ok(
            id,
            serde_json::to_value(answer).expect("an operation answer is JSON-serializable"),
        ),
        Err(refusal) => Response::refused(id, &refusal),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TasksPage {
    pub revision: u64,
    pub sessions: Vec<TaskSession>,
    pub next_after: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSession {
    pub session: String,
    pub connected: bool,
    pub last_updated_at: u64,
    pub tasks: Vec<crate::tasks::Task>,
}

#[derive(Serialize)]
pub(crate) struct Registration {
    pub registration: super::registry::PublicRegistration,
}
#[derive(Serialize)]
pub(crate) struct Writer {
    pub writer: super::registry::publication::WriterBinding,
}
#[derive(Serialize)]
pub(crate) struct Channel {
    pub channel: super::registry::publication::PublicChannel,
    /// This recipient's mailbox state. Absent when the caller has no managed
    /// subject, and `attention_unavailable` when it could not be read: an
    /// accepted publication is never undone by a hint failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention: Option<super::public_contracts::Attention>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attention_unavailable: Option<bool>,
}

impl Channel {
    /// A channel answer for a caller with no recipient mailbox.
    pub(crate) fn new(channel: super::registry::publication::PublicChannel) -> Self {
        Self {
            channel,
            attention: None,
            attention_unavailable: None,
        }
    }

    pub(crate) fn with_attention(
        self,
        attention: Option<super::public_contracts::Attention>,
        unavailable: Option<bool>,
    ) -> Self {
        Self {
            attention,
            attention_unavailable: unavailable,
            ..self
        }
    }
}
#[derive(Serialize)]
pub(crate) struct Process {
    pub process: super::registry_read::AgentEntry,
}
#[derive(Serialize)]
pub(crate) struct Spawn {
    pub spawn: super::registry::PublicSpawnTopology,
}
#[derive(Serialize)]
pub(crate) struct Container {
    pub container: PublicContainer,
}
#[derive(Serialize)]
pub(crate) struct Containers {
    pub containers: Vec<PublicContainer>,
    pub has_more: bool,
    pub next_after: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct Subject {
    pub subject: super::lifecycle::PublicClaim,
    pub source: ObservationSource,
}
#[derive(Serialize)]
pub(crate) struct Subjects {
    pub subjects: Vec<super::lifecycle::PublicClaim>,
    pub has_more: bool,
    pub next_after: Option<String>,
    pub source: ObservationSource,
}
#[derive(Serialize)]
pub(crate) struct Reconciled<T> {
    pub reconciled: T,
}
#[derive(Serialize)]
pub(crate) struct ObservationSource {
    pub observation: ObservationProgress,
}
#[derive(Serialize)]
pub(crate) struct ObservationProgress {
    pub interval_ms: u64,
    pub last_pass_at: Option<String>,
}
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum Management {
    Managed { subject_id: String },
    Unmanaged,
    Duplicate,
}
#[derive(Serialize)]
pub(crate) struct Context {
    pub writer: super::registry::publication::WriterBinding,
    pub context: super::registry::PublicContext,
    pub warning: Option<String>,
    pub management: Management,
}
#[derive(Serialize)]
pub(crate) struct Inspected {
    pub subject: super::public_contracts::subject::Subject,
    pub output: OutputRead,
    pub foreground: Option<ForegroundEvidence>,
}
#[derive(Serialize)]
pub(crate) struct PublishedMessage {
    pub message: super::public_contracts::Message,
    pub created: bool,
}
#[derive(Serialize)]
pub(crate) struct ReadMessage {
    pub message: super::public_contracts::Message,
    pub receipt: Option<super::public_contracts::MessageReceipt>,
}
#[derive(Serialize)]
pub(crate) struct AcknowledgedMessage {
    pub receipt: super::public_contracts::MessageReceipt,
    pub created: bool,
}
#[derive(Serialize)]
pub(crate) struct Cursor {
    pub epoch: String,
    pub sequence: u64,
}
#[derive(Serialize)]
pub(crate) struct CursorExpired {
    pub epoch: String,
    pub cursor: Cursor,
    pub earliest: u64,
}
#[derive(Serialize)]
pub(crate) struct ExpiredEvents {
    pub cursor_expired: CursorExpired,
}
#[derive(Serialize)]
pub(crate) struct Events {
    pub epoch: String,
    pub events: Vec<super::store::Event>,
    pub cursor: Cursor,
    pub tail: u64,
    pub more: bool,
    pub earliest: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskSnapshotData {
    pub revision: u64,
    pub sessions: Vec<TaskSession>,
}
