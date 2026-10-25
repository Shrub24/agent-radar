//! Agent-facing records: process identities remain inside the daemon.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::control_plane::lifecycle::ManagedMetadata;
use crate::control_plane::registry::RegistryLocation;
use crate::control_plane::registry::publication::{
    ExpectedWriter, PublicSnapshotFacts, PublicWriterFacts, PublicationUpdate, PublisherIdentity,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    #[default]
    Live,
    Retained,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentState {
    Starting,
    Live,
    Detached,
    Unknown,
}

impl Selection {
    pub fn includes(self, state: AttachmentState) -> bool {
        match self {
            Self::Live => state == AttachmentState::Live,
            Self::Retained => state == AttachmentState::Detached,
            Self::All => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChannelFacts {
    pub writer: PublicWriterFacts,
    pub snapshot: Option<PublicSnapshotFacts>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub subject_id: String,
    pub session_id: Option<String>,
    pub managed: ManagedMetadata,
    pub state: AttachmentState,
    pub location: Option<RegistryLocation>,
    /// Recorded scope ownership, not proof of current cgroup membership.
    pub scoped: Option<bool>,
    pub execution: Option<ChannelFacts>,
    pub assignment: Option<ChannelFacts>,
    pub facts: Option<ChannelFacts>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetParams {
    pub subject_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetResult {
    pub subject: Subject,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChildGetParams {
    pub registration_id: String,
    pub subject_id: String,
}

/// Closes a child's pane. The caller names the child by subject; Radar resolves
/// the launch, the pane and the process that occupies it.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChildCloseParams {
    pub registration_id: String,
    pub subject_id: String,
    pub intent: crate::control_plane::registry::CloseIntent,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    #[serde(default)]
    pub selection: Selection,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChildrenParams {
    pub registration_id: String,
    #[serde(default)]
    pub selection: Option<Selection>,
    #[serde(default)]
    pub after: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignmentAcquire {
    pub registration_id: String,
    pub subject_id: String,
    pub publisher: PublisherIdentity,
    #[serde(default)]
    pub replace: Option<ExpectedWriter>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignmentPublish {
    pub registration_id: String,
    pub subject_id: String,
    pub update: PublicationUpdate,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignmentRetire {
    pub registration_id: String,
    pub subject_id: String,
    pub writer_handle: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListResult {
    pub subjects: Vec<Subject>,
    pub next: Option<String>,
}
