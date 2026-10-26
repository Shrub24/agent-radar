//! Public DTOs of the shared UI session (`ui.session.v1`).
//!
//! Every frontend attaches to one daemon-hosted session. These shapes are the
//! wire form of its state and of the actions it accepts: row identity, the
//! projected tree, the session fields, typed actions and their outcomes. They
//! are explicit DTOs rather than the internal tree and navigation types, so the
//! internal types can change without changing the published contract.

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

/// The projected tree as content. `hash` identifies the rows; `rows` is omitted
/// when the caller already holds that hash, so an unchanged tree is not sent
/// again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TreeProjection {
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<Vec<TreeRow>>,
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
    ResolveInteraction {
        interaction_id: String,
        result: InteractionResult,
    },
    CancelInteraction {
        interaction_id: String,
    },
    ClaimPresentation {
        interaction_id: String,
    },
    AcknowledgePresentation {
        effect_id: String,
    },
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
}

/// The outcome of one action. A retry of an identical action returns the
/// recorded outcome; a conflicting reuse of its id is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionOutcome {
    Applied {
        update_id: u64,
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

/// Attach: a fresh attachment, identified by a UUID the client mints, and the
/// authoritative baseline it starts from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachParams {
    /// Minted by the client for each communication incarnation.
    pub attachment_id: String,
    pub role: AttachmentRole,
    /// The interaction a presenter claims at attach time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttachResult {
    pub attachment_id: String,
    pub publication: Publication,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetachParams {
    pub attachment_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetachResult {
    pub released: bool,
}

/// Subscribe: the attachment's one primary push stream. The first frame is the
/// current [`Publication`]; each later frame is a newer one. A frame carries
/// tree rows only when they differ from the last frame sent on the stream, so
/// `known_tree_hash` lets a client that already holds content skip it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubscribeParams {
    pub attachment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_tree_hash: Option<String>,
}

/// Retrieves retained tree content by hash. An evicted hash is refused with
/// `content_expired`; the caller then reads the current state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentGetParams {
    pub hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentGetResult {
    pub rows: Vec<TreeRow>,
}

/// Submits one action. `action_id` is the attachment's counter, starting at 1:
/// the session applies each id once, in order, and replays a retried one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ActionParams {
    pub attachment_id: String,
    pub action_id: u64,
    pub action: UiAction,
}

/// One published state: the ordered update id, the tree by content hash and the
/// lightweight session fields by content hash. `tree.rows` is present only when
/// the receiver does not already hold that hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub update_id: u64,
    pub tree: TreeProjection,
    pub state_hash: String,
    pub session: SessionState,
}
