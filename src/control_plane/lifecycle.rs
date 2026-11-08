//! Daemon-owned launch subjects: the physical lifecycle of a tracked spawn.
//!
//! A tracked launch starts `radar run --launch` in its pane. That process claims
//! its own incarnation with a private, one-use correlation; the daemon verifies
//! that it is the pane's foreground process and was started for the requested
//! launch, records it, and only then answers with the program from its own record.
//! The process enters the registration's scope and execs the program. The exec
//! keeps the PID and birth identity, so the captured process is the program that
//! runs. A correlation is spent at capture; a revoked subject never captures or binds.
//!
//! Three lifetimes are kept apart here, and confusing them is what makes lifecycle
//! code wrong:
//!
//! - A **subject** is the logical agent: its identity, its managed metadata and
//!   which attempt is current. It outlives the attempts that run it.
//! - A **claim** is one attempt: the execution record, with its registration, its
//!   captured process identity, its scope and how it ended. Termination addresses
//!   the claim, never a re-resolved subject, so a successor is never signalled by
//!   an operation that meant its predecessor.
//! - A **pane** is a terminal resource that an execution runs in. Its lifetime is
//!   neither the subject's nor the claim's: a pane can outlive the program it ran
//!   (an interactive shell keeps it), and a claim can outlive its pane. A record's
//!   [`Placement`] is the association, and it is only usable while the instance
//!   that created the pane is the one serving now.
//!
//! Every transition of a subject runs under the registry's transition lock. That
//! lock is the one commit fence shared with registration binding, so a cancel and
//! a launch claim, or a cancel and a registration, have exactly one winner.
//! The lock is never held across a wait for a process to exit.

mod observation;
pub use observation::{
    ABSENCE_CONFIRMATIONS, ABSENCE_WINDOW, EndedEvidence, EndedReason, Evidence, Observation,
    Observer, PaneAbsence, PaneEvidence, observe_exit_from, observe_pane,
};

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::identity::{ClaimId, RegistrationId, SubjectId};
use serde_json::{Value, json};

use crate::control_plane::answer::{Admitted, Claimed};
use crate::control_plane::protocol::{MAX_LINE_BYTES, PROTOCOL_VERSION};
use crate::control_plane::registry::publication::write_atomic;
use crate::control_plane::registry::{
    LocalProcfsVerifier, ProcessVerifier, Registry, read_record_file,
};
use crate::control_plane::store::{format_millis, random_uuid};
use crate::model::{ProcessIdentity, SessionUuid};
use crate::process_control::{self, Acquired, Effective};
use crate::runtime::LaunchRequest;

/// Environment variables `radar run --launch` reads and removes before the program runs.
pub const CLAIM_ENV: &str = "PI_RADAR_LAUNCH_CLAIM";
pub const CORRELATION_ENV: &str = "PI_RADAR_LAUNCH_CORRELATION";
pub const MANAGED_PARENT_SUBJECT_ENV: &str = "PI_RADAR_MANAGED_PARENT_SUBJECT_ID";

/// The internal method `radar run --launch` calls to claim its recorded launch.
pub const CLAIM_METHOD: &str = "launch.claim";

/// The subcommand a pane-death hook runs, and the internal method it calls.
pub const WAKE_COMMAND: &str = "lifecycle-wake";
pub const WAKE_METHOD: &str = "lifecycle.wake";

/// Admission of an in-place managed launch into a pane the caller already occupies.
pub const ADMIT_METHOD: &str = "launch.admit";
pub const COMPLETE_METHOD: &str = "launch.complete";

/// Refusal for a claim the subject cannot take: unknown, spent, revoked, or no
/// longer awaiting a process. It says nothing about which.
pub const NOT_AWAITING: &str = "launch is not awaiting its process";

/// Refusal for a captured process that was not started with the requested launch.
pub const LAUNCH_MISMATCH: &str = "launch context does not match the requested launch";

const SUBJECT_VERSION: u32 = 4;
const MAX_SUBJECT_BYTES: usize = 64 * 1024;
/// How long a cancelled process gets to exit after SIGTERM before the operation
/// reports the exit as not verified.
const EXIT_DEADLINE: Duration = Duration::from_secs(2);
/// How long a verified scope is given to end after it was escalated.
const ESCALATION_DEADLINE: Duration = Duration::from_secs(2);
/// How long a launch waits for the daemon before it gives up without running
/// the program.
const RELEASE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Launch recorded; no process has claimed it yet.
    Pending,
    /// The launch's process is captured and has been given its program.
    Captured,
    /// A registration presented the token with the captured identity.
    Bound,
    /// Cancelled before binding: no registration may bind it.
    Revoked,
}

/// The launch a tracked spawn asked for. It is private: it holds the env values
/// the caller supplied, and it is never returned by any read.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchContext {
    pub executable: String,
    pub argv: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub env: BTreeMap<String, String>,
}

impl LaunchContext {
    /// Whether the process was started for this launch: it is the launch chain,
    /// in the working directory requested if any, with each requested
    /// environment value. Variables the backend adds to every pane are not part
    /// of the launch and are not compared.
    fn launched_as(&self, effective: &Effective) -> bool {
        // The program comes from this record, so the process is checked for being
        // the launch chain that will fetch it, not for carrying the program.
        let is_claim = effective
            .cmdline
            .windows(2)
            .any(|pair| pair[0] == "run" && pair[1] == "--launch");
        let cwd_matches = self
            .cwd
            .as_deref()
            .is_none_or(|cwd| fs::canonicalize(cwd).ok().as_deref() == effective.cwd.as_deref());
        is_claim
            && cwd_matches
            && self
                .env
                .iter()
                .all(|(name, value)| effective.env.get(name) == Some(value))
    }
}

/// How a subject came to be tracked: launched by a tracked spawn, or enrolled from
/// a running lead the daemon did not launch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Spawned,
    Enrolled,
    /// Admitted by `radar run --agent`: a launch with no spawn edge or parent.
    Admitted,
}

/// What a managed process is for. An agent is a session that will present its
/// registration and be attached to a subject; a command is only a process Radar
/// keeps in a scope. Agents get the subject, claim and fenced-control model and a
/// command never does, so a command's registration is refused when presented as
/// an agent's.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    #[default]
    Agent,
    Command,
}

/// One durable launch subject. The correlation is a private credential and is
/// spent at capture; no public read returns it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManagedMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_subject_id: Option<SubjectId>,
}

impl ManagedMetadata {
    pub fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("owner", &self.owner),
            ("run", &self.run),
            ("label", &self.label),
            ("role", &self.role),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty()
                    || value.len() > 256
                    || value.chars().any(char::is_control))
            {
                return Err(format!(
                    "managed {name} must be non-empty, control-free, and at most 256 bytes"
                ));
            }
        }
        Ok(())
    }
}

/// The durable identity of one agent: the facts that outlive its attempts.
///
/// A [`Claim`] records one launch or session attachment.
/// The agent does not: its identity, its managed metadata and which attempt is
/// current are held here once, instead of being copied forward onto every
/// successor. `current_claim` names the exact authorized attempt.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubjectRecord {
    pub version: u32,
    pub subject_id: SubjectId,
    #[serde(default)]
    pub managed: ManagedMetadata,
    /// The Pi session this subject runs, when one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The attempt that is current for this subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_claim: Option<ClaimId>,
    /// Last attachment provenance retained when a claim still exists but is not current.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_claim: Option<ClaimId>,
    pub recorded_at: String,
    pub updated_at: String,
}

/// Where one execution runs, and the backend generation that was serving when
/// its placement was established.
///
/// A pane id outlives the server that answered for it, so the location alone is
/// not authority: an operation that would remove the pane compares
/// `instance` against the backend serving now, and a placement recorded without
/// one removes nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub location: crate::control_plane::registry::RegistryLocation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<ProcessIdentity>,
}

impl Placement {
    /// The pane this placement names, when it names one.
    pub fn pane(&self) -> Option<&str> {
        self.location.pane.as_deref()
    }

    fn validate(&self) -> Result<(), String> {
        self.location.validate()?;
        if self.location.pane.is_none() {
            return Err("placement must name a pane".into());
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    pub claim_id: ClaimId,
    pub subject_id: SubjectId,
    #[serde(default)]
    pub managed: ManagedMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(flatten)]
    pub process: ProcessRecord,
}

/// Facts of one process lifetime; changing a session attachment does not change
/// their owner. The registration is allocated before a managed process starts.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessRecord {
    pub version: u32,
    /// The scope this process was wrapped to run in, when Radar launched it into
    /// one. Written once at admission, by the path that wraps the launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<crate::managed_scope::Scope>,
    pub registration_id: RegistrationId,
    pub spawn_request_id: String,
    #[serde(default)]
    pub origin: Origin,
    #[serde(default, skip_serializing_if = "is_agent")]
    pub kind: ProcessKind,
    pub state: State,
    pub launch: LaunchContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation: Option<String>,
    /// The captured birth identity of the launched process.
    #[serde(default, rename = "process", skip_serializing_if = "Option::is_none")]
    pub identity: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<Observation>,
    /// A bound pane's absence under confirmation. Weak evidence, so it is a
    /// pending window rather than a transition until it is confirmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_absence: Option<PaneAbsence>,
    /// Set when a launch ended without a verified process exit, with the evidence
    /// that established it. Distinct from `exited_at`, which only a verified
    /// process exit sets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_externally_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_evidence: Option<EndedEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_registration_id: Option<RegistrationId>,
    /// Recorded before a SIGTERM is sent, so a signal is never sent twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signalled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<String>,
    /// How the program ended, once its supervisor reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<Completion>,
    /// What authenticates this execution's supervisor. Private, like the
    /// correlation: it never leaves the recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_key: Option<String>,
    /// The pane this execution runs in, recorded where it was established: at
    /// admission, or when a tracked launch claimed its program. An execution is
    /// found again by its placement, whether or not a spawn edge requested it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// Recorded once, by the cancel that revoked an unbound launch. A bound
    /// launch's pane belongs to its child and is closed through spawn.close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_cleanup: Option<PaneCleanup>,
    pub recorded_at: String,
}

/// What a spawn needs to launch a tracked child: the subject it belongs to and
/// the correlation its launch claim presents.
pub struct Ticket {
    pub registration_id: RegistrationId,
    /// Exact execution record ID bound to the spawn edge.
    pub claim_id: ClaimId,
    /// Stable logical subject identity issued for this process.
    pub subject_id: SubjectId,
    /// The exact Radar launch operation that created this execution.
    pub launch_id: String,
    pub parent_subject_id: Option<SubjectId>,
    pub correlation: String,
}

/// The daemon's own executable and socket, which every tracked launch runs.
#[derive(Clone, Debug)]
pub struct Bootstrap {
    pub executable: String,
    pub socket: String,
}

/// What became of the launched process in a cancel. Each fact is separate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exit {
    /// No process identity was captured, so nothing was signalled. The launch
    /// was revoked before any launch could be given a program.
    Unresolved,
    /// The captured process is verified gone.
    Exited,
    /// The process could not be verified gone; the reason names why.
    Unverified(String),
}

/// What a cancel or stop did. Revocation, signal dispatch and exit are reported apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Termination {
    /// The launch was revoked by this cancel rather than already revoked or bound.
    pub revoked: bool,
    /// SIGTERM was dispatched by this cancel.
    pub signal_attempted: bool,
    /// The execution outlived the graceful window inside a verified scope, so the
    /// remainder of that scope was killed.
    pub escalated: bool,
    pub exit: Exit,
}

/// One shutdown window, shared by everything ending together.
///
/// The graceful part is how long a signalled program has to exit on its own; the
/// hard part is how long an escalation is then given. Nothing waits per
/// execution, so a branch of agents costs one window, not one per agent.
#[derive(Clone, Copy)]
pub struct Shutdown {
    graceful_until: std::time::Instant,
    hard_until: std::time::Instant,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Shutdown {
    pub fn new() -> Self {
        let start = std::time::Instant::now();
        Self {
            graceful_until: start + EXIT_DEADLINE,
            hard_until: start + EXIT_DEADLINE + ESCALATION_DEADLINE,
        }
    }

    fn graceful(&self) -> Duration {
        self.graceful_until
            .saturating_duration_since(std::time::Instant::now())
    }

    fn hard(&self) -> Duration {
        self.hard_until
            .saturating_duration_since(std::time::Instant::now())
    }
}

/// How a managed program ended, as the process supervising it saw it. It is a
/// fact about the execution, recorded when it happens: a retained pane is not
/// durable storage for it, and its absence never blocks a lifecycle transition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub observed_at: String,
}

/// The pane-removal outcome, independent of process exit and completion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "reason")]
pub enum PaneCleanup {
    /// One close was dispatched in the instance that created the pane, and it
    /// answered.
    Cleaned,
    /// The pane was already gone, so nothing was dispatched.
    Absent,
    /// Nothing was dispatched, for this reason.
    Refused(String),
    /// A close was dispatched and its answer was lost. It is never dispatched
    /// again for this launch.
    Unknown(String),
}

impl From<crate::runtime::CloseOutcome> for PaneCleanup {
    /// A backend's answer to one pane close, in the vocabulary the launch records.
    fn from(outcome: crate::runtime::CloseOutcome) -> Self {
        match outcome {
            crate::runtime::CloseOutcome::Completed => Self::Cleaned,
            crate::runtime::CloseOutcome::Refused(message) => Self::Refused(message),
            crate::runtime::CloseOutcome::Unknown(message) => Self::Unknown(message),
        }
    }
}

/// What one close established, in parts that answer different questions.
///
/// Which executions ended and how, what became of the pane or place that hosted
/// them, and — when a supervisor reported it — how each program ended. A reader
/// takes the part it needs; nothing here has to be recovered from prose, and no
/// part is inferred from another. In particular a removal that completed says
/// nothing about a program's code, and a verified exit says nothing about the
/// pane, which for a spontaneous exit is deliberately retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ClosedExecution {
    /// One entry per execution this close found and addressed, in the order it
    /// addressed them. Empty when the target hosted none: a close of a place is
    /// never reported as one execution's outcome.
    pub executions: Vec<EndedExecution>,
    /// Absent when the operation removes nothing: a stop ends the process and
    /// leaves the pane where it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<ResourceEnd>,
    /// Whether escalation was attempted after the graceful window. This is not
    /// proof that SIGKILL succeeded; each execution carries its own outcome.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub escalated: bool,
}

/// One execution this close addressed, and what became of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EndedExecution {
    /// The logical agent this execution ran as, when one is recorded. A directly
    /// admitted command has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<crate::identity::SubjectId>,
    /// The process incarnation, which every recorded execution has.
    pub registration_id: crate::identity::RegistrationId,
    pub end: ExecutionEnd,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<Completion>,
}

impl EndedExecution {
    /// How a reader names this execution: its agent when it has one, otherwise
    /// the process it ran as.
    pub fn name(&self) -> &str {
        self.subject_id
            .as_ref()
            .map_or(self.registration_id.as_str(), SubjectId::as_str)
            .get(..8)
            .unwrap_or_default()
    }
}

/// Whether the execution this close addressed is known to have ended.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum ExecutionEnd {
    /// The captured process was verified gone.
    Exited,
    /// What happened to it could not be established, and this says why.
    Unverified(String),
    /// Nothing was signalled: the claim it named is not the execution this
    /// operation may end.
    Unsignalled(String),
}

/// What became of the pane that hosted the execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case", tag = "state", content = "reason")]
pub enum ResourceEnd {
    /// A close was dispatched in the instance that created the pane, and it
    /// answered.
    Removed,
    /// The pane was already gone when the removal was attempted.
    Absent,
    /// Nothing was dispatched, for this reason.
    Kept(String),
    /// A close was dispatched and its answer was lost. It is never dispatched
    /// again for this execution.
    Unverified(String),
}

impl From<&PaneCleanup> for ResourceEnd {
    fn from(cleanup: &PaneCleanup) -> Self {
        match cleanup {
            PaneCleanup::Cleaned => Self::Removed,
            PaneCleanup::Absent => Self::Absent,
            PaneCleanup::Refused(reason) => Self::Kept(reason.clone()),
            PaneCleanup::Unknown(reason) => Self::Unverified(reason.clone()),
        }
    }
}

impl std::fmt::Display for ResourceEnd {
    /// One short phrase, for the effect lists that still name what happened in
    /// order. Reads from the same vocabulary a typed answer uses, so the two
    /// cannot describe the same close differently.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Removed => write!(f, "removed"),
            Self::Absent => write!(f, "already absent"),
            Self::Kept(_) => write!(f, "kept"),
            Self::Unverified(_) => write!(f, "cleanup unverified"),
        }
    }
}

impl From<&Exit> for ExecutionEnd {
    fn from(exit: &Exit) -> Self {
        match exit {
            Exit::Exited => Self::Exited,
            Exit::Unverified(reason) => Self::Unverified(reason.clone()),
            Exit::Unresolved => Self::Unsignalled(
                "the recorded process identity could not be resolved to this execution".into(),
            ),
        }
    }
}

enum Stage {
    Settled(Exit),
    Watch {
        descriptor: Option<std::os::fd::OwnedFd>,
        signal: bool,
        scope: Option<crate::managed_scope::VerifiedScope>,
        /// Why the recorded scope could not be verified, when it could not. The
        /// process is still fenced by its own identity, so a TERM is still
        /// authorised; only escalation is not.
        scope_note: Option<String>,
    },
}

/// A pending subject for `launch`, with fresh ids. Its record is what is bounded.
fn draft(
    spawn_request_id: &str,
    launch: LaunchContext,
    session: Option<String>,
    identity: Option<(SubjectId, ClaimId)>,
    managed: ManagedMetadata,
    now_ms: i64,
) -> Claim {
    // A subject identity invented for a reservation is minted, never borrowed
    // from the claim: one string cannot be both a claim and its subject.
    let subject_id = identity.map(|(id, _)| id).unwrap_or_else(SubjectId::random);
    Claim {
        claim_id: ClaimId::random(),
        subject_id,
        managed,
        session,
        process: draft_process(spawn_request_id, launch, RegistrationId::random(), now_ms),
    }
}

fn is_agent(kind: &ProcessKind) -> bool {
    *kind == ProcessKind::Agent
}

fn draft_process(
    spawn_request_id: &str,
    launch: LaunchContext,
    registration_id: RegistrationId,
    now_ms: i64,
) -> ProcessRecord {
    ProcessRecord {
        version: SUBJECT_VERSION,
        // Every launch Radar starts is wrapped into a scope of its own, named from
        // its registration. This records that intent; membership is established
        // again by `Scope::verify` before any stop or recovery acts on it.
        scope: crate::managed_scope::Scope::for_registration(registration_id.as_str()).ok(),
        registration_id,
        spawn_request_id: spawn_request_id.to_string(),
        origin: Origin::Spawned,
        kind: ProcessKind::Agent,
        state: State::Pending,
        launch,
        correlation: Some(random_uuid()),
        identity: None,
        bound_registration_id: None,
        observation: None,
        pane_absence: None,
        ended_externally_at: None,
        ended_evidence: None,
        signalled_at: None,
        exited_at: None,
        pane_cleanup: None,
        completion: None,
        completion_key: None,
        placement: None,
        recorded_at: format_millis(now_ms),
    }
}

/// What a caller establishes when it admits an execution in place: the process
/// itself, what kind of execution it is, what authenticates its supervisor, and
/// where it runs.
pub struct Admission {
    pub kind: ProcessKind,
    pub identity: ProcessIdentity,
    pub completion_key: String,
    pub placement: Option<Placement>,
}

/// A managed command needs a process registration, not an invented agent.
pub fn create_process(
    registry: &Registry,
    spawn_request_id: &str,
    launch: LaunchContext,
    admission: Admission,
    now_ms: i64,
) -> Result<ProcessRecord, String> {
    check_bounds(&launch)?;
    if let Some(placement) = &admission.placement {
        placement.validate()?;
    }
    let mut process = draft_process(spawn_request_id, launch, RegistrationId::random(), now_ms);
    process.kind = admission.kind;
    process.completion_key = Some(admission.completion_key);
    process.placement = admission.placement;
    process.origin = Origin::Admitted;
    process.correlation = None;
    let _transition = registry.transition();
    let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    ensure_process(registry, &mut log, &process)?;
    // Captured here so the first presence fences its bind on the verified identity.
    log.append(
        crate::control_plane::events::Stream::Process(&process.registration_id),
        &format_millis(now_ms),
        crate::control_plane::events::Source::Launch,
        crate::control_plane::events::Transition::Captured(admission.identity.clone()),
    )?;
    process.state = State::Captured;
    process.identity = Some(admission.identity);
    Ok(process)
}

/// A captured managed agent binds by its execution identity, independently of spawn provenance.
pub(crate) fn managed_agent(
    registry: &Registry,
    registration_id: &RegistrationId,
) -> Result<bool, String> {
    Ok(
        read_process(registry, registration_id)?.is_some_and(|process| {
            process.kind == ProcessKind::Agent
                && matches!(process.state, State::Captured | State::Bound)
        }),
    )
}

/// What a spawn read says about its child's end: the claim's recorded completion,
/// or a verified exit whose status was never reported, or nothing yet. It reads
/// what was recorded and asks no backend, so a removed pane changes nothing.
pub fn spawn_exit(
    registry: &Registry,
    claim_id: Option<&ClaimId>,
) -> crate::control_plane::registry::PublicSpawnExitObservation {
    use crate::control_plane::registry::PublicSpawnExitObservation::{Exited, Unknown};
    let process = claim_id
        .and_then(|id| read_claim(registry, id).ok().flatten())
        .map(|claim| claim.process);
    match process {
        Some(ProcessRecord {
            completion: Some(completion),
            ..
        }) => Exited {
            code: completion.code,
            signal: completion.signal,
            observed_at: completion.observed_at,
        },
        Some(ProcessRecord {
            exited_at: Some(observed_at),
            ..
        }) => Exited {
            code: None,
            signal: None,
            observed_at,
        },
        _ => Unknown,
    }
}

/// Records how an admitted program ended, on the report of the supervisor that
/// holds the key minted for it. The program is found by its captured identity, so
/// a report names no registration; the key is what makes it that supervisor's.
/// A second report for the same execution changes nothing.
pub(crate) fn record_completion(
    registry: &Registry,
    process: &ProcessIdentity,
    key: &str,
    code: Option<i32>,
    signal: Option<i32>,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let registration_id = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .registration_of(process)
        .cloned()
        .ok_or("no launch holds this process")?;
    let record = read_process(registry, &registration_id)?.ok_or("launch is not recorded")?;
    if record.completion_key.as_deref() != Some(key) {
        return Err("report does not come from this launch's supervisor".into());
    }
    if record.completion.is_some() {
        return Ok(());
    }
    record_process_transition(
        registry,
        &registration_id,
        crate::control_plane::events::Source::Launch,
        crate::control_plane::events::Transition::Completed(Completion {
            code,
            signal,
            observed_at: format_millis(now_ms),
        }),
        now_ms,
    )
}

/// Binds a managed agent to its presented registration, fenced on the captured identity.
pub(crate) fn bind_managed(
    registry: &Registry,
    registration_id: &RegistrationId,
    process: Option<&ProcessIdentity>,
    now_ms: i64,
) -> Result<(), String> {
    // Called under the caller's registry transition lock, as `bind_locked` is.
    let record = read_process(registry, registration_id)?.ok_or("launch is not recorded")?;
    if process.is_none() || record.identity.as_ref() != process {
        return Err("registration does not match the launched process".into());
    }
    match record.state {
        State::Captured => record_process_transition(
            registry,
            registration_id,
            crate::control_plane::events::Source::Registration,
            crate::control_plane::events::Transition::Bound {
                registration_id: registration_id.clone(),
            },
            now_ms,
        ),
        State::Bound if record.bound_registration_id.as_ref() == Some(registration_id) => Ok(()),
        _ => Err("launch is already bound to another registration".into()),
    }
}

/// Refuses a launch whose subject record would exceed its bound. The record is
/// the launch's only durable correlation, so an oversized launch is never started.
pub fn check_bounds(launch: &LaunchContext) -> Result<(), String> {
    check_bounds_with_metadata(launch, &ManagedMetadata::default())
}

pub fn check_bounds_with_metadata(
    launch: &LaunchContext,
    managed: &ManagedMetadata,
) -> Result<(), String> {
    managed.validate()?;
    let draft = draft("", launch.clone(), None, None, managed.clone(), 0);
    let bytes = serde_json::to_vec_pretty(&draft).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_SUBJECT_BYTES {
        return Err("the launch is too large to track".into());
    }
    Ok(())
}

/// Records a pending subject for a spawn whose edge is already recorded, before
/// any effect that could launch its process.
pub fn create(
    registry: &Registry,
    spawn_request_id: &str,
    launch: LaunchContext,
    session: Option<String>,
    now_ms: i64,
) -> Result<Ticket, String> {
    create_with_intent(
        registry,
        spawn_request_id,
        launch,
        session,
        None,
        ManagedMetadata::default(),
        now_ms,
    )
}

/// Creates one execution under an existing logical subject. The caller supplies
/// this only for a durable, exact successor operation.
pub fn create_with_identity(
    registry: &Registry,
    spawn_request_id: &str,
    launch: LaunchContext,
    session: Option<String>,
    identity: Option<(SubjectId, ClaimId)>,
    now_ms: i64,
) -> Result<Ticket, String> {
    create_with_intent(
        registry,
        spawn_request_id,
        launch,
        session,
        identity,
        ManagedMetadata::default(),
        now_ms,
    )
}

pub fn create_with_intent(
    registry: &Registry,
    spawn_request_id: &str,
    launch: LaunchContext,
    session: Option<String>,
    identity: Option<(SubjectId, ClaimId)>,
    managed: ManagedMetadata,
    now_ms: i64,
) -> Result<Ticket, String> {
    managed.validate()?;
    let _transition = registry.transition();
    if let Some((subject_id, expected)) = &identity {
        let record = read_subject_record(registry, subject_id)?.ok_or("subject is not recorded")?;
        // A released subject has no current claim, so its last one is what a
        // continuation succeeds.
        if record.current_claim.as_ref().or(record.last_claim.as_ref()) != Some(expected) {
            return Err("the subject's current claim changed; no launch was reserved".into());
        }
    }
    let subject = draft(spawn_request_id, launch, session, identity, managed, now_ms);
    write(registry, &subject)?;
    crate::control_plane::registry::presence::reserve(
        registry,
        &subject.process.registration_id,
        &subject.claim_id,
        &subject.subject_id,
        subject.session.clone(),
    )?;
    let mut record =
        read_subject_record(registry, &subject.subject_id)?.ok_or("subject is not recorded")?;
    record.current_claim = Some(subject.claim_id.clone());
    record.last_claim = Some(subject.claim_id.clone());
    record.updated_at = subject.process.recorded_at.clone();
    write_subject_record(registry, &record)?;
    Ok(Ticket {
        registration_id: subject.process.registration_id,
        claim_id: subject.claim_id,
        subject_id: subject.subject_id,
        launch_id: subject.process.spawn_request_id,
        parent_subject_id: subject.managed.parent_subject_id,
        correlation: subject.process.correlation.unwrap_or_default(),
    })
}

/// The attempt with this id, or `None` when none was recorded.
///
/// Keyed on the claim; the subject it serves comes from the attachment, so an
/// attempt is never found through the identity of a subject.
pub fn read_claim(registry: &Registry, claim_id: &ClaimId) -> Result<Option<Claim>, String> {
    let Some(facts) = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .facts(claim_id)
    else {
        return Ok(None);
    };
    let execution = load_process(
        registry,
        &facts.attachment.registration_id,
        &facts.execution,
    )?;
    let subject = Claim {
        claim_id: claim_id.clone(),
        subject_id: facts
            .attachment
            .subject_id
            .clone()
            .ok_or("execution has no readable subject identity")?,
        managed: facts.attachment.managed.clone(),
        session: facts.attachment.session.clone(),
        process: execution,
    };
    Ok(Some(subject))
}

/// Process identity and evidence remain readable without any session attachment.
pub fn read_process(
    registry: &Registry,
    registration_id: &RegistrationId,
) -> Result<Option<ProcessRecord>, String> {
    let facts = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .process(registration_id)
        .cloned();
    facts
        .map(|facts| load_process(registry, registration_id, &facts))
        .transpose()
}

/// Decodes every subject identity and process recipe this build serves, naming
/// the file of the first one it cannot read.
pub(crate) fn verify(registry: &Registry) -> Result<(), String> {
    let mut after: Option<String> = None;
    loop {
        let page = subject_ids_after(registry, after.as_deref(), 100)?;
        let Some(last) = page.last().cloned() else {
            break;
        };
        for id in &page {
            read_subject_record(registry, id).map_err(|error| {
                format!(
                    "{}: {error}",
                    subject_record_path(registry, id.as_str()).display()
                )
            })?;
        }
        after = Some(last.to_string());
    }
    let processes: Vec<(RegistrationId, crate::control_plane::events::ProcessFacts)> = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .processes()
        .map(|(id, facts)| (id.clone(), facts.clone()))
        .collect();
    for (registration_id, facts) in processes {
        let path = recipe_path(registry, registration_id.as_str());
        load_process(registry, &registration_id, &facts)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    Ok(())
}

fn load_process(
    registry: &Registry,
    registration_id: &RegistrationId,
    facts: &crate::control_plane::events::ProcessFacts,
) -> Result<ProcessRecord, String> {
    let bytes = read_record_file(
        &recipe_path(registry, registration_id.as_str()),
        MAX_SUBJECT_BYTES,
    )?;
    let mut process: ProcessRecord =
        serde_json::from_slice(&bytes).map_err(|error| format!("not a process recipe: {error}"))?;
    if process.version != SUBJECT_VERSION || &process.registration_id != registration_id {
        return Err("process recipe does not match its registration or version".into());
    }
    facts.apply_to(&mut process);
    Ok(process)
}

fn read_recorded(registry: &Registry, claim_id: &ClaimId) -> Result<Claim, String> {
    read_claim(registry, claim_id)?
        .ok_or_else(|| "tracked launch subject is unavailable".to_string())
}

/// Resolve a subject's current attachment, or its last one when detached.
pub fn current_attachment(
    registry: &Registry,
    subject_id: &SubjectId,
) -> Result<Option<Claim>, String> {
    let Some(record) = read_subject_record(registry, subject_id)? else {
        return Ok(None);
    };
    let Some(claim_id) = record.current_claim.as_ref().or(record.last_claim.as_ref()) else {
        return Ok(None);
    };
    read_claim(registry, claim_id)
}

/// One agent's durable identity, authority pointer and attachment history.
pub fn read_subject_record(
    registry: &Registry,
    subject_id: &SubjectId,
) -> Result<Option<SubjectRecord>, String> {
    let path = subject_record_path(registry, subject_id.as_str());
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let bytes = read_record_file(&path, MAX_SUBJECT_BYTES)?;
    let record: SubjectRecord =
        serde_json::from_slice(&bytes).map_err(|_| "not a subject record".to_string())?;
    if &record.subject_id != subject_id {
        return Err("subject record is filed under another subject".into());
    }
    Ok(Some(record))
}

fn subject_record_for_session(
    registry: &Registry,
    session: &str,
) -> Result<Option<SubjectRecord>, String> {
    let directory = registry.subjects().join("identity");
    if !directory.try_exists().map_err(|error| error.to_string())? {
        return Ok(None);
    }
    let mut found = None;
    for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .path()
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("json")
        {
            continue;
        }
        let path = entry.path();
        let subject_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(SubjectId::parse)
            .ok_or("subject record is filed under a name that is not a subject id")?;
        let Some(record) = read_subject_record(registry, &subject_id)? else {
            continue;
        };
        if record.session.as_deref() != Some(session) {
            continue;
        }
        if found.is_some() {
            return Err("the session is recorded for more than one subject".into());
        }
        found = Some(record);
    }
    Ok(found)
}

/// Records which attempt is current for its subject, and which facts the subject
/// carries. Called by the attempt writer, so every attempt change maintains it.
fn note_attempt(registry: &Registry, claim: &Claim) -> Result<(), String> {
    let existing = read_subject_record(registry, &claim.subject_id)?;
    let mut record = match existing {
        Some(record) if record.current_claim.as_ref() != Some(&claim.claim_id) => return Ok(()),
        Some(record) => record,
        None => SubjectRecord {
            version: SUBJECT_VERSION,
            subject_id: claim.subject_id.clone(),
            managed: claim.managed.clone(),
            session: claim.session.clone(),
            current_claim: Some(claim.claim_id.clone()),
            last_claim: Some(claim.claim_id.clone()),
            recorded_at: claim.process.recorded_at.clone(),
            updated_at: claim.process.recorded_at.clone(),
        },
    };
    if claim.session.is_some() {
        record.session = claim.session.clone();
    }
    record.updated_at = claim.process.recorded_at.clone();
    write_subject_record(registry, &record)
}

/// One execution that is running where a placement puts it.
pub struct Placed {
    pub registration_id: RegistrationId,
    /// The logical subject this execution belongs to, when one is recorded. A
    /// directly admitted execution has none.
    pub subject_id: Option<SubjectId>,
    pub placement: Placement,
}

/// Records where an execution runs, as one ordered fact of that execution.
///
/// Placement is what a domain operation starts from when it holds a pane rather
/// than an execution, so it is recorded where it was established — at admission,
/// or when a tracked launch claimed its program — and not inferred later from a
/// process's own claims about itself.
pub fn place(
    registry: &Registry,
    registration_id: &RegistrationId,
    placement: Placement,
    now_ms: i64,
) -> Result<(), String> {
    placement.validate()?;
    let _transition = registry.transition();
    match read_process(registry, registration_id)? {
        Some(process) if live(&process) => {}
        Some(_) => return Err("an execution that has ended runs nowhere".into()),
        None => return Err("no execution is recorded for this registration".into()),
    }
    record_process_transition(
        registry,
        registration_id,
        crate::control_plane::events::Source::Launch,
        crate::control_plane::events::Transition::Placed(placement),
        now_ms,
    )
}

/// Every execution whose last recorded placement is still live: the evidence a
/// container close starts from, and the reason an agent the listing filtered out
/// is still closed with the container it runs in.
pub fn placements(registry: &Registry) -> Vec<Placed> {
    let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    // Placement is one fact about one execution, and the log holds executions in
    // two places: the attachment of a registered process and the execution record
    // itself. Whichever carries the placement is the same execution, so they are
    // read together and keyed by registration.
    let mut placements: BTreeMap<RegistrationId, Placed> = BTreeMap::new();
    let mut record = |registration_id: &RegistrationId,
                      facts: &crate::control_plane::events::ProcessFacts,
                      subject_id: Option<&SubjectId>| {
        let Some(placement) = facts.placement.clone() else {
            return;
        };
        if ended(facts) {
            return;
        }
        // A subject is a fact of the attachment, so an execution read first
        // without one takes the one its attachment names.
        let known = placements
            .entry(registration_id.clone())
            .or_insert_with(|| Placed {
                registration_id: registration_id.clone(),
                subject_id: None,
                placement,
            });
        if known.subject_id.is_none() {
            known.subject_id = subject_id.cloned();
        }
    };
    for (registration_id, facts) in log.processes() {
        record(registration_id, facts, None);
    }
    for id in log.ids() {
        if let Some(facts) = log.facts(id) {
            record(
                &facts.attachment.registration_id,
                &facts.execution,
                facts.attachment.subject_id.as_ref(),
            );
        }
    }
    placements.into_values().collect()
}

/// Whether an execution has ended, on the evidence that says so. Being revoked is
/// not one of them.
fn ended(process: &crate::control_plane::events::ProcessFacts) -> bool {
    process.exited_at.is_some()
        || process.ended_externally_at.is_some()
        || process.completion.is_some()
}

fn live(process: &ProcessRecord) -> bool {
    !matches!(process.state, State::Revoked)
        && process.exited_at.is_none()
        && process.ended_externally_at.is_none()
        && process.completion.is_none()
}

pub(crate) fn clear_subject_attachments(registry: &Registry) -> Result<(), String> {
    let directory = registry.subjects().join("identity");
    let metadata = match fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("subject identity store is not a real directory".into());
    }
    for entry in fs::read_dir(&directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let subject_id = path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(SubjectId::parse)
            .ok_or("subject record is filed under a name that is not a subject id")?;
        let mut record = read_subject_record(registry, &subject_id)?
            .ok_or("subject record disappeared during reset")?;
        record.current_claim = None;
        record.last_claim = None;
        write_subject_record(registry, &record)?;
    }
    Ok(())
}

fn write_subject_record(registry: &Registry, record: &SubjectRecord) -> Result<(), String> {
    let directory = registry.subjects().join("identity");
    crate::control_plane::registry::prepare_directory(&directory)?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    write_atomic(
        &directory,
        &subject_record_path(registry, record.subject_id.as_str()),
        &bytes,
        MAX_SUBJECT_BYTES,
    )
}

/// Release authority, not the process or the attempt's recorded facts.
pub fn release_attachment(
    registry: &Registry,
    claim_id: &ClaimId,
    subject_id: Option<&SubjectId>,
) -> Result<(), String> {
    let Some(subject_id) = subject_id else {
        return Ok(());
    };
    let _transition = registry.transition();
    if let Some(mut record) = read_subject_record(registry, subject_id)?
        && record.current_claim.as_ref() == Some(claim_id)
    {
        record.current_claim = None;
        write_subject_record(registry, &record)?;
    }
    Ok(())
}

fn subject_record_path(registry: &Registry, subject_id: &str) -> PathBuf {
    registry
        .subjects()
        .join("identity")
        .join(format!("{subject_id}.json"))
}

/// Durable subjects, including detached identities with no remaining claim.
pub fn subject_records_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<SubjectRecord>, String> {
    subject_ids_after(registry, after, limit)?
        .into_iter()
        .map(|id| {
            read_subject_record(registry, &id)?
                .ok_or_else(|| "subject disappeared during listing".into())
        })
        .collect()
}

/// Subject ids with a recorded identity, ordered, after `after`.
fn subject_ids_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<SubjectId>, String> {
    let directory = registry.subjects().join("identity");
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut ids = BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(SubjectId::parse)
            .ok_or_else(|| format!("{}: not named by a subject id", path.display()))?;
        if after.is_none_or(|cut| id.as_str() > cut) {
            ids.insert(id);
        }
    }
    Ok(ids.into_iter().take(limit).collect())
}

/// Lists one current attachment per subject, ordered by subject id.
pub fn list_subjects_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Claim>, String> {
    let mut claims = Vec::new();
    let mut after = after.map(str::to_owned);
    while claims.len() < limit {
        let page = subject_records_after(registry, after.as_deref(), limit - claims.len())?;
        if page.is_empty() {
            break;
        }
        for record in page {
            after = Some(record.subject_id.to_string());
            if let Some(claim) = current_attachment(registry, &record.subject_id)? {
                claims.push(claim);
            }
        }
    }
    Ok(claims)
}

/// Records what a cancel did about the launch's own pane. The first outcome is
/// durable and is what a later cancel of the same launch reports: nothing is
/// closed twice, and an uncertain cleanup is never retried into a second one.
pub fn record_pane_cleanup(
    registry: &Registry,
    claim_id: &ClaimId,
    outcome: PaneCleanup,
    now_ms: i64,
) -> Result<PaneCleanup, String> {
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, claim_id)?;
    if let Some(recorded) = subject.process.pane_cleanup.clone() {
        return Ok(recorded);
    }
    // A pane is only removed for a launch whose process is gone: revoked before it
    // ever bound, or bound and verified exited. The pane itself is checked where it
    // is closed; this is the launch-side half of the same rule.
    if subject.process.state != State::Revoked && subject.process.exited_at.is_none() {
        return Err("the launch has not ended, so its pane is not cleaned up".into());
    }
    subject.process.pane_cleanup = Some(outcome.clone());
    subject.process.recorded_at = format_millis(now_ms);
    write(registry, &subject)?;
    Ok(outcome)
}

/// Captures the process a launch claim named, spending its correlation.
///
/// Refuses unless the subject is still pending with this correlation, and the
/// process was started with the requested launch. A cancel that won the race
/// revoked the subject; a replayed claim finds the spent correlation.
pub fn capture(
    registry: &Registry,
    claim_id: &ClaimId,
    correlation: &str,
    process: &ProcessIdentity,
    effective: &Effective,
    completion_key: String,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let mut subject = read_claim(registry, claim_id)?.ok_or_else(|| NOT_AWAITING.to_string())?;
    if subject.process.state != State::Pending
        || subject.process.correlation.as_deref() != Some(correlation)
    {
        return Err(NOT_AWAITING.into());
    }
    if !subject.process.launch.launched_as(effective) {
        return Err(LAUNCH_MISMATCH.into());
    }
    subject.process.state = State::Captured;
    subject.process.correlation = None;
    subject.process.identity = Some(process.clone());
    subject.process.completion_key = Some(completion_key);
    subject.process.recorded_at = format_millis(now_ms);
    store_recipe(registry, &subject.process)?;
    write(registry, &subject)
}

/// Decides whether a registration may bind the tracked launch, and records the
/// binding in the subject. Runs under the registry transition lock, before the
/// edge is spent, so the subject is always at least as far along as the edge.
///
/// A launch binds exactly once, to the captured identity; the same registration
/// replaying its bind is answered, and any other presenter is refused.
pub(crate) fn fence_registration(
    registry: &Registry,
    claim_id: &ClaimId,
    process: Option<&ProcessIdentity>,
    registration_id: Option<&RegistrationId>,
    now_ms: i64,
) -> Result<(), String> {
    let mut subject = read_recorded(registry, claim_id)?;
    if registration_id.is_some_and(|id| id != &subject.process.registration_id) {
        return Err("registration does not identify the launched process".into());
    }
    match (subject.process.state, registration_id) {
        (State::Bound, Some(agent))
            if subject.process.bound_registration_id.as_ref() == Some(agent)
                && subject.process.identity.as_ref() == process =>
        {
            Ok(())
        }
        (State::Bound, _) => Err("launch is already bound to another registration".into()),
        (State::Captured, Some(agent))
            if process.is_some() && subject.process.identity.as_ref() == process =>
        {
            subject.process.state = State::Bound;
            subject.process.bound_registration_id = Some(agent.clone());
            subject.process.recorded_at = format_millis(now_ms);
            write(registry, &subject)
        }
        (State::Captured, _) => Err("registration does not match the launched process".into()),
        (State::Pending, _) => Err("launch has not been released to its process".into()),
        (State::Revoked, _) => Err("launch was cancelled".into()),
    }
}

/// Cancels a tracked launch: revokes it while it is unbound, then stops its
/// captured process. A revoked launch never binds afterwards.
pub fn cancel(
    registry: &Registry,
    claim_id: &ClaimId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Termination, String> {
    let revoked = revoke(registry, claim_id, now_ms)?;
    let execution = process_of_claim(registry, claim_id)?.registration_id;
    terminate(
        registry,
        &execution,
        verifier,
        now_ms,
        revoked,
        &Shutdown::new(),
    )
}

/// Stops a tracked launch's captured process. The launch itself is left as it is:
/// a bound launch stays bound, and an unbound one is not revoked.
pub fn stop(
    registry: &Registry,
    claim_id: &ClaimId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Termination, String> {
    let execution = process_of_claim(registry, claim_id)?.registration_id;
    terminate(
        registry,
        &execution,
        verifier,
        now_ms,
        false,
        &Shutdown::new(),
    )
}

/// How long a finished execution is given to report its end before the pane that
/// holds its supervisor is removed.
const COMPLETION_GRACE: Duration = Duration::from_millis(500);

/// Waits, boundedly, for the supervisor's report of how a program ended.
///
/// The report is what carries the code, and the supervisor is what sends it, so a
/// close that removes the pane first can destroy it. The wait never becomes a
/// claim to have one: an absent report is a diagnostic, not a blocker.
pub fn await_completion(registry: &Registry, registration: &RegistrationId) -> Option<Completion> {
    await_completions(registry, std::slice::from_ref(registration));
    process_of_registration(registry, registration)
        .ok()
        .and_then(|process| process.completion)
}

/// Waits, boundedly, for the supervisor's report of every execution in one place.
///
/// One window for the whole set, not one per member: a branch of five agents must
/// not pay five graces before its root is removed, and the reports are all on their
/// way at the same time. Reads the records only: nothing here dispatches.
pub fn await_completions(registry: &Registry, registrations: &[RegistrationId]) {
    let until = std::time::Instant::now() + COMPLETION_GRACE;
    loop {
        let outstanding = registrations.iter().any(|registration| {
            process_of_registration(registry, registration)
                .is_ok_and(|process| process.identity.is_some() && process.completion.is_none())
        });
        if !outstanding || std::time::Instant::now() >= until {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Records a launch's exit if its captured process is verifiably gone, and
/// reports whether it is. A notification calls this; it is never itself proof.
pub fn observe_exit(
    registry: &Registry,
    claim_id: &ClaimId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<bool, String> {
    observe_exit_from(registry, claim_id, verifier, now_ms, Observer::Read)
}

/// How a registration stands against one Pi session. A session has at most one
/// attachment: the live registration whose published session is that session.
#[derive(Clone, PartialEq, Eq)]
pub enum Attachment {
    /// This registration already holds the session's live subject.
    Own(Box<Claim>),
    /// Another registration's live subject holds the session.
    Other(Box<Claim>),
    /// No live registration holds the session; this one may bind it.
    Free,
}

/// Whether a tracked subject is attached to a live process: bound, not verifiably
/// exited, and its registration still publishes the subject's session.
///
/// Only two things release a session: a verified process exit, or the same
/// registration publishing a different session. A lost pane does not release it,
/// because absence of a pane is not proof that the process is gone; a stale
/// heartbeat does not release it either.
pub fn is_attached(registry: &Registry, subject: &Claim, now_ms: i64) -> Result<bool, String> {
    if read_subject_record(registry, &subject.subject_id)?
        .is_none_or(|record| record.current_claim.as_ref() != Some(&subject.claim_id))
    {
        return Ok(false);
    }
    if matches!(subject.process.state, State::Pending | State::Captured)
        && subject.process.exited_at.is_none()
    {
        return Ok(true);
    }
    let (Some(agent), Some(session)) = (
        subject.process.bound_registration_id.as_ref(),
        subject.session.as_deref(),
    ) else {
        return Ok(false);
    };
    if subject.process.state != State::Bound || subject.process.exited_at.is_some() {
        return Ok(false);
    }
    if crate::control_plane::registry::presence::decision(registry, &subject.claim_id)?.is_some_and(
        |claim| claim.standing == crate::control_plane::registry::presence::Standing::Reserved,
    ) {
        return Ok(true);
    }
    if let Some(claim) = crate::control_plane::registry::presence::current(registry, agent)? {
        return Ok(claim.session_id.as_deref() == Some(session)
            && claim.claim_id == subject.claim_id
            && matches!(
                claim.standing,
                crate::control_plane::registry::presence::Standing::Granted
                    | crate::control_plane::registry::presence::Standing::Pending
            ));
    }
    Ok(registry
        .context(agent, now_ms)?
        .and_then(|context| context.context.session)
        .as_deref()
        == Some(session))
}

/// Records the session a launched process reports, once, when its bound subject
/// was launched without one: a daemon-spawned child does not know its session
/// until the process publishes it. A session another live registration holds is
/// not recorded, so the launch stays unattached rather than duplicating it.
pub fn record_launch_session(
    registry: &Registry,
    registration_id: &RegistrationId,
    session: &str,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let mut after: Option<String> = None;
    loop {
        let page = list_after(registry, after.as_deref(), SWEEP_LIMIT)?;
        if let Some(found) = page.iter().find(|subject| {
            subject.process.bound_registration_id.as_ref() == Some(registration_id)
                && subject.process.state == State::Bound
                && subject.process.exited_at.is_none()
                && subject.process.ended_externally_at.is_none()
        }) {
            if found.process.origin != Origin::Spawned
                || registry
                    .spawn_edge(&found.process.spawn_request_id)?
                    .is_some_and(|edge| edge.granted_at.is_some())
            {
                return Ok(());
            }
            if found
                .session
                .as_deref()
                .is_some_and(|expected| expected != session)
            {
                return Err("the reported session does not match the continued subject".into());
            }
            if subject_record_for_session(registry, session)?
                .is_some_and(|record| record.subject_id != found.subject_id)
            {
                return Err(
                    "this session belongs to an existing subject; continue it explicitly".into(),
                );
            }
            if found.session.is_some() {
                return Ok(());
            }
            if !matches!(
                attachment(registry, registration_id, session, now_ms)?,
                Attachment::Other(_)
            ) {
                // The stored record is the one the transition is checked against.
                let mut subject = read_recorded(registry, &found.claim_id)?;
                subject.session = Some(session.to_string());
                subject.process.recorded_at = format_millis(now_ms);
                write(registry, &subject)?;
            }
            return Ok(());
        }
        match page.last() {
            Some(last) if page.len() == SWEEP_LIMIT => after = Some(last.claim_id.to_string()),
            _ => return Ok(()),
        }
    }
}

/// Releases a session whose holding process is verifiably gone.
///
/// A holder is judged by its own process identity, never by its pane: a live
/// process holds its session wherever it runs, including a pane it was moved to
/// or a pane that was closed while the process kept running. Only a process the
/// process table confirms absent, or whose PID now belongs to another process,
/// has exited. Procfs that cannot be read is not evidence of absence, so the
/// holder keeps the session.
///
/// Returns whether the holder was released. Reads procfs, so callers run it
/// outside the registry transition.
pub fn release_if_gone(
    registry: &Registry,
    holder: &Claim,
    verifier: &dyn crate::control_plane::registry::process::ProcessVerifier,
    now_ms: i64,
) -> Result<bool, String> {
    use crate::control_plane::registry::process::{ProcessVerification, verify_identity};
    match verify_identity(holder.process.identity.as_ref(), verifier) {
        ProcessVerification::Absent | ProcessVerification::Mismatched => {
            record_exit(registry, &holder.process, now_ms)?;
            Ok(true)
        }
        ProcessVerification::Verified | ProcessVerification::Unavailable => Ok(false),
    }
}

/// How `registration_id` stands against `session`, read before any verification so a
/// registration that cannot bind costs no process inspection. The session's subject
/// has one authority pointer, so this is the only place that decides who holds it.
pub fn attachment(
    registry: &Registry,
    registration_id: &RegistrationId,
    session: &str,
    now_ms: i64,
) -> Result<Attachment, String> {
    let Some(record) = subject_record_for_session(registry, session)? else {
        return Ok(Attachment::Free);
    };
    let Some(holder) = record
        .current_claim
        .as_ref()
        .map(|claim_id| read_claim(registry, claim_id))
        .transpose()?
        .flatten()
    else {
        return Ok(Attachment::Free);
    };
    if !is_attached(registry, &holder, now_ms)? {
        return Ok(Attachment::Free);
    }
    let process = &holder.process;
    Ok(
        if process.bound_registration_id.as_ref() == Some(registration_id)
            || &process.registration_id == registration_id
        {
            Attachment::Own(Box::new(holder))
        } else {
            Attachment::Other(Box::new(holder))
        },
    )
}

/// Binds a verified, unlaunched process to its registration's current Pi session.
/// Returns the subject that holds the session.
///
/// - The registration already holds the session: its subject, unchanged.
/// - Another live registration holds it: refused; the session has one attachment.
/// - A prior execution of the session is no longer attached (its process exited,
///   or the process moved on): a new execution of that same logical subject, so
///   its children, inbox and managed metadata continue with the session.
/// - Otherwise a new logical subject for the session.
///
/// The decision is made inside the transition, so a lost reply or a concurrent
/// report cannot create a second binding.
pub fn enroll_subject(
    registry: &Registry,
    enrollment_id: &ClaimId,
    registration_id: &RegistrationId,
    process: &ProcessIdentity,
    session: &str,
    launch: LaunchContext,
    now_ms: i64,
) -> Result<ClaimId, String> {
    let _transition = registry.transition();
    let execution = read_process(registry, registration_id)?;
    if execution
        .as_ref()
        .is_some_and(|p| p.identity.as_ref() != Some(process))
    {
        return Err("the registration's process birth identity changed".into());
    }
    match attachment(registry, registration_id, session, now_ms)? {
        Attachment::Other(_) => {
            return Err("the session is already attached to another live process".into());
        }
        Attachment::Own(own) if &own.claim_id == enrollment_id => return Ok(own.claim_id),
        Attachment::Own(_) | Attachment::Free => {}
    }
    // A reattachment continues the session's subject, so its children, inbox and
    // managed metadata continue with it.
    let (logical, managed) = match subject_record_for_session(registry, session)? {
        Some(record) => (Some(record.subject_id), record.managed),
        None => (None, ManagedMetadata::default()),
    };
    // A session with no subject yet gets a new one, minted rather than borrowed
    // from the enrollment's claim.
    let logical = logical.unwrap_or_else(SubjectId::random);
    let execution = execution.unwrap_or_else(|| {
        let mut adopted = draft_process(
            enrollment_id.as_str(),
            launch,
            registration_id.clone(),
            now_ms,
        );
        adopted.origin = Origin::Enrolled;
        adopted.scope = None;
        adopted
    });
    let mut subject = Claim {
        claim_id: enrollment_id.clone(),
        subject_id: logical,
        managed,
        session: Some(session.to_string()),
        process: execution,
    };
    subject.process.state = State::Bound;
    subject.process.correlation = None;
    subject.process.identity = Some(process.clone());
    subject.process.bound_registration_id = Some(registration_id.clone());
    subject.process.recorded_at = format_millis(now_ms);
    write(registry, &subject)?;
    let mut record =
        read_subject_record(registry, &subject.subject_id)?.ok_or("subject is not recorded")?;
    record.current_claim = Some(subject.claim_id.clone());
    record.last_claim = Some(subject.claim_id.clone());
    write_subject_record(registry, &record)?;
    Ok(subject.claim_id)
}

/// Withdraw an attempt that never dispatched after its incumbent could not stop.
/// Pointer restoration is exact: a later claim can never be displaced here.
pub fn cancel_reservation(
    registry: &Registry,
    predecessor: &ClaimId,
    pending: &ClaimId,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let mut next = read_recorded(registry, pending)?;
    if next.process.state != State::Pending || next.process.identity.is_some() {
        return Err(
            "the successor may have dispatched; its reservation cannot be withdrawn".into(),
        );
    }
    let mut record =
        read_subject_record(registry, &next.subject_id)?.ok_or("subject is not recorded")?;
    if record.current_claim.as_ref() != Some(pending) {
        return Err("the subject's claim changed while stopping its previous process".into());
    }
    next.process.state = State::Revoked;
    next.process.correlation = None;
    next.process.recorded_at = format_millis(now_ms);
    write(registry, &next)?;
    record.current_claim = Some(predecessor.clone());
    record.last_claim = Some(predecessor.clone());
    record.updated_at = format_millis(now_ms);
    write_subject_record(registry, &record)
}

/// How long a launch has to present before it is withdrawn. A constant until
/// someone needs to tighten it; it counts from the edge's own recorded time, which
/// a restart of the daemon does not move.
const LAUNCH_DEADLINE_MS: i64 = 30_000;

/// Why a launch is withdrawn, when it is: it never presented and either its
/// deadline passed or it was cancelled first.
///
/// `recorded_at` is the launch's own reservation time. A revoke rewrites the
/// claim's stamp, so a caller that holds an older one measures the deadline from
/// that instead of from a stamp its own withdrawal moved.
fn withdrawal(state: State, recorded_at: &str, now_ms: i64) -> Option<&'static str> {
    let overdue = crate::timestamp::timestamp_millis(recorded_at)
        .is_some_and(|recorded| now_ms - recorded >= LAUNCH_DEADLINE_MS);
    match state {
        State::Revoked if !overdue => Some("the launch was cancelled before it presented"),
        State::Revoked | State::Pending | State::Captured if overdue => {
            Some("the launch did not present within 30 seconds")
        }
        _ => None,
    }
}

/// Revokes a claim, stops what it captured, closes the pane it left and releases
/// it. Answers whether the launch is finished, which is what lets a caller record
/// the withdrawal: a stop that did not settle is retried by the next pass.
fn stop_and_release(
    registry: &Registry,
    claim: &Claim,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    cleanup: &mut impl FnMut(&ClaimId) -> Result<bool, String>,
) -> Result<bool, String> {
    revoke(registry, &claim.claim_id, now_ms)?;
    // A revoked launch captures nothing afterwards, so one with no identity has
    // no process to stop.
    let stopped = match cancel(registry, &claim.claim_id, verifier, now_ms)?.exit {
        Exit::Exited => true,
        Exit::Unresolved => read_claim(registry, &claim.claim_id)?
            .is_some_and(|claim| claim.process.identity.is_none()),
        Exit::Unverified(_) => false,
    };
    if !stopped || !cleanup(&claim.claim_id)? {
        return Ok(false);
    }
    release_attachment(registry, &claim.claim_id, Some(&claim.subject_id))?;
    Ok(true)
}

/// Whether this claim is already withdrawn: only a withdrawal releases a revoked
/// claim from its subject while leaving the claim itself recorded.
fn released(
    registry: &Registry,
    subject_id: Option<&SubjectId>,
    claim_id: &ClaimId,
) -> Result<bool, String> {
    let Some(subject_id) = subject_id else {
        return Ok(false);
    };
    Ok(read_subject_record(registry, subject_id)?
        .is_some_and(|record| record.current_claim.as_ref() != Some(claim_id)))
}

/// One claim, under the presence lock both withdrawal paths hold.
fn withdraw_locked(
    registry: &Registry,
    claim_id: &ClaimId,
    deadline_from: Option<&str>,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    cleanup: &mut impl FnMut(&ClaimId) -> Result<bool, String>,
) -> Result<Option<&'static str>, String> {
    let Some(claim) = read_claim(registry, claim_id)? else {
        return Ok(None);
    };
    if crate::control_plane::registry::presence::attachment(
        registry,
        &claim.process.registration_id,
    )?
    .is_some()
    {
        return Ok(None);
    }
    let basis = deadline_from.unwrap_or(&claim.process.recorded_at);
    let Some(reason) = withdrawal(claim.process.state, basis, now_ms) else {
        return Ok(None);
    };
    if released(registry, Some(&claim.subject_id), claim_id)? {
        return Ok(None);
    }
    if !stop_and_release(registry, &claim, verifier, now_ms, cleanup)? {
        return Ok(None);
    }
    Ok(Some(reason))
}

/// Withdraws a launch that never presented, whether or not a spawn edge asked for
/// it: a recipe launch has no edge, and a claim left pending is a subject row that
/// never resolves.
///
/// Answers the reason it was withdrawn, so a caller holding an edge can record it.
///
/// Presence takes this same lock through grant, so a stale pass never revokes a
/// launch that granted while its page was being read.
pub fn withdraw_unclaimed(
    registry: &Registry,
    claim_id: &ClaimId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    mut cleanup: impl FnMut(&ClaimId) -> Result<bool, String>,
) -> Result<Option<&'static str>, String> {
    let _presence = registry.presence.lock().unwrap_or_else(|e| e.into_inner());
    withdraw_locked(registry, claim_id, None, verifier, now_ms, &mut cleanup)
}

/// The claims a withdrawal pass considers: every reservation whose deadline
/// passed and which nothing attached. Paged like the exit sweep, so a fleet larger
/// than one page is covered over successive passes.
pub fn unclaimed(
    registry: &Registry,
    after: Option<&str>,
    now_ms: i64,
) -> (Vec<ClaimId>, Option<String>) {
    let ids: Vec<String> = {
        let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
        log.ids()
            .filter(|claim| {
                log.facts(claim).is_some_and(|facts| {
                    withdrawal(facts.execution.state, &facts.execution.recorded_at, now_ms)
                        .is_some()
                        && !released(registry, facts.attachment.subject_id.as_ref(), claim)
                            .unwrap_or(false)
                })
            })
            .map(ToString::to_string)
            .collect()
    };
    let (page, next) = crate::control_plane::store::page_after(&ids, after, SWEEP_LIMIT);
    (
        page.iter().filter_map(|id| ClaimId::parse(id)).collect(),
        next,
    )
}

/// Withdraws a launch whose process never presented and marks its edge, so a
/// reader sees an outcome instead of an absence. The edge is marked only once the
/// process is verifiably gone, so the next pass retries a stop that did not
/// settle. Returns the claim when the edge was marked, for the caller to close the
/// pane the launch left.
pub fn withdraw_unpresented(
    registry: &Registry,
    edge: &crate::control_plane::registry::SpawnEdge,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    mut cleanup: impl FnMut(&ClaimId) -> Result<bool, String>,
) -> Result<Option<ClaimId>, String> {
    // A page read before a grant must not mark an edge that has since granted.
    let _presence = registry.presence.lock().unwrap_or_else(|e| e.into_inner());
    let Some(edge) = registry.spawn_edge(&edge.request_id)? else {
        return Ok(None);
    };
    let (Some(claim_id), true) = (edge.claim_id.clone(), edge.granted_at.is_none()) else {
        return Ok(None);
    };
    if edge.withdrawn.is_some() {
        return Ok(None);
    }
    let Some(reason) = withdraw_locked(
        registry,
        &claim_id,
        Some(&edge.recorded_at),
        verifier,
        now_ms,
        &mut cleanup,
    )?
    else {
        return Ok(None);
    };
    registry.record_spawn_withdrawn(&edge.request_id, reason, now_ms)?;
    Ok(Some(claim_id))
}

/// Pi's value-taking flags that a restart keeps, from `pi --help`. Any other flag
/// is treated as boolean, so a bare argument after it refuses rather than replays.
const VALUE_FLAGS: &[&str] = &[
    "--provider",
    "--model",
    "--api-key",
    "--system-prompt",
    "--append-system-prompt",
    "--mode",
    "--session-dir",
    "--name",
    "-n",
    "--models",
    "--tools",
    "-t",
    "--exclude-tools",
    "-xt",
    "--thinking",
    "--extension",
    "-e",
    "--skill",
    "--prompt-template",
    "--theme",
    "--use-theme",
    "--export",
    "--tui-mode",
    "--fff-mode",
    "--fff-frecency-db",
    "--fff-history-db",
    "--fff-profile",
];
/// Flags whose value selects the session a process starts in. A restart replaces them.
const SESSION_VALUE_FLAGS: &[&str] = &["--session", "--session-id", "--fork"];
/// Flags that select a session without a value, which a restart replaces.
const SESSION_FLAGS: &[&str] = &["--continue", "-c", "--resume", "-r", "--no-session"];

/// The launch that resumes `session`: the captured argv with its prior session
/// selectors removed and `--session <session>` appended.
///
/// Fail-closed: a bare argument that is not a known flag's value may be a
/// message, which a restart would replay on every relaunch, so it refuses. An
/// unknown valued flag refuses for the same reason rather than being guessed at.
pub fn resume_launch(launch: &LaunchContext, session: &str) -> Result<LaunchContext, String> {
    let mut argv = Vec::new();
    let mut args = launch.argv.iter();
    while let Some(arg) = args.next() {
        if SESSION_FLAGS.contains(&arg.as_str()) {
            continue;
        }
        if SESSION_VALUE_FLAGS.contains(&arg.as_str()) {
            args.next();
            continue;
        }
        if SESSION_VALUE_FLAGS.iter().any(|flag| {
            arg.strip_prefix(flag)
                .is_some_and(|rest| rest.starts_with('='))
        }) {
            continue;
        }
        if !arg.starts_with('-') {
            return Err(
                "the captured launch has a bare argument that may be a message; restart refuses it"
                    .into(),
            );
        }
        argv.push(arg.clone());
        if VALUE_FLAGS.contains(&arg.as_str()) {
            let value = args
                .next()
                .ok_or("the captured launch has a flag with no value")?;
            argv.push(value.clone());
        }
    }
    argv.push("--session".to_string());
    argv.push(session.to_string());
    Ok(LaunchContext {
        executable: launch.executable.clone(),
        argv,
        cwd: launch.cwd.clone(),
        env: launch.env.clone(),
    })
}

/// The launch a running process was started with, from its verified kernel record.
/// Radar's own launch and token variables are dropped, since a replacement is
/// given fresh ones.
pub fn captured_launch(effective: &Effective) -> Result<LaunchContext, String> {
    let (executable, argv) = effective
        .cmdline
        .split_first()
        .ok_or("the process has no recorded command line")?;
    let cwd = match &effective.cwd {
        Some(path) => Some(
            path.to_str()
                .ok_or("the working directory is not text")?
                .to_string(),
        ),
        None => None,
    };
    let env = effective
        .env
        .iter()
        .filter(|(name, _)| {
            ![
                CLAIM_ENV,
                CORRELATION_ENV,
                MANAGED_PARENT_SUBJECT_ENV,
                crate::runtime::LAUNCH_TOKEN_ENV,
            ]
            .contains(&name.as_str())
        })
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    Ok(LaunchContext {
        executable: executable.clone(),
        argv: argv.to_vec(),
        cwd,
        env,
    })
}

/// The public projection of a subject: identities, binding and recorded facts.
/// It carries no launch data and no correlation, which are private.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicClaim {
    pub subject_id: crate::identity::SubjectId,
    pub claim_id: crate::identity::ClaimId,
    pub registration_id: crate::identity::RegistrationId,
    pub managed: ManagedMetadata,
    pub spawn_request_id: String,
    pub state: State,
    pub process: Option<ProcessIdentity>,
    pub observation: Option<Observation>,
    pub pane_absence: Option<PaneAbsence>,
    pub ended_externally_at: Option<String>,
    pub ended_evidence: Option<EndedEvidence>,
    pub pane_cleanup: Option<PaneCleanup>,
    pub completion: Option<Completion>,
    pub bound_registration_id: Option<crate::identity::RegistrationId>,
    pub signalled_at: Option<String>,
    pub exited_at: Option<String>,
    pub recorded_at: String,
    pub origin: Origin,
    pub session: Option<String>,
}

pub fn public_view(subject: &Claim) -> PublicClaim {
    PublicClaim {
        subject_id: subject.subject_id.clone(),
        claim_id: subject.claim_id.clone(),
        registration_id: subject.process.registration_id.clone(),
        managed: subject.managed.clone(),
        spawn_request_id: subject.process.spawn_request_id.clone(),
        state: subject.process.state,
        process: subject.process.identity.clone(),
        observation: subject.process.observation.clone(),
        pane_absence: subject.process.pane_absence.clone(),
        ended_externally_at: subject.process.ended_externally_at.clone(),
        ended_evidence: subject.process.ended_evidence.clone(),
        pane_cleanup: subject.process.pane_cleanup.clone(),
        completion: subject.process.completion.clone(),
        bound_registration_id: subject.process.bound_registration_id.clone(),
        signalled_at: subject.process.signalled_at.clone(),
        exited_at: subject.process.exited_at.clone(),
        recorded_at: subject.process.recorded_at.clone(),
        origin: subject.process.origin,
        session: subject.session.clone(),
    }
}

impl From<PublicClaim> for crate::registry_observation::LifecycleSubject {
    fn from(view: PublicClaim) -> Self {
        Self {
            subject_id: view.subject_id,
            claim_id: Some(view.claim_id),
            spawn_request_id: Some(view.spawn_request_id),
            bound_registration_id: view.bound_registration_id,
            process: view.process,
            state: view.state,
            managed: view.managed,
            session: view.session,
            exited_at: view.exited_at,
            ended_externally_at: view.ended_externally_at,
        }
    }
}

/// Subjects in id order after `after`, at most `limit`, for a bounded listing.
/// Only names are sorted; each returned record is read and validated.
pub fn list_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Claim>, String> {
    let entries = match fs::read_dir(registry.subjects()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("launch subjects: {error}")),
    };
    let mut ids: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .map(str::to_string)
        })
        .filter(|id| SessionUuid::parse(id).is_some())
        .filter(|id| after.is_none_or(|after| id.as_str() > after))
        .collect();
    ids.extend(
        registry
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .ids()
            .filter(|id| after.is_none_or(|after| id.as_str() > after))
            .map(ToString::to_string),
    );
    ids.sort();
    ids.dedup();
    ids.truncate(limit);
    ids.iter()
        .map(|id| {
            let claim = ClaimId::parse(id).ok_or("a recorded claim is not named by a claim id")?;
            read_recorded(registry, &claim)
        })
        .collect()
}

/// Most subject records one recovery pass examines. A pass is bounded so a long
/// history cannot hold the registry's transition lock for an unbounded time.
pub const SWEEP_LIMIT: usize = 256;

/// Re-observes recorded launches whose exit notification may have been missed,
/// including ones whose daemon restarted while the process ran or exited.
///
/// Each launch is judged only by its verified birth identity, through the same
/// path as a wake. Nothing is signalled, relaunched or repeated here.
///
/// Paging is fair rather than a sorted prefix: the pass continues after the last
/// execution it examined and reports where to resume, so a fleet larger than one
/// page is covered over successive passes instead of only the first page. Returns
/// how many examined launches are verified exited, including ones already
/// recorded, and the cursor the next pass continues from — `None` when the pass
/// reached the end of the listing.
pub fn sweep(
    registry: &Registry,
    verifier: &dyn ProcessVerifier,
    after: Option<&str>,
    now_ms: i64,
) -> Result<(usize, Option<String>), String> {
    let mut ids: Vec<String> = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .ids()
        .map(ToString::to_string)
        .collect();
    for entry in fs::read_dir(registry.subjects()).map_err(|e| format!("launch subjects: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if let Some(id) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            && SessionUuid::parse(id).is_some()
        {
            ids.push(id.to_owned());
        }
    }
    ids.sort();
    ids.dedup();
    let (page, next) = crate::control_plane::store::page_after(&ids, after, SWEEP_LIMIT);
    let mut exited = 0;
    for id in page.iter().filter_map(|id| ClaimId::parse(id)) {
        if observe_exit_from(registry, &id, verifier, now_ms, Observer::Sweep) == Ok(true) {
            exited += 1;
        }
    }
    Ok((exited, next))
}

/// Revokes a claim that never bound. Answers whether one was still open.
///
/// A reservation whose launch was refused will never present, and a claim left
/// pending draws a lifecycle row that nothing resolves.
pub(crate) fn revoke(registry: &Registry, claim_id: &ClaimId, now_ms: i64) -> Result<bool, String> {
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, claim_id)?;
    if !matches!(subject.process.state, State::Pending | State::Captured) {
        return Ok(false);
    }
    subject.process.state = State::Revoked;
    subject.process.correlation = None;
    subject.process.recorded_at = format_millis(now_ms);
    write(registry, &subject)?;
    Ok(true)
}

/// Sends SIGTERM to the captured process through its pidfd, or to the scope that
/// holds it, if one was verified.
fn send_term(
    descriptor: Option<&std::os::fd::OwnedFd>,
    scope: Option<&crate::managed_scope::VerifiedScope>,
    shutdown: &Shutdown,
) -> Result<(), String> {
    match scope {
        Some(scope) => crate::managed_scope::stop_until(&[scope], shutdown.graceful_until),
        None => process_control::terminate(descriptor.ok_or("no process descriptor is available")?)
            .map_err(|error| error.to_string()),
    }
}

/// What escalating one execution established.
struct Escalated {
    /// The scope was killed and nothing is left inside it: nothing that ran there
    /// is running now.
    emptied: bool,
    /// Why the escalation itself did not happen or did not finish.
    failure: Option<String>,
}

/// Escalates one execution that outlived its graceful window.
///
/// This is the only place a shutdown turns into SIGKILL, so the deadline that
/// calls it today and the operator request that will call it later cannot drift
/// apart. Only a scope verified at signal time is escalated: a process fenced by
/// its own identity but held in an unreadable scope has no group to kill, and
/// killing by name pattern would reach work Radar never claimed.
fn escalate(scope: &crate::managed_scope::VerifiedScope, shutdown: &Shutdown) -> Escalated {
    if let Err(error) = escalate_scopes(&[scope], shutdown) {
        return Escalated {
            emptied: false,
            failure: Some(format!("scope escalation failed: {error}")),
        };
    }
    await_escalation(scope, shutdown)
}

fn escalate_scopes(
    scopes: &[&crate::managed_scope::VerifiedScope],
    shutdown: &Shutdown,
) -> Result<(), String> {
    crate::managed_scope::kill_until(scopes, shutdown.hard_until)
}

fn await_escalation(scope: &crate::managed_scope::VerifiedScope, shutdown: &Shutdown) -> Escalated {
    // One bounded wait for the group to empty. The manager kills every member it
    // still holds, so the cgroup is the evidence, not the pidfd of the process
    // that started it.
    let mut remaining = match scope.remaining() {
        Ok(remaining) => remaining,
        Err(error) => {
            return Escalated {
                emptied: false,
                failure: Some(format!("scope membership is unresolved: {error}")),
            };
        }
    };
    while !remaining.is_empty() && !shutdown.hard().is_zero() {
        std::thread::sleep(Duration::from_millis(20));
        remaining = match scope.remaining() {
            Ok(remaining) => remaining,
            Err(error) => {
                return Escalated {
                    emptied: false,
                    failure: Some(format!("scope membership is unresolved: {error}")),
                };
            }
        };
    }
    if remaining.is_empty() {
        return Escalated {
            emptied: true,
            failure: None,
        };
    }
    Escalated {
        emptied: false,
        failure: Some(format!("still running: {}", remaining.join(", "))),
    }
}

/// Waits out the shared window for one signalled execution, escalates a verified
/// scope that outlives it, and records the exit it establishes.
///
/// Only a scope verified at signal time is escalated, and only after the graceful
/// part of the window ran out. A process fenced by its own identity but held in an
/// unreadable scope is reported as still running: nothing about it is inferred.
fn await_end(
    registry: &Registry,
    process: &ProcessRecord,
    descriptor: Option<&std::os::fd::OwnedFd>,
    scope: Option<&crate::managed_scope::VerifiedScope>,
    note: Option<&str>,
    shutdown: &Shutdown,
    now_ms: i64,
) -> Result<(Exit, bool), String> {
    let exited = descriptor
        .is_none_or(|descriptor| process_control::exited_within(descriptor, shutdown.graceful()));
    let survivors = match scope {
        Some(scope) => scope.remaining()?,
        None => Vec::new(),
    };
    if exited && survivors.is_empty() {
        record_exit(registry, process, now_ms)?;
        return Ok((Exit::Exited, false));
    }
    let Some(scope) = scope else {
        return Ok((still_running(Some(process), note), false));
    };
    let escalated = escalate(scope, shutdown);
    if escalated.emptied {
        // An emptied, verified group is the evidence that nothing which ran there
        // still runs — including when the pidfd of the process that started it
        // never reported, which is what a zombie or an unreaped child looks like.
        record_exit(registry, process, now_ms)?;
        return Ok((Exit::Exited, true));
    }
    Ok((
        Exit::Unverified(
            escalated
                .failure
                .unwrap_or_else(|| "the escalation did not finish".into()),
        ),
        true,
    ))
}

/// What a signalled execution that outlived the window is reported as, named by
/// the kernel's own word for it rather than by anything the daemon stores.
fn still_running(process: Option<&ProcessRecord>, note: Option<&str>) -> Exit {
    // The name comes from the launch's own record, not from `/proc`: this text
    // names what the daemon could not end, and reading a pid's current command
    // would report whatever holds that pid now.
    let name = process
        .map(|process| {
            Path::new(&process.launch.executable)
                .file_name()
                .map_or_else(
                    || process.launch.executable.clone(),
                    |name| name.to_string_lossy().into_owned(),
                )
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "process".to_string());
    match note {
        Some(note) => Exit::Unverified(format!("still running: {name} ({note})")),
        None => Exit::Unverified(format!("still running: {name}")),
    }
}

/// Ends one execution: TERM under the shared window, escalation of a verified
/// scope, and the exit it establishes.
fn terminate(
    registry: &Registry,
    execution: &RegistrationId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    revoked: bool,
    shutdown: &Shutdown,
) -> Result<Termination, String> {
    let (descriptor, signal, scope, note) =
        match stage(registry, execution, verifier, now_ms, shutdown)? {
            Stage::Settled(exit) => {
                return Ok(Termination {
                    revoked,
                    signal_attempted: false,
                    escalated: false,
                    exit,
                });
            }
            Stage::Watch {
                descriptor,
                signal,
                scope,
                scope_note,
            } => (descriptor, signal, scope, scope_note),
        };
    // The record the exit is written against, read once the signal intent is
    // known: `stage` already established that the identity has not moved.
    let process = process_of_registration(registry, execution)?;
    let mut signal_attempted = false;
    if signal {
        signal_attempted = true;
        if let Err(error) = send_term(descriptor.as_ref(), scope.as_ref(), shutdown) {
            return Ok(Termination {
                revoked,
                signal_attempted,
                escalated: false,
                exit: Exit::Unverified(format!("SIGTERM could not be sent: {error}")),
            });
        }
    }
    let (exit, escalated) = await_end(
        registry,
        &process,
        descriptor.as_ref(),
        scope.as_ref(),
        note.as_deref(),
        shutdown,
        now_ms,
    )?;
    Ok(Termination {
        revoked,
        signal_attempted,
        escalated,
        exit,
    })
}

/// The locked half of a stop: the signal intent, and the descriptor to wait on.
/// Settles without waiting when there is no captured process to signal.
fn stage(
    registry: &Registry,
    execution: &RegistrationId,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    shutdown: &Shutdown,
) -> Result<Stage, String> {
    let process = process_of_registration(registry, execution)?;
    let Some(identity) = process.identity.as_ref() else {
        return Ok(Stage::Settled(Exit::Unresolved));
    };
    if process.exited_at.is_some() {
        return Ok(Stage::Settled(Exit::Exited));
    }
    let acquired = process_control::acquire(identity, verifier);
    let descriptor = match acquired {
        Ok(Acquired::Process(fd)) => Some(fd),
        Ok(Acquired::Absent | Acquired::Replaced) => None,
        Err(_) => return Ok(Stage::Settled(Exit::Unresolved)),
    };
    let (scope, scope_note) = match process.scope.clone() {
        Some(scope) => match if descriptor.is_some() {
            scope.verify_until(
                process.registration_id.as_str(),
                identity,
                verifier,
                shutdown.graceful_until,
            )
        } else {
            scope.verify_unit_until(process.registration_id.as_str(), shutdown.graceful_until)
        } {
            Ok(scope) => (Some(scope), None),
            // The process is fenced by its own identity, so an unreadable scope
            // does not withhold a TERM; it withholds escalation, and it is named
            // in the outcome so the uncertainty is not silent.
            Err(message) => (None, Some(message)),
        },
        None => (None, None),
    };
    if descriptor.is_none()
        && scope
            .as_ref()
            .is_none_or(|scope| scope.remaining().is_ok_and(|left| left.is_empty()))
    {
        record_exit(registry, &process, now_ms)?;
        return Ok(Stage::Settled(Exit::Exited));
    }
    let _transition = registry.transition();
    let current = process_of_registration(registry, execution)?;
    if current.identity != process.identity {
        return Ok(Stage::Settled(Exit::Unresolved));
    }
    if current.exited_at.is_some() {
        return Ok(Stage::Settled(Exit::Exited));
    }
    let signal = current.signalled_at.is_none();
    if signal {
        record_process_transition(
            registry,
            &current.registration_id,
            crate::control_plane::events::Source::ProcessControl,
            crate::control_plane::events::Transition::SignalRequested,
            now_ms,
        )?;
    }
    Ok(Stage::Watch {
        descriptor,
        signal,
        scope,
        scope_note,
    })
}

/// The outcome of cancelling several executions under one window.
pub struct Cancelled {
    /// One entry per execution the batch addressed, in the order it addressed
    /// them, each with its own outcome.
    pub ended: Vec<EndedExecution>,
    pub escalated: bool,
}

impl Cancelled {
    /// Every execution whose exit is not verified, which is every execution this
    /// close may not claim ended. A process that could not be signalled is one of
    /// these: it is not known to be gone.
    pub fn survivors(&self) -> impl Iterator<Item = &EndedExecution> {
        self.ended
            .iter()
            .filter(|ended| !matches!(ended.end, ExecutionEnd::Exited))
    }
}

/// One execution a batch addresses: the process incarnation to stop, and the agent
/// it runs as when one is recorded.
pub struct ExecutionRef {
    pub registration_id: RegistrationId,
    pub subject_id: Option<SubjectId>,
}

/// Cancels every named execution together: each is fenced, signalled once, and
/// then the whole set is waited on under a single deadline.
///
/// One window rather than one per execution is what makes ending a place holding
/// several agents bounded: the operator waits one window, not a series of them.
pub fn cancel_many(
    registry: &Registry,
    targets: &[ExecutionRef],
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Cancelled {
    let shutdown = Shutdown::new();
    let mut ended: Vec<EndedExecution> = Vec::new();
    let mut watching = Vec::new();
    for target in targets {
        match stage(
            registry,
            &target.registration_id,
            verifier,
            now_ms,
            &shutdown,
        ) {
            Ok(Stage::Settled(exit)) => ended.push(ended_execution(registry, target, exit)),
            // Only a verified exit is an exit. An unresolved or unverified verdict
            // says the daemon could not establish one, so the agent is a survivor:
            // recording an exit here would authorise closing the place over it.
            Ok(Stage::Watch {
                descriptor,
                signal,
                scope,
                scope_note,
            }) => watching.push((target, descriptor, signal, scope, scope_note)),
            Err(error) => {
                tracing::warn!(%error, execution = %target.registration_id, "the stop could not be staged");
                ended.push(unverified_execution(registry, target, error));
            }
        }
    }
    // Every member is signalled before any of them is waited on: one manager
    // transaction for the scopes, one pidfd signal for each bare process.
    let scopes: Vec<_> = watching
        .iter()
        .filter(|(_, _, signal, _, _)| *signal)
        .filter_map(|(_, _, _, scope, _)| scope.as_ref())
        .collect();
    // Whether each member exited is judged from evidence below, so a failed stop
    // is reported but never decides the outcome.
    if let Err(error) = crate::managed_scope::stop_until(&scopes, shutdown.graceful_until) {
        tracing::warn!(%error, "scope stop failed");
    }
    let mut signalled = Vec::new();
    for (target, descriptor, signal, scope, note) in watching {
        if signal
            && scope.is_none()
            && descriptor
                .as_ref()
                .is_none_or(|descriptor| process_control::terminate(descriptor).is_err())
        {
            ended.push(unverified_execution(registry, target, UNRESPONSIVE.into()));
            continue;
        }
        signalled.push((target, descriptor, scope, note));
    }
    let mut pending = Vec::new();
    for (target, descriptor, scope, note) in signalled {
        let gone = descriptor.as_ref().is_none_or(|descriptor| {
            process_control::exited_within(descriptor, shutdown.graceful())
        });
        let empty = scope.as_ref().map_or(Ok(true), |scope| {
            scope.remaining().map(|left| left.is_empty())
        });
        match empty {
            Ok(true) if gone => match record_exit_of_registration(registry, target, now_ms) {
                Ok(()) => ended.push(ended_execution(registry, target, Exit::Exited)),
                Err(error) => ended.push(unverified_execution(registry, target, error)),
            },
            Err(error) => {
                tracing::warn!(%error, execution = %target.registration_id, "scope membership is unresolved");
                ended.push(unverified_execution(registry, target, error));
            }
            _ => pending.push((target, scope, note)),
        }
    }
    let scopes: Vec<_> = pending
        .iter()
        .filter_map(|(_, scope, _)| scope.as_ref())
        .collect();
    let escalated = !scopes.is_empty();
    // Dispatch the entire escalation before waiting on any one group.
    let dispatch = escalate_scopes(&scopes, &shutdown);
    for (target, scope, note) in pending {
        let Some(scope) = scope else {
            let exit = still_running(
                process_of_registration(registry, &target.registration_id)
                    .ok()
                    .as_ref(),
                note.as_deref(),
            );
            tracing::warn!(?exit, execution = %target.registration_id, "execution exit is unverified");
            ended.push(ended_execution(registry, target, exit));
            continue;
        };
        let end = await_escalation(&scope, &shutdown);
        if end.emptied && record_exit_of_registration(registry, target, now_ms).is_ok() {
            ended.push(ended_execution(registry, target, Exit::Exited));
        } else {
            tracing::warn!(failure = ?end.failure, dispatch = ?dispatch, execution = %target.registration_id, "escalation exit is unverified");
            ended.push(ended_execution(
                registry,
                target,
                Exit::Unverified(UNRESPONSIVE.into()),
            ));
        }
    }
    Cancelled { ended, escalated }
}

/// What a shutdown reports when a process outlived the window and cannot be
/// asked anything further.
const UNRESPONSIVE: &str = "the process outlived the shutdown window";

/// The execution a batch addressed, with what the records say about it.
fn ended_execution(registry: &Registry, target: &ExecutionRef, exit: Exit) -> EndedExecution {
    EndedExecution {
        subject_id: target.subject_id.clone(),
        registration_id: target.registration_id.clone(),
        end: ExecutionEnd::from(&exit),
        completion: read_process(registry, &target.registration_id)
            .ok()
            .flatten()
            .and_then(|process| process.completion),
    }
}

/// A verdict this batch could not establish, and why.
fn unverified_execution(
    registry: &Registry,
    target: &ExecutionRef,
    reason: String,
) -> EndedExecution {
    ended_execution(registry, target, Exit::Unverified(reason))
}

/// Records the exit of the execution a batch addressed.
fn record_exit_of_registration(
    registry: &Registry,
    target: &ExecutionRef,
    now_ms: i64,
) -> Result<(), String> {
    let process = process_of_registration(registry, &target.registration_id)?;
    record_exit(registry, &process, now_ms)
}

/// The execution a claim addresses, through the attempt it belongs to.
fn process_of_claim(registry: &Registry, claim: &ClaimId) -> Result<ProcessRecord, String> {
    Ok(read_recorded(registry, claim)?.process)
}

/// The execution one process incarnation is, without needing its attempt.
fn process_of_registration(
    registry: &Registry,
    registration: &RegistrationId,
) -> Result<ProcessRecord, String> {
    read_process(registry, registration)?.ok_or_else(|| "the execution is not recorded".into())
}

fn record_process_transition(
    registry: &Registry,
    registration_id: &RegistrationId,
    source: crate::control_plane::events::Source,
    transition: crate::control_plane::events::Transition,
    now_ms: i64,
) -> Result<(), String> {
    registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .append(
            crate::control_plane::events::Stream::Process(registration_id),
            &format_millis(now_ms),
            source,
            transition,
        )
}

fn record_exit(registry: &Registry, process: &ProcessRecord, now_ms: i64) -> Result<(), String> {
    let _transition = registry.transition();
    // Another observer may have recorded the exit since this record was read, and
    // a second `ExitVerified` is an invalid transition rather than a no-op.
    let recorded = if process.exited_at.is_some() {
        true
    } else {
        read_process(registry, &process.registration_id)?
            .is_some_and(|current| current.exited_at.is_some())
    };
    if !recorded {
        record_process_transition(
            registry,
            &process.registration_id,
            crate::control_plane::events::Source::ProcessControl,
            crate::control_plane::events::Transition::ExitVerified,
            now_ms,
        )?;
    }
    Ok(())
}

fn record_exit_locked(registry: &Registry, subject: &mut Claim, now_ms: i64) -> Result<(), String> {
    if subject.process.exited_at.is_none() {
        subject.process.exited_at = Some(format_millis(now_ms));
        subject.process.recorded_at = format_millis(now_ms);
        write(registry, subject)?;
    }
    Ok(())
}

/// The command a tracked launch runs: `radar run --launch`, which claims the
/// recorded launch and receives its program from the daemon. The caller's cwd and
/// environment overlay are passed through, with the private correlation added.
pub fn wrap(
    bootstrap: &Bootstrap,
    ticket: &Ticket,
    target: LaunchRequest,
) -> Result<LaunchRequest, String> {
    let LaunchRequest {
        pane_id,
        spawn_token,
        cwd,
        mut env,
        ..
    } = target;
    env.insert(CLAIM_ENV.to_string(), ticket.claim_id.to_string());
    env.insert(CORRELATION_ENV.to_string(), ticket.correlation.clone());
    if let Some(parent_subject_id) = &ticket.parent_subject_id {
        env.insert(
            MANAGED_PARENT_SUBJECT_ENV.to_string(),
            parent_subject_id.to_string(),
        );
    }
    // A launch that could not enter its scope would run nothing, so it is refused
    // here, before a pane is typed into.
    crate::managed_scope::Scope::for_registration(ticket.registration_id.as_str())?
        .wrap("", &[])?;
    Ok(LaunchRequest {
        pane_id,
        executable: bootstrap.executable.clone(),
        argv: vec![
            "run".to_string(),
            "--launch".to_string(),
            "--socket".to_string(),
            bootstrap.socket.clone(),
        ],
        spawn_token,
        cwd,
        env,
    })
}

/// The shell command a pane-death hook runs for one launch. It names the launch
/// and the server generation the pane was created in; it is a hint, not evidence.
pub fn wake_command(
    bootstrap: &Bootstrap,
    subject_id: &str,
    server: &ProcessIdentity,
) -> Result<String, String> {
    // The paths are single-quoted for the shell, so a quote in either would end
    // the quoting early.
    if [&bootstrap.executable, &bootstrap.socket]
        .iter()
        .any(|path| path.contains('\''))
    {
        return Err("the daemon's paths cannot be quoted for a pane-death hook".into());
    }
    Ok(format!(
        "'{}' {WAKE_COMMAND} --socket '{}' --subject {subject_id} --server {}:{}:{}",
        bootstrap.executable, bootstrap.socket, server.pid, server.start_ticks, server.boot_id
    ))
}

/// Runs as the wake a pane-death hook starts, with the arguments the command
/// line already parsed and checked.
///
/// It asks the daemon to reconcile the launch and reports nothing: the hook has no
/// reader, and a wake that never arrives is recovered by the next read.
pub fn run_wake(socket: &str, subject: &str, server: &str) -> i32 {
    let Some(server) = parse_server_identity(server) else {
        eprintln!("radar {WAKE_COMMAND}: malformed server identity");
        return 2;
    };
    match exchange(
        Path::new(socket),
        WAKE_METHOD,
        json!({"subject_id": subject, "server": server}),
    ) {
        Ok(_) => 0,
        Err(message) => {
            eprintln!("radar {WAKE_COMMAND}: {message}");
            1
        }
    }
}

/// `pid:start_ticks:boot_id`, the generation a pane was created in.
fn parse_server_identity(server: &str) -> Option<ProcessIdentity> {
    let mut fields = server.splitn(3, ':');
    let pid = fields.next()?.parse().ok()?;
    let start_ticks = fields.next()?.parse().ok()?;
    Some(ProcessIdentity {
        boot_id: fields.next()?.to_string(),
        pid,
        start_ticks,
    })
}

/// Runs `radar run --launch --socket <path>` inside a tracked pane: claims the
/// recorded launch, then becomes its program inside the registration's scope.
///
/// Returns only on failure. An unclaimed launch never runs its program.
pub fn run_launch(socket: &str) -> i32 {
    let Ok(key) = std::env::var(SUPERVISION_ENV) else {
        return supervise(socket);
    };
    let claimed = match claim(Path::new(socket), key) {
        Ok(claimed) => claimed,
        Err(message) => {
            eprintln!("radar run: {message}");
            return 111;
        }
    };
    enter_scope(
        claimed.registration_id.as_str(),
        &claimed.executable,
        &claimed.argv,
        ProcessKind::Agent,
        true,
    )
}

/// What passes a supervisor's key to the process it supervises. The process
/// removes it before the program runs, so the program never holds the key.
const SUPERVISION_ENV: &str = "RADAR_RUN_SUPERVISION";

/// Runs `radar run -- COMMAND [ARG...]`. This process supervises: it runs the
/// admitted launch as its child and reports how the program ended, because only
/// a parent can learn that, and the pane's shell is not the program's parent.
pub fn run_managed(socket: &str, kind: ProcessKind, program: &[String]) -> i32 {
    match std::env::var(SUPERVISION_ENV) {
        Ok(key) => admit_and_enter(socket, kind, program, key),
        Err(_) => supervise(socket),
    }
}

/// Runs this same invocation as a child and reports its end to the daemon.
///
/// The child is the one admitted and the one that becomes the program, so the
/// identity the daemon records is the program's own. A terminal interrupt reaches
/// both: the supervisor ignores it and carries on to the report.
fn supervise(socket: &str) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    let key = random_uuid();
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => {
            eprintln!("radar run: {error}");
            return 1;
        }
    };
    let interrupted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGQUIT] {
        if let Err(error) = signal_hook::flag::register(signal, interrupted.clone()) {
            eprintln!("radar run: {error}");
            return 1;
        }
    }
    let mut child = match Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(SUPERVISION_ENV, &key)
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("radar run: {error}");
            return 1;
        }
    };
    let process = LocalProcfsVerifier
        .inspect(child.id() as i32)
        .ok()
        .flatten();
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("radar run: {error}");
            return 1;
        }
    };
    if let Some(process) = process {
        // A launch refused before admission has no record to report to; neither
        // that nor an unreachable daemon changes how the program's end is shown.
        if let Err(error) = exchange(
            Path::new(socket),
            COMPLETE_METHOD,
            json!({
                "process": process,
                "key": key,
                "code": status.code(),
                "signal": status.signal(),
            }),
        ) {
            eprintln!("radar run: the program's end was not recorded: {error}");
        }
    }
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

/// Admits the command in this verified pane, then becomes it inside the
/// execution's scope.
///
/// Returns only on failure. The program is never executed directly: the scope
/// wrapper is what runs, so a scope that cannot be created runs nothing.
fn admit_and_enter(socket: &str, kind: ProcessKind, program: &[String], key: String) -> i32 {
    if program.is_empty() {
        eprintln!("radar run: no command to run");
        return 2;
    }
    let Ok(pane) = std::env::var("TMUX_PANE") else {
        eprintln!("radar run: this is not a tmux pane");
        return 2;
    };
    let Some(process) = LocalProcfsVerifier
        .inspect(std::process::id() as i32)
        .ok()
        .flatten()
    else {
        eprintln!("radar run: this process has no identity to admit");
        return 2;
    };
    let cwd = std::env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());
    let answer = match exchange(
        Path::new(socket),
        ADMIT_METHOD,
        json!({
            "request_id": random_uuid(),
            "pane": pane,
            "process": process,
            "executable": program[0],
            "argv": &program[1..],
            "cwd": cwd,
            "kind": kind,
            "completion_key": key,
        }),
    ) {
        Ok(answer) => answer,
        Err(message) => {
            eprintln!("radar run: {message}");
            return 1;
        }
    };
    let Ok(Admitted { registration_id }) = serde_json::from_value(answer) else {
        eprintln!("radar run: the daemon admitted the launch without a registration");
        return 1;
    };
    enter_scope(
        registration_id.as_str(),
        &program[0],
        &program[1..],
        kind,
        false,
    )
}

/// Becomes `executable argv` inside the registration's scope, so the group exists
/// before the program runs and a scope that cannot be created runs nothing. Only
/// an agent is given its registration: a command's children must never inherit an
/// identity they could present. A claimed launch also drops its launch variables.
fn enter_scope(
    registration_id: &str,
    executable: &str,
    argv: &[String],
    kind: ProcessKind,
    claimed: bool,
) -> i32 {
    let scope = match crate::managed_scope::Scope::for_registration(registration_id) {
        Ok(scope) => scope,
        Err(message) => {
            eprintln!("radar run: {message}");
            return 1;
        }
    };
    let (launcher, argv) = match scope.wrap(executable, argv) {
        Ok(wrapped) => wrapped,
        Err(message) => {
            eprintln!("radar run: {message}");
            return 1;
        }
    };
    let mut launch = Command::new(launcher);
    launch.args(&argv).env_remove(SUPERVISION_ENV);
    if claimed {
        launch.env_remove(CLAIM_ENV).env_remove(CORRELATION_ENV);
    }
    match kind {
        ProcessKind::Agent => {
            launch.env(crate::runtime::LAUNCH_TOKEN_ENV, registration_id);
        }
        ProcessKind::Command => {
            launch.env_remove(crate::runtime::LAUNCH_TOKEN_ENV);
        }
    }
    let error = launch.exec();
    eprintln!("radar run: could not enter the execution scope: {error}");
    127
}

fn claim(socket: &Path, completion_key: String) -> Result<Claimed, String> {
    let claim_id = std::env::var(CLAIM_ENV).map_err(|_| "no launch claim".to_string())?;
    let correlation =
        std::env::var(CORRELATION_ENV).map_err(|_| "no launch correlation".to_string())?;
    let process = LocalProcfsVerifier
        .inspect(std::process::id() as i32)?
        .ok_or_else(|| "this process has no identity to claim".to_string())?;
    let result = exchange(
        socket,
        CLAIM_METHOD,
        json!({
            "claim_id": claim_id,
            "correlation": correlation,
            "process": process,
            "completion_key": completion_key,
        }),
    )?;
    serde_json::from_value(result).map_err(|_| "launch was not released".to_string())
}

/// One request on the daemon socket, answered with its result. A refusal is an
/// error: its text is the daemon's, and it never carries a correlation.
fn exchange(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| format!("daemon socket is unavailable: {error}"))?;
    stream
        .set_read_timeout(Some(RELEASE_DEADLINE))
        .and_then(|()| stream.set_write_timeout(Some(RELEASE_DEADLINE)))
        .map_err(|error| error.to_string())?;
    let id = random_uuid();
    let request = json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    });
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|error| format!("could not send the request: {error}"))?;
    let mut line = String::new();
    BufReader::new((&stream).take(MAX_LINE_BYTES as u64))
        .read_line(&mut line)
        .map_err(|error| format!("no answer from the daemon: {error}"))?;
    let response: crate::control_plane::protocol::Response =
        serde_json::from_str(&line).map_err(|_| "the daemon's answer is unreadable".to_string())?;
    match response {
        crate::control_plane::protocol::Response::Result {
            id: answered,
            result,
        } if answered == id => Ok(result),
        _ => Err("the daemon refused the request".into()),
    }
}

fn write(registry: &Registry, subject: &Claim) -> Result<(), String> {
    write_attempt(registry, subject)?;
    note_attempt(registry, subject)
}

/// Replaces the private recipe, which is otherwise written once at creation. A
/// fact only the claiming process can supply, like its supervisor's key, is
/// stored here before the transition that depends on it is recorded.
fn store_recipe(registry: &Registry, process: &ProcessRecord) -> Result<(), String> {
    let directory = registry.subjects().join("private");
    let bytes =
        serde_json::to_vec_pretty(process).map_err(|_| "cannot encode private process recipe")?;
    write_atomic(
        &directory,
        &recipe_path(registry, process.registration_id.as_str()),
        &bytes,
        MAX_SUBJECT_BYTES,
    )
}

fn ensure_process(
    registry: &Registry,
    log: &mut crate::control_plane::events::Log,
    process: &ProcessRecord,
) -> Result<(), String> {
    if log.process(&process.registration_id).is_some() {
        return Ok(());
    }
    let directory = registry.subjects().join("private");
    crate::control_plane::registry::prepare_directory(&directory)?;
    let recipe = recipe_path(registry, process.registration_id.as_str());
    if !recipe.try_exists().map_err(|e| e.to_string())? {
        let bytes = serde_json::to_vec_pretty(process)
            .map_err(|_| "cannot encode private process recipe")?;
        write_atomic(&directory, &recipe, &bytes, MAX_SUBJECT_BYTES)?;
    }
    log.append(
        crate::control_plane::events::Stream::Process(&process.registration_id),
        &process.recorded_at,
        if process.origin != Origin::Enrolled {
            crate::control_plane::events::Source::Launch
        } else {
            crate::control_plane::events::Source::Registration
        },
        crate::control_plane::events::Transition::ProcessCreated {
            registration_id: process.registration_id.clone(),
            facts: crate::control_plane::events::ProcessFacts::from(process),
        },
    )
}

fn write_attempt(registry: &Registry, subject: &Claim) -> Result<(), String> {
    use crate::control_plane::events::{Facts, Source, Stream, Transition};
    let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    ensure_process(registry, &mut log, &subject.process)?;
    let previous = match log.facts(&subject.claim_id) {
        Some(facts) => facts,
        None => {
            let facts = Facts::from_subject(subject);
            let (source, transition) = match subject.process.origin {
                Origin::Spawned => (Source::Launch, Transition::Created(facts)),
                Origin::Enrolled => (Source::Registration, Transition::Enrolled(facts)),
                Origin::Admitted => return Err("an admitted process has no subject".into()),
            };
            return log.append(
                Stream::Claim(&subject.claim_id),
                &subject.process.recorded_at,
                source,
                transition,
            );
        }
    };
    let target = Facts::from_subject(subject);
    if previous == target {
        return Ok(());
    }
    let (source, transition) = if previous.execution.state != target.execution.state {
        match target.execution.state {
            State::Captured => (
                Source::Bootstrap,
                Transition::Captured(
                    target
                        .execution
                        .process
                        .clone()
                        .ok_or("capture has no process")?,
                ),
            ),
            State::Bound => (
                Source::Registration,
                Transition::Bound {
                    registration_id: target
                        .execution
                        .bound_registration_id
                        .clone()
                        .ok_or("binding has no agent")?,
                },
            ),
            State::Revoked => (Source::ProcessControl, Transition::Revoked),
            State::Pending => return Err("cannot return a subject to pending".into()),
        }
    } else if previous.execution.observation != target.execution.observation {
        (
            Source::ProcessObservation,
            Transition::Observed(
                target
                    .execution
                    .observation
                    .clone()
                    .ok_or("cannot remove a process observation")?,
            ),
        )
    } else if previous.execution.exited_at != target.execution.exited_at {
        (Source::ProcessObservation, Transition::ExitVerified)
    } else if previous.execution.completion != target.execution.completion {
        (
            Source::Launch,
            Transition::Completed(
                target
                    .execution
                    .completion
                    .clone()
                    .ok_or("cannot remove a completion")?,
            ),
        )
    } else if previous.execution.pane_cleanup != target.execution.pane_cleanup {
        (
            Source::PaneControl,
            Transition::PaneCleanup(
                target
                    .execution
                    .pane_cleanup
                    .clone()
                    .ok_or("cannot remove a pane cleanup outcome")?,
            ),
        )
    } else if previous.execution.signalled_at != target.execution.signalled_at {
        (Source::ProcessControl, Transition::SignalRequested)
    } else if previous.execution.ended_externally_at != target.execution.ended_externally_at {
        (
            Source::ProcessObservation,
            Transition::EndedExternally(
                target
                    .execution
                    .ended_evidence
                    .clone()
                    .ok_or("an ended-externally outcome needs its evidence")?,
            ),
        )
    } else if previous.execution.pane_absence != target.execution.pane_absence {
        (
            Source::ProcessObservation,
            Transition::AbsencePending(target.execution.pane_absence.clone()),
        )
    } else if previous.attachment.session.is_none() && target.attachment.session.is_some() {
        (
            Source::Registration,
            Transition::SessionRecorded {
                session: target
                    .attachment
                    .session
                    .clone()
                    .ok_or("a recorded session is named")?,
            },
        )
    } else {
        return Err("unsupported lifecycle transition".into());
    };
    log.append_checked(
        &subject.claim_id,
        &subject.process.recorded_at,
        source,
        transition,
        &target,
    )
}

fn recipe_path(registry: &Registry, subject_id: &str) -> PathBuf {
    registry
        .subjects()
        .join("private")
        .join(format!("{subject_id}.json"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    const SPAWN: &str = "5b2e8c41-0a7d-4f36-b1d9-6c3a7e5f8b1d";

    /// A state root under the temp directory, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("radar-lifecycle-{label}-{}", random_uuid()));
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&path)
                .expect("scratch root");
            Self(path)
        }

        /// Opens the registry the way a daemon restart does.
        fn registry(&self) -> Registry {
            Registry::open(&self.0).expect("registry")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A verifier for a launch that has no captured process: any inspection is a
    /// failure of the claim that nothing was signalled.
    struct Uninspected;

    impl ProcessVerifier for Uninspected {
        fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
            panic!("an uncaptured launch must not inspect any process")
        }
    }

    /// A child that is killed if the test panics before it is waited for.
    struct Child(std::process::Child);

    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn launch() -> LaunchContext {
        LaunchContext {
            executable: "/usr/bin/pi".into(),
            argv: vec!["--resume".into()],
            cwd: None,
            env: BTreeMap::from([("RADAR_TEST_MARK".to_string(), "value".to_string())]),
        }
    }

    /// The kernel's view of the launch chain a pane was started with.
    fn started_as(launch: &LaunchContext) -> Effective {
        Effective {
            cmdline: ["radar", "run", "--launch", "--socket", "/run/radar.sock"]
                .map(String::from)
                .to_vec(),
            cwd: None,
            env: launch.env.clone(),
        }
    }

    fn identity(pid: i32) -> ProcessIdentity {
        ProcessIdentity {
            boot_id: "boot".into(),
            pid,
            start_ticks: 42,
        }
    }

    /// A verifier whose identity cannot be read at all.
    struct Unreadable;

    impl ProcessVerifier for Unreadable {
        fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
            Err("the process identity cannot be read".into())
        }
    }

    #[test]
    fn a_launch_cancelled_before_capture_is_never_released() {
        let scratch = Scratch::new("before-capture");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = &ticket.claim_id;

        let cancelled = cancel(&registry, subject, &Uninspected, 2).expect("cancel");
        assert_eq!(
            cancelled,
            Termination {
                revoked: true,
                escalated: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
        let claim = capture(
            &registry,
            subject,
            &ticket.correlation,
            &identity(7),
            &started_as(&launch()),
            random_uuid(),
            3,
        );
        assert_eq!(claim, Err(NOT_AWAITING.to_string()));

        // Revocation is durable: a reopened registry still refuses every bind.
        let reopened = scratch.registry();
        assert_eq!(
            read_claim(&reopened, subject)
                .expect("read")
                .expect("subject")
                .process
                .state,
            State::Revoked
        );
        assert_eq!(
            fence_registration(
                &reopened,
                subject,
                Some(&identity(7)),
                Some(&ticket.registration_id),
                4
            ),
            Err("launch was cancelled".to_string())
        );
        // A repeated cancel revokes nothing further and signals nothing.
        let again = cancel(&reopened, subject, &Uninspected, 5).expect("cancel");
        assert_eq!(
            again,
            Termination {
                revoked: false,
                escalated: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
    }

    #[test]
    fn accepted_managed_launch_metadata_is_durable_and_inherited_by_successors() {
        let scratch = Scratch::new("managed-metadata");
        let registry = scratch.registry();
        let metadata = ManagedMetadata {
            owner: Some("extension".into()),
            run: Some("run-17".into()),
            label: Some("reviewer".into()),
            role: Some("worker".into()),
            parent_subject_id: SubjectId::parse("01a10c77-8a6b-7035-8a0e-b1fa607bb507"),
        };
        let first = create_with_intent(&registry, SPAWN, launch(), None, None, metadata.clone(), 1)
            .expect("accepted launch intent");
        assert_eq!(first.parent_subject_id, metadata.parent_subject_id);
        let persisted = read_claim(&registry, &first.claim_id)
            .expect("read subject")
            .expect("subject");
        assert_eq!(persisted.managed, metadata);
        assert_eq!(
            json!(public_view(&persisted))["managed"]["owner"],
            "extension"
        );

        let successor = create_with_intent(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((first.subject_id.clone(), first.claim_id.clone())),
            persisted.managed.clone(),
            2,
        )
        .expect("successor intent");
        assert_eq!(successor.parent_subject_id, metadata.parent_subject_id);
        assert_eq!(
            read_claim(&registry, &successor.claim_id)
                .expect("read successor")
                .expect("successor")
                .managed,
            metadata
        );
        let invalid = ManagedMetadata {
            label: Some("x".repeat(257)),
            ..ManagedMetadata::default()
        };
        assert!(
            create_with_intent(&registry, &random_uuid(), launch(), None, None, invalid, 3,)
                .is_err()
        );
    }

    #[test]
    fn a_reserved_session_is_held_by_its_own_process_and_by_no_other() {
        let scratch = Scratch::new("reserved-holder");
        let registry = scratch.registry();
        let session = random_uuid();
        let first = create_with_intent(
            &registry,
            SPAWN,
            launch(),
            Some(session.clone()),
            None,
            ManagedMetadata::default(),
            1,
        )
        .expect("reservation");
        assert!(matches!(
            attachment(&registry, &first.registration_id, &session, 2).expect("own"),
            Attachment::Own(_)
        ));
        assert!(matches!(
            attachment(&registry, &RegistrationId::random(), &session, 2).expect("other"),
            Attachment::Other(_)
        ));
    }

    #[test]
    fn a_subject_resolves_to_its_current_attachment() {
        let scratch = Scratch::new("logical-current");
        let registry = scratch.registry();
        let first = create(&registry, SPAWN, launch(), None, 1).expect("first execution");
        let logical_id = first.subject_id.clone();
        let second = create_with_identity(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((logical_id.clone(), first.claim_id.clone())),
            2,
        )
        .expect("successor execution");

        let current = current_attachment(&registry, &logical_id)
            .expect("read logical subject")
            .expect("current execution");
        assert_eq!(current.claim_id, second.claim_id);
        assert_eq!(current.subject_id, logical_id);
        // The agent carries the plain name; the attempt is named by its claim.
        let view = json!(public_view(&current));
        assert_eq!(view["subject_id"], logical_id.as_str());
        assert_eq!(view["claim_id"], second.claim_id.as_str());
        assert!(view.get("execution_id").is_none(), "{view}");

        let listed = list_subjects_after(&registry, None, 10).expect("subject list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].claim_id, second.claim_id);
    }

    #[test]
    fn an_admitted_agent_binds_only_its_own_launched_process() {
        let scratch = Scratch::new("admitted-bind");
        let registry = scratch.registry();
        let identity = ProcessIdentity {
            boot_id: "boot".into(),
            pid: 4242,
            start_ticks: 7,
        };
        let process = create_process(
            &registry,
            &random_uuid(),
            launch(),
            Admission {
                kind: ProcessKind::Agent,
                identity: identity.clone(),
                completion_key: random_uuid(),
                placement: None,
            },
            1,
        )
        .expect("admitted process");
        let admitted = process.registration_id.clone();
        let presenting =
            |identity: ProcessIdentity| crate::control_plane::registry::RegistrationRequest {
                source: "pi".into(),
                incarnation: random_uuid(),
                session: None,
                location: None,
                process: Some(identity),
                owner: None,
                run: None,
                label: None,
                launch: None,
                spawn_token: Some(admitted.to_string()),
            };

        let registration = registry
            .register(&presenting(identity.clone()), 2)
            .expect("presence binds the admitted registration");
        assert_eq!(registration.registration_id, admitted);

        let other = ProcessIdentity {
            pid: 4243,
            ..identity
        };
        assert!(
            registry.register(&presenting(other), 3).is_err(),
            "a different process cannot bind the admitted registration"
        );
    }

    #[test]
    fn a_process_recipe_of_another_version_fails_verification_naming_its_file() {
        let scratch = Scratch::new("verify");
        let registry = scratch.registry();
        let process = create_process(
            &registry,
            &random_uuid(),
            launch(),
            Admission {
                kind: ProcessKind::Agent,
                identity: ProcessIdentity {
                    boot_id: "boot".into(),
                    pid: 4242,
                    start_ticks: 7,
                },
                completion_key: random_uuid(),
                placement: None,
            },
            1,
        )
        .expect("admitted process");
        verify(&registry).expect("a record this build wrote verifies");

        let path = recipe_path(&registry, process.registration_id.as_str());
        let mut recipe: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("recipe")).expect("json");
        recipe["version"] = (SUBJECT_VERSION - 1).into();
        fs::write(&path, serde_json::to_vec(&recipe).expect("json")).expect("old recipe");
        let refusal = verify(&registry).expect_err("an old recipe refuses");
        assert!(
            refusal.contains(process.registration_id.as_str()),
            "{refusal}"
        );
    }

    #[test]
    fn a_completion_is_recorded_once_for_the_supervisor_that_holds_the_key() {
        let scratch = Scratch::new("completion");
        let registry = scratch.registry();
        let identity = ProcessIdentity {
            boot_id: "boot".into(),
            pid: 4242,
            start_ticks: 7,
        };
        let key = random_uuid();
        let process = create_process(
            &registry,
            &random_uuid(),
            launch(),
            Admission {
                kind: ProcessKind::Agent,
                identity: identity.clone(),
                completion_key: key.clone(),
                placement: None,
            },
            1,
        )
        .expect("admitted process");
        let recorded = |registry: &Registry| {
            read_process(registry, &process.registration_id)
                .expect("read")
                .expect("process")
                .completion
        };

        assert!(
            record_completion(&registry, &identity, "not-the-key", Some(0), None, 2).is_err(),
            "a report without the supervisor's key records nothing"
        );
        let other = ProcessIdentity {
            pid: 4243,
            ..identity.clone()
        };
        assert!(record_completion(&registry, &other, &key, Some(0), None, 2).is_err());
        assert_eq!(recorded(&registry), None);

        record_completion(&registry, &identity, &key, Some(7), None, 3).expect("report");
        record_completion(&registry, &identity, &key, None, Some(9), 4).expect("replay");
        let completion = recorded(&scratch.registry()).expect("completion survives a reopen");
        assert_eq!((completion.code, completion.signal), (Some(7), None));
        assert_eq!(
            await_completion(&registry, &process.registration_id),
            Some(completion),
            "removal must find a report even when the execution has no subject"
        );
    }

    fn placed_at(registry: &Registry, pane: &str) -> Option<Placed> {
        placements(registry)
            .into_iter()
            .find(|placed| placed.placement.pane() == Some(pane))
    }

    #[test]
    fn an_execution_is_found_by_the_pane_it_was_placed_in_until_it_ends() {
        let scratch = Scratch::new("placement");
        let registry = scratch.registry();
        let identity = ProcessIdentity {
            boot_id: "boot".into(),
            pid: 4242,
            start_ticks: 7,
        };
        let key = random_uuid();
        let process = create_process(
            &registry,
            &random_uuid(),
            launch(),
            Admission {
                kind: ProcessKind::Agent,
                identity: identity.clone(),
                completion_key: key.clone(),
                placement: None,
            },
            1,
        )
        .expect("admitted process");
        assert_eq!(
            process.placement, None,
            "an execution is placed where it runs, not where it was admitted from"
        );
        let placement = Placement {
            location: crate::control_plane::registry::RegistryLocation {
                backend: "fake".into(),
                instance: Some("server".into()),
                workspace: Some("wA".into()),
                tab: Some("wA:t1".into()),
                pane: Some("wA:p1".into()),
            },
            instance: Some(ProcessIdentity {
                pid: 900,
                ..identity.clone()
            }),
        };
        place(&registry, &process.registration_id, placement.clone(), 2).expect("placed");
        assert_eq!(
            read_process(&scratch.registry(), &process.registration_id)
                .expect("read")
                .expect("process")
                .placement,
            Some(placement),
            "placement survives a reopen"
        );
        let found = placed_at(&registry, "wA:p1").expect("the execution placed there");
        assert_eq!(found.registration_id, process.registration_id);
        assert_eq!(
            found.subject_id, None,
            "an admitted execution has no subject"
        );

        // A pane that names no execution, and an execution that has ended, are
        // both no answer: placement is where something runs now.
        assert!(placed_at(&registry, "wA:p2").is_none());
        record_completion(&registry, &identity, &key, Some(0), None, 3).expect("report");
        assert!(
            placed_at(&registry, "wA:p1").is_none(),
            "an execution that ended is not running in its old pane"
        );
    }

    #[test]
    fn a_placement_without_a_pane_or_of_an_ended_execution_is_refused() {
        let scratch = Scratch::new("placement-shape");
        let registry = scratch.registry();
        let identity = ProcessIdentity {
            boot_id: "boot".into(),
            pid: 4242,
            start_ticks: 7,
        };
        let key = random_uuid();
        let process = create_process(
            &registry,
            &random_uuid(),
            launch(),
            Admission {
                kind: ProcessKind::Agent,
                identity: identity.clone(),
                completion_key: key.clone(),
                placement: None,
            },
            1,
        )
        .expect("admitted process");
        let placement = |pane: Option<&str>| Placement {
            location: crate::control_plane::registry::RegistryLocation {
                backend: "fake".into(),
                instance: None,
                workspace: None,
                tab: None,
                pane: pane.map(str::to_owned),
            },
            instance: None,
        };
        assert!(place(&registry, &process.registration_id, placement(None), 2).is_err());
        assert!(
            place(
                &registry,
                &RegistrationId::parse("00000000-0000-4000-8000-000000000999").expect("id"),
                placement(Some("wA:p1")),
                2
            )
            .is_err(),
            "an execution that is not recorded is not placed"
        );
        assert!(
            place(
                &registry,
                &process.registration_id,
                placement(Some("wA:p1")),
                2
            )
            .is_ok()
        );
        record_completion(&registry, &identity, &key, Some(0), None, 3).expect("report");
        assert!(
            place(
                &registry,
                &process.registration_id,
                placement(Some("wA:p1")),
                4
            )
            .is_err(),
            "an ended execution runs nowhere"
        );
    }

    #[test]
    fn an_enrollment_of_a_new_session_mints_a_subject_of_its_own() {
        let scratch = Scratch::new("enroll-mints");
        let registry = scratch.registry();
        let claim_id = ClaimId::random();
        let process = ProcessIdentity {
            boot_id: "test-boot".into(),
            pid: 1,
            start_ticks: 1,
        };
        enroll_subject(
            &registry,
            &claim_id,
            &RegistrationId::random(),
            &process,
            &random_uuid(),
            launch(),
            1,
        )
        .expect("enrolled");

        let claim = read_claim(&registry, &claim_id)
            .expect("read claim")
            .expect("claim");
        assert_ne!(claim.subject_id.as_str(), claim_id.as_str());
        let record = read_subject_record(&registry, &claim.subject_id)
            .expect("read subject")
            .expect("the minted subject is recorded");
        assert_eq!(record.current_claim.as_ref(), Some(&claim_id));
    }

    #[test]
    fn a_new_claim_after_reset_reuses_the_subject_for_its_session() {
        let scratch = Scratch::new("reset-session-reattach");
        let registry = scratch.registry();
        let subject_id = SubjectId::parse("00000000-0000-4000-8000-000000000101").expect("id");
        let parent_id = "00000000-0000-4000-8000-000000000102";
        let session = random_uuid();
        write_subject_record(
            &registry,
            &SubjectRecord {
                version: SUBJECT_VERSION,
                subject_id: subject_id.clone(),
                managed: ManagedMetadata {
                    parent_subject_id: SubjectId::parse(parent_id),
                    ..ManagedMetadata::default()
                },
                session: Some(session.clone()),
                current_claim: None,
                last_claim: None,
                recorded_at: format_millis(1),
                updated_at: format_millis(1),
            },
        )
        .expect("retained subject record");

        let claim_id = ClaimId::random();
        let registration_id = RegistrationId::random();
        let process = ProcessIdentity {
            boot_id: "test-boot".into(),
            pid: 1,
            start_ticks: 1,
        };
        enroll_subject(
            &registry,
            &claim_id,
            &registration_id,
            &process,
            &session,
            launch(),
            2,
        )
        .expect("fresh attachment");

        let claim = read_claim(&registry, &claim_id)
            .expect("read claim")
            .expect("new claim");
        assert_eq!(claim.subject_id, subject_id);
        assert_eq!(
            claim
                .managed
                .parent_subject_id
                .as_ref()
                .map(SubjectId::as_str),
            Some(parent_id)
        );
    }

    #[test]
    fn a_subject_record_names_the_current_attempt_and_keeps_the_subject_across_it() {
        let scratch = Scratch::new("subject-record");
        let registry = scratch.registry();
        let first = create(&registry, SPAWN, launch(), None, 1).expect("first attempt");
        let logical_id = first.subject_id.clone();

        let record = read_subject_record(&registry, &logical_id)
            .expect("read record")
            .expect("the attempt created its subject");
        assert_eq!(record.subject_id, logical_id);
        assert_eq!(record.current_claim.as_ref(), Some(&first.claim_id));

        // A successor moves the pointer, and the subject itself is unchanged.
        let second = create_with_identity(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((logical_id.clone(), first.claim_id.clone())),
            2,
        )
        .expect("successor attempt");
        let record = read_subject_record(&registry, &logical_id)
            .expect("read record")
            .expect("the subject still exists");
        assert_eq!(record.current_claim.as_ref(), Some(&second.claim_id));

        // An earlier attempt reporting late does not displace the current one.
        let earlier = read_claim(&registry, &first.claim_id)
            .expect("read")
            .expect("attempt");
        note_attempt(&registry, &earlier).expect("note the earlier attempt");
        let record = read_subject_record(&registry, &logical_id)
            .expect("read record")
            .expect("subject");
        assert_eq!(record.current_claim.as_ref(), Some(&second.claim_id));
    }

    #[test]
    fn a_claim_keeps_its_attempt_facts_out_of_the_subject_record() {
        let scratch = Scratch::new("claim-facts");
        let registry = scratch.registry();
        let claim = create(&registry, SPAWN, launch(), None, 1).expect("claim");
        let subject_id = claim.subject_id.clone();

        // The attempt's facts survive a daemon restart on the attempt itself.
        let reopened = scratch.registry();
        let read_back = read_claim(&reopened, &claim.claim_id)
            .expect("read")
            .expect("claim");
        assert_eq!(read_back.process.registration_id, claim.registration_id);
        assert_ne!(claim.registration_id.as_str(), claim.claim_id.as_str());
        // A subject invented for a reservation is its own identity, not its claim.
        assert_ne!(claim.subject_id.as_str(), claim.claim_id.as_str());
        let reserved =
            crate::control_plane::registry::presence::current(&reopened, &claim.registration_id)
                .expect("reservation read")
                .expect("reservation");
        assert_eq!(
            reserved.standing,
            crate::control_plane::registry::presence::Standing::Reserved
        );
        assert!(
            crate::control_plane::registry::presence::attachment(
                &reopened,
                &claim.registration_id,
            )
            .expect("authority read")
            .is_none(),
            "a reservation grants no attachment authority"
        );

        let record = read_subject_record(&reopened, &subject_id)
            .expect("read record")
            .expect("subject");
        assert_eq!(record.current_claim.as_ref(), Some(&claim.claim_id));
        let stored = fs::read_to_string(subject_record_path(&reopened, subject_id.as_str()))
            .expect("record file");
        for attempt_only in ["successor", "signalled_at", "exited_at", "pane_cleanup"] {
            assert!(
                !stored.contains(attempt_only),
                "{attempt_only} belongs to the claim, not the subject: {stored}"
            );
        }
    }

    #[test]
    fn a_subject_admits_one_successor_at_a_time() {
        let scratch = Scratch::new("claim-exclusive");
        let registry = scratch.registry();
        let first = create(&registry, SPAWN, launch(), None, 1).expect("first claim");
        let subject_id = first.subject_id.clone();
        let reserved = create_with_identity(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((subject_id.clone(), first.claim_id.clone())),
            2,
        )
        .expect("successor reserves the predecessor");
        assert!(
            create_with_identity(
                &registry,
                &random_uuid(),
                launch(),
                None,
                Some((subject_id.clone(), first.claim_id.clone())),
                3,
            )
            .is_err(),
            "a rival launch must not pre-mint a second successor"
        );
        let record = read_subject_record(&registry, &subject_id)
            .expect("read record")
            .expect("subject");
        assert_eq!(record.current_claim.as_ref(), Some(&reserved.claim_id));
    }

    #[test]
    fn an_undispatched_restart_reservation_restores_its_incumbent() {
        let scratch = Scratch::new("reservation-cancel");
        let registry = scratch.registry();
        let first = create(&registry, SPAWN, launch(), None, 1).expect("incumbent");
        let request = random_uuid();
        let pending = create_with_identity(
            &registry,
            &request,
            launch(),
            None,
            Some((first.subject_id.clone(), first.claim_id.clone())),
            2,
        )
        .expect("reservation");
        cancel_reservation(&registry, &first.claim_id, &pending.claim_id, 3).expect("withdraw");
        let reopened = scratch.registry();
        let current = current_attachment(&reopened, &first.subject_id)
            .unwrap()
            .unwrap();
        assert_eq!(current.claim_id, first.claim_id);
        assert_eq!(
            read_claim(&reopened, &pending.claim_id)
                .unwrap()
                .unwrap()
                .process
                .state,
            State::Revoked
        );
        create_with_identity(
            &reopened,
            &random_uuid(),
            launch(),
            None,
            Some((first.subject_id, first.claim_id)),
            4,
        )
        .expect("a subsequent restart can reserve");
    }

    #[test]
    fn releasing_a_claim_keeps_the_attempt_and_detaches_the_subject() {
        let scratch = Scratch::new("claim-release");
        let registry = scratch.registry();
        let claim = create(&registry, SPAWN, launch(), None, 1).expect("claim");
        let subject_id = claim.subject_id.clone();
        let pending = read_claim(&registry, &claim.claim_id)
            .expect("read")
            .expect("claim");
        assert!(is_attached(&registry, &pending, 2).expect("attached"));

        release_attachment(&registry, &claim.claim_id, Some(&subject_id)).expect("release");

        let attempt = read_claim(&registry, &claim.claim_id)
            .expect("read")
            .expect("the attempt stays readable");
        assert_eq!(attempt.process.state, State::Pending);
        assert!(!is_attached(&registry, &attempt, 2).expect("attached"));
        let record = read_subject_record(&registry, &subject_id)
            .expect("read record")
            .expect("subject");
        assert_eq!(record.current_claim, None);
    }

    #[test]
    fn a_captured_claim_is_spent_and_survives_reopen() {
        let scratch = Scratch::new("spent");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = &ticket.claim_id;
        capture(
            &registry,
            subject,
            &ticket.correlation,
            &identity(7),
            &started_as(&launch()),
            random_uuid(),
            2,
        )
        .expect("capture");

        let reopened = scratch.registry();
        assert_eq!(
            capture(
                &reopened,
                subject,
                &ticket.correlation,
                &identity(7),
                &started_as(&launch()),
                random_uuid(),
                3,
            ),
            Err(NOT_AWAITING.to_string()),
            "a replayed correlation is spent"
        );
        fence_registration(
            &reopened,
            subject,
            Some(&identity(7)),
            Some(&ticket.registration_id),
            4,
        )
        .expect("the captured identity binds");
        let prior_session = random_uuid();
        let prior = create(
            &reopened,
            &random_uuid(),
            launch(),
            Some(prior_session.clone()),
            3,
        )
        .expect("an existing subject");
        release_attachment(&reopened, &prior.claim_id, Some(&prior.subject_id)).unwrap();
        assert!(
            record_launch_session(&reopened, &ticket.registration_id, &prior_session, 4)
                .expect_err("a fresh launch cannot adopt an existing subject's session")
                .contains("continue it explicitly")
        );
        let session = random_uuid();
        record_launch_session(&reopened, &ticket.registration_id, &session, 4)
            .expect("record session");
        assert!(
            record_launch_session(&reopened, &ticket.registration_id, &random_uuid(), 4)
                .expect_err("first presence must match the recorded session")
                .contains("does not match")
        );
        crate::control_plane::registry::presence::begin(
            &reopened,
            &ticket.registration_id,
            &ClaimId::random(),
            &session,
            None,
        )
        .expect("pending report");
        let reserved = read_claim(&reopened, subject)
            .expect("read")
            .expect("launch");
        assert!(
            is_attached(&reopened, &reserved, 4).expect("reservation held"),
            "an unverified report must not free the launch's reserved attachment"
        );
        assert_eq!(
            fence_registration(
                &reopened,
                subject,
                Some(&identity(8)),
                Some(&ticket.registration_id),
                5
            ),
            Err("launch is already bound to another registration".to_string())
        );
    }

    #[test]
    fn a_process_not_started_with_the_requested_launch_is_not_captured() {
        let scratch = Scratch::new("mismatch");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = &ticket.claim_id;

        let mut altered = started_as(&launch());
        altered
            .env
            .insert("RADAR_TEST_MARK".into(), "changed".into());
        assert_eq!(
            capture(
                &registry,
                subject,
                &ticket.correlation,
                &identity(7),
                &altered,
                random_uuid(),
                2
            ),
            Err(LAUNCH_MISMATCH.to_string())
        );
        let still_pending = read_claim(&registry, subject)
            .expect("read")
            .expect("subject");
        assert_eq!(still_pending.process.state, State::Pending);
        assert_eq!(
            still_pending.process.correlation.as_deref(),
            Some(ticket.correlation.as_str())
        );
    }

    #[test]
    fn a_registration_bound_before_cancel_stops_exactly_its_process() {
        let scratch = Scratch::new("bound-first");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        unscoped(&registry, &ticket);
        let subject = &ticket.claim_id;
        let mut child = Child(Command::new("sleep").arg("300").spawn().expect("sleep"));
        let pid = child.0.id() as i32;
        let process = LocalProcfsVerifier
            .inspect(pid)
            .expect("procfs")
            .expect("the child is alive");
        capture(
            &registry,
            subject,
            &ticket.correlation,
            &process,
            &started_as(&launch()),
            random_uuid(),
            2,
        )
        .expect("capture");
        fence_registration(
            &registry,
            subject,
            Some(&process),
            Some(&ticket.registration_id),
            3,
        )
        .expect("bind");

        let cancelled = cancel(&registry, subject, &LocalProcfsVerifier, 4).expect("cancel");
        assert_eq!(
            cancelled,
            Termination {
                revoked: false,
                escalated: false,
                signal_attempted: true,
                exit: Exit::Exited,
            }
        );
        let status = child.0.wait().expect("reaped");
        assert_eq!(
            status.signal(),
            Some(rustix::process::Signal::TERM.as_raw())
        );
        assert_eq!(
            read_claim(&registry, subject)
                .expect("read")
                .expect("subject")
                .process
                .state,
            State::Bound
        );
    }

    fn pane(pane: &str) -> crate::control_plane::registry::RegistryLocation {
        crate::control_plane::registry::RegistryLocation {
            backend: "tmux".into(),
            instance: None,
            workspace: Some("wA".into()),
            tab: None,
            pane: Some(pane.into()),
        }
    }

    /// A captured child bound to the test's agent, which is what a pane's absence
    /// is evidence about.
    fn bound_child(registry: &Registry) -> (ClaimId, Child) {
        let (subject_id, child, process) = captured_child(registry);
        let registration = read_claim(registry, &subject_id)
            .expect("read")
            .expect("launch")
            .process
            .registration_id;
        fence_registration(
            registry,
            &subject_id,
            Some(&process),
            Some(&registration),
            2,
        )
        .expect("bind");
        (subject_id, child)
    }

    #[test]
    fn one_missing_listing_never_ends_a_launch_and_is_cleared_by_the_next_present_read() {
        let scratch = Scratch::new("absence-cleared");
        let registry = scratch.registry();
        let (subject_id, _child) = bound_child(&registry);
        let location = pane("%1");

        assert!(
            !observe_pane(
                &registry,
                &subject_id,
                &location,
                PaneEvidence::Absent,
                1_000
            )
            .expect("absent")
        );
        let pending = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject")
            .process
            .pane_absence
            .expect("a pending window");
        assert_eq!(pending.confirmations, 1);

        assert!(
            !observe_pane(
                &registry,
                &subject_id,
                &location,
                PaneEvidence::Present,
                1_100
            )
            .expect("present")
        );
        let subject = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.process.pane_absence.is_none(),
            "a present read clears the window"
        );
        assert!(subject.process.ended_externally_at.is_none());
        assert!(subject.process.exited_at.is_none());
    }

    #[test]
    fn a_burst_of_absences_inside_one_window_is_not_confirmation() {
        let scratch = Scratch::new("absence-burst");
        let registry = scratch.registry();
        let (subject_id, _child) = bound_child(&registry);
        let location = pane("%1");

        for now in [1_000, 1_100, 1_200] {
            assert!(
                !observe_pane(&registry, &subject_id, &location, PaneEvidence::Absent, now)
                    .expect("absent")
            );
        }
        let subject = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.process.ended_externally_at.is_none(),
            "the window has not elapsed"
        );
        assert_eq!(
            subject.process.pane_absence.expect("pending").confirmations,
            3
        );
    }

    #[test]
    fn a_confirmed_pane_loss_ends_the_launch_externally_with_its_evidence() {
        let scratch = Scratch::new("absence-confirmed");
        let registry = scratch.registry();
        let (subject_id, _child) = bound_child(&registry);
        let location = pane("%1");

        assert!(
            !observe_pane(
                &registry,
                &subject_id,
                &location,
                PaneEvidence::Absent,
                1_000
            )
            .expect("absent")
        );
        assert!(
            !observe_pane(
                &registry,
                &subject_id,
                &location,
                PaneEvidence::Absent,
                4_000
            )
            .expect("absent")
        );
        assert!(
            observe_pane(
                &registry,
                &subject_id,
                &location,
                PaneEvidence::Absent,
                8_000
            )
            .expect("absent")
        );

        let subject = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.process.pane_absence.is_none(),
            "the window is consumed by the outcome"
        );
        assert!(subject.process.ended_externally_at.is_some());
        // A pane's absence is not a process's death, so no exit is claimed.
        assert!(subject.process.exited_at.is_none());
        let evidence = subject.process.ended_evidence.expect("evidence summary");
        assert_eq!(evidence.reason, EndedReason::PaneLost);
        assert_eq!(evidence.source, "tmux");
        assert_eq!(evidence.location, location);
        assert!(evidence.confirmations >= ABSENCE_CONFIRMATIONS);
    }

    #[test]
    fn an_unavailable_read_never_confirms_absence_and_never_ends_a_launch() {
        let scratch = Scratch::new("absence-unavailable");
        let registry = scratch.registry();
        let (subject_id, _child) = bound_child(&registry);
        let location = pane("%1");

        for now in [1_000, 8_000, 20_000] {
            assert!(
                !observe_pane(
                    &registry,
                    &subject_id,
                    &location,
                    PaneEvidence::Unavailable,
                    now
                )
                .expect("unavailable")
            );
        }
        let subject = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.process.pane_absence.is_none(),
            "an unavailable read is not a confirmation"
        );
        assert!(subject.process.ended_externally_at.is_none());
    }

    #[test]
    fn a_verified_process_exit_outranks_later_pane_evidence() {
        let scratch = Scratch::new("exit-outranks-pane");
        let registry = scratch.registry();
        let (subject_id, mut child) = bound_child(&registry);
        let location = pane("%1");

        let _ = child.0.kill();
        let _ = child.0.wait();
        assert!(observe_exit(&registry, &subject_id, &LocalProcfsVerifier, 2_000).expect("exit"));
        assert!(
            observe_pane(
                &registry,
                &subject_id,
                &location,
                PaneEvidence::Absent,
                9_000
            )
            .expect("already ended")
        );

        let subject = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.process.exited_at.is_some(),
            "the verified exit stands"
        );
        assert!(subject.process.ended_externally_at.is_none());
        assert!(subject.process.ended_evidence.is_none());
    }

    #[test]
    fn an_unbound_launch_has_no_pane_evidence_to_confirm() {
        let scratch = Scratch::new("absence-unbound");
        let registry = scratch.registry();
        let (subject_id, _child, _process) = captured_child(&registry);
        let location = pane("%1");

        for now in [1_000, 4_000, 8_000] {
            assert!(
                !observe_pane(&registry, &subject_id, &location, PaneEvidence::Absent, now)
                    .expect("absent")
            );
        }
        let subject = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(subject.process.pane_absence.is_none());
        assert!(subject.process.ended_externally_at.is_none());
    }

    /// Captures a live child as a tracked launch, bound to the test's agent.
    /// Most fixtures start a bare process, so what they exercise is the signal
    /// path: the launch they record never entered a scope, and nothing claims one.
    fn unscoped(registry: &Registry, ticket: &Ticket) {
        let path = recipe_path(registry, ticket.registration_id.as_str());
        let mut recipe: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("recipe")).expect("json");
        recipe.as_object_mut().expect("object").remove("scope");
        std::fs::write(&path, serde_json::to_vec(&recipe).expect("encode")).expect("rewrite");
    }

    fn captured_child(registry: &Registry) -> (ClaimId, Child, ProcessIdentity) {
        let ticket = create(registry, SPAWN, launch(), None, 1).expect("subject");
        unscoped(registry, &ticket);
        // A sleep outside any scope: the fixture is about the signal path.
        let child = Child(Command::new("sleep").arg("300").spawn().expect("sleep"));
        let process = LocalProcfsVerifier
            .inspect(child.0.id() as i32)
            .expect("procfs")
            .expect("the child is alive");
        capture(
            registry,
            &ticket.claim_id,
            &ticket.correlation,
            &process,
            &started_as(&launch()),
            random_uuid(),
            2,
        )
        .expect("capture");
        (ticket.claim_id, child, process)
    }

    #[test]
    fn a_scoped_launch_whose_unit_is_missing_is_signalled_on_its_process_not_killed() {
        let scratch = Scratch::new("scope-missing");
        let registry = scratch.registry();
        let (subject, mut child, _) = captured_child(&registry);
        // The launch was wrapped into a scope, but no such unit exists: it was
        // collected, expired, or never created. That is not evidence of an exit.
        let recipe = recipe_path(
            &registry,
            read_recorded(&registry, &subject)
                .expect("claim")
                .process
                .registration_id
                .as_str(),
        );
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&recipe).expect("recipe")).expect("json");
        json["scope"] = serde_json::to_value(
            crate::managed_scope::Scope::for_registration(
                read_recorded(&registry, &subject)
                    .expect("claim")
                    .process
                    .registration_id
                    .as_str(),
            )
            .expect("scope"),
        )
        .expect("scope json");
        std::fs::write(&recipe, serde_json::to_vec(&json).expect("encode")).expect("rewrite");

        // The scope is the authority for killing; the process's own verified
        // identity is the authority for a TERM. A unit that cannot be read
        // withholds the first and names itself in the outcome.
        let cancelled = cancel(&registry, &subject, &LocalProcfsVerifier, 4).expect("cancel");

        assert!(
            cancelled.signal_attempted,
            "the fenced process was signalled"
        );
        assert!(
            !cancelled.escalated,
            "a scope that could not be read is never killed"
        );
        assert_eq!(cancelled.exit, Exit::Exited, "the fenced process ended");
        let record = read_claim(&registry, &subject)
            .expect("read")
            .expect("subject");
        assert!(record.process.exited_at.is_some(), "the exit was recorded");
        assert!(
            child.0.try_wait().expect("wait").is_some(),
            "the process is gone"
        );
    }

    /// Reports every PID as absent, as a process table that no longer lists it does.
    struct Gone;

    impl ProcessVerifier for Gone {
        fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
            Ok(None)
        }
    }

    /// Reports every PID as owned by another process, as a reused PID is.
    struct Reused;

    impl ProcessVerifier for Reused {
        fn inspect(&self, pid: i32) -> Result<Option<ProcessIdentity>, String> {
            Ok(Some(ProcessIdentity {
                boot_id: "boot".into(),
                pid,
                start_ticks: 7,
            }))
        }
    }

    #[test]
    fn a_holder_whose_process_is_alive_or_unreadable_keeps_its_session() {
        // The holder's pane is irrelevant: only its process decides. A live process
        // holds the session, and an unreadable one is not evidence it is gone.
        let scratch = Scratch::new("holder-alive");
        let registry = scratch.registry();
        let (subject_id, _child, _process) = captured_child(&registry);
        let holder = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");

        assert!(!release_if_gone(&registry, &holder, &LocalProcfsVerifier, 3).expect("verify"));
        assert!(!release_if_gone(&registry, &holder, &Unreadable, 3).expect("verify"));
        let held = read_claim(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            held.process.exited_at.is_none(),
            "a live or unreadable holder is not an exit"
        );
    }

    #[test]
    fn a_holder_whose_process_is_gone_or_reused_releases_its_session_as_a_verified_exit() {
        let scratch = Scratch::new("holder-gone");
        let registry = scratch.registry();

        let (gone_id, _gone, _) = captured_child(&registry);
        let holder = read_claim(&registry, &gone_id)
            .expect("read")
            .expect("subject");
        assert!(release_if_gone(&registry, &holder, &Gone, 3).expect("verify"));
        let exited = read_claim(&registry, &gone_id)
            .expect("read")
            .expect("subject");
        assert!(exited.process.exited_at.is_some());
        assert!(
            exited.process.ended_externally_at.is_none(),
            "a confirmed exit is not a pane loss"
        );

        let (reused_id, _reused, _) = captured_child(&registry);
        let holder = read_claim(&registry, &reused_id)
            .expect("read")
            .expect("subject");
        assert!(release_if_gone(&registry, &holder, &Reused, 4).expect("verify"));
        assert!(
            read_claim(&registry, &reused_id)
                .expect("read")
                .expect("subject")
                .process
                .exited_at
                .is_some()
        );
    }

    #[test]
    fn a_process_that_exited_while_no_daemon_observed_it_is_recorded_by_the_sweep() {
        let scratch = Scratch::new("sweep-exited");
        let registry = scratch.registry();
        let (subject, mut child, _) = captured_child(&registry);
        child.0.kill().expect("kill");
        child.0.wait().expect("reaped");

        // A restarted daemon opens the same records and runs its first pass.
        let restarted = scratch.registry();
        assert_eq!(
            sweep(&restarted, &LocalProcfsVerifier, None, 3),
            Ok((1, None))
        );
        let recorded = read_claim(&restarted, &subject)
            .expect("read")
            .expect("subject");
        assert!(recorded.process.exited_at.is_some());
        assert!(
            recorded.process.signalled_at.is_none(),
            "the sweep never signals"
        );
    }

    #[test]
    fn a_live_process_is_neither_recorded_nor_signalled_by_the_sweep() {
        let scratch = Scratch::new("sweep-live");
        let registry = scratch.registry();
        let (subject, mut child, _) = captured_child(&registry);

        assert_eq!(
            sweep(&registry, &LocalProcfsVerifier, None, 3),
            Ok((0, None))
        );
        assert!(
            child.0.try_wait().expect("try_wait").is_none(),
            "still running"
        );
        let kept = read_claim(&registry, &subject)
            .expect("read")
            .expect("subject");
        assert!(kept.process.exited_at.is_none());
        assert!(kept.process.signalled_at.is_none());
        let observation = kept.process.observation.expect("running observation");
        assert_eq!(observation.evidence, Evidence::Running);
        assert_eq!(observation.source, Observer::Sweep);
        let tail = registry.events.lock().unwrap().tail();
        assert_eq!(
            sweep(&registry, &LocalProcfsVerifier, None, 4),
            Ok((0, None))
        );
        assert_eq!(
            registry.events.lock().unwrap().tail(),
            tail,
            "unchanged samples are not new transitions"
        );
    }

    #[test]
    fn unavailable_reconciliation_preserves_the_process_and_can_recover() {
        let scratch = Scratch::new("observation-unavailable");
        let registry = scratch.registry();
        let (subject, mut child, _) = captured_child(&registry);
        assert!(observe_exit(&registry, &subject, &Unreadable, 3).is_err());
        let unknown = read_claim(&registry, &subject).unwrap().unwrap();
        assert_eq!(
            unknown.process.observation.unwrap().evidence,
            Evidence::Unavailable
        );
        assert!(unknown.process.exited_at.is_none());
        assert!(unknown.process.signalled_at.is_none());
        assert_eq!(
            observe_exit(&registry, &subject, &LocalProcfsVerifier, 4),
            Ok(false)
        );
        assert_eq!(
            read_claim(&registry, &subject)
                .unwrap()
                .unwrap()
                .process
                .observation
                .unwrap()
                .evidence,
            Evidence::Running
        );
        assert!(child.0.try_wait().unwrap().is_none());
    }

    #[test]
    fn a_pane_cleanup_is_recorded_once_and_only_for_a_revoked_launch() {
        let scratch = Scratch::new("pane-cleanup");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = &ticket.claim_id;

        // A launch no cancel revoked is not cleaned up: its pane is still the
        // launch's, and only a cancel acts on it.
        assert!(record_pane_cleanup(&registry, subject, PaneCleanup::Cleaned, 2).is_err());

        let cancelled = cancel(&registry, subject, &Uninspected, 3).expect("cancel");
        assert!(cancelled.revoked);
        let refused = PaneCleanup::Refused("kept for the operator".into());
        assert_eq!(
            record_pane_cleanup(&registry, subject, refused.clone(), 4).expect("cleanup"),
            refused
        );

        // The first outcome is the durable one: a later cancel reports it rather
        // than closing anything again.
        assert_eq!(
            record_pane_cleanup(&registry, subject, PaneCleanup::Cleaned, 5).expect("cleanup"),
            refused
        );
        drop(registry);

        // The outcome is replayed from the log, not read from a mutable file.
        let reopened = crate::control_plane::Registry::open(&scratch.0).expect("reopen");
        let restored = read_claim(&reopened, subject)
            .expect("read")
            .expect("subject");
        assert_eq!(restored.process.pane_cleanup, Some(refused));
        assert_eq!(restored.process.state, State::Revoked);
    }

    #[test]
    fn stop_and_exit_observation_race_commits_one_terminal_transition() {
        use std::sync::{Arc, Barrier};

        let scratch = Scratch::new("stop-observation-race");
        let registry = Arc::new(scratch.registry());
        let (subject_id, mut child, _) = captured_child(&registry);
        let barrier = Arc::new(Barrier::new(3));

        let cancelling_registry = Arc::clone(&registry);
        let cancelling_barrier = Arc::clone(&barrier);
        let cancelling_subject = subject_id.clone();
        let cancel = std::thread::spawn(move || {
            cancelling_barrier.wait();
            cancel(
                &cancelling_registry,
                &cancelling_subject,
                &LocalProcfsVerifier,
                10,
            )
        });

        let observing_registry = Arc::clone(&registry);
        let observing_barrier = Arc::clone(&barrier);
        let observing_subject = subject_id.clone();
        let observe = std::thread::spawn(move || {
            observing_barrier.wait();
            observe_exit(
                &observing_registry,
                &observing_subject,
                &LocalProcfsVerifier,
                10,
            )
        });

        barrier.wait();
        let cancellation = cancel.join().expect("cancel thread").expect("cancel");
        let observation = observe.join().expect("observe thread");
        assert!(cancellation.revoked);
        assert!(observation.is_ok());
        assert!(child.0.try_wait().expect("wait child").is_some());

        let subject = read_claim(&registry, &subject_id)
            .expect("read terminal subject")
            .expect("subject");
        assert!(subject.process.exited_at.is_some());
        assert!(subject.process.signalled_at.is_some());
        let exits = registry
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read_after(0, 128)
            .expect("read lifecycle events")
            .into_iter()
            // The exit is one physical fact, recorded once against the process
            // that produced it rather than against whichever claim reported it.
            .filter(|event| {
                event.subject_id.as_deref() == Some(subject.process.registration_id.as_str())
            })
            .filter(|event| {
                matches!(
                    event.transition,
                    crate::control_plane::events::Transition::ExitVerified
                )
            })
            .count();
        assert_eq!(exits, 1, "stop and observation commit one exit transition");
    }

    #[test]
    fn concurrent_hints_coalesce_without_blocking_an_independent_subject() {
        use std::sync::{Arc, Mutex, mpsc};
        struct Blocked {
            entered: mpsc::Sender<()>,
            release: Mutex<mpsc::Receiver<()>>,
        }
        impl ProcessVerifier for Blocked {
            fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
                self.entered.send(()).unwrap();
                let _ = self.release.lock().unwrap().recv();
                Err("source unavailable".into())
            }
        }
        let scratch = Scratch::new("coalesced-observation");
        let registry = Arc::new(scratch.registry());
        let (subject, _child, _) = captured_child(&registry);
        let other = create(&registry, &random_uuid(), launch(), None, 1).unwrap();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let verifier = Arc::new(Blocked {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        });
        let observing = Arc::clone(&registry);
        let observed = subject.clone();
        let source = Arc::clone(&verifier);
        let first =
            std::thread::spawn(move || observe_exit(&observing, &observed, source.as_ref(), 3));
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            observe_exit(&registry, &subject, &Uninspected, 4),
            Ok(false),
            "a duplicate hint must not inspect again"
        );
        let independent = Arc::clone(&registry);
        let (done_tx, done_rx) = mpsc::channel();
        let second = std::thread::spawn(move || {
            done_tx
                .send(cancel(&independent, &other.claim_id, &Uninspected, 4))
                .unwrap()
        });
        let done = done_rx.recv_timeout(Duration::from_secs(1));
        release_tx.send(()).unwrap();
        assert!(first.join().unwrap().is_err());
        second.join().unwrap();
        assert!(
            done.expect("independent cancellation must not wait for the blocked source")
                .unwrap()
                .revoked
        );
    }

    #[test]
    fn a_stop_leaves_an_unbound_launch_unrevoked() {
        let scratch = Scratch::new("stop-pending");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let stopped = stop(&registry, &ticket.claim_id, &Uninspected, 2).expect("stop");
        assert_eq!(
            stopped,
            Termination {
                revoked: false,
                escalated: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
        let subject = read_claim(&registry, &ticket.claim_id)
            .expect("read")
            .expect("subject");
        assert_eq!(subject.process.state, State::Pending);
    }

    /// The executions a batch addresses, as a place's placements name them: the
    /// process incarnation, not the attempt it belongs to.
    fn targets(registry: &Registry, claims: &[ClaimId]) -> Vec<ExecutionRef> {
        claims
            .iter()
            .map(|claim| {
                let process = read_claim(registry, claim)
                    .expect("read")
                    .expect("subject")
                    .process;
                ExecutionRef {
                    registration_id: process.registration_id,
                    subject_id: None,
                }
            })
            .collect()
    }

    #[test]
    fn cancelling_several_executions_signals_them_together_and_records_each_exit() {
        let scratch = Scratch::new("cancel-many");
        let registry = scratch.registry();
        let mut children = Vec::new();
        let mut subjects = Vec::new();
        for _ in 0..2 {
            let (subject, child, _process) = captured_child(&registry);
            subjects.push(subject);
            children.push(child);
        }

        let cancelled = cancel_many(
            &registry,
            &targets(&registry, &subjects),
            &LocalProcfsVerifier,
            3,
        );

        assert_eq!(cancelled.survivors().count(), 0);
        assert_eq!(
            cancelled
                .ended
                .iter()
                .filter(|ended| ended.end == ExecutionEnd::Exited)
                .count(),
            2,
            "both executions exited"
        );
        for subject_id in &subjects {
            let subject = read_claim(&registry, subject_id)
                .expect("read")
                .expect("subject");
            assert!(subject.process.exited_at.is_some(), "an exit was recorded");
        }
        for child in &mut children {
            let _ = child.0.wait();
        }
    }

    #[test]
    fn a_cancel_whose_window_closes_reports_the_execution_as_a_survivor() {
        let scratch = Scratch::new("cancel-survivor");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        // A process that ignores SIGTERM: the window must end with a survivor, not
        // a claim that it exited.
        let mut child = Child(
            Command::new("sh")
                .arg("-c")
                .arg("trap '' TERM; sleep 300")
                .spawn()
                .expect("shell"),
        );
        unscoped(&registry, &ticket);
        std::thread::sleep(std::time::Duration::from_millis(200));
        let process = LocalProcfsVerifier
            .inspect(child.0.id() as i32)
            .expect("procfs")
            .expect("the child is alive");
        capture(
            &registry,
            &ticket.claim_id,
            &ticket.correlation,
            &process,
            &started_as(&launch()),
            random_uuid(),
            2,
        )
        .expect("capture");

        let cancelled = cancel_many(
            &registry,
            &targets(&registry, std::slice::from_ref(&ticket.claim_id)),
            &LocalProcfsVerifier,
            3,
        );

        assert_eq!(
            cancelled
                .survivors()
                .map(|ended| ended.registration_id.clone())
                .collect::<Vec<_>>(),
            vec![ticket.registration_id.clone()]
        );
        assert!(cancelled.ended[0].end != ExecutionEnd::Exited);
        let subject = read_claim(&registry, &ticket.claim_id)
            .expect("read")
            .expect("subject");
        assert!(subject.process.exited_at.is_none(), "no exit was recorded");
        let _ = child.0.kill();
        let _ = child.0.wait();
    }

    #[test]
    fn a_stop_of_a_reused_pid_signals_nothing_and_records_the_claim_gone() {
        let scratch = Scratch::new("stop-reused");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = &ticket.claim_id;
        let mut child = Child(Command::new("sleep").arg("300").spawn().expect("sleep"));
        let pid = child.0.id() as i32;
        let live = LocalProcfsVerifier
            .inspect(pid)
            .expect("procfs")
            .expect("the child is alive");
        // The claim names this PID as it was at some earlier incarnation.
        let claimed = ProcessIdentity {
            start_ticks: live.start_ticks + 1,
            ..live
        };
        capture(
            &registry,
            subject,
            &ticket.correlation,
            &claimed,
            &started_as(&launch()),
            random_uuid(),
            2,
        )
        .expect("capture");

        let stopped = stop(&registry, subject, &LocalProcfsVerifier, 3).expect("stop");
        assert_eq!(
            stopped,
            Termination {
                revoked: false,
                escalated: false,
                signal_attempted: false,
                exit: Exit::Exited,
            }
        );
        assert!(child.0.try_wait().expect("child state").is_none());
    }

    #[test]
    fn a_stop_of_an_unreadable_identity_is_unresolved_and_signals_nothing() {
        let scratch = Scratch::new("stop-unreadable");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = &ticket.claim_id;
        capture(
            &registry,
            subject,
            &ticket.correlation,
            &identity(7),
            &started_as(&launch()),
            random_uuid(),
            2,
        )
        .expect("capture");

        let stopped = stop(&registry, subject, &Unreadable, 3).expect("stop");
        assert_eq!(
            stopped,
            Termination {
                revoked: false,
                escalated: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
        let record = read_claim(&registry, subject)
            .expect("read")
            .expect("subject");
        assert_eq!(record.process.signalled_at, None);
        assert_eq!(record.process.exited_at, None);
    }
}

#[cfg(test)]
mod resume_recipe_tests {
    use super::*;

    fn launch(argv: &[&str]) -> LaunchContext {
        LaunchContext {
            executable: "pi".into(),
            argv: argv.iter().map(|arg| arg.to_string()).collect(),
            cwd: None,
            env: BTreeMap::new(),
        }
    }

    #[test]
    fn resume_replaces_prior_session_selectors_with_the_live_session() {
        let resumed = resume_launch(
            &launch(&[
                "--session",
                "old",
                "--model",
                "m",
                "--continue",
                "--name",
                "lead",
            ]),
            "1234",
        )
        .expect("resume");
        assert_eq!(
            resumed.argv,
            ["--model", "m", "--name", "lead", "--session", "1234"]
        );
    }

    #[test]
    fn resume_strips_equals_form_and_fork_selectors() {
        let resumed =
            resume_launch(&launch(&["--session=old", "--fork", "x", "-r"]), "9").expect("resume");
        assert_eq!(resumed.argv, ["--session", "9"]);
    }

    #[test]
    fn resume_refuses_a_bare_message_argument() {
        assert!(resume_launch(&launch(&["--model", "m", "fix the build"]), "1").is_err());
    }

    #[test]
    fn captured_launch_drops_radar_launch_and_token_variables() {
        let mut env = BTreeMap::new();
        env.insert(CLAIM_ENV.to_string(), "s".to_string());
        env.insert(CORRELATION_ENV.to_string(), "c".to_string());
        env.insert(MANAGED_PARENT_SUBJECT_ENV.to_string(), "parent".to_string());
        env.insert(
            crate::runtime::LAUNCH_TOKEN_ENV.to_string(),
            "t".to_string(),
        );
        env.insert("KEEP".to_string(), "v".to_string());
        let effective = Effective {
            cmdline: vec!["pi".into(), "--session".into(), "x".into()],
            cwd: None,
            env,
        };
        let captured = captured_launch(&effective).expect("captured");
        assert_eq!(captured.executable, "pi");
        assert_eq!(captured.argv, ["--session", "x"]);
        assert_eq!(captured.env.keys().collect::<Vec<_>>(), ["KEEP"]);
    }
}
