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
    Claim, EndedEvidence, Observation, Origin, PaneAbsence, PaneCleanup, ProcessRecord, State,
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

const VERSION: u32 = 2;
const MAX_EVENT_BYTES: usize = 16 * 1024;

/// A secret-free baseline for explicitly imported records and new subjects.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    pub attachment: AttachmentFacts,
    pub execution: ProcessFacts,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentFacts {
    pub registration_id: String,
    pub session: Option<String>,
    /// Stable logical subject identity. Absent only in pre-migration events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[serde(rename = "logical_subject_id")]
    pub subject_id: Option<String>,
    /// One-based execution generation within the logical subject.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_generation: Option<u32>,
    /// True when legacy links or later evidence cannot establish this execution's continuity.
    #[serde(default)]
    pub identity_unresolved: bool,
    #[serde(default)]
    pub managed: crate::control_plane::lifecycle::ManagedMetadata,
}

/// One registration's public process evidence. Recipes and captured environment
/// remain private; attachments reference this projection rather than owning it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessFacts {
    pub spawn_request_id: String,
    pub origin: Origin,
    pub state: State,
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
    pub fn from_subject(subject: &Claim) -> Self {
        Self {
            attachment: AttachmentFacts {
                registration_id: subject.execution.registration_id.clone(),
                subject_id: subject.subject_id.clone(),
                execution_generation: None,
                identity_unresolved: subject.identity_unresolved,
                managed: subject.managed.clone(),
                session: subject.session.clone(),
            },
            execution: ProcessFacts::from(&subject.execution),
        }
    }
}

impl From<&ProcessRecord> for ProcessFacts {
    fn from(process: &ProcessRecord) -> Self {
        Self {
            spawn_request_id: process.spawn_request_id.clone(),
            origin: process.origin,
            state: process.state,
            successor: process.successor.clone(),
            process: process.process.clone(),
            observation: process.observation.clone(),
            pane_absence: process.pane_absence.clone(),
            ended_externally_at: process.ended_externally_at.clone(),
            ended_evidence: process.ended_evidence.clone(),
            pane_cleanup: process.pane_cleanup.clone(),
            bound_registration_id: process.bound_registration_id.clone(),
            signalled_at: process.signalled_at.clone(),
            exited_at: process.exited_at.clone(),
            recorded_at: process.recorded_at.clone(),
        }
    }
}

impl ProcessFacts {
    pub fn apply_to(&self, process: &mut ProcessRecord) {
        process.state = self.state;
        process.successor = self.successor.clone();
        process.process = self.process.clone();
        process.observation = self.observation.clone();
        process.pane_absence = self.pane_absence.clone();
        process.ended_externally_at = self.ended_externally_at.clone();
        process.ended_evidence = self.ended_evidence.clone();
        process.pane_cleanup = self.pane_cleanup.clone();
        process.bound_registration_id = self.bound_registration_id.clone();
        process.signalled_at = self.signalled_at.clone();
        process.exited_at = self.exited_at.clone();
        process.recorded_at = self.recorded_at.clone();
        if self.state != State::Pending {
            process.correlation = None;
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
    ProcessCreated {
        registration_id: String,
        facts: ProcessFacts,
    },
    Created(Facts),
    Enrolled(Facts),
    Imported(Facts),
    /// Adds stable logical identity to one legacy execution record without
    /// rewriting its existing event history.
    IdentityMapped {
        #[serde(rename = "logical_subject_id")]
        subject_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        execution_generation: Option<u32>,
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
    SuccessorCancelled {
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
    facts: BTreeMap<String, AttachmentFacts>,
    processes: BTreeMap<String, ProcessFacts>,
    operations: BTreeMap<String, RequestRecord>,
    containers: BTreeMap<String, Container>,
    last_subject_event: BTreeMap<String, String>,
    last_operation_event: BTreeMap<String, String>,
    failed: bool,
}

impl std::fmt::Debug for Facts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Facts")
            .field("state", &self.execution.state)
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
            processes: BTreeMap::new(),
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

    pub fn facts(&self, claim_id: &str) -> Option<Facts> {
        let attachment = self.facts.get(claim_id)?;
        let execution = self
            .processes
            .get(&attachment.registration_id)
            .expect("every attachment references its recorded process");
        Some(Facts {
            attachment: attachment.clone(),
            execution: execution.clone(),
        })
    }
    pub fn process(&self, registration_id: &str) -> Option<&ProcessFacts> {
        self.processes.get(registration_id)
    }

    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.facts.keys()
    }

    /// Materialized attachment views over the one process owner per registration.
    pub fn all_facts(&self) -> Vec<(String, Facts)> {
        self.facts
            .keys()
            .map(|id| (id.clone(), self.facts(id).expect("recorded attachment")))
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
        if &self.reduce_event(&event)? != expected {
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
        if let Transition::ProcessCreated {
            registration_id, ..
        } = &event.transition
        {
            self.validate_process_creation(subject_id, registration_id)?;
        } else if event.transition.is_container() {
            reduce_container(self.containers.get(subject_id), &event)?;
        } else {
            self.reduce_event(&event)?;
        }
        self.persist(&event)?;
        self.replay(&event)
    }

    fn validate_process_creation(&self, key: &str, registration_id: &str) -> Result<(), String> {
        if key != registration_id || SessionUuid::parse(registration_id).is_none() {
            return Err("process event does not identify its registration".into());
        }
        if self.processes.contains_key(registration_id) {
            return Err("process registration is already recorded".into());
        }
        Ok(())
    }

    fn reduce_event(&self, event: &Event) -> Result<Facts, String> {
        let id = event
            .subject_id
            .as_deref()
            .ok_or("event has no record identity")?;
        let previous = self.facts(id);
        let mut next = reduce(previous.as_ref(), event)?;
        if previous.is_none()
            && let Some(process) = self.processes.get(&next.attachment.registration_id)
        {
            // A new attachment can reference an existing process, never reset
            // its recipe identity, birth, signal, exit or replacement evidence.
            next.execution.recorded_at = process.recorded_at.clone();
            if &next.execution != process {
                return Err("a new attachment cannot replace existing process facts".into());
            }
        }
        Ok(next)
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
        if let Transition::ProcessCreated {
            registration_id,
            facts,
        } = &event.transition
        {
            self.validate_process_creation(subject_id, registration_id)?;
            self.processes
                .insert(registration_id.clone(), facts.clone());
        } else if event.transition.is_container() {
            let container = reduce_container(self.containers.get(subject_id), event)?;
            self.containers.insert(subject_id.to_owned(), container);
        } else {
            let facts = self.reduce_event(event)?;
            self.processes
                .insert(facts.attachment.registration_id.clone(), facts.execution);
            self.facts.insert(subject_id.to_owned(), facts.attachment);
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
            facts.attachment.identity_unresolved = true;
        }
        Transition::IdentityMapped {
            subject_id: logical_subject_id,
            execution_generation,
            identity_unresolved,
        } => {
            if facts.attachment.subject_id.is_some() || logical_subject_id.is_empty() {
                return Err(format!(
                    "identity mapping conflicts with current facts (logical={:?}, generation={:?}, new logical={logical_subject_id:?}, new generation={execution_generation:?})",
                    facts.attachment.subject_id, facts.attachment.execution_generation
                ));
            }
            facts.attachment.subject_id = Some(logical_subject_id.clone());
            facts.attachment.execution_generation = *execution_generation;
            facts.attachment.identity_unresolved = *identity_unresolved;
        }
        Transition::Captured(process) if facts.execution.state == State::Pending => {
            facts.execution.state = State::Captured;
            facts.execution.process = Some(process.clone());
        }
        Transition::Bound { registration_id } if facts.execution.state == State::Captured => {
            facts.execution.state = State::Bound;
            facts.execution.bound_registration_id = Some(registration_id.clone());
        }
        Transition::Revoked
            if matches!(facts.execution.state, State::Pending | State::Captured) =>
        {
            facts.execution.state = State::Revoked
        }
        Transition::SignalRequested
            if facts.execution.process.is_some()
                && facts.execution.signalled_at.is_none()
                && facts.execution.exited_at.is_none() =>
        {
            facts.execution.signalled_at = Some(event.at.clone())
        }
        Transition::Observed(observation)
            if facts.execution.process.as_ref() == Some(&observation.process)
                && facts.execution.exited_at.is_none() =>
        {
            facts.execution.observation = Some(observation.clone());
        }
        Transition::AbsencePending(absence)
            if facts.execution.exited_at.is_none()
                && facts.execution.ended_externally_at.is_none() =>
        {
            facts.execution.pane_absence = absence.clone();
        }
        Transition::EndedExternally(evidence)
            if facts.execution.exited_at.is_none()
                && facts.execution.ended_externally_at.is_none() =>
        {
            facts.execution.ended_externally_at = Some(event.at.clone());
            facts.execution.ended_evidence = Some(evidence.clone());
            // The confirmed absence is the reason, so no window is left pending.
            facts.execution.pane_absence = None;
        }
        Transition::ExitVerified
            if facts.execution.process.is_some() && facts.execution.exited_at.is_none() =>
        {
            facts.execution.exited_at = Some(event.at.clone())
        }
        // Only a launch this daemon revoked left a pane nobody owns, and its
        // cleanup outcome is recorded once: a later cancel reports it.
        Transition::PaneCleanup(outcome)
            if facts.execution.state == State::Revoked
                && facts.execution.pane_cleanup.is_none() =>
        {
            facts.execution.pane_cleanup = Some(outcome.clone())
        }
        Transition::SuccessorReserved { spawn_request_id }
            if facts.execution.successor.is_none() =>
        {
            facts.execution.successor = Some(spawn_request_id.clone())
        }
        Transition::SuccessorCancelled { spawn_request_id }
            if facts.execution.successor.as_deref() == Some(spawn_request_id.as_str()) =>
        {
            facts.execution.successor = None;
        }
        Transition::SessionRecorded { session }
            if facts.execution.state == State::Bound
                && facts.attachment.session.is_none()
                && facts.execution.exited_at.is_none()
                && facts.execution.ended_externally_at.is_none() =>
        {
            facts.attachment.session = Some(session.clone())
        }
        _ => return Err("invalid lifecycle transition".into()),
    }
    facts.execution.recorded_at = event.at.clone();
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
