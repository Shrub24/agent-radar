//! The runtime seam: what Radar asks of the terminal multiplexer it reads and
//! acts on.
//!
//! Both workers reach a runtime through this one small interface, in Radar's
//! own normalized values. Collection reads a [`FleetObservation`] and
//! [`ForegroundEvidence`]; focus is asked to move to a [`Target`]. Nothing on
//! either path names a runtime's executable, CLI grammar, JSON shapes or
//! socket; those live in the adapter ([`crate::herdr`]), the only module that
//! sees them. A second runtime is a second adapter at assembly time, not a
//! change to the collector, the focuser, the reconciler or the view.
//!
//! Properties every implementation owes its caller:
//!
//! - **Cancellation is prompt.** `cancel` is set when Radar is shutting down.
//!   An implementation must abandon an outstanding command and return without
//!   waiting out its timeout, and must leave no child process behind.
//! - **Unreadable evidence is not disappearance.** A pane whose foreground
//!   query fails, times out or cannot be decoded is
//!   [`ForegroundEvidence::Inconclusive`], never a failed refresh and never
//!   proof that the pane went away.
//! - **An operation states its certainty.** An effect reports
//!   [`Completed`](CreateOutcome::Completed), [`Refused`](CreateOutcome::Refused)
//!   or [`Unknown`](CreateOutcome::Unknown): whether the runtime confirmed the
//!   effect, positively rejected it before applying it, or left the outcome
//!   unestablished. A caller that might have acted is never told it certainly
//!   did not.
//! - **Capabilities are declared, not assumed.** [`Self::capabilities`] lists
//!   what this backend implements, and an operation outside that list is refused
//!   as unsupported rather than attempted.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::model::{FleetObservation, ForegroundEvidence};

/// The certainty of a focus attempt at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FocusOutcome {
    /// The backend positively confirmed the requested focus.
    Completed,
    /// The backend positively rejected the request before applying it.
    Refused(String),
    /// Dispatch or completion may have happened, but no trustworthy answer arrived.
    Unknown(String),
}

impl FocusOutcome {
    /// Existing direct-runtime surfaces display a concise human diagnostic.
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Completed => None,
            Self::Refused(message) | Self::Unknown(message) => Some(message),
        }
    }
}

/// Where `Enter` wants a runtime's focus moved: an existing normalized
/// location the view already produces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A pane row: its workspace, tab and pane in one request.
    Pane(String),
    /// A workspace row.
    Workspace(String),
}

/// Where a lifecycle action asks a runtime to close: an existing normalized
/// location. A runtime that cannot close refuses explicitly through [`Err`];
/// the operator flow is the same either way.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseTarget {
    Pane(String),
    Tab(String),
}

impl CloseTarget {
    /// The normalized location id this target names: the record's target key,
    /// shared with every other operation over the same location.
    pub fn id(&self) -> &str {
        match self {
            Self::Pane(id) | Self::Tab(id) => id,
        }
    }

    /// What the target is, for a one-line message: `pane wA:p1`.
    pub fn description(&self) -> String {
        match self {
            Self::Pane(pane_id) => format!("pane {pane_id}"),
            Self::Tab(tab_id) => format!("tab {tab_id}"),
        }
    }
}

/// The certainty of a close attempt at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The backend positively confirmed it performed the close.
    Completed,
    /// The backend positively refused the request before applying it.
    Refused(String),
    /// Dispatch or completion may have happened, but no trustworthy answer arrived.
    Unknown(String),
}

impl CloseOutcome {
    /// The existing direct-close surfaces display a one-line diagnostic.
    pub fn diagnostic(&self) -> Option<&str> {
        match self {
            Self::Completed => None,
            Self::Refused(message) | Self::Unknown(message) => Some(message),
        }
    }
}

/// Which way a pane split puts the new pane relative to the one it splits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Right,
    Down,
}

/// What a caller asks a runtime to create.
///
/// Creation is a mux primitive and says nothing about what will run in the new
/// location: no command is derived, no session is restored and no agent is
/// started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CreateRequest {
    /// A new workspace.
    Workspace { focus: bool },
    /// A new tab in `workspace_id`. The workspace is named by the caller rather
    /// than taken from whichever pane the multiplexer currently has focused.
    Tab { workspace_id: String, focus: bool },
    /// A new pane beside `pane_id`.
    PaneSplit {
        pane_id: String,
        direction: SplitDirection,
        focus: bool,
    },
}

/// The identities a runtime reported for one creation, as the runtime named
/// them.
///
/// A field is absent when the answer did not name it; nothing here is derived
/// from the request, so an identity a runtime did not report is never invented.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedLocation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
}

impl CreatedLocation {
    /// Whether the runtime named anything it created.
    pub fn is_empty(&self) -> bool {
        self.workspace_id.is_none() && self.tab_id.is_none() && self.pane_id.is_none()
    }

    /// The runtime's own id for `kind`, if its answer named one.
    pub fn identity(&self, kind: CreatedKind) -> Option<CreatedIdentity> {
        let id = match kind {
            CreatedKind::Workspace => &self.workspace_id,
            CreatedKind::Tab => &self.tab_id,
            CreatedKind::Pane => &self.pane_id,
        };
        id.clone().map(|id| CreatedIdentity { kind, id })
    }

    /// What was created, one effect per reported identity: `created tab wA:t2`.
    pub fn effects(&self) -> Vec<String> {
        let mut effects = Vec::new();
        if let Some(id) = &self.workspace_id {
            effects.push(format!("created workspace {id}"));
        }
        if let Some(id) = &self.tab_id {
            effects.push(format!("created tab {id}"));
        }
        if let Some(id) = &self.pane_id {
            effects.push(format!("created pane {id}"));
        }
        effects
    }
}

/// The kind of location a creation made, in the protocol's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatedKind {
    Workspace,
    Tab,
    Pane,
}

/// The identity a creation reported, as the runtime named it: what kind of
/// location, and the runtime's own id for it.
///
/// This is what a client acts on — the record's structured answer to "what did
/// I just create". The prose in [`CreatedLocation::effects`] stays for a human
/// reading the record; nothing parses it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreatedIdentity {
    pub kind: CreatedKind,
    pub id: String,
}

impl CreateRequest {
    /// The kind of location this request asks for, which is the identity its
    /// answer is expected to name.
    pub fn created_kind(&self) -> CreatedKind {
        match self {
            Self::Workspace { .. } => CreatedKind::Workspace,
            Self::Tab { .. } => CreatedKind::Tab,
            Self::PaneSplit { .. } => CreatedKind::Pane,
        }
    }
}

/// The certainty of a create attempt at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateOutcome {
    /// The runtime created the location and reported what it created.
    Completed(CreatedLocation),
    /// The runtime positively rejected the request before creating anything.
    Refused(String),
    /// Dispatch or completion may have happened, but no trustworthy answer
    /// arrived: a caller must not create again on the assumption it did not.
    Unknown(String),
}

/// A resolved child command: an absolute executable, its argv, and the pane it
/// runs in.
///
/// `spawn_token` is the private correlation token the child presents when it
/// registers. The token travels to the child in its environment under
/// [`LAUNCH_TOKEN_ENV`] and must never appear in an inventory, projection or
/// diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    pub pane_id: String,
    pub executable: String,
    #[serde(default)]
    pub argv: Vec<String>,
    pub spawn_token: String,
}

/// Environment variable carrying [`LaunchRequest::spawn_token`] to the child.
pub const LAUNCH_TOKEN_ENV: &str = "PI_RADAR_SPAWN_TOKEN";

/// The capability a backend declares when it can run a resolved child command.
pub const LAUNCH_CAPABILITY: &str = "launch";

impl LaunchRequest {
    /// Whether this is a command the seam can carry: a named pane, an absolute
    /// printable executable, argv free of terminal control sequences, and a
    /// non-empty printable token.
    ///
    /// A newline in argv is *not* refused here: it is ordinary text in an
    /// argument, and whether it can be delivered is the backend's own limit
    /// ([`RuntimeProvider::launch`]). Like input text, a control character that
    /// a terminal would act on cannot be carried as an argument without
    /// deciding for the caller what the terminal does with it, so it is refused.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_location_identifier(&self.pane_id) {
            return Err("`pane_id` is not a valid pane identifier".to_string());
        }
        validate_command(&self.executable, &self.argv)?;
        if self.spawn_token.is_empty() || self.spawn_token.chars().any(char::is_control) {
            return Err("`spawn_token` must be non-empty printable text".to_string());
        }
        Ok(())
    }
}

/// Whether a command a caller resolved is one this seam can carry: an absolute
/// printable executable, and argv free of terminal control sequences other than
/// a newline, which is ordinary argument text.
///
/// The wire boundary and the launch it becomes share these rules, so a command a
/// caller may ask for is exactly a command this seam accepts. What a *backend*
/// can deliver is a separate limit, stated by that backend's own `launch`.
pub fn validate_command(executable: &str, argv: &[String]) -> Result<(), String> {
    if !executable.starts_with('/') || executable.chars().any(char::is_control) {
        return Err("`executable` must be an absolute printable path".to_string());
    }
    if argv.iter().any(|argument| {
        argument
            .chars()
            .any(|character| character.is_control() && character != '\n')
    }) {
        return Err("`argv` cannot contain terminal control characters".to_string());
    }
    Ok(())
}

/// The certainty of one launch attempt at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// The backend confirmed the command was handed to the pane.
    Completed,
    /// The backend positively rejected the request before handing it over.
    Refused(String),
    /// The command may have reached the pane, but no trustworthy answer
    /// arrived: a caller must not launch again on the assumption it did not run.
    Unknown(String),
}

/// What a pane is asked to receive.
///
/// Literal text and named keys never travel together, so "write these bytes"
/// cannot be read as "press these names".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputPayload {
    /// Literal text, written as the pane's input. Nothing interprets it: no
    /// shell, no key name, no escape sequence.
    Text { text: String },
    /// Named key presses, e.g. `esc` or `ctrl+c`.
    Keys { keys: Vec<String> },
}

/// One input request: a pane, and what it receives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputRequest {
    pub pane_id: String,
    pub payload: InputPayload,
}

/// Longest literal input one request may carry.
pub const MAX_INPUT_BYTES: usize = 4096;

/// Most keys one request may name, and how long one name may be.
pub const MAX_KEYS: usize = 16;
pub const MAX_KEY_BYTES: usize = 32;

impl InputRequest {
    /// Whether this is input the seam can carry without ambiguity: literal text
    /// carrying no control sequence a terminal would act on, or a bounded list
    /// of plain key names.
    ///
    /// A control character cannot be sent as text without deciding for the
    /// caller what the terminal will do with it, so it is refused here — named
    /// keys are the way to ask for those. A refusal is the caller's mistake and
    /// happens before anything is dispatched.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_location_identifier(&self.pane_id) {
            return Err("`pane_id` is not a valid pane identifier".to_string());
        }
        match &self.payload {
            InputPayload::Text { text } => {
                if text.is_empty() {
                    return Err("`text` is empty".to_string());
                }
                if text.len() > MAX_INPUT_BYTES {
                    return Err(format!("`text` exceeds {MAX_INPUT_BYTES} bytes"));
                }
                if text.chars().any(|character| {
                    character.is_control() && character != '\n' && character != '\t'
                }) {
                    return Err(
                        "`text` carries a control sequence; named keys are how those are sent"
                            .to_string(),
                    );
                }
                Ok(())
            }
            InputPayload::Keys { keys } => {
                if keys.is_empty() {
                    return Err("`keys` is empty".to_string());
                }
                if keys.len() > MAX_KEYS {
                    return Err(format!("`keys` names more than {MAX_KEYS} keys"));
                }
                for key in keys {
                    let plain = !key.is_empty()
                        && key.len() <= MAX_KEY_BYTES
                        && key.chars().all(|character| {
                            character.is_ascii_alphanumeric()
                                || matches!(character, '+' | '-' | '_')
                        });
                    if !plain {
                        return Err(format!(
                            "`{key}` is not a plain key name: at most {MAX_KEY_BYTES} characters of letters, digits, `+`, `-` or `_`"
                        ));
                    }
                }
                Ok(())
            }
        }
    }
}

/// Whether a string can name a pane or a workspace on this seam.
fn valid_location_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// The certainty of an input attempt at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputOutcome {
    /// The runtime accepted the input.
    Completed,
    /// The runtime positively rejected the input before delivering anything.
    Refused(String),
    /// Dispatch may have happened, but no trustworthy answer arrived.
    Unknown(String),
}

/// Which part of a pane's output a read asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputSource {
    /// What the pane is rendering now.
    Visible,
    /// Recently rendered output, soft wraps included.
    Recent,
    /// Recently rendered output with soft wraps joined.
    RecentUnwrapped,
    /// The bottom-buffer snapshot agent detection reads.
    Detection,
}

/// One output read: which pane, which part of its output, and how much.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRequest {
    pub pane_id: String,
    pub source: OutputSource,
    /// How many rows to ask for; the runtime's own default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// Keep terminal styling instead of plain text.
    #[serde(default)]
    pub ansi: bool,
}

/// A snapshot of pane output.
///
/// Reading consumes nothing: the same request answered twice describes the same
/// pane content unless `revision` moved, and no cursor or acknowledgement is
/// advanced. `truncated` says a bound was reached, whether the runtime's or the
/// caller's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRead {
    pub text: String,
    pub truncated: bool,
    /// The runtime's revision of the pane's content at the read, when it
    /// reports one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
}

/// The certainty of an output read at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputOutcome {
    /// The runtime returned a snapshot.
    Completed(OutputRead),
    /// The runtime positively rejected the request before reading anything.
    Refused(String),
    /// The read may have reached the runtime, but no usable answer arrived.
    Unknown(String),
}

/// The agent lifecycle words a publisher may report, as the backend names them.
///
/// A word outside this set is not a fact this seam can carry, so it is refused at
/// the wire rather than passed on to be guessed at later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportedState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

impl ReportedState {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Unknown => "unknown",
        }
    }
}

/// Longest one reported string may be, in bytes. A report is a display fact and a
/// record is a bounded file, so the seam states its own ceiling rather than
/// passing an arbitrarily long string into a durable record.
///
/// An empty string is allowed where the caller may mean "nothing": a title can be
/// set to nothing, a source and an agent name cannot.
pub const MAX_REPORTED_TEXT_BYTES: usize = 1024;

/// How many display tokens or state labels one report may carry. Herdr's own
/// bound for tokens, applied to both maps so neither can grow a record without
/// end.
pub const MAX_REPORTED_ENTRIES: usize = 16;

/// The longest validity a caller may ask for, as the backend bounds it: one day.
pub const MAX_REPORT_TTL_MS: u64 = 86_400_000;

/// The agent lifecycle state a pane's publisher observed, in the caller's words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateReport {
    pub pane_id: String,
    /// The publisher's own name for itself, carried through as given.
    pub source: String,
    pub agent: String,
    pub state: ReportedState,
    /// What the publisher says about that state, in its own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The publisher's own sequence number. Carried through, never compared here:
    /// ordering reports is the publisher's business, not this seam's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
}

/// The session a pane's agent is running in, as its publisher names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionReport {
    pub pane_id: String,
    pub source: String,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_start_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
}

/// Which location a display report describes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportTarget {
    Pane { pane_id: String },
    Workspace { workspace_id: String },
}

impl ReportTarget {
    /// The normalized id this report describes: the record's target key, shared
    /// with every other request over the same location.
    pub fn id(&self) -> &str {
        match self {
            Self::Pane { pane_id } => pane_id,
            Self::Workspace { workspace_id } => workspace_id,
        }
    }
}

/// Display-only metadata a publisher reports: what one location's row shows.
///
/// Every value is the caller's own. Nothing here is derived from the pane's
/// processes, from its agent or from another publisher's report, and a value the
/// caller did not send is absent rather than inferred.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetadataReport {
    pub target: ReportTarget,
    pub source: String,
    /// The publisher's own display tokens. `None` withdraws a token this
    /// publisher reported before, which is how a label is taken back.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tokens: BTreeMap<String, Option<String>>,
    /// The agent this metadata is about, when the publisher names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The source whose pane the display should describe, where one pane has more
    /// than one publisher.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies_to_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    /// Display text per agent state word.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub state_labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "clear_flag_is_unset")]
    pub clear_title: bool,
    #[serde(default, skip_serializing_if = "clear_flag_is_unset")]
    pub clear_display_agent: bool,
    #[serde(default, skip_serializing_if = "clear_flag_is_unset")]
    pub clear_state_labels: bool,
    /// How long the reported metadata stays valid, when the caller says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
}

/// What a caller reports: one publisher's own facts about one location.
///
/// A report is a bridge, not a registry: it says what a publisher says, and
/// nothing here claims to have merged reports, recovered work or established who
/// owns a location.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReportRequest {
    /// The agent lifecycle state a pane's publisher observed.
    State(StateReport),
    /// The session identity a pane's publisher observed.
    Session(SessionReport),
    /// Display-only metadata for a pane or a workspace.
    Metadata(MetadataReport),
}

/// A clear flag a caller did not set does not travel: `false` is the backend's
/// own default, so the wire carries only the clears that were asked for.
fn clear_flag_is_unset(cleared: &bool) -> bool {
    !*cleared
}

impl ReportRequest {
    /// The location this report describes: the record's target key, so a report
    /// conflicts with another mutation of the same location like any other.
    pub fn target(&self) -> &str {
        match self {
            Self::State(report) => &report.pane_id,
            Self::Session(report) => &report.pane_id,
            Self::Metadata(report) => report.target.id(),
        }
    }

    /// The one-line effect a completed report records, naming what the publisher
    /// named.
    pub fn effect(&self) -> String {
        match self {
            Self::State(report) => format!("reported state {}", report.state.as_str()),
            Self::Session(report) => match &report.session_id {
                Some(id) => format!("reported session {id}"),
                None => "reported session".to_string(),
            },
            Self::Metadata(_) => "reported metadata".to_string(),
        }
    }

    /// Whether this is a report the seam can carry to a backend.
    ///
    /// Checked at the wire boundary, before anything is recorded or dispatched: a
    /// location has to be named, a publisher has to name itself and its agent, and
    /// every reported value has to be bounded text. A refusal is the caller's
    /// mistake and happens before a record exists.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::State(report) => {
                report_identity(&report.pane_id)?;
                report_publisher("source", &report.source)?;
                report_publisher("agent", &report.agent)?;
                reported_text("message", report.message.as_deref())
            }
            Self::Session(report) => {
                report_identity(&report.pane_id)?;
                report_publisher("source", &report.source)?;
                report_publisher("agent", &report.agent)?;
                reported_text("session_id", report.session_id.as_deref())?;
                reported_text("session_path", report.session_path.as_deref())?;
                reported_text(
                    "session_start_source",
                    report.session_start_source.as_deref(),
                )
            }
            Self::Metadata(report) => {
                report_identity(report.target.id())?;
                report_publisher("source", &report.source)?;
                reported_text("agent", report.agent.as_deref())?;
                reported_text("applies_to_source", report.applies_to_source.as_deref())?;
                if report.tokens.len() > MAX_REPORTED_ENTRIES {
                    return Err(format!(
                        "`tokens` names more than {MAX_REPORTED_ENTRIES} tokens"
                    ));
                }
                for (name, value) in &report.tokens {
                    report_entry_name("token", name)?;
                    reported_text("token value", value.as_deref())?;
                }
                if report.state_labels.len() > MAX_REPORTED_ENTRIES {
                    return Err(format!(
                        "`state_labels` names more than {MAX_REPORTED_ENTRIES} labels"
                    ));
                }
                for (word, label) in &report.state_labels {
                    report_entry_name("state label", word)?;
                    reported_text("state label text", Some(label))?;
                }
                if let Some(ttl) = report.ttl_ms
                    && (ttl == 0 || ttl > MAX_REPORT_TTL_MS)
                {
                    return Err(format!(
                        "`ttl_ms` must be between 1 and {MAX_REPORT_TTL_MS}"
                    ));
                }
                // A workspace has no title, agent badge or per-state labels to
                // show, so a report that sets one is a caller asking for
                // something this seam cannot forward.
                if matches!(report.target, ReportTarget::Workspace { .. }) {
                    if report.title.is_some()
                        || report.display_agent.is_some()
                        || report.applies_to_source.is_some()
                        || report.agent.is_some()
                        || report.clear_title
                        || report.clear_display_agent
                        || !report.state_labels.is_empty()
                        || report.clear_state_labels
                    {
                        return Err(
                            "a workspace report carries tokens only, not a title, agent or state labels"
                                .to_string(),
                        );
                    }
                    if report.tokens.is_empty() {
                        return Err("a workspace report names at least one token".to_string());
                    }
                }
                reported_text("title", report.title.as_deref())?;
                reported_text("display_agent", report.display_agent.as_deref())
            }
        }
    }
}

/// A location a report names: a pane or a workspace id, and nothing else.
fn report_identity(id: &str) -> Result<(), String> {
    if valid_location_identifier(id) {
        Ok(())
    } else {
        Err("the report's target is not a valid identifier".to_string())
    }
}

/// A publisher's own name: the source, or the agent it reports about.
fn report_publisher(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("`{name}` is required and must be non-empty"));
    }
    reported_text(name, Some(value))
}

/// A token or state-label name: the shape a display map's key may take.
fn report_entry_name(kind: &str, name: &str) -> Result<(), String> {
    let plain = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'));
    if plain {
        Ok(())
    } else {
        Err(format!(
            "`{name}` is not a {kind} name: at most 32 characters of letters, digits, `-` or `_`"
        ))
    }
}

/// A reported value: bounded, and carrying nothing a terminal would act on.
fn reported_text(name: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    if value.len() > MAX_REPORTED_TEXT_BYTES {
        return Err(format!("`{name}` exceeds {MAX_REPORTED_TEXT_BYTES} bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "`{name}` carries a control character; a report is display text"
        ));
    }
    Ok(())
}

/// The certainty of a report at the runtime boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportOutcome {
    /// The runtime accepted the report.
    Completed,
    /// The runtime positively rejected the report before recording anything.
    Refused(String),
    /// Dispatch may have happened, but no trustworthy answer arrived. A caller
    /// must not report again on the assumption it did not.
    Unknown(String),
}

/// What Radar asks of one runtime: its facts, the one action on it, and the mux
/// primitives a client asks for by name.
pub trait RuntimeProvider: Send + Sync {
    /// The capabilities this runtime implements, in the protocol's words.
    ///
    /// Advertising one is a promise: an operation whose capability is absent is
    /// refused as unsupported rather than attempted. The default advertises
    /// nothing, so a runtime that has not stated its capabilities is never
    /// credited with one.
    fn capabilities(&self) -> &'static [&'static str] {
        &[]
    }

    /// The whole normalized inventory, or a user-facing diagnostic.
    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String>;

    /// Foreground evidence for one pane, inconclusive when unreadable.
    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence;

    /// Moves the runtime's focus to `target`.
    ///
    /// `Err` carries the one-line refusal or failure a user reads. Like every
    /// operation here, a cancelled request must abandon an outstanding
    /// command or exchange, return without waiting out its deadline, and leave
    /// no child process behind.
    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String>;

    /// Focus with dispatch certainty for durable callers. Legacy adapters may
    /// only return a diagnostic, so conservatively treat their errors as unknown.
    fn focus_outcome(&self, target: &Target, cancel: &AtomicBool) -> FocusOutcome {
        match self.focus(target, cancel) {
            Ok(()) => FocusOutcome::Completed,
            Err(message) => FocusOutcome::Unknown(message),
        }
    }

    /// Closes `target` through the runtime, for a location Radar has positive
    /// evidence is unmanaged. An adapter that cannot close returns an explicit
    /// refusal; nothing here may kill a process directly.
    ///
    /// Same cancellation duty as [`Self::focus`]. A close already handed to the
    /// runtime is not retracted by cancelling: cancellation only abandons a
    /// request still waiting.
    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String>;

    /// Close with dispatch certainty for durable callers. Legacy adapters may
    /// only return a diagnostic, so conservatively treat their errors as unknown.
    fn close_outcome(&self, target: &CloseTarget, cancel: &AtomicBool) -> CloseOutcome {
        match self.close(target, cancel) {
            Ok(()) => CloseOutcome::Completed,
            Err(message) => CloseOutcome::Unknown(message),
        }
    }

    /// Creates one location, returning what the runtime reports it created.
    ///
    /// Same cancellation duty as [`Self::focus`]. The default refuses: a runtime
    /// that has not implemented creation must not appear to have created
    /// something, and a caller must be able to tell that this is unimplemented
    /// rather than failed.
    fn create(&self, _request: &CreateRequest, _cancel: &AtomicBool) -> CreateOutcome {
        CreateOutcome::Refused("this runtime does not implement creation".to_string())
    }

    /// Sends literal text or named keys to a pane.
    ///
    /// The payload has already been validated as input this seam can carry
    /// ([`InputRequest::validate`]); an implementation passes it on as input,
    /// with no shell and no interpretation.
    fn input(&self, _request: &InputRequest, _cancel: &AtomicBool) -> InputOutcome {
        InputOutcome::Refused("this runtime does not implement pane input".to_string())
    }

    /// Runs one already-resolved child command in a pane this daemon created.
    ///
    /// The request has already been validated ([`LaunchRequest::validate`]), and
    /// the command was resolved here: no shell interprets it and the adapter adds
    /// no arguments of its own. Launch is an explicit capability, deliberately
    /// separate from pane input: a runtime may accept both, either, or neither,
    /// and advertising `launch` promises exactly this one create-and-run step,
    /// never ongoing process lifecycle. Implementations report
    /// [`LaunchOutcome::Unknown`] whenever dispatch may have happened without a
    /// trustworthy answer, because a caller must not launch twice.
    ///
    /// An adapter that has to type the command into a pane shell reports
    /// [`LaunchOutcome::Refused`] for an argv holding a newline: a multi-line
    /// line has no consumption acknowledgement, so the daemon does not type one.
    fn launch(&self, _request: &LaunchRequest, _cancel: &AtomicBool) -> LaunchOutcome {
        LaunchOutcome::Refused("this runtime does not implement launch".to_string())
    }

    /// Reads a bounded snapshot of one pane's output, consuming nothing.
    ///
    /// A runtime that has no non-consuming read must not advertise `output` in
    /// [`Self::capabilities`]: a read that drained the pane's scrollback or
    /// advanced a cursor would change what the operator sees.
    fn output(&self, _request: &OutputRequest, _cancel: &AtomicBool) -> OutputOutcome {
        OutputOutcome::Refused("this runtime does not implement output reads".to_string())
    }

    /// Forwards one publisher's report to the backend, as the caller gave it.
    ///
    /// The values have already been validated as reportable
    /// ([`ReportRequest::validate`]); an implementation passes them on unchanged.
    /// It establishes nothing of its own: no agent fact is derived, no report is
    /// merged into a store, and a backend that cannot report says so instead of
    /// substituting another operation.
    fn report(&self, _request: &ReportRequest, _cancel: &AtomicBool) -> ReportOutcome {
        ReportOutcome::Refused("this runtime does not implement metadata reporting".to_string())
    }
}

/// A shared handle is a provider too, so the collector and focuser can own the
/// seam while their caller keeps one adapter handle.
impl<T: RuntimeProvider + ?Sized> RuntimeProvider for Arc<T> {
    fn capabilities(&self) -> &'static [&'static str] {
        (**self).capabilities()
    }

    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        (**self).inventory(cancel)
    }

    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence {
        (**self).foreground_evidence(pane_id, cancel)
    }

    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        (**self).focus(target, cancel)
    }

    fn focus_outcome(&self, target: &Target, cancel: &AtomicBool) -> FocusOutcome {
        (**self).focus_outcome(target, cancel)
    }

    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        (**self).close(target, cancel)
    }

    fn close_outcome(&self, target: &CloseTarget, cancel: &AtomicBool) -> CloseOutcome {
        (**self).close_outcome(target, cancel)
    }

    fn create(&self, request: &CreateRequest, cancel: &AtomicBool) -> CreateOutcome {
        (**self).create(request, cancel)
    }

    fn input(&self, request: &InputRequest, cancel: &AtomicBool) -> InputOutcome {
        (**self).input(request, cancel)
    }

    fn launch(&self, request: &LaunchRequest, cancel: &AtomicBool) -> LaunchOutcome {
        (**self).launch(request, cancel)
    }

    fn output(&self, request: &OutputRequest, cancel: &AtomicBool) -> OutputOutcome {
        (**self).output(request, cancel)
    }

    fn report(&self, request: &ReportRequest, cancel: &AtomicBool) -> ReportOutcome {
        (**self).report(request, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Decodes a report the way the daemon does, so the wire shape this seam
    /// publishes is the one its tests exercise.
    fn decode(value: serde_json::Value) -> Result<ReportRequest, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }

    #[test]
    fn a_state_report_carries_the_publishers_own_words() {
        let report = decode(json!({
            "kind": "state",
            "pane_id": "wA:p1",
            "source": "pi-herdsman",
            "agent": "worker",
            "state": "working",
            "message": "2 tasks",
            "sequence": 4,
        }))
        .expect("a state report");
        let ReportRequest::State(state) = &report else {
            panic!("{report:?}");
        };
        assert_eq!(state.state, ReportedState::Working);
        assert_eq!(state.sequence, Some(4));
        assert_eq!(report.target(), "wA:p1");
        assert_eq!(report.effect(), "reported state working");
        assert_eq!(report.validate(), Ok(()));
    }

    #[test]
    fn a_display_report_carries_tokens_a_withdrawn_token_and_its_ttl() {
        let report = decode(json!({
            "kind": "metadata",
            "target": {"kind": "pane", "pane_id": "wA:p1"},
            "source": "herdsman",
            "tokens": {"summary": "3 tasks", "title-suffix": null},
            "applies_to_source": "herdr:pi",
            "state_labels": {"working": "thinking"},
            "ttl_ms": 30000,
        }))
        .expect("a display report");
        let ReportRequest::Metadata(metadata) = &report else {
            panic!("{report:?}");
        };
        assert_eq!(metadata.tokens["summary"], Some("3 tasks".to_string()));
        assert_eq!(metadata.tokens["title-suffix"], None);
        assert_eq!(metadata.ttl_ms, Some(30000));
        assert_eq!(report.validate(), Ok(()));
    }

    /// The seam is not a passthrough: a field it does not know is refused rather
    /// than dropped, so a caller is never told a value was reported that was not.
    #[test]
    fn an_unknown_field_or_kind_is_refused_rather_than_ignored() {
        assert!(
            decode(json!({
                "kind": "state",
                "pane_id": "wA:p1",
                "source": "s",
                "agent": "a",
                "state": "idle",
                "resume_argv": ["pi", "--continue"],
            }))
            .is_err()
        );
        assert!(decode(json!({"kind": "nowhere", "pane_id": "wA:p1"})).is_err());
        assert!(decode(json!({"kind": "metadata", "source": "s"})).is_err());
    }

    #[test]
    fn a_report_names_its_location_publisher_and_agent() {
        let cases = [
            json!({"kind": "state", "pane_id": "", "source": "s", "agent": "a", "state": "idle"}),
            json!({"kind": "state", "pane_id": "wA:p1", "source": "", "agent": "a", "state": "idle"}),
            json!({"kind": "state", "pane_id": "wA:p1", "source": "s", "state": "idle"}),
            json!({"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "busy"}),
            json!({"kind": "session", "pane_id": "wA:p1", "source": "s", "agent": "a", "session_id": 7}),
            json!({"kind": "metadata", "target": {"kind": "workspace", "workspace_id": "wA"}, "source": "s", "tokens": {}}),
        ];
        for case in cases {
            // A missing required field is refused by the decoder; a value that
            // does not name a location or a state word is refused by validation.
            let refusal = match decode(case.clone()) {
                Ok(report) => report.validate(),
                Err(error) => Err(error),
            };
            assert!(refusal.is_err(), "{case}");
        }
    }

    #[test]
    fn reported_values_are_bounded_and_free_of_terminal_control() {
        let long = "x".repeat(MAX_REPORTED_TEXT_BYTES + 1);
        let mut tokens = serde_json::Map::new();
        tokens.insert("too long".to_string(), json!("x"));
        let cases = [
            json!({"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle", "message": long}),
            json!({"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle", "message": "a\u{1b}[31mred"}),
            json!({"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": tokens}),
            json!({"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": {"ok": "line\nbreak"}}),
            json!({"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": {"ok": "v"}, "ttl_ms": 0}),
            json!({"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": {"ok": "v"}, "ttl_ms": MAX_REPORT_TTL_MS + 1}),
            json!({"kind": "metadata", "target": {"kind": "workspace", "workspace_id": "wA"}, "source": "s", "tokens": {"ok": "v"}, "title": "no"}),
        ];
        for case in cases {
            let report = decode(case.clone()).unwrap_or_else(|error| panic!("{case}: {error}"));
            assert!(report.validate().is_err(), "{case}");
        }
        // A title can be set to nothing — that is a publisher clearing its own
        // title — and a workspace report is its tokens.
        let empty_title = decode(json!({
            "kind": "metadata",
            "target": {"kind": "pane", "pane_id": "wA:p1"},
            "source": "s",
            "tokens": {"ok": "v"},
            "title": "",
        }))
        .expect("a display report");
        assert_eq!(empty_title.validate(), Ok(()));
    }

    #[test]
    fn a_report_round_trips_through_its_own_wire_shape() {
        let report = ReportRequest::Metadata(MetadataReport {
            target: ReportTarget::Workspace {
                workspace_id: "wA".into(),
            },
            source: "herdsman".into(),
            tokens: [("role".to_string(), Some("lead".to_string()))]
                .into_iter()
                .collect(),
            clear_title: false,
            clear_display_agent: false,
            clear_state_labels: false,
            agent: None,
            applies_to_source: None,
            title: None,
            display_agent: None,
            state_labels: BTreeMap::new(),
            ttl_ms: Some(5000),
            sequence: None,
        });
        let encoded = serde_json::to_value(&report).expect("a report encodes");
        assert_eq!(
            encoded,
            json!({
                "kind": "metadata",
                "target": {"kind": "workspace", "workspace_id": "wA"},
                "source": "herdsman",
                "tokens": {"role": "lead"},
                "ttl_ms": 5000,
            })
        );
        assert_eq!(decode(encoded), Ok(report));
    }
}
