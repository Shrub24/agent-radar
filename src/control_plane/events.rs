//! The daemon's authoritative transitions, in one daemon-scoped FIFO: managed
//! subjects, the containers they run in, and the operations that act on them.
//! Replay only reduces data; it never reacquires a process handle, dispatches an
//! effect or touches a backend.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use serde::{Deserialize, Serialize};

use super::lifecycle::{
    EndedEvidence, Observation, Origin, PaneAbsence, PaneCleanup, State, Subject,
};
use crate::control_plane::containers::{Container, ContainerObservation};
use crate::control_plane::registry::publication::write_atomic;
use crate::control_plane::registry::{prepare_directory, read_record_file};
use crate::control_plane::store::RequestRecord;
use crate::control_plane::store::random_uuid;
use crate::model::{ProcessIdentity, SessionUuid};

pub(crate) mod operations;

type SharedLogs = Mutex<BTreeMap<PathBuf, Weak<Mutex<Log>>>>;
static LOGS: OnceLock<SharedLogs> = OnceLock::new();

pub fn shared(directory: &Path) -> Result<Arc<Mutex<Log>>, String> {
    prepare_directory(directory)?;
    let directory = fs::canonicalize(directory).map_err(|e| e.to_string())?;
    let mut logs = LOGS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    logs.retain(|_, log| log.strong_count() != 0);
    if let Some(log) = logs.get(&directory).and_then(Weak::upgrade) {
        return Ok(log);
    }
    let log = Arc::new(Mutex::new(Log::open(&directory)?));
    logs.insert(directory, Arc::downgrade(&log));
    Ok(log)
}

#[cfg(test)]
mod tests;

const VERSION: u32 = 1;
const MAX_EVENT_BYTES: usize = 16 * 1024;

/// A secret-free baseline for explicitly imported records and new subjects.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    /// Stable logical subject identity. Absent only in pre-migration events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logical_subject_id: Option<String>,
    /// One-based execution generation within the logical subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_generation: Option<u32>,
    /// True when legacy links or later evidence cannot establish this execution's continuity.
    #[serde(default)]
    pub identity_unresolved: bool,
    #[serde(default)]
    pub managed: crate::control_plane::lifecycle::ManagedMetadata,
    pub spawn_request_id: String,
    pub origin: Origin,
    pub state: State,
    pub session: Option<String>,
    pub successor: Option<String>,
    pub process: Option<ProcessIdentity>,
    #[serde(default)]
    pub observation: Option<Observation>,
    #[serde(default)]
    pub pane_absence: Option<PaneAbsence>,
    #[serde(default)]
    pub ended_externally_at: Option<String>,
    #[serde(default)]
    pub ended_evidence: Option<EndedEvidence>,
    #[serde(default)]
    pub pane_cleanup: Option<PaneCleanup>,
    #[serde(alias = "bound_agent_id")]
    pub bound_registration_id: Option<String>,
    pub signalled_at: Option<String>,
    pub exited_at: Option<String>,
    pub recorded_at: String,
}

impl Facts {
    pub fn from_subject(subject: &Subject) -> Self {
        Self {
            logical_subject_id: subject.logical_subject_id.clone(),
            execution_generation: subject.execution_generation,
            identity_unresolved: subject.identity_unresolved,
            managed: subject.managed.clone(),
            spawn_request_id: subject.spawn_request_id.clone(),
            origin: subject.origin,
            state: subject.state,
            session: subject.session.clone(),
            successor: subject.successor.clone(),
            process: subject.process.clone(),
            observation: subject.observation.clone(),
            pane_absence: subject.pane_absence.clone(),
            ended_externally_at: subject.ended_externally_at.clone(),
            ended_evidence: subject.ended_evidence.clone(),
            pane_cleanup: subject.pane_cleanup.clone(),
            bound_registration_id: subject.bound_registration_id.clone(),
            signalled_at: subject.signalled_at.clone(),
            exited_at: subject.exited_at.clone(),
            recorded_at: subject.recorded_at.clone(),
        }
    }

    pub fn apply_to(&self, subject: &mut Subject) {
        subject.logical_subject_id = self.logical_subject_id.clone();
        subject.execution_generation = self.execution_generation;
        subject.identity_unresolved = self.identity_unresolved;
        subject.managed = self.managed.clone();
        subject.session = self.session.clone();
        subject.state = self.state;
        subject.successor = self.successor.clone();
        subject.process = self.process.clone();
        subject.observation = self.observation.clone();
        subject.pane_absence = self.pane_absence.clone();
        subject.ended_externally_at = self.ended_externally_at.clone();
        subject.ended_evidence = self.ended_evidence.clone();
        subject.pane_cleanup = self.pane_cleanup.clone();
        subject.bound_registration_id = self.bound_registration_id.clone();
        subject.signalled_at = self.signalled_at.clone();
        subject.exited_at = self.exited_at.clone();
        subject.recorded_at = self.recorded_at.clone();
        if self.state != State::Pending {
            subject.correlation = None;
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Transition {
    Created(Facts),
    Enrolled(Facts),
    Imported(Facts),
    /// Adds stable logical identity to one legacy execution record without
    /// rewriting its existing event history.
    IdentityMapped {
        logical_subject_id: String,
        execution_generation: u32,
        identity_unresolved: bool,
    },
    /// Marks continuity evidence ambiguous without rewriting an existing mapping.
    IdentityUnresolved,
    Captured(ProcessIdentity),
    Bound {
        #[serde(alias = "agent_id")]
        registration_id: String,
    },
    Revoked,
    SignalRequested,
    ExitVerified,
    Observed(Observation),
    /// Starts, updates or clears the bounded confirmation of a bound pane's
    /// absence. Weak evidence, so it is not itself a lifecycle outcome.
    AbsencePending(Option<PaneAbsence>),
    /// Ends a launch externally, naming the evidence that established it. Only a
    /// verified process exit records `ExitVerified`.
    EndedExternally(EndedEvidence),
    PaneCleanup(PaneCleanup),
    SuccessorReserved {
        spawn_request_id: String,
    },
    /// Records the Pi session a launched process reported, once, on a bound subject
    /// that was launched without one.
    SessionRecorded {
        session: String,
    },
    OperationAdmitted(RequestRecord),
    OperationStarted(RequestRecord),
    OperationSettled(RequestRecord),
    OperationImported(RequestRecord),
    ContainerCreated(Container),
    ContainerObserved(ContainerObservation),
    ContainerRemoved,
}

impl Transition {
    /// A container transition carries a container record, not a managed
    /// subject's facts, and is reduced separately.
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            Self::ContainerCreated(_) | Self::ContainerObserved(_) | Self::ContainerRemoved
        )
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Launch,
    Operator,
    Bootstrap,
    Registration,
    ProcessControl,
    ProcessObservation,
    PaneControl,
    RecoveryImport,
    IdentityMigration,
    /// A topology effect this daemon dispatched: a container it created, deleted
    /// or reloaded.
    TopologyControl,
    /// A reconciliation of container bindings against a backend inventory.
    TopologyObservation,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u32,
    pub seq: u64,
    pub event_id: String,
    pub subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<String>,
    pub at: String,
    pub source: Source,
    pub transition: Transition,
}

/// One daemon-scoped FIFO, protected by the owning registry. Immutable files
/// reuse the registry's fsync/rename discipline; public pointer retention cannot
/// discard this authority.
#[derive(Debug)]
pub struct Log {
    directory: PathBuf,
    next_seq: u64,
    facts: BTreeMap<String, Facts>,
    operations: BTreeMap<String, RequestRecord>,
    containers: BTreeMap<String, Container>,
    last_subject_event: BTreeMap<String, String>,
    last_operation_event: BTreeMap<String, String>,
    failed: bool,
}

impl std::fmt::Debug for Facts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Facts")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Log {
    pub fn open(directory: &Path) -> Result<Self, String> {
        prepare_directory(directory)?;
        let mut files = Vec::new();
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or("invalid event filename")?;
            if name.starts_with(".tmp-") {
                continue;
            }
            let seq = name
                .strip_suffix(".json")
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or("invalid event filename")?;
            files.push((seq, entry.path()));
        }
        files.sort_by_key(|(seq, _)| *seq);
        let mut log = Self {
            directory: directory.to_owned(),
            next_seq: 1,
            facts: BTreeMap::new(),
            operations: BTreeMap::new(),
            containers: BTreeMap::new(),
            last_subject_event: BTreeMap::new(),
            last_operation_event: BTreeMap::new(),
            failed: false,
        };
        for (seq, path) in files {
            let bytes = read_record_file(&path, MAX_EVENT_BYTES)?;
            let event: Event = serde_json::from_slice(&bytes).map_err(|_| "invalid event")?;
            if seq != log.next_seq
                || event.seq != seq
                || event.version != VERSION
                || event
                    .subject_id
                    .as_deref()
                    .is_some_and(|id| SessionUuid::parse(id).is_none())
                || SessionUuid::parse(&event.event_id).is_none()
            {
                return Err("event sequence or identity is invalid".into());
            }
            log.replay(&event)?;
            log.next_seq += 1;
        }
        Ok(log)
    }

    pub fn tail(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    pub fn read_after(&self, after: u64, limit: usize) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        for seq in after.saturating_add(1)..self.next_seq {
            if events.len() == limit {
                break;
            }
            let bytes = read_record_file(
                &self.directory.join(format!("{seq:020}.json")),
                MAX_EVENT_BYTES,
            )?;
            let event: Event =
                serde_json::from_slice(&bytes).map_err(|_| "invalid committed event")?;
            if event.seq != seq || event.version != VERSION {
                return Err("committed event identity changed".into());
            }
            events.push(event);
        }
        Ok(events)
    }

    pub fn facts(&self, subject_id: &str) -> Option<&Facts> {
        self.facts.get(subject_id)
    }
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.facts.keys()
    }

    /// Stable snapshot of current per-execution facts, used by one-time
    /// migrations without exposing the reducer's mutable map.
    pub fn all_facts(&self) -> Vec<(String, Facts)> {
        self.facts
            .iter()
            .map(|(id, facts)| (id.clone(), facts.clone()))
            .collect()
    }
    pub fn container(&self, id: &str) -> Option<&Container> {
        self.containers.get(id)
    }
    pub fn container_ids(&self) -> impl Iterator<Item = &String> {
        self.containers.keys()
    }

    pub fn append_checked(
        &mut self,
        subject_id: &str,
        at: &str,
        source: Source,
        transition: Transition,
        expected: &Facts,
    ) -> Result<(), String> {
        let event = Event {
            version: VERSION,
            seq: self.next_seq,
            event_id: random_uuid(),
            subject_id: Some(subject_id.to_owned()),
            operation_id: None,
            causation_id: self.last_subject_event.get(subject_id).cloned(),
            at: at.to_owned(),
            source: source.clone(),
            transition: transition.clone(),
        };
        if &reduce(self.facts.get(subject_id), &event)? != expected {
            return Err("lifecycle transition changes unrelated facts".into());
        }
        self.append(subject_id, at, source, transition)
    }

    /// Appends a container transition only if it leaves that container's record
    /// otherwise unchanged, so a concurrent observation is never overwritten.
    pub fn append_container_checked(
        &mut self,
        container_id: &str,
        at: &str,
        source: Source,
        transition: Transition,
        expected: &Container,
    ) -> Result<(), String> {
        let event = Event {
            version: VERSION,
            seq: self.next_seq,
            event_id: random_uuid(),
            subject_id: Some(container_id.to_owned()),
            operation_id: None,
            causation_id: self.last_subject_event.get(container_id).cloned(),
            at: at.to_owned(),
            source: source.clone(),
            transition: transition.clone(),
        };
        // `recorded_at` is the append's own bookkeeping, so the check is about
        // the facts a caller read rather than about when it read them.
        let mut expected = expected.clone();
        expected.recorded_at = at.to_owned();
        if reduce_container(self.containers.get(container_id), &event)? != expected {
            return Err("container transition changes unrelated facts".into());
        }
        self.append(container_id, at, source, transition)
    }

    pub fn append(
        &mut self,
        subject_id: &str,
        at: &str,
        source: Source,
        transition: Transition,
    ) -> Result<(), String> {
        if self.failed {
            return Err("event storage requires recovery".into());
        }
        let event = Event {
            version: VERSION,
            seq: self.next_seq,
            event_id: random_uuid(),
            subject_id: Some(subject_id.to_owned()),
            operation_id: None,
            causation_id: self.last_subject_event.get(subject_id).cloned(),
            at: at.to_owned(),
            source,
            transition,
        };
        if event.transition.is_container() {
            reduce_container(self.containers.get(subject_id), &event)?;
        } else {
            reduce(self.facts.get(subject_id), &event)?;
        }
        self.persist(&event)?;
        self.replay(&event)
    }

    fn persist(&mut self, event: &Event) -> Result<(), String> {
        if self.failed {
            return Err("event storage requires recovery".into());
        }
        let bytes = serde_json::to_vec(&event).map_err(|_| "cannot encode event")?;
        let path = self.directory.join(format!("{:020}.json", self.next_seq));
        if path.exists() {
            self.failed = true;
            return Err("event sequence already exists".into());
        }
        if let Err(error) = write_atomic(&self.directory, &path, &bytes, MAX_EVENT_BYTES) {
            // A failed directory sync can mean the rename happened. Freeze the
            // writer rather than overwrite that sequence or dispatch an effect.
            self.failed = true;
            return Err(error);
        }
        self.next_seq += 1;
        if let Some(root) = self.directory.parent() {
            crate::control_plane::store::notify_events(root);
        }
        Ok(())
    }

    fn replay(&mut self, event: &Event) -> Result<(), String> {
        if event.operation_id.is_some() {
            return self.replay_operation(event);
        }
        let subject_id = event.subject_id.as_deref().ok_or("event has no subject")?;
        if event
            .causation_id
            .as_ref()
            .is_some_and(|id| self.last_subject_event.get(subject_id) != Some(id))
        {
            return Err("event causation is invalid".into());
        }
        if event.transition.is_container() {
            let container = reduce_container(self.containers.get(subject_id), event)?;
            self.containers.insert(subject_id.to_owned(), container);
        } else {
            let facts = reduce(self.facts.get(subject_id), event)?;
            self.facts.insert(subject_id.to_owned(), facts);
        }
        self.last_subject_event
            .insert(subject_id.to_owned(), event.event_id.clone());
        Ok(())
    }
}

fn reduce(previous: Option<&Facts>, event: &Event) -> Result<Facts, String> {
    let mut facts = match (&event.transition, previous) {
        (
            Transition::Created(facts) | Transition::Enrolled(facts) | Transition::Imported(facts),
            None,
        ) => return Ok(facts.clone()),
        (_, Some(facts)) => facts.clone(),
        _ => return Err("lifecycle transition has no baseline".into()),
    };
    match &event.transition {
        Transition::IdentityUnresolved => {
            facts.identity_unresolved = true;
        }
        Transition::IdentityMapped {
            logical_subject_id,
            execution_generation,
            identity_unresolved,
        } => {
            if facts.logical_subject_id.is_some()
                || facts.execution_generation.is_some()
                || logical_subject_id.is_empty()
                || *execution_generation == 0
            {
                return Err(format!(
                    "identity mapping conflicts with current facts (logical={:?}, generation={:?}, new logical={logical_subject_id:?}, new generation={execution_generation})",
                    facts.logical_subject_id, facts.execution_generation
                ));
            }
            facts.logical_subject_id = Some(logical_subject_id.clone());
            facts.execution_generation = Some(*execution_generation);
            facts.identity_unresolved = *identity_unresolved;
        }
        Transition::Captured(process) if facts.state == State::Pending => {
            facts.state = State::Captured;
            facts.process = Some(process.clone());
        }
        Transition::Bound { registration_id } if facts.state == State::Captured => {
            facts.state = State::Bound;
            facts.bound_registration_id = Some(registration_id.clone());
        }
        Transition::Revoked if matches!(facts.state, State::Pending | State::Captured) => {
            facts.state = State::Revoked
        }
        Transition::SignalRequested
            if facts.process.is_some()
                && facts.signalled_at.is_none()
                && facts.exited_at.is_none() =>
        {
            facts.signalled_at = Some(event.at.clone())
        }
        Transition::Observed(observation)
            if facts.process.as_ref() == Some(&observation.process)
                && facts.exited_at.is_none() =>
        {
            facts.observation = Some(observation.clone());
        }
        Transition::AbsencePending(absence)
            if facts.exited_at.is_none() && facts.ended_externally_at.is_none() =>
        {
            facts.pane_absence = absence.clone();
        }
        Transition::EndedExternally(evidence)
            if facts.exited_at.is_none() && facts.ended_externally_at.is_none() =>
        {
            facts.ended_externally_at = Some(event.at.clone());
            facts.ended_evidence = Some(evidence.clone());
            // The confirmed absence is the reason, so no window is left pending.
            facts.pane_absence = None;
        }
        Transition::ExitVerified if facts.process.is_some() && facts.exited_at.is_none() => {
            facts.exited_at = Some(event.at.clone())
        }
        // Only a launch this daemon revoked left a pane nobody owns, and its
        // cleanup outcome is recorded once: a later cancel reports it.
        Transition::PaneCleanup(outcome)
            if facts.state == State::Revoked && facts.pane_cleanup.is_none() =>
        {
            facts.pane_cleanup = Some(outcome.clone())
        }
        Transition::SuccessorReserved { spawn_request_id }
            if facts.exited_at.is_some() && facts.successor.is_none() =>
        {
            facts.successor = Some(spawn_request_id.clone())
        }
        Transition::SessionRecorded { session }
            if facts.state == State::Bound
                && facts.session.is_none()
                && facts.exited_at.is_none()
                && facts.ended_externally_at.is_none() =>
        {
            facts.session = Some(session.clone())
        }
        _ => return Err("invalid lifecycle transition".into()),
    }
    facts.recorded_at = event.at.clone();
    Ok(facts)
}

/// A container is created once and then only observed or removed. An
/// observation never rewrites identity or binding, and a removal is recorded
/// once so a reused handle cannot resurrect the container.
fn reduce_container(previous: Option<&Container>, event: &Event) -> Result<Container, String> {
    let mut container = match (&event.transition, previous) {
        (Transition::ContainerCreated(container), None) => return Ok(container.clone()),
        (_, Some(container)) => container.clone(),
        _ => return Err("container transition has no baseline".into()),
    };
    match &event.transition {
        Transition::ContainerObserved(observation) if container.removed_at.is_none() => {
            observation.apply_to(&mut container, &event.at);
        }
        Transition::ContainerRemoved if container.removed_at.is_none() => {
            container.removed_at = Some(event.at.clone());
        }
        _ => return Err("invalid container transition".into()),
    }
    container.recorded_at = event.at.clone();
    Ok(container)
}
