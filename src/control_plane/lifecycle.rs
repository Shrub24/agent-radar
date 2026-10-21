//! Daemon-owned launch subjects: the physical lifecycle of a tracked spawn.
//!
//! A tracked launch runs the requested program through a bootstrap that is the
//! Radar binary itself. The bootstrap claims its own process incarnation with a
//! private, one-use correlation; the daemon verifies that the claimed process is
//! the pane's foreground process and was started with the requested launch, then
//! records it and only then releases the bootstrap to exec the program. The exec
//! keeps the PID and birth identity, so the captured process is the program that
//! runs. A correlation is spent at capture; a revoked subject never captures or binds.
//!
//! Every transition of a subject runs under the registry's transition lock. That
//! lock is the one commit fence shared with registration binding, so a cancel and
//! a bootstrap claim, or a cancel and a registration, have exactly one winner.
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
use serde_json::{Value, json};

use crate::control_plane::protocol::{MAX_LINE_BYTES, PROTOCOL_VERSION};
use crate::control_plane::registry::publication::write_atomic;
use crate::control_plane::registry::{
    LocalProcfsVerifier, ProcessVerifier, Registry, read_record_file,
};
use crate::control_plane::store::{format_millis, random_uuid};
use crate::model::{ProcessIdentity, SessionUuid};
use crate::process_control::{self, Acquired, Effective};
use crate::runtime::LaunchRequest;

/// Environment variables the bootstrap reads and removes before the program runs.
pub const SUBJECT_ENV: &str = "PI_RADAR_LAUNCH_SUBJECT";
pub const CORRELATION_ENV: &str = "PI_RADAR_LAUNCH_CORRELATION";
// Retired: the managed subject is the attached execution, not an injected identity.
// These names are still dropped from recipes captured before that change.
pub const MANAGED_SUBJECT_ENV: &str = "PI_RADAR_MANAGED_SUBJECT_ID";
pub const MANAGED_LAUNCH_ENV: &str = "PI_RADAR_MANAGED_LAUNCH_ID";
pub const MANAGED_GENERATION_ENV: &str = "PI_RADAR_MANAGED_EXECUTION_GENERATION";
pub const MANAGED_PARENT_SUBJECT_ENV: &str = "PI_RADAR_MANAGED_PARENT_SUBJECT_ID";

/// The internal method a bootstrap calls to claim its process.
pub const BOOTSTRAP_METHOD: &str = "spawn.bootstrap";

/// The subcommand a pane-death hook runs, and the internal method it calls.
pub const WAKE_COMMAND: &str = "lifecycle-wake";
pub const WAKE_METHOD: &str = "lifecycle.wake";

/// The subcommand the Radar binary runs as the bootstrap.
pub const BOOTSTRAP_COMMAND: &str = "launch-bootstrap";

/// Refusal for a claim the subject cannot take: unknown, spent, revoked, or no
/// longer awaiting a process. It says nothing about which.
pub const NOT_AWAITING: &str = "launch is not awaiting a bootstrap";

/// Refusal for a captured process that was not started with the requested launch.
pub const LAUNCH_MISMATCH: &str = "launch context does not match the requested launch";

const SUBJECT_VERSION: u32 = 2;
const MAX_SUBJECT_BYTES: usize = 64 * 1024;
/// How long a cancelled process gets to exit after SIGTERM before the operation
/// reports the exit as not verified.
const EXIT_DEADLINE: Duration = Duration::from_secs(2);
/// How long a bootstrap waits for the daemon before it gives up without running
/// the program.
const RELEASE_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Launch recorded; no process has claimed it yet.
    Pending,
    /// The bootstrap's process is captured and released to exec.
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
    /// Whether the process was started with this launch: the program and its
    /// argv, its working directory when one was requested, and each requested
    /// environment value. Variables the backend adds to every pane are not part
    /// of the launch and are not compared.
    fn launched_as(&self, effective: &Effective) -> bool {
        let Some(separator) = effective.cmdline.iter().position(|arg| arg == "--") else {
            return false;
        };
        let mut program = vec![self.executable.clone()];
        program.extend(self.argv.iter().cloned());
        let cwd_matches = self
            .cwd
            .as_deref()
            .is_none_or(|cwd| fs::canonicalize(cwd).ok().as_deref() == effective.cwd.as_deref());
        &effective.cmdline[separator + 1..] == program.as_slice()
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
    pub parent_subject_id: Option<String>,
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
        if self
            .parent_subject_id
            .as_deref()
            .is_some_and(|id| SessionUuid::parse(id).is_none())
        {
            return Err("managed parent_subject_id must be a canonical UUID".into());
        }
        Ok(())
    }
}

/// The durable identity of one agent: the facts that outlive its attempts.
///
/// An attempt ([`Subject`]) is one launch or adoption and dies with its process.
/// The agent does not: its identity, its managed metadata and which attempt is
/// current are held here once, instead of being copied forward onto every
/// successor. `current_claim` names today's attempt record by its `subject_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubjectRecord {
    pub version: u32,
    pub subject_id: String,
    #[serde(default)]
    pub managed: ManagedMetadata,
    /// The Pi session this subject runs, when one is known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The attempt that is current for this subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_claim: Option<String>,
    /// The current attempt's generation, kept so a later attempt can be compared
    /// without reading the events log while its lock is held.
    #[serde(default)]
    pub current_generation: u32,
    pub recorded_at: String,
    pub updated_at: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subject {
    pub version: u32,
    /// Execution-record ID retained for private storage and exact process fences.
    pub subject_id: String,
    /// Stable logical subject ID shared by every exact successor execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_subject_id: Option<String>,
    /// One-based generation within the logical subject; absent only before migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_generation: Option<u32>,
    #[serde(default)]
    pub identity_unresolved: bool,
    pub spawn_request_id: String,
    #[serde(default)]
    pub origin: Origin,
    pub state: State,
    #[serde(default)]
    pub managed: ManagedMetadata,
    pub launch: LaunchContext,
    /// The Pi session a restart resumes. Present only on a restartable subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// The replacement spawn dispatched for this process generation, recorded
    /// before its dispatch so a second replacement is never launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation: Option<String>,
    /// The captured birth identity of the launched process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessIdentity>,
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
    #[serde(alias = "bound_agent_id")]
    pub bound_registration_id: Option<String>,
    /// Recorded before a SIGTERM is sent, so a signal is never sent twice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signalled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<String>,
    /// Recorded once, by the cancel that revoked an unbound launch. A bound
    /// launch's pane belongs to its child and is closed through spawn.close.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_cleanup: Option<PaneCleanup>,
    pub recorded_at: String,
}

/// What a spawn needs to launch a tracked child: the subject it belongs to and
/// the correlation its bootstrap presents.
pub struct Ticket {
    /// Exact execution record ID bound to the spawn edge.
    pub subject_id: String,
    /// Stable logical subject identity issued for this process.
    pub logical_subject_id: String,
    /// The exact Radar launch operation that created this execution.
    pub launch_id: String,
    pub execution_generation: u32,
    pub parent_subject_id: Option<String>,
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
    /// was revoked before any bootstrap could release a program.
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
    pub exit: Exit,
}

/// What a cancel did about the pane an unbound launch left behind. Reported
/// apart from revocation and from verified process exit, and not an exit claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "reason")]
pub enum PaneCleanup {
    /// One close was dispatched in the instance that created the pane, and it
    /// answered.
    Cleaned,
    /// Nothing was dispatched, for this reason.
    Refused(String),
    /// A close was dispatched and its answer was lost. It is never dispatched
    /// again for this launch.
    Unknown(String),
}

impl PaneCleanup {
    /// What the outcome reads as on a cancel's effects list.
    pub fn effect(&self) -> String {
        match self {
            Self::Cleaned => "pane cleaned up".to_string(),
            Self::Refused(reason) => format!("pane kept: {reason}"),
            Self::Unknown(reason) => format!("pane cleanup unverified: {reason}"),
        }
    }
}

enum Stage {
    Settled(Exit),
    Watch {
        descriptor: std::os::fd::OwnedFd,
        signal: bool,
    },
}

/// A pending subject for `launch`, with fresh ids. Its record is what is bounded.
fn draft(
    spawn_request_id: &str,
    launch: LaunchContext,
    session: Option<String>,
    identity: Option<(String, u32)>,
    managed: ManagedMetadata,
    now_ms: i64,
) -> Subject {
    let execution_id = random_uuid();
    let (logical_subject_id, execution_generation) =
        identity.unwrap_or_else(|| (execution_id.clone(), 1));
    Subject {
        observation: None,
        pane_absence: None,
        ended_externally_at: None,
        ended_evidence: None,
        version: SUBJECT_VERSION,
        subject_id: execution_id,
        logical_subject_id: Some(logical_subject_id),
        execution_generation: Some(execution_generation),
        identity_unresolved: false,
        spawn_request_id: spawn_request_id.to_string(),
        origin: Origin::Spawned,
        state: State::Pending,
        managed,
        launch,
        correlation: Some(random_uuid()),
        process: None,
        bound_registration_id: None,
        signalled_at: None,
        exited_at: None,
        pane_cleanup: None,
        recorded_at: format_millis(now_ms),
        session,
        successor: None,
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
    identity: Option<(String, u32)>,
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
    identity: Option<(String, u32)>,
    managed: ManagedMetadata,
    now_ms: i64,
) -> Result<Ticket, String> {
    let _transition = registry.transition();
    if identity
        .as_ref()
        .is_some_and(|(_, generation)| *generation == 0)
    {
        return Err("execution generation must be positive".into());
    }
    managed.validate()?;
    let subject = draft(spawn_request_id, launch, session, identity, managed, now_ms);
    write(registry, &subject)?;
    Ok(Ticket {
        subject_id: subject.subject_id,
        logical_subject_id: subject.logical_subject_id.unwrap_or_default(),
        launch_id: subject.spawn_request_id,
        execution_generation: subject.execution_generation.unwrap_or(1),
        parent_subject_id: subject.managed.parent_subject_id,
        correlation: subject.correlation.unwrap_or_default(),
    })
}

/// The subject with this id, or `None` when none was recorded.
pub fn read(registry: &Registry, subject_id: &str) -> Result<Option<Subject>, String> {
    if SessionUuid::parse(subject_id).is_none() {
        return Err("`subject_id` must be a canonical UUID".into());
    }
    let facts = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .facts(subject_id)
        .cloned();
    let path = if facts.is_some() {
        recipe_path(registry, subject_id)
    } else {
        path(registry, subject_id)
    };
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("launch subject {subject_id}: {error}")),
        Ok(_) => {}
    }
    let bytes = read_record_file(&path, MAX_SUBJECT_BYTES)?;
    let mut subject: Subject =
        serde_json::from_slice(&bytes).map_err(|error| format!("not a launch subject: {error}"))?;
    if !matches!(subject.version, 1 | SUBJECT_VERSION) || subject.subject_id != subject_id {
        return Err("launch subject does not match its name or version".into());
    }
    // Reading a legacy file upgrades only the in-memory view. The event log is
    // migrated separately and remains the authoritative durable record.
    subject.version = SUBJECT_VERSION;
    if let Some(facts) = facts {
        facts.apply_to(&mut subject);
    }
    Ok(Some(subject))
}

fn read_recorded(registry: &Registry, subject_id: &str) -> Result<Subject, String> {
    read(registry, subject_id)?.ok_or_else(|| "tracked launch subject is unavailable".to_string())
}

/// Reads the current execution for a stable public logical subject ID.
pub fn read_logical(
    registry: &Registry,
    logical_subject_id: &str,
) -> Result<Option<Subject>, String> {
    if SessionUuid::parse(logical_subject_id).is_none() {
        return Err("`subject_id` must be a canonical UUID".into());
    }
    let latest = latest_attempt_for_logical(registry, logical_subject_id)?;
    // The record is authoritative once it exists; an attempt written before it
    // existed is promoted here so the subject stops being derived by scanning.
    let record = match read_subject_record(registry, logical_subject_id)? {
        Some(record) => Some(record),
        None => {
            let attempt = match &latest {
                Some(attempt_id) => read(registry, attempt_id)?,
                None => None,
            };
            match attempt {
                Some(attempt) if attempt.logical_subject_id.is_some() => {
                    note_attempt(registry, &attempt)?;
                    read_subject_record(registry, logical_subject_id)?
                }
                _ => None,
            }
        }
    };
    if let Some(record) = record {
        return match record.current_claim {
            Some(claim_id) => read(registry, &claim_id),
            None => Ok(None),
        };
    }
    match latest {
        Some(attempt_id) => read(registry, &attempt_id),
        None => read(registry, logical_subject_id),
    }
}

/// The newest attempt assigned to one stable logical subject, if any.
fn latest_attempt_for_logical(
    registry: &Registry,
    logical_subject_id: &str,
) -> Result<Option<String>, String> {
    let mut execution: Option<(u32, String)> = None;
    let mut matching_count = 0usize;
    let mut has_unresolved = false;
    for (execution_id, facts) in registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .all_facts()
    {
        if facts.logical_subject_id.as_deref() != Some(logical_subject_id) {
            continue;
        }
        matching_count += 1;
        has_unresolved |= facts.identity_unresolved;
        let generation = facts.execution_generation.unwrap_or(0);
        match &execution {
            Some((current_generation, _current_id)) if *current_generation > generation => {}
            Some((current_generation, current_id))
                if *current_generation == generation && current_id != &execution_id =>
            {
                return Err(format!(
                    "logical subject {logical_subject_id} has multiple executions at generation {generation}"
                ));
            }
            _ => execution = Some((generation, execution_id)),
        }
    }
    if matching_count > 1 && has_unresolved {
        return Err(format!(
            "logical subject {logical_subject_id} has unresolved execution continuity"
        ));
    }
    Ok(execution.map(|(_, execution_id)| execution_id))
}

/// One agent's durable record, promoted from its attempts when none was written.
pub fn read_subject_record(
    registry: &Registry,
    subject_id: &str,
) -> Result<Option<SubjectRecord>, String> {
    if SessionUuid::parse(subject_id).is_none() {
        return Err("`subject_id` must be a canonical UUID".into());
    }
    let path = subject_record_path(registry, subject_id);
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let bytes = read_record_file(&path, MAX_SUBJECT_BYTES)?;
    let record: SubjectRecord =
        serde_json::from_slice(&bytes).map_err(|_| "not a subject record".to_string())?;
    if record.subject_id != subject_id {
        return Err("subject record is filed under another subject".into());
    }
    Ok(Some(record))
}

/// Records which attempt is current for its subject, and which facts the subject
/// carries. Called by the attempt writer, so every attempt change maintains it.
fn note_attempt(registry: &Registry, subject: &Subject) -> Result<(), String> {
    let logical_id = subject
        .logical_subject_id
        .clone()
        .unwrap_or_else(|| subject.subject_id.clone());
    let generation = subject.execution_generation.unwrap_or(1);
    let existing = read_subject_record(registry, &logical_id)?;
    let mut record = match existing {
        Some(record) => record,
        None => SubjectRecord {
            version: SUBJECT_VERSION,
            subject_id: logical_id.clone(),
            managed: subject.managed.clone(),
            session: subject.session.clone(),
            current_claim: None,
            current_generation: 0,
            recorded_at: subject.recorded_at.clone(),
            updated_at: subject.recorded_at.clone(),
        },
    };
    // An earlier attempt reporting late never displaces the current one.
    if generation < record.current_generation {
        return Ok(());
    }
    let changed = record.current_claim.as_deref() != Some(subject.subject_id.as_str())
        || record.managed != subject.managed
        || record.session != subject.session;
    if !changed {
        return Ok(());
    }
    record.current_claim = Some(subject.subject_id.clone());
    record.current_generation = generation;
    record.managed = subject.managed.clone();
    if subject.session.is_some() || record.session.is_none() {
        record.session = subject.session.clone();
    }
    record.updated_at = subject.recorded_at.clone();
    write_subject_record(registry, &record)
}

fn write_subject_record(registry: &Registry, record: &SubjectRecord) -> Result<(), String> {
    let directory = registry.subjects().join("identity");
    crate::control_plane::registry::prepare_directory(&directory)?;
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
    write_atomic(
        &directory,
        &subject_record_path(registry, &record.subject_id),
        &bytes,
        MAX_SUBJECT_BYTES,
    )
}

fn subject_record_path(registry: &Registry, subject_id: &str) -> PathBuf {
    registry
        .subjects()
        .join("identity")
        .join(format!("{subject_id}.json"))
}

/// All exact execution IDs durably assigned to one stable logical subject.
pub fn execution_ids_for_logical(registry: &Registry, logical_subject_id: &str) -> Vec<String> {
    registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .all_facts()
        .into_iter()
        .filter(|(_, facts)| facts.logical_subject_id.as_deref() == Some(logical_subject_id))
        .map(|(execution_id, _)| execution_id)
        .collect()
}

/// Lists one latest execution per stable logical subject, ordered by logical ID.
pub fn list_logical_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Subject>, String> {
    let facts = registry
        .events
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .all_facts();
    let mut known_executions = BTreeSet::new();
    let mut latest = BTreeMap::<String, (u32, String)>::new();
    let mut logical_counts = BTreeMap::<String, usize>::new();
    let mut unresolved_logicals = BTreeSet::new();
    for (execution_id, facts) in facts {
        known_executions.insert(execution_id.clone());
        let logical_id = facts
            .logical_subject_id
            .unwrap_or_else(|| execution_id.clone());
        let generation = facts.execution_generation.unwrap_or(1);
        *logical_counts.entry(logical_id.clone()).or_default() += 1;
        if facts.identity_unresolved {
            unresolved_logicals.insert(logical_id.clone());
        }
        match latest.get(&logical_id) {
            Some((old_generation, _old_id)) if *old_generation > generation => {}
            Some((old_generation, old_id))
                if *old_generation == generation && old_id != &execution_id =>
            {
                return Err(format!(
                    "logical subject {logical_id} has multiple executions at generation {generation}"
                ));
            }
            _ => {
                latest.insert(logical_id, (generation, execution_id));
            }
        }
    }
    if let Some(logical_id) = unresolved_logicals
        .iter()
        .find(|id| logical_counts.get(*id).copied().unwrap_or_default() > 1)
    {
        return Err(format!(
            "logical subject {logical_id} has unresolved execution continuity"
        ));
    }
    // Include pre-event legacy records without deserializing every record in the
    // page scan. A record with no event facts is its own generation-one root.
    let entries = match fs::read_dir(registry.subjects()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("launch subjects: {error}")),
    };
    for entry in entries {
        let entry = entry.map_err(|error| format!("launch subject directory entry: {error}"))?;
        let filename = entry.file_name();
        let Some(id) = filename
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .filter(|id| SessionUuid::parse(id).is_some())
        else {
            continue;
        };
        if !known_executions.contains(id) {
            latest
                .entry(id.to_string())
                .or_insert_with(|| (1, id.to_string()));
        }
    }
    latest
        .into_iter()
        .filter(|(logical_id, _)| after.is_none_or(|cursor| logical_id.as_str() > cursor))
        .take(limit)
        .map(|(_, (_, execution_id))| read_recorded(registry, &execution_id))
        .collect()
}

/// Records what a cancel did about the launch's own pane. The first outcome is
/// durable and is what a later cancel of the same launch reports: nothing is
/// closed twice, and an uncertain cleanup is never retried into a second one.
pub fn record_pane_cleanup(
    registry: &Registry,
    subject_id: &str,
    outcome: PaneCleanup,
    now_ms: i64,
) -> Result<PaneCleanup, String> {
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, subject_id)?;
    if let Some(recorded) = subject.pane_cleanup.clone() {
        return Ok(recorded);
    }
    if subject.state != State::Revoked {
        return Err("only a revoked launch's own pane is cleaned up".into());
    }
    subject.pane_cleanup = Some(outcome.clone());
    subject.recorded_at = format_millis(now_ms);
    write(registry, &subject)?;
    Ok(outcome)
}

/// Captures the process a bootstrap claimed, spending its correlation.
///
/// Refuses unless the subject is still pending with this correlation, and the
/// process was started with the requested launch. A cancel that won the race
/// revoked the subject; a replayed claim finds the spent correlation.
pub fn capture(
    registry: &Registry,
    subject_id: &str,
    correlation: &str,
    process: &ProcessIdentity,
    effective: &Effective,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let mut subject = read(registry, subject_id)?.ok_or_else(|| NOT_AWAITING.to_string())?;
    if subject.state != State::Pending || subject.correlation.as_deref() != Some(correlation) {
        return Err(NOT_AWAITING.into());
    }
    if !subject.launch.launched_as(effective) {
        return Err(LAUNCH_MISMATCH.into());
    }
    subject.state = State::Captured;
    subject.correlation = None;
    subject.process = Some(process.clone());
    subject.recorded_at = format_millis(now_ms);
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
    subject_id: &str,
    process: Option<&ProcessIdentity>,
    registration_id: Option<&str>,
    now_ms: i64,
) -> Result<(), String> {
    let mut subject = read_recorded(registry, subject_id)?;
    match (subject.state, registration_id) {
        (State::Bound, Some(agent))
            if subject.bound_registration_id.as_deref() == Some(agent)
                && subject.process.as_ref() == process =>
        {
            Ok(())
        }
        (State::Bound, _) => Err("launch is already bound to another registration".into()),
        (State::Captured, Some(agent))
            if process.is_some() && subject.process.as_ref() == process =>
        {
            subject.state = State::Bound;
            subject.bound_registration_id = Some(agent.to_string());
            subject.recorded_at = format_millis(now_ms);
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
    subject_id: &str,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Termination, String> {
    let revoked = revoke(registry, subject_id, now_ms)?;
    terminate(registry, subject_id, verifier, now_ms, revoked)
}

/// Stops a tracked launch's captured process. The launch itself is left as it is:
/// a bound launch stays bound, and an unbound one is not revoked.
pub fn stop(
    registry: &Registry,
    subject_id: &str,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Termination, String> {
    terminate(registry, subject_id, verifier, now_ms, false)
}

/// Records a launch's exit if its captured process is verifiably gone, and
/// reports whether it is. A notification calls this; it is never itself proof.
pub fn observe_exit(
    registry: &Registry,
    subject_id: &str,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<bool, String> {
    observe_exit_from(registry, subject_id, verifier, now_ms, Observer::Read)
}

/// How a registration stands against one Pi session. A session has at most one
/// attachment: the live registration whose published session is that session.
#[derive(Clone, PartialEq, Eq)]
pub enum Attachment {
    /// This registration already holds the session's live subject.
    Own(Box<Subject>),
    /// Another registration's live subject holds the session.
    Other(Box<Subject>),
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
pub fn is_attached(registry: &Registry, subject: &Subject, now_ms: i64) -> Result<bool, String> {
    let (Some(agent), Some(session)) = (
        subject.bound_registration_id.as_deref(),
        subject.session.as_deref(),
    ) else {
        return Ok(false);
    };
    if subject.state != State::Bound || subject.exited_at.is_some() {
        return Ok(false);
    }
    if let Some(claim) = crate::control_plane::registry::presence::current(registry, agent)? {
        return Ok(claim.session_id == session
            && claim.standing == crate::control_plane::registry::presence::Standing::Granted);
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
    registration_id: &str,
    session: &str,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let mut after: Option<String> = None;
    loop {
        let page = list_after(registry, after.as_deref(), SWEEP_LIMIT)?;
        if let Some(found) = page.iter().find(|subject| {
            subject.bound_registration_id.as_deref() == Some(registration_id)
                && subject.session.is_none()
                && subject.state == State::Bound
                && subject.exited_at.is_none()
                && subject.ended_externally_at.is_none()
        }) {
            if !matches!(
                attachment(registry, registration_id, session, now_ms)?,
                Attachment::Other(_)
            ) {
                // The stored record is the one the transition is checked against.
                let mut subject = read_recorded(registry, &found.subject_id)?;
                subject.session = Some(session.to_string());
                subject.recorded_at = format_millis(now_ms);
                write(registry, &subject)?;
            }
            return Ok(());
        }
        match page.last() {
            Some(last) if page.len() == SWEEP_LIMIT => after = Some(last.subject_id.clone()),
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
    holder: &Subject,
    verifier: &dyn crate::control_plane::registry::process::ProcessVerifier,
    now_ms: i64,
) -> Result<bool, String> {
    use crate::control_plane::registry::process::{ProcessVerification, verify_identity};
    match verify_identity(holder.process.as_ref(), verifier) {
        ProcessVerification::Absent | ProcessVerification::Mismatched => {
            record_exit(registry, &holder.subject_id, now_ms)?;
            Ok(true)
        }
        ProcessVerification::Verified | ProcessVerification::Unavailable => Ok(false),
    }
}

/// How `registration_id` stands against `session`, read before any verification so a
/// registration that cannot bind costs no process inspection.
pub fn attachment(
    registry: &Registry,
    registration_id: &str,
    session: &str,
    now_ms: i64,
) -> Result<Attachment, String> {
    let mut after: Option<String> = None;
    let mut found = Attachment::Free;
    loop {
        let page = list_after(registry, after.as_deref(), SWEEP_LIMIT)?;
        for subject in &page {
            if subject.session.as_deref() != Some(session)
                || !is_attached(registry, subject, now_ms)?
            {
                continue;
            }
            if subject.bound_registration_id.as_deref() == Some(registration_id) {
                return Ok(Attachment::Own(Box::new(subject.clone())));
            }
            found = Attachment::Other(Box::new(subject.clone()));
        }
        match page.last() {
            Some(last) if page.len() == SWEEP_LIMIT => after = Some(last.subject_id.clone()),
            _ => return Ok(found),
        }
    }
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
    enrollment_id: &str,
    registration_id: &str,
    process: &ProcessIdentity,
    session: &str,
    launch: LaunchContext,
    now_ms: i64,
) -> Result<String, String> {
    let _transition = registry.transition();
    let mut prior: Option<Subject> = None;
    let mut after: Option<String> = None;
    loop {
        let page = list_after(registry, after.as_deref(), SWEEP_LIMIT)?;
        for subject in &page {
            if subject.session.as_deref() != Some(session) || subject.state != State::Bound {
                continue;
            }
            if is_attached(registry, subject, now_ms)? {
                return if subject.bound_registration_id.as_deref() == Some(registration_id) {
                    Ok(subject.subject_id.clone())
                } else {
                    Err("the session is already attached to another live process".into())
                };
            }
            if prior
                .as_ref()
                .is_none_or(|p| p.recorded_at < subject.recorded_at)
            {
                prior = Some(subject.clone());
            }
        }
        match page.last() {
            Some(last) if page.len() == SWEEP_LIMIT => after = Some(last.subject_id.clone()),
            _ => break,
        }
    }
    // A reattachment continues the prior logical subject; a new session starts one.
    let (logical, generation, managed) = match &prior {
        Some(prior) if prior.identity_unresolved => {
            return Err(
                "the session's prior subject has unresolved identity; resolve it before reattaching"
                    .into(),
            );
        }
        Some(prior) => (
            Some(
                prior
                    .logical_subject_id
                    .clone()
                    .unwrap_or_else(|| prior.subject_id.clone()),
            ),
            prior
                .execution_generation
                .unwrap_or(1)
                .checked_add(1)
                .ok_or("execution generation is exhausted")?,
            prior.managed.clone(),
        ),
        None => (None, 1, ManagedMetadata::default()),
    };
    let subject_id = random_uuid();
    let logical = logical.unwrap_or_else(|| subject_id.clone());
    let subject = Subject {
        observation: None,
        pane_absence: None,
        ended_externally_at: None,
        ended_evidence: None,
        version: SUBJECT_VERSION,
        subject_id: subject_id.clone(),
        logical_subject_id: Some(logical),
        execution_generation: Some(generation),
        identity_unresolved: false,
        spawn_request_id: enrollment_id.to_string(),
        origin: Origin::Enrolled,
        state: State::Bound,
        managed,
        launch,
        session: Some(session.to_string()),
        successor: None,
        correlation: None,
        process: Some(process.clone()),
        bound_registration_id: Some(registration_id.to_string()),
        signalled_at: None,
        exited_at: None,
        pane_cleanup: None,
        recorded_at: format_millis(now_ms),
    };
    write(registry, &subject)?;
    Ok(subject_id)
}

/// Records the replacement dispatched for this process generation, before the
/// dispatch. A generation that already has a successor refuses, so no second
/// replacement can be launched for it.
pub fn set_successor(
    registry: &Registry,
    subject_id: &str,
    successor: &str,
    now_ms: i64,
) -> Result<(), String> {
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, subject_id)?;
    if let Some(existing) = &subject.successor {
        return Err(format!(
            "this process is already superseded by spawn {existing}"
        ));
    }
    subject.successor = Some(successor.to_string());
    subject.recorded_at = format_millis(now_ms);
    write(registry, &subject)
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
/// Radar's own bootstrap and token variables are dropped, since a replacement is
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
                SUBJECT_ENV,
                CORRELATION_ENV,
                MANAGED_SUBJECT_ENV,
                MANAGED_LAUNCH_ENV,
                MANAGED_GENERATION_ENV,
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
pub fn public_view(subject: &Subject) -> Value {
    json!({
        "subject_id": subject.logical_subject_id.as_deref().unwrap_or(&subject.subject_id),
        "execution_id": subject.subject_id,
        "execution_generation": subject.execution_generation.unwrap_or(1),
        "identity_unresolved": subject.identity_unresolved,
        "managed": subject.managed,
        "spawn_request_id": subject.spawn_request_id,
        "state": subject.state,
        "process": subject.process,
        "observation": subject.observation,
        "pane_absence": subject.pane_absence,
        "ended_externally_at": subject.ended_externally_at,
        "ended_evidence": subject.ended_evidence,
        "pane_cleanup": subject.pane_cleanup,
        "bound_registration_id": subject.bound_registration_id,
        "signalled_at": subject.signalled_at,
        "exited_at": subject.exited_at,
        "recorded_at": subject.recorded_at,
        "origin": subject.origin,
        "session": subject.session,
        "successor": subject.successor,
    })
}

/// Subjects in id order after `after`, at most `limit`, for a bounded listing.
/// Only names are sorted; each returned record is read and validated.
/// Assigns legacy execution records a stable logical subject and generation.
///
/// The mapping follows only a predecessor's exact successor request to the
/// spawn edge that names the exact child execution record. Missing, forked or
/// cyclic links are left as separate roots; they never cause a merge or replay.
pub fn migrate_identity(registry: &Registry, now_ms: i64) -> Result<(), String> {
    let _transition = registry.transition();
    let records = registry
        .events
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .all_facts();
    if records.iter().all(|(_, facts)| {
        facts.logical_subject_id.is_some() && facts.execution_generation.is_some()
    }) {
        return Ok(());
    }
    let records: BTreeMap<_, _> = records.into_iter().collect();
    let mut after = None;
    let mut edges = BTreeMap::new();
    loop {
        let page = registry.spawn_edges_after(after.as_deref(), 256)?;
        if page.is_empty() {
            break;
        }
        for edge in &page {
            edges.insert(edge.request_id.clone(), edge.subject_id.clone());
        }
        after = page.last().map(|edge| edge.request_id.clone());
        if page.len() < 256 {
            break;
        }
    }

    let mut outgoing = BTreeMap::new();
    let mut incoming_count = BTreeMap::<String, usize>::new();
    let mut unresolved = BTreeSet::new();
    for (execution_id, facts) in &records {
        let Some(successor_request) = facts.successor.as_ref() else {
            continue;
        };
        let Some(Some(next_id)) = edges.get(successor_request) else {
            unresolved.insert(execution_id.clone());
            continue;
        };
        let Some(next) = records.get(next_id) else {
            unresolved.insert(execution_id.clone());
            continue;
        };
        if next.spawn_request_id != *successor_request {
            unresolved.insert(execution_id.clone());
            unresolved.insert(next_id.clone());
            continue;
        }
        outgoing.insert(execution_id.clone(), next_id.clone());
        *incoming_count.entry(next_id.clone()).or_default() += 1;
    }
    // A fork is ambiguous: retain no selected edge and mark the affected root
    // and child unresolved rather than choosing a successor by sort order.
    for (parent, child) in &outgoing {
        if incoming_count.get(child).is_some_and(|count| *count > 1) {
            unresolved.insert(parent.clone());
            unresolved.insert(child.clone());
        }
    }
    outgoing.retain(|_, child| incoming_count.get(child) == Some(&1));

    // Find cycles before constructing roots, then discard their links so every
    // affected execution gets an independent, non-speculative identity.
    let mut cyclic = BTreeSet::new();
    for start in records.keys() {
        let mut path = Vec::new();
        let mut positions = BTreeMap::new();
        let mut current = start;
        while let Some(next) = outgoing.get(current) {
            if let Some(position) = positions.get(current) {
                cyclic.extend(path[*position..].iter().cloned());
                break;
            }
            positions.insert(current.clone(), path.len());
            path.push(current.clone());
            current = next;
        }
    }
    unresolved.extend(cyclic.iter().cloned());
    outgoing.retain(|parent, child| !cyclic.contains(parent) && !cyclic.contains(child));
    let mut incoming = BTreeMap::<String, String>::new();
    for (parent, child) in &outgoing {
        incoming.insert(child.clone(), parent.clone());
    }

    let mut visited = BTreeSet::new();
    let mut mappings = Vec::new();
    for execution_id in records.keys() {
        if cyclic.contains(execution_id)
            || visited.contains(execution_id)
            || incoming.contains_key(execution_id)
        {
            continue;
        }
        let mut chain = Vec::new();
        let mut current = execution_id.clone();
        loop {
            if !visited.insert(current.clone()) {
                break;
            }
            chain.push(current.clone());
            match outgoing.get(&current) {
                Some(next) => current = next.clone(),
                None => break,
            }
        }
        if chain.is_empty() {
            continue;
        }
        let logical_id = chain[0].clone();
        let expected: Option<Vec<_>> = chain
            .iter()
            .enumerate()
            .map(|(index, id)| {
                Some((
                    id.clone(),
                    logical_id.clone(),
                    u32::try_from(index).ok()?.checked_add(1)?,
                ))
            })
            .collect();
        let Some(expected) = expected else {
            unresolved.extend(chain.iter().cloned());
            continue;
        };
        // A partially completed migration is safe to resume only when every
        // already-written identity agrees with the deterministic chain mapping.
        if expected.iter().any(|(id, logical, generation)| {
            let facts = &records[id];
            facts
                .logical_subject_id
                .as_ref()
                .is_some_and(|old| old != logical)
                || facts
                    .execution_generation
                    .is_some_and(|old| old != *generation)
        }) {
            unresolved.extend(chain.iter().cloned());
            continue;
        }
        mappings.extend(expected.into_iter().filter(|(id, _, _)| {
            records[id].logical_subject_id.is_none() || records[id].execution_generation.is_none()
        }));
    }
    // Cyclic records were not visited by a root walk; assign each a distinct root.
    for execution_id in cyclic {
        let facts = &records[&execution_id];
        if facts.logical_subject_id.is_none() && facts.execution_generation.is_none() {
            mappings.push((execution_id.clone(), execution_id, 1));
        }
    }

    let pending_unresolved: Vec<_> = unresolved
        .iter()
        .filter(|id| {
            records.get(id.as_str()).is_some_and(|facts| {
                facts.logical_subject_id.is_some()
                    && facts.execution_generation.is_some()
                    && !facts.identity_unresolved
            })
        })
        .cloned()
        .collect();
    if mappings.is_empty() && pending_unresolved.is_empty() {
        return Ok(());
    }
    let at = format_millis(now_ms);
    let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    for (execution_id, logical_subject_id, execution_generation) in mappings {
        log.append(
            &execution_id,
            &at,
            crate::control_plane::events::Source::IdentityMigration,
            crate::control_plane::events::Transition::IdentityMapped {
                logical_subject_id,
                execution_generation,
                identity_unresolved: unresolved.contains(&execution_id),
            },
        )
        .map_err(|error| {
            let prior = records.get(&execution_id).expect("migration record");
            format!(
                "migrate lifecycle execution {execution_id} (logical={:?}, generation={:?}): {error}",
                prior.logical_subject_id, prior.execution_generation
            )
        })?;
    }
    for execution_id in pending_unresolved {
        log.append(
            &execution_id,
            &at,
            crate::control_plane::events::Source::IdentityMigration,
            crate::control_plane::events::Transition::IdentityUnresolved,
        )
        .map_err(|error| format!("mark lifecycle execution {execution_id} unresolved: {error}"))?;
    }
    Ok(())
}

pub fn list_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<Subject>, String> {
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
            .cloned(),
    );
    ids.sort();
    ids.dedup();
    ids.truncate(limit);
    ids.iter().map(|id| read_recorded(registry, id)).collect()
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
        .cloned()
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
    for id in page {
        if observe_exit_from(registry, &id, verifier, now_ms, Observer::Sweep) == Ok(true) {
            exited += 1;
        }
    }
    Ok((exited, next))
}

/// Revokes a launch that is still unbound, and says whether it did.
fn revoke(registry: &Registry, subject_id: &str, now_ms: i64) -> Result<bool, String> {
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, subject_id)?;
    if !matches!(subject.state, State::Pending | State::Captured) {
        return Ok(false);
    }
    subject.state = State::Revoked;
    subject.correlation = None;
    subject.recorded_at = format_millis(now_ms);
    write(registry, &subject)?;
    Ok(true)
}

/// Sends SIGTERM to the captured process through its pidfd, if one is captured
/// and not yet signalled, and waits boundedly for the exit. The registry lock is
/// released during the wait.
fn terminate(
    registry: &Registry,
    subject_id: &str,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
    revoked: bool,
) -> Result<Termination, String> {
    let (descriptor, signal) = match stage(registry, subject_id, verifier, now_ms)? {
        Stage::Settled(exit) => {
            return Ok(Termination {
                revoked,
                signal_attempted: false,
                exit,
            });
        }
        Stage::Watch { descriptor, signal } => (descriptor, signal),
    };
    let mut signal_attempted = false;
    if signal {
        signal_attempted = true;
        if let Err(error) = process_control::terminate(&descriptor) {
            return Ok(Termination {
                revoked,
                signal_attempted,
                exit: Exit::Unverified(format!("SIGTERM could not be sent: {error}")),
            });
        }
    }
    let exit = if process_control::exited_within(&descriptor, EXIT_DEADLINE) {
        record_exit(registry, subject_id, now_ms)?;
        Exit::Exited
    } else {
        Exit::Unverified("exit was not verified before the deadline".into())
    };
    Ok(Termination {
        revoked,
        signal_attempted,
        exit,
    })
}

/// The locked half of a stop: the signal intent, and the descriptor to wait on.
/// Settles without waiting when there is no captured process to signal.
fn stage(
    registry: &Registry,
    subject_id: &str,
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Result<Stage, String> {
    let subject = read_recorded(registry, subject_id)?;
    let Some(process) = subject.process.clone() else {
        return Ok(Stage::Settled(Exit::Unresolved));
    };
    if subject.exited_at.is_some() {
        return Ok(Stage::Settled(Exit::Exited));
    }
    let acquired = process_control::acquire(&process, verifier);
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, subject_id)?;
    if subject.process.as_ref() != Some(&process) {
        return Ok(Stage::Settled(Exit::Unresolved));
    }
    if subject.exited_at.is_some() {
        return Ok(Stage::Settled(Exit::Exited));
    }
    let descriptor = match acquired {
        Ok(Acquired::Process(descriptor)) => descriptor,
        Ok(Acquired::Absent | Acquired::Replaced) => {
            record_exit_locked(registry, &mut subject, now_ms)?;
            return Ok(Stage::Settled(Exit::Exited));
        }
        // An identity that cannot be read is not a process to signal.
        Err(_) => return Ok(Stage::Settled(Exit::Unresolved)),
    };
    let signal = subject.signalled_at.is_none();
    if signal {
        subject.signalled_at = Some(format_millis(now_ms));
        subject.recorded_at = format_millis(now_ms);
        write(registry, &subject)?;
    }
    Ok(Stage::Watch { descriptor, signal })
}

/// The outcome of cancelling several executions under one window.
pub struct Cancelled {
    /// Executions whose exit was verified and recorded.
    pub stopped: Vec<String>,
    /// Executions still alive when the window closed, by subject id. A process
    /// that could not be signalled is one of these: it is not known to be gone.
    pub survivors: Vec<String>,
}

/// Cancels every named execution together: each is fenced, signalled once, and
/// then the whole set is waited on under a single deadline.
///
/// One window rather than one per execution is what makes ending a place holding
/// several agents bounded: the operator waits one window, not a series of them.
pub fn cancel_many(
    registry: &Registry,
    subject_ids: &[String],
    verifier: &dyn ProcessVerifier,
    now_ms: i64,
) -> Cancelled {
    let mut report = Cancelled {
        stopped: Vec::new(),
        survivors: Vec::new(),
    };
    // Descriptors are held for the shared wait, so nothing is signalled twice and
    // each wait is on the process that was signalled.
    let mut watching: Vec<(String, std::os::fd::OwnedFd)> = Vec::new();
    for subject_id in subject_ids {
        match stage(registry, subject_id, verifier, now_ms) {
            // Already verified gone: the exit is recorded and there is nothing to
            // wait for.
            Ok(Stage::Settled(Exit::Exited)) => report.stopped.push(subject_id.clone()),
            // No captured process, or one already recorded as ended.
            Ok(Stage::Settled(_)) => match record_exit(registry, subject_id, now_ms) {
                Ok(()) => report.stopped.push(subject_id.clone()),
                Err(_) => report.survivors.push(subject_id.clone()),
            },
            Ok(Stage::Watch { descriptor, signal }) => {
                if signal && process_control::terminate(&descriptor).is_err() {
                    report.survivors.push(subject_id.clone());
                    continue;
                }
                watching.push((subject_id.clone(), descriptor));
            }
            // An identity that cannot be read is not a process to signal and not
            // one to call gone.
            Err(_) => report.survivors.push(subject_id.clone()),
        }
    }
    let descriptors: Vec<&std::os::fd::OwnedFd> =
        watching.iter().map(|(_, descriptor)| descriptor).collect();
    let exited = process_control::exited_all_within(&descriptors, EXIT_DEADLINE);
    for ((subject_id, _), exited) in watching.iter().zip(exited.iter()) {
        if *exited && record_exit(registry, subject_id, now_ms).is_ok() {
            report.stopped.push(subject_id.clone());
        } else {
            report.survivors.push(subject_id.clone());
        }
    }
    report
}

fn record_exit(registry: &Registry, subject_id: &str, now_ms: i64) -> Result<(), String> {
    let _transition = registry.transition();
    let mut subject = read_recorded(registry, subject_id)?;
    record_exit_locked(registry, &mut subject, now_ms)
}

fn record_exit_locked(
    registry: &Registry,
    subject: &mut Subject,
    now_ms: i64,
) -> Result<(), String> {
    if subject.exited_at.is_none() {
        subject.exited_at = Some(format_millis(now_ms));
        subject.recorded_at = format_millis(now_ms);
        write(registry, subject)?;
    }
    Ok(())
}

/// The command a tracked launch runs: the bootstrap, carrying the target program
/// and the private correlation. The target's executable, argv, cwd and the
/// caller's environment overlay are passed through unchanged.
pub fn wrap(bootstrap: &Bootstrap, ticket: &Ticket, target: LaunchRequest) -> LaunchRequest {
    let LaunchRequest {
        pane_id,
        executable,
        argv,
        spawn_token,
        cwd,
        mut env,
    } = target;
    let mut wrapped = vec![
        BOOTSTRAP_COMMAND.to_string(),
        "--socket".to_string(),
        bootstrap.socket.clone(),
        "--".to_string(),
        executable,
    ];
    wrapped.extend(argv);
    env.insert(SUBJECT_ENV.to_string(), ticket.subject_id.clone());
    env.insert(CORRELATION_ENV.to_string(), ticket.correlation.clone());
    if let Some(parent_subject_id) = &ticket.parent_subject_id {
        env.insert(
            MANAGED_PARENT_SUBJECT_ENV.to_string(),
            parent_subject_id.clone(),
        );
    }
    LaunchRequest {
        pane_id,
        executable: bootstrap.executable.clone(),
        argv: wrapped,
        spawn_token,
        cwd,
        env,
    }
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

/// Runs as the bootstrap inside a tracked pane: `--socket <path> -- <program>
/// [argument...]`.
///
/// Claims this process with the daemon, then replaces itself with the program.
/// Returns only on failure, and an unreleased bootstrap never runs the program.
pub fn run_bootstrap(socket: &str, program: &[String]) -> i32 {
    if program.is_empty() {
        eprintln!("radar {BOOTSTRAP_COMMAND}: no program to run");
        return 2;
    }
    if let Err(message) = release(Path::new(socket)) {
        eprintln!("radar {BOOTSTRAP_COMMAND}: {message}");
        return 111;
    }
    let error = Command::new(&program[0])
        .args(&program[1..])
        .env_remove(SUBJECT_ENV)
        .env_remove(CORRELATION_ENV)
        .exec();
    eprintln!("radar {BOOTSTRAP_COMMAND}: could not run the program: {error}");
    127
}

fn release(socket: &Path) -> Result<(), String> {
    let subject = std::env::var(SUBJECT_ENV).map_err(|_| "no launch subject".to_string())?;
    let correlation =
        std::env::var(CORRELATION_ENV).map_err(|_| "no launch correlation".to_string())?;
    let process = LocalProcfsVerifier
        .inspect(std::process::id() as i32)?
        .ok_or_else(|| "this process has no identity to claim".to_string())?;
    let result = exchange(
        socket,
        BOOTSTRAP_METHOD,
        json!({"subject_id": subject, "correlation": correlation, "process": process}),
    )?;
    match result.get("released") {
        Some(Value::Bool(true)) => Ok(()),
        _ => Err("launch was not released".into()),
    }
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
    let request = json!({
        "version": PROTOCOL_VERSION,
        "id": random_uuid(),
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
    let response: Value =
        serde_json::from_str(&line).map_err(|_| "the daemon's answer is unreadable".to_string())?;
    match response.get("result") {
        Some(result) => Ok(result.clone()),
        None => Err("the daemon refused the request".into()),
    }
}

fn write(registry: &Registry, subject: &Subject) -> Result<(), String> {
    write_attempt(registry, subject)?;
    note_attempt(registry, subject)
}

fn write_attempt(registry: &Registry, subject: &Subject) -> Result<(), String> {
    use crate::control_plane::events::{Facts, Source, Transition};
    let mut log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
    let previous = log.facts(&subject.subject_id).cloned();
    let previous = if let Some(facts) = previous {
        Some(facts)
    } else {
        let legacy_path = path(registry, &subject.subject_id);
        let legacy = if legacy_path.try_exists().map_err(|e| e.to_string())? {
            let bytes = read_record_file(&legacy_path, MAX_SUBJECT_BYTES)?;
            Some(
                serde_json::from_slice::<Subject>(&bytes)
                    .map_err(|_| "invalid legacy lifecycle subject")?,
            )
        } else {
            None
        };
        let seed = legacy.as_ref().unwrap_or(subject);
        let directory = registry.subjects().join("private");
        crate::control_plane::registry::prepare_directory(&directory)?;
        let bytes = serde_json::to_vec_pretty(seed)
            .map_err(|_| "cannot encode private lifecycle recipe")?;
        write_atomic(
            &directory,
            &recipe_path(registry, &subject.subject_id),
            &bytes,
            MAX_SUBJECT_BYTES,
        )?;
        if let Some(legacy) = legacy {
            let facts = Facts::from_subject(&legacy);
            log.append(
                &subject.subject_id,
                &legacy.recorded_at,
                Source::RecoveryImport,
                Transition::Imported(facts.clone()),
            )?;
            Some(facts)
        } else {
            let facts = Facts::from_subject(subject);
            let (source, transition) = match subject.origin {
                Origin::Spawned => (Source::Launch, Transition::Created(facts)),
                Origin::Enrolled => (Source::Operator, Transition::Enrolled(facts)),
            };
            return log.append(
                &subject.subject_id,
                &subject.recorded_at,
                source,
                transition,
            );
        }
    };
    let previous = previous.expect("baseline established");
    let target = Facts::from_subject(subject);
    if previous == target {
        return Ok(());
    }
    let (source, transition) = if previous.state != target.state {
        match target.state {
            State::Captured => (
                Source::Bootstrap,
                Transition::Captured(target.process.clone().ok_or("capture has no process")?),
            ),
            State::Bound => (
                Source::Registration,
                Transition::Bound {
                    registration_id: target
                        .bound_registration_id
                        .clone()
                        .ok_or("binding has no agent")?,
                },
            ),
            State::Revoked => (Source::ProcessControl, Transition::Revoked),
            State::Pending => return Err("cannot return a subject to pending".into()),
        }
    } else if previous.observation != target.observation {
        (
            Source::ProcessObservation,
            Transition::Observed(
                target
                    .observation
                    .clone()
                    .ok_or("cannot remove a process observation")?,
            ),
        )
    } else if previous.exited_at != target.exited_at {
        (Source::ProcessObservation, Transition::ExitVerified)
    } else if previous.pane_cleanup != target.pane_cleanup {
        (
            Source::PaneControl,
            Transition::PaneCleanup(
                target
                    .pane_cleanup
                    .clone()
                    .ok_or("cannot remove a pane cleanup outcome")?,
            ),
        )
    } else if previous.signalled_at != target.signalled_at {
        (Source::ProcessControl, Transition::SignalRequested)
    } else if previous.successor != target.successor {
        (
            Source::Operator,
            Transition::SuccessorReserved {
                spawn_request_id: target
                    .successor
                    .clone()
                    .ok_or("cannot remove a successor")?,
            },
        )
    } else if previous.ended_externally_at != target.ended_externally_at {
        (
            Source::ProcessObservation,
            Transition::EndedExternally(
                target
                    .ended_evidence
                    .clone()
                    .ok_or("an ended-externally outcome needs its evidence")?,
            ),
        )
    } else if previous.pane_absence != target.pane_absence {
        (
            Source::ProcessObservation,
            Transition::AbsencePending(target.pane_absence.clone()),
        )
    } else if previous.session.is_none() && target.session.is_some() {
        (
            Source::Registration,
            Transition::SessionRecorded {
                session: target
                    .session
                    .clone()
                    .ok_or("a recorded session is named")?,
            },
        )
    } else {
        return Err("unsupported lifecycle transition".into());
    };
    log.append_checked(
        &subject.subject_id,
        &subject.recorded_at,
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

fn path(registry: &Registry, subject_id: &str) -> PathBuf {
    registry.subjects().join(format!("{subject_id}.json"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    const SPAWN: &str = "5b2e8c41-0a7d-4f36-b1d9-6c3a7e5f8b1d";
    const AGENT: &str = "7d1f3a2b-4c5e-4f60-8a9b-0c1d2e3f4a5b";

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

    /// The kernel's view of a bootstrap that was started with `launch`.
    fn started_as(launch: &LaunchContext) -> Effective {
        let mut cmdline = vec![
            "radar".to_string(),
            BOOTSTRAP_COMMAND.to_string(),
            "--socket".to_string(),
            "/run/radar.sock".to_string(),
            "--".to_string(),
            launch.executable.clone(),
        ];
        cmdline.extend(launch.argv.iter().cloned());
        Effective {
            cmdline,
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
        let subject = ticket.subject_id.as_str();

        let cancelled = cancel(&registry, subject, &Uninspected, 2).expect("cancel");
        assert_eq!(
            cancelled,
            Termination {
                revoked: true,
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
            3,
        );
        assert_eq!(claim, Err(NOT_AWAITING.to_string()));

        // Revocation is durable: a reopened registry still refuses every bind.
        let reopened = scratch.registry();
        assert_eq!(
            read(&reopened, subject)
                .expect("read")
                .expect("subject")
                .state,
            State::Revoked
        );
        assert_eq!(
            fence_registration(&reopened, subject, Some(&identity(7)), Some(AGENT), 4),
            Err("launch was cancelled".to_string())
        );
        // A repeated cancel revokes nothing further and signals nothing.
        let again = cancel(&reopened, subject, &Uninspected, 5).expect("cancel");
        assert_eq!(
            again,
            Termination {
                revoked: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
    }

    fn legacy_facts(
        spawn_request_id: &str,
        successor: Option<&str>,
    ) -> crate::control_plane::events::Facts {
        crate::control_plane::events::Facts {
            logical_subject_id: None,
            execution_generation: None,
            identity_unresolved: false,
            managed: ManagedMetadata::default(),
            spawn_request_id: spawn_request_id.to_string(),
            origin: Origin::Spawned,
            state: State::Bound,
            session: None,
            successor: successor.map(str::to_string),
            process: None,
            observation: None,
            pane_absence: None,
            ended_externally_at: None,
            ended_evidence: None,
            pane_cleanup: None,
            bound_registration_id: None,
            signalled_at: None,
            exited_at: None,
            recorded_at: "2026-01-01T00:00:00.000Z".into(),
        }
    }

    fn record_legacy_execution(
        registry: &Registry,
        execution_id: &str,
        spawn_request_id: &str,
        successor: Option<&str>,
    ) {
        registry
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .append(
                execution_id,
                "2026-01-01T00:00:00.000Z",
                crate::control_plane::events::Source::Launch,
                crate::control_plane::events::Transition::Created(legacy_facts(
                    spawn_request_id,
                    successor,
                )),
            )
            .expect("legacy execution");
    }

    fn record_exact_spawn(
        registry: &Registry,
        parent: &crate::control_plane::registry::Registration,
        request_id: &str,
        execution_id: &str,
    ) {
        registry
            .record_spawn(
                &crate::control_plane::registry::SpawnRequest {
                    request_id: request_id.into(),
                    parent: parent.registration_id.clone(),
                    parent_session_id: "2a4c6e80-1357-4b9d-8f01-23456789abcd".into(),
                    parent_subject_id: None,
                },
                1,
            )
            .expect("spawn intent");
        registry
            .attach_subject(request_id, execution_id)
            .expect("exact execution attachment");
    }

    #[test]
    fn identity_migration_follows_only_exact_unique_successor_edges_and_is_idempotent() {
        let scratch = Scratch::new("identity-migration");
        let registry = scratch.registry();
        let root = random_uuid();
        let next = random_uuid();
        let isolated = random_uuid();
        let missing = random_uuid();
        let missing_request = random_uuid();
        let fork_a = random_uuid();
        let fork_b = random_uuid();
        let fork_child = random_uuid();
        let cycle_a = random_uuid();
        let cycle_b = random_uuid();
        let parent = registry
            .register(
                &crate::control_plane::registry::RegistrationRequest {
                    source: "migration-test".into(),
                    incarnation: random_uuid(),
                    session: Some("2a4c6e80-1357-4b9d-8f01-23456789abcd".into()),
                    owner: None,
                    run: None,
                    label: None,
                    location: None,
                    process: None,
                    launch: None,
                    spawn_token: None,
                },
                1,
            )
            .expect("parent registration");
        registry
            .publish_context(
                &crate::control_plane::registry::ContextRequest {
                    registration_id: parent.registration_id.clone(),
                    publisher: crate::control_plane::registry::PublisherIdentity {
                        source: parent.request.source.clone(),
                        incarnation: parent.request.incarnation.clone(),
                        reporting_owner: None,
                    },
                    writer_handle: None,
                    replace: None,
                    sequence: 1,
                    lease_ms: Some(10_000),
                    observed_at: None,
                    context: crate::control_plane::registry::ContextValue {
                        session: Some("2a4c6e80-1357-4b9d-8f01-23456789abcd".into()),
                        launch_identity: None,
                    },
                },
                1,
            )
            .expect("publish parent context");
        let root_request = random_uuid();
        let next_request = random_uuid();
        let fork_request = random_uuid();
        let cycle_request_a = random_uuid();
        let cycle_request_b = random_uuid();

        record_legacy_execution(&registry, &root, &root_request, Some(&next_request));
        record_legacy_execution(&registry, &next, &next_request, None);
        record_legacy_execution(&registry, &isolated, &random_uuid(), None);
        record_legacy_execution(&registry, &missing, &random_uuid(), Some(&missing_request));
        record_legacy_execution(&registry, &fork_a, &random_uuid(), Some(&fork_request));
        record_legacy_execution(&registry, &fork_b, &random_uuid(), Some(&fork_request));
        record_legacy_execution(&registry, &fork_child, &fork_request, None);
        record_legacy_execution(
            &registry,
            &cycle_a,
            &cycle_request_a,
            Some(&cycle_request_b),
        );
        record_legacy_execution(
            &registry,
            &cycle_b,
            &cycle_request_b,
            Some(&cycle_request_a),
        );
        record_exact_spawn(&registry, &parent, &next_request, &next);
        record_exact_spawn(&registry, &parent, &fork_request, &fork_child);
        record_exact_spawn(&registry, &parent, &cycle_request_a, &cycle_a);
        record_exact_spawn(&registry, &parent, &cycle_request_b, &cycle_b);

        migrate_identity(&registry, 2).expect("migrate legacy identities");
        let log = registry.events.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            log.facts(&root)
                .and_then(|facts| facts.logical_subject_id.as_deref()),
            Some(root.as_str())
        );
        assert_eq!(
            log.facts(&root)
                .and_then(|facts| facts.execution_generation),
            Some(1)
        );
        assert_eq!(
            log.facts(&next)
                .and_then(|facts| facts.logical_subject_id.as_deref()),
            Some(root.as_str())
        );
        assert_eq!(
            log.facts(&next)
                .and_then(|facts| facts.execution_generation),
            Some(2)
        );
        for execution in [
            &isolated,
            &missing,
            &fork_a,
            &fork_b,
            &fork_child,
            &cycle_a,
            &cycle_b,
        ] {
            let facts = log.facts(execution).expect("execution facts");
            assert_eq!(
                facts.logical_subject_id.as_deref(),
                Some(execution.as_str())
            );
            assert_eq!(facts.execution_generation, Some(1));
            assert_eq!(facts.identity_unresolved, execution != &isolated);
        }
        let migrated_tail = log.tail();
        drop(log);

        let reopened = scratch.registry();
        migrate_identity(&reopened, 3).expect("repeat migration");
        assert_eq!(
            reopened
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .tail(),
            migrated_tail,
            "reopen and rerun must not append duplicate identity mappings"
        );
        assert_eq!(
            reopened
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .facts(&next)
                .and_then(|facts| facts.logical_subject_id.as_deref()),
            Some(root.as_str())
        );
    }

    #[test]
    fn accepted_managed_launch_metadata_is_durable_and_inherited_by_successors() {
        let scratch = Scratch::new("managed-metadata");
        let registry = scratch.registry();
        let metadata = ManagedMetadata {
            owner: Some("herdr".into()),
            run: Some("run-17".into()),
            label: Some("reviewer".into()),
            role: Some("worker".into()),
            parent_subject_id: Some("01a10c77-8a6b-7035-8a0e-b1fa607bb507".into()),
        };
        let first = create_with_intent(&registry, SPAWN, launch(), None, None, metadata.clone(), 1)
            .expect("accepted launch intent");
        assert_eq!(first.parent_subject_id, metadata.parent_subject_id);
        let persisted = read(&registry, &first.subject_id)
            .expect("read subject")
            .expect("subject");
        assert_eq!(persisted.managed, metadata);
        assert_eq!(public_view(&persisted)["managed"]["owner"], "herdr");

        let successor = create_with_intent(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((first.subject_id.clone(), 2)),
            persisted.managed.clone(),
            2,
        )
        .expect("successor intent");
        assert_eq!(successor.parent_subject_id, metadata.parent_subject_id);
        assert_eq!(
            read(&registry, &successor.subject_id)
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
    fn public_logical_subject_resolves_to_its_latest_execution() {
        let scratch = Scratch::new("logical-current");
        let registry = scratch.registry();
        let first = create(&registry, SPAWN, launch(), None, 1).expect("first execution");
        let logical_id = first.subject_id.clone();
        let second = create_with_identity(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((logical_id.clone(), 2)),
            2,
        )
        .expect("successor execution");

        let current = read_logical(&registry, &logical_id)
            .expect("read logical subject")
            .expect("current execution");
        assert_eq!(current.subject_id, second.subject_id);
        assert_eq!(
            current.logical_subject_id.as_deref(),
            Some(logical_id.as_str())
        );
        assert_eq!(current.execution_generation, Some(2));
        let view = public_view(&current);
        assert_eq!(view["subject_id"], logical_id);
        assert_eq!(view["execution_id"], second.subject_id);
        assert_eq!(view["execution_generation"], 2);

        let listed = list_logical_after(&registry, None, 10).expect("logical list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].subject_id, second.subject_id);
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
        assert_eq!(
            record.current_claim.as_deref(),
            Some(first.subject_id.as_str())
        );
        assert_eq!(record.current_generation, 1);

        // A successor moves the pointer, and the subject itself is unchanged.
        let second = create_with_identity(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((logical_id.clone(), 2)),
            2,
        )
        .expect("successor attempt");
        let record = read_subject_record(&registry, &logical_id)
            .expect("read record")
            .expect("the subject still exists");
        assert_eq!(
            record.current_claim.as_deref(),
            Some(second.subject_id.as_str())
        );
        assert_eq!(record.current_generation, 2);
        assert_eq!(
            record.recorded_at,
            read(&registry, &first.subject_id)
                .expect("read")
                .expect("attempt")
                .recorded_at
        );

        // An earlier attempt reporting late does not displace the current one.
        let earlier = read(&registry, &first.subject_id)
            .expect("read")
            .expect("attempt");
        note_attempt(&registry, &earlier).expect("note the earlier attempt");
        let record = read_subject_record(&registry, &logical_id)
            .expect("read record")
            .expect("subject");
        assert_eq!(
            record.current_claim.as_deref(),
            Some(second.subject_id.as_str())
        );
    }

    #[test]
    fn an_attempt_written_before_its_subject_record_promotes_one() {
        let scratch = Scratch::new("subject-promote");
        let registry = scratch.registry();
        // A root attempt names itself, so promotion is proved with a real
        // successor chain whose record is removed before it is read.
        let root = create(&registry, SPAWN, launch(), None, 1).expect("root");
        let logical_id = root.subject_id.clone();
        let successor = create_with_identity(
            &registry,
            &random_uuid(),
            launch(),
            None,
            Some((logical_id.clone(), 2)),
            2,
        )
        .expect("successor");
        let path = subject_record_path(&registry, &logical_id);
        fs::remove_file(&path).expect("remove the record");

        let current = read_logical(&registry, &logical_id)
            .expect("read logical")
            .expect("current attempt");
        assert_eq!(current.subject_id, successor.subject_id);
        let promoted = read_subject_record(&registry, &logical_id)
            .expect("read record")
            .expect("promotion wrote a record");
        assert_eq!(
            promoted.current_claim.as_deref(),
            Some(successor.subject_id.as_str())
        );
        assert_eq!(promoted.current_generation, 2);
    }

    #[test]
    fn a_captured_claim_is_spent_and_survives_reopen() {
        let scratch = Scratch::new("spent");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = ticket.subject_id.as_str();
        capture(
            &registry,
            subject,
            &ticket.correlation,
            &identity(7),
            &started_as(&launch()),
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
                3,
            ),
            Err(NOT_AWAITING.to_string()),
            "a replayed correlation is spent"
        );
        fence_registration(&reopened, subject, Some(&identity(7)), Some(AGENT), 4)
            .expect("the captured identity binds");
        assert_eq!(
            fence_registration(&reopened, subject, Some(&identity(8)), Some(AGENT), 5),
            Err("launch is already bound to another registration".to_string())
        );
    }

    #[test]
    fn a_process_not_started_with_the_requested_launch_is_not_captured() {
        let scratch = Scratch::new("mismatch");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = ticket.subject_id.as_str();

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
                2
            ),
            Err(LAUNCH_MISMATCH.to_string())
        );
        let still_pending = read(&registry, subject).expect("read").expect("subject");
        assert_eq!(still_pending.state, State::Pending);
        assert_eq!(
            still_pending.correlation.as_deref(),
            Some(ticket.correlation.as_str())
        );
    }

    #[test]
    fn a_registration_bound_before_cancel_stops_exactly_its_process() {
        let scratch = Scratch::new("bound-first");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = ticket.subject_id.as_str();
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
            2,
        )
        .expect("capture");
        fence_registration(&registry, subject, Some(&process), Some(AGENT), 3).expect("bind");

        let cancelled = cancel(&registry, subject, &LocalProcfsVerifier, 4).expect("cancel");
        assert_eq!(
            cancelled,
            Termination {
                revoked: false,
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
            read(&registry, subject)
                .expect("read")
                .expect("subject")
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
    fn bound_child(registry: &Registry) -> (String, Child) {
        let (subject_id, child, process) = captured_child(registry);
        fence_registration(registry, &subject_id, Some(&process), Some(AGENT), 2).expect("bind");
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
        let pending = read(&registry, &subject_id)
            .expect("read")
            .expect("subject")
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
        let subject = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.pane_absence.is_none(),
            "a present read clears the window"
        );
        assert!(subject.ended_externally_at.is_none());
        assert!(subject.exited_at.is_none());
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
        let subject = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.ended_externally_at.is_none(),
            "the window has not elapsed"
        );
        assert_eq!(subject.pane_absence.expect("pending").confirmations, 3);
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

        let subject = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.pane_absence.is_none(),
            "the window is consumed by the outcome"
        );
        assert!(subject.ended_externally_at.is_some());
        // A pane's absence is not a process's death, so no exit is claimed.
        assert!(subject.exited_at.is_none());
        let evidence = subject.ended_evidence.expect("evidence summary");
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
        let subject = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            subject.pane_absence.is_none(),
            "an unavailable read is not a confirmation"
        );
        assert!(subject.ended_externally_at.is_none());
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

        let subject = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(subject.exited_at.is_some(), "the verified exit stands");
        assert!(subject.ended_externally_at.is_none());
        assert!(subject.ended_evidence.is_none());
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
        let subject = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(subject.pane_absence.is_none());
        assert!(subject.ended_externally_at.is_none());
    }

    /// Captures a live child as a tracked launch, bound to the test's agent.
    fn captured_child(registry: &Registry) -> (String, Child, ProcessIdentity) {
        let ticket = create(registry, SPAWN, launch(), None, 1).expect("subject");
        let child = Child(Command::new("sleep").arg("300").spawn().expect("sleep"));
        let process = LocalProcfsVerifier
            .inspect(child.0.id() as i32)
            .expect("procfs")
            .expect("the child is alive");
        capture(
            registry,
            &ticket.subject_id,
            &ticket.correlation,
            &process,
            &started_as(&launch()),
            2,
        )
        .expect("capture");
        (ticket.subject_id, child, process)
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
        let holder = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");

        assert!(!release_if_gone(&registry, &holder, &LocalProcfsVerifier, 3).expect("verify"));
        assert!(!release_if_gone(&registry, &holder, &Unreadable, 3).expect("verify"));
        let held = read(&registry, &subject_id)
            .expect("read")
            .expect("subject");
        assert!(
            held.exited_at.is_none(),
            "a live or unreadable holder is not an exit"
        );
    }

    #[test]
    fn a_holder_whose_process_is_gone_or_reused_releases_its_session_as_a_verified_exit() {
        let scratch = Scratch::new("holder-gone");
        let registry = scratch.registry();

        let (gone_id, _gone, _) = captured_child(&registry);
        let holder = read(&registry, &gone_id).expect("read").expect("subject");
        assert!(release_if_gone(&registry, &holder, &Gone, 3).expect("verify"));
        let exited = read(&registry, &gone_id).expect("read").expect("subject");
        assert!(exited.exited_at.is_some());
        assert!(
            exited.ended_externally_at.is_none(),
            "a confirmed exit is not a pane loss"
        );

        let (reused_id, _reused, _) = captured_child(&registry);
        let holder = read(&registry, &reused_id).expect("read").expect("subject");
        assert!(release_if_gone(&registry, &holder, &Reused, 4).expect("verify"));
        assert!(
            read(&registry, &reused_id)
                .expect("read")
                .expect("subject")
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
        let recorded = read(&restarted, &subject).expect("read").expect("subject");
        assert!(recorded.exited_at.is_some());
        assert!(recorded.signalled_at.is_none(), "the sweep never signals");
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
        let kept = read(&registry, &subject).expect("read").expect("subject");
        assert!(kept.exited_at.is_none());
        assert!(kept.signalled_at.is_none());
        let observation = kept.observation.expect("running observation");
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
        let unknown = read(&registry, &subject).unwrap().unwrap();
        assert_eq!(unknown.observation.unwrap().evidence, Evidence::Unavailable);
        assert!(unknown.exited_at.is_none());
        assert!(unknown.signalled_at.is_none());
        assert_eq!(
            observe_exit(&registry, &subject, &LocalProcfsVerifier, 4),
            Ok(false)
        );
        assert_eq!(
            read(&registry, &subject)
                .unwrap()
                .unwrap()
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
        let subject = ticket.subject_id.as_str();

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
        let restored = read(&reopened, subject).expect("read").expect("subject");
        assert_eq!(restored.pane_cleanup, Some(refused));
        assert_eq!(restored.state, State::Revoked);
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

        let subject = read(&registry, &subject_id)
            .expect("read terminal subject")
            .expect("subject");
        assert!(subject.exited_at.is_some());
        assert!(subject.signalled_at.is_some());
        let exits = registry
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .read_after(0, 128)
            .expect("read lifecycle events")
            .into_iter()
            .filter(|event| event.subject_id.as_deref() == Some(subject_id.as_str()))
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
                .send(cancel(&independent, &other.subject_id, &Uninspected, 4))
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

        let stopped = stop(&registry, &ticket.subject_id, &Uninspected, 2).expect("stop");
        assert_eq!(
            stopped,
            Termination {
                revoked: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
        let subject = read(&registry, &ticket.subject_id)
            .expect("read")
            .expect("subject");
        assert_eq!(subject.state, State::Pending);
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

        let cancelled = cancel_many(&registry, &subjects, &LocalProcfsVerifier, 3);

        assert_eq!(cancelled.survivors, Vec::<String>::new());
        assert_eq!(cancelled.stopped.len(), 2, "both executions exited");
        for subject_id in &subjects {
            let subject = read(&registry, subject_id).expect("read").expect("subject");
            assert!(subject.exited_at.is_some(), "an exit was recorded");
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
        std::thread::sleep(std::time::Duration::from_millis(200));
        let process = LocalProcfsVerifier
            .inspect(child.0.id() as i32)
            .expect("procfs")
            .expect("the child is alive");
        capture(
            &registry,
            &ticket.subject_id,
            &ticket.correlation,
            &process,
            &started_as(&launch()),
            2,
        )
        .expect("capture");

        let cancelled = cancel_many(
            &registry,
            std::slice::from_ref(&ticket.subject_id),
            &LocalProcfsVerifier,
            3,
        );

        assert_eq!(cancelled.survivors, vec![ticket.subject_id.clone()]);
        assert!(cancelled.stopped.is_empty());
        let subject = read(&registry, &ticket.subject_id)
            .expect("read")
            .expect("subject");
        assert!(subject.exited_at.is_none(), "no exit was recorded");
        let _ = child.0.kill();
        let _ = child.0.wait();
    }

    #[test]
    fn a_stop_of_a_reused_pid_signals_nothing_and_records_the_claim_gone() {
        let scratch = Scratch::new("stop-reused");
        let registry = scratch.registry();
        let ticket = create(&registry, SPAWN, launch(), None, 1).expect("subject");
        let subject = ticket.subject_id.as_str();
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
            2,
        )
        .expect("capture");

        let stopped = stop(&registry, subject, &LocalProcfsVerifier, 3).expect("stop");
        assert_eq!(
            stopped,
            Termination {
                revoked: false,
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
        let subject = ticket.subject_id.as_str();
        capture(
            &registry,
            subject,
            &ticket.correlation,
            &identity(7),
            &started_as(&launch()),
            2,
        )
        .expect("capture");

        let stopped = stop(&registry, subject, &Unreadable, 3).expect("stop");
        assert_eq!(
            stopped,
            Termination {
                revoked: false,
                signal_attempted: false,
                exit: Exit::Unresolved,
            }
        );
        let record = read(&registry, subject).expect("read").expect("subject");
        assert_eq!(record.signalled_at, None);
        assert_eq!(record.exited_at, None);
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
    fn captured_launch_drops_radar_bootstrap_and_token_variables() {
        let mut env = BTreeMap::new();
        env.insert(SUBJECT_ENV.to_string(), "s".to_string());
        env.insert(CORRELATION_ENV.to_string(), "c".to_string());
        env.insert(MANAGED_SUBJECT_ENV.to_string(), "logical".to_string());
        env.insert(MANAGED_LAUNCH_ENV.to_string(), "launch".to_string());
        env.insert(MANAGED_GENERATION_ENV.to_string(), "2".to_string());
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
