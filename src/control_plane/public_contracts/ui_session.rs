//! Public DTOs of the shared UI session (`ui.session.v1`).
//!
//! Every frontend attaches to one daemon-hosted session. These shapes are the
//! wire form of its state and of the actions it accepts: row identity, the
//! projected tree, the session fields, typed actions and their outcomes. They
//! are explicit DTOs rather than the internal tree and navigation types, so the
//! internal types can change without changing the published contract.

use crate::content::Content;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Who is attached. A presenter is the modal process a detached popup runs; it
/// may claim exactly one interaction and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentRole {
    Dashboard,
    Sidebar,
    Presenter,
}

/// Stable identity of one tree row. Mirrors the internal row identity one for
/// one; a row is addressed by this, never by its position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiRowId {
    Workspace {
        id: String,
    },
    Agent {
        pane_id: String,
    },
    Registered {
        agent_id: String,
    },
    Pane {
        pane_id: String,
    },
    Task {
        owner: String,
        session: Option<String>,
        id: String,
    },
    Lifecycle {
        subject_id: String,
    },
}

/// Semantic state of an agent row. `Other` carries a state the daemon reports
/// that this contract does not name, so a new state never fails decoding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentStateDto {
    Idle,
    Working,
    Waiting,
    Blocked,
    Settling,
    Done,
    Unknown,
    Lost,
    Other { name: String },
}

/// How siblings are ordered. Ordering never moves a row out of its parent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RowOrderDto {
    #[default]
    Source,
    State,
    Name,
}

/// Which ordinary panes are listed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PaneViewDto {
    Hidden,
    #[default]
    Running,
    All,
}

/// One row of the projected tree, as drawn: what it is, where it sits and what
/// its branch looks like. Details beyond this stay with the frontend, which
/// reads them from observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeRow {
    pub id: UiRowId,
    pub depth: u32,
    pub label: String,
    pub state: Option<AgentStateDto>,
    pub has_children: bool,
    pub collapsed: bool,
    pub needs_attention: bool,
    pub working: bool,
}

/// The shared filter. `editor` names the attachment that owns the query while
/// it is being typed; it is `None` when nobody is editing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FilterState {
    pub query: String,
    pub editor: Option<String>,
}

/// The lightweight shared fields: selection, order, visibility and the filter.
/// Terminal geometry, details pages and process readings are not here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionState {
    pub selected: Option<UiRowId>,
    pub order: RowOrderDto,
    pub pane_view: PaneViewDto,
    pub show_finished: bool,
    pub tasks: TaskRowsDto,
    /// Folded branches, sorted.
    pub collapsed: Vec<UiRowId>,
    pub filter: FilterState,
    /// Advances every time the interaction gate opens or closes. Input is
    /// stamped with the generation it was interpreted under.
    pub gate: u64,
    /// The one shared interaction that currently blocks the session, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocking: Option<Blocking>,
    pub operation: Option<CommandOutcome>,
    pub interaction_notice: Option<String>,
}

/// A shared interaction in progress, as every view sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Blocking {
    pub interaction_id: String,
    pub command: SharedCommand,
    pub stage: InteractionStage,
    pub owner: String,
    pub presenter: Option<String>,
    pub choices: Vec<String>,
    pub note: Option<String>,
    /// What the presenter asks. A project picker lists choices; a confirmation
    /// shows the question and what answering it loses.
    pub kind: InteractionKind,
    pub prompt: Option<String>,
    pub losses: Vec<String>,
}

/// Retained command outcome, correlated by the frozen domain request id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommandOutcome {
    pub request_id: String,
    pub status: CommandStatus,
    pub answer: Option<serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CommandStatus {
    DispatcherAccepted,
    DispatchRefused,
    SubmissionUncertain,
    Admitted,
    Completed,
    Refused,
    Unknown,
}

impl CommandStatus {
    /// The word this status is drawn and logged with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DispatcherAccepted => "dispatcher accepted",
            Self::DispatchRefused => "dispatch refused",
            Self::SubmissionUncertain => "submission uncertain",
            Self::Admitted => "admitted",
            Self::Completed => "completed",
            Self::Refused => "refused",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStage {
    /// The invoking frontend is gathering what the interaction offers.
    Preparing,
    /// The choices are fixed and a presenter is to answer.
    AwaitingResult,
}

/// How a presenter asks: choose a project, or agree to a frozen effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Project,
    Confirm,
}

/// Whether background tasks are listed, remembered separately for each pane view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskRowsDto {
    pub agents: bool,
    pub running: bool,
    pub all: bool,
}

/// Commands the palette runs on the tree. Terminal-local presentation commands
/// (details, focus swap, notices, sidebar placement) are not here: they never
/// reach the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharedCommand {
    Focus,
    NewWorkspace,
    LaunchConfigured,
    Restart,
    Delete,
    CreateContainer,
    ReloadContainer,
}

/// Something a shared interaction asks for, and the typed answer that resolves
/// it. A project choice is the one interaction Slice C introduces first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionResult {
    Project { path: String },
    Confirm { accepted: bool },
}

/// What a frontend supplies for a configured launch: its own environment, which
/// the daemon does not share. Everything else about the launch is derived from
/// the shared inventory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchSpec {
    pub command: Vec<String>,
    pub shell: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// One action a frontend submits. Each variant is a semantic request, not a key
/// or a mutation: the session decides its effect and publishes the outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiAction {
    SelectRow {
        row: UiRowId,
    },
    Move {
        delta: i32,
    },
    Collapse {
        row: Option<UiRowId>,
    },
    Expand {
        row: Option<UiRowId>,
    },
    ToggleFold {
        row: Option<UiRowId>,
    },
    /// Folds or unfolds one named branch without moving the selection.
    ToggleBranch {
        row: UiRowId,
    },
    CyclePaneView,
    CycleOrder,
    ToggleFinished,
    ToggleTasks,
    ShowEverything,
    NextAttention {
        forward: bool,
    },
    NextWorking {
        forward: bool,
    },
    BeginFilterEdit,
    SetFilterQuery {
        query: String,
    },
    EndFilterEdit,
    ClearFilter,
    RunCommand {
        command: SharedCommand,
        row: Option<UiRowId>,
    },
    /// A command that needs what only the invoking frontend knows. The session
    /// derives its target from the shared inventory and dispatches it.
    PrepareLaunch {
        spec: LaunchSpec,
    },
    ResolveInteraction {
        interaction_id: String,
        result: InteractionResult,
    },
    /// The invoking frontend's gathered choices, which fix what a result may be.
    ProvideChoices {
        interaction_id: String,
        choices: Vec<String>,
        #[serde(default)]
        note: Option<String>,
    },
    CancelInteraction {
        interaction_id: String,
    },
    ClaimPresentation {
        interaction_id: String,
    },
    PresentationFailed {
        interaction_id: String,
        message: String,
    },
}

impl UiAction {
    /// Main-tree input must identify the gate under which it was interpreted.
    pub fn is_ordinary(&self) -> bool {
        !matches!(
            self,
            Self::ResolveInteraction { .. }
                | Self::CancelInteraction { .. }
                | Self::ProvideChoices { .. }
                | Self::ClaimPresentation { .. }
                | Self::PresentationFailed { .. }
        )
    }
}

/// Why the session refused an action that it could otherwise have accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefusalCode {
    /// The row named by the action is not visible or no longer exists.
    UnknownRow,
    /// The action needs the filter editor, and another attachment owns it.
    NotEditor,
    /// The interaction named by the action is not open.
    NoInteraction,
    /// A shared interaction is open and this action cannot run during it.
    Blocked,
    /// The command has no target in the current selection.
    NoTarget,
    /// The action id was already used for a different action.
    Conflict,
    /// This attachment's role may not submit that action.
    Role,
    /// The session cannot serve this action yet.
    Unavailable,
    /// The input was interpreted under an earlier interaction gate.
    StaleGate,
    /// The result is not one of the choices the interaction offered.
    BadResult,
}

/// The outcome of one action. A retry of an identical action returns the
/// recorded outcome; a conflicting reuse of its id is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionOutcome {
    Applied {
        update_id: u64,
        /// The interaction this action opened, when it opened one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interaction_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    Refused {
        code: RefusalCode,
        message: String,
    },
    Replayed {
        update_id: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interaction_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
    },
    /// The recorded receipt for this id has been evicted. The frontend reads
    /// the current state rather than resending the action.
    ReceiptExpired,
    /// The action id is ahead of the next expected one. `expected` is the id
    /// the session will accept next.
    Gap {
        expected: u64,
    },
    Busy,
    Detached,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachResult {
    pub attachment_id: String,
    pub update_id: u64,
}

// Binary TUI publication/content protocol. Domain RPCs retain their JSON DTOs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Head {
    pub update_id: u64,
    pub observation: String,
    pub tree: String,
    pub session: String,
    pub gate: u64,
    pub diagnostic: Option<PublicationFailure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum PublicationFailure {
    #[error("content_too_large · showing last coherent view")]
    ContentTooLarge,
    #[error("{message}")]
    Encoding { message: String },
}

impl PublicationFailure {
    pub fn description(&self) -> &str {
        match self {
            Self::ContentTooLarge => "content_too_large · showing last coherent view",
            Self::Encoding { message } => message,
        }
    }
}

impl From<crate::content::Error> for PublicationFailure {
    fn from(error: crate::content::Error) -> Self {
        match error {
            crate::content::Error::TooLarge => Self::ContentTooLarge,
            other => Self::Encoding {
                message: other.to_string(),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    #[serde(rename = "metadata")]
    pub head: Head,
    #[serde(deserialize_with = "three_payloads")]
    pub content: Vec<Content>,
}

fn three_payloads<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Vec<Content>, D::Error> {
    struct Payloads;
    impl<'de> serde::de::Visitor<'de> for Payloads {
        type Value = Vec<Content>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("at most three lane payloads")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut input: A,
        ) -> Result<Self::Value, A::Error> {
            if input.size_hint().is_some_and(|size| size > 3) {
                return Err(serde::de::Error::custom("too many lane payloads"));
            }
            let mut payloads = Vec::with_capacity(3);
            while let Some(payload) = input.next_element()? {
                if payloads.len() == 3 {
                    return Err(serde::de::Error::custom("too many lane payloads"));
                }
                payloads.push(payload);
            }
            Ok(payloads)
        }
    }
    decoder.deserialize_seq(Payloads)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case")]
pub enum Request {
    Attach {
        attachment_id: String,
        role: AttachmentRole,
        interaction_id: Option<String>,
    },
    Action {
        attachment_id: String,
        action_id: u64,
        gate: Option<u64>,
        action: UiAction,
    },
    Content {
        hash: String,
    },
    Detach {
        attachment_id: String,
    },
    Demand {
        attachment_id: String,
        processes: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Reply {
    Attached,
    Action { outcome: ActionOutcome },
    Content { content: Content },
    Expired { head: Head },
    Demand,
    Refused { code: String, message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum Open {
    Actions,
    Subscribe {
        attachment_id: String,
        session_only: bool,
    },
}
