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
    Claim, Completion, EndedEvidence, Observation, Origin, PaneAbsence, PaneCleanup, ProcessRecord,
    State,
};
use crate::control_plane::containers::{Container, ContainerObservation};
use crate::control_plane::registry::publication::write_atomic;
use crate::control_plane::registry::{prepare_directory, read_record_file};
use crate::control_plane::store::RequestRecord;
use crate::control_plane::store::random_uuid;
use crate::identity::{ClaimId, RegistrationId, SubjectId};
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

/// A journal this build cannot replay is not an operational hiccup: no restart
/// repairs it, so the daemon refuses the state and stops instead of looping.
/// Every such refusal starts with this prefix, which the daemon entry point
/// turns into a distinct exit status.
pub const UNREADABLE_STATE: &str = "saved state cannot be read:";

/// Whether a diagnostic means the state root holds a journal this build cannot
/// read, as opposed to a socket, permission or backend failure.
pub fn is_unreadable_state(diagnostic: &str) -> bool {
    diagnostic.starts_with(UNREADABLE_STATE)
}

const VERSION: u32 = 3;
const MAX_EVENT_BYTES: usize = 16 * 1024;

/// A secret-free baseline for explicitly imported records and new subjects.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// What a stream's reduction produced. An attachment and an execution are
/// separate records: a claim owns who is attached, an execution owns what the
/// process did.
pub enum Recorded {
    Attachment(Box<AttachmentFacts>),
    Execution(Box<ProcessFacts>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Reserved,
    Pending,
    Granted,
    Duplicate,
    Released,
}

impl Standing {
    /// Whether this claim may carry subject authority. It is eligibility only:
    /// the subject's granted attachment must still be this claim.
    pub fn grants_authority(self) -> bool {
        matches!(self, Self::Granted)
    }

    pub fn reserves_attachment(self) -> bool {
        matches!(self, Self::Reserved | Self::Pending)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Duplicate | Self::Released)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentFacts {
    pub registration_id: RegistrationId,
    pub session: Option<String>,
    /// The durable agent this attachment is one process of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<SubjectId>,
    #[serde(default)]
    pub managed: crate::control_plane::lifecycle::ManagedMetadata,
    /// Where this attachment stands among the claims on its subject. The claim is
    /// the authority; a subject's current, reserved and previous grant are read
    /// from these standings in recorded order.
    pub standing: Standing,
    /// When this claim was granted, so a released claim still records that it once
    /// held its subject. Set once; a later release does not clear it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<String>,
    /// Why this attachment ended without ever being fulfilled, when it was an
    /// intended restart or continuation that could not happen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

/// One registration's public process evidence. Recipes and captured environment
/// remain private; attachments reference this projection rather than owning it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessFacts {
    pub spawn_request_id: String,
    pub origin: Origin,
    pub state: State,
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
    #[serde(default)]
    pub completion: Option<Completion>,
    #[serde(default)]
    pub placement: Option<crate::control_plane::lifecycle::Placement>,
    pub bound_registration_id: Option<RegistrationId>,
    pub signalled_at: Option<String>,
    pub exited_at: Option<String>,
    pub recorded_at: String,
}

impl AttachmentFacts {
    pub fn of(subject: &Claim) -> Self {
        Self {
            registration_id: subject.process.registration_id.clone(),
            subject_id: Some(subject.subject_id.clone()),
            managed: subject.managed.clone(),
            session: subject.session.clone(),
            // Standing is recorded by its own transition, never implied by
            // writing an attachment: this baseline describes the facts.
            standing: Standing::Pending,
            granted_at: None,
            failure: None,
        }
    }
}

impl From<&ProcessRecord> for ProcessFacts {
    fn from(process: &ProcessRecord) -> Self {
        Self {
            spawn_request_id: process.spawn_request_id.clone(),
            origin: process.origin,
            state: process.state,
            process: process.identity.clone(),
            observation: process.observation.clone(),
            pane_absence: process.pane_absence.clone(),
            ended_externally_at: process.ended_externally_at.clone(),
            ended_evidence: process.ended_evidence.clone(),
            pane_cleanup: process.pane_cleanup.clone(),
            completion: process.completion.clone(),
            placement: process.placement.clone(),
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
        process.identity = self.process.clone();
        process.observation = self.observation.clone();
        process.pane_absence = self.pane_absence.clone();
        process.ended_externally_at = self.ended_externally_at.clone();
        process.ended_evidence = self.ended_evidence.clone();
        process.pane_cleanup = self.pane_cleanup.clone();
        process.completion = self.completion.clone();
        process.placement = self.placement.clone();
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
        registration_id: RegistrationId,
        facts: Box<ProcessFacts>,
    },
    Created(Box<AttachmentFacts>),
    Enrolled(Box<AttachmentFacts>),
    Captured(ProcessIdentity),
    Bound {
        registration_id: RegistrationId,
    },
    Revoked,
    SignalRequested,
    ExitVerified,
    Observed(Observation),
    /// Establishes where an execution runs. Recorded once per pane it occupies,
    /// so a movement is a new fact rather than a silent rewrite of an old one.
    Placed(crate::control_plane::lifecycle::Placement),
    /// Starts, updates or clears the bounded confirmation of a bound pane's
    /// absence. Weak evidence, so it is not itself a lifecycle outcome.
    AbsencePending(Option<PaneAbsence>),
    /// Ends a launch externally, naming the evidence that established it. Only a
    /// verified process exit records `ExitVerified`.
    EndedExternally(EndedEvidence),
    PaneCleanup(PaneCleanup),
    /// Records how the program ended, once, as its supervisor saw it. It is
    /// independent of the verified exit: either may arrive first.
    Completed(Completion),
    /// Records the Pi session a launched process reported, once, on a bound subject
    /// that was launched without one.
    SessionRecorded {
        session: String,
    },
    /// Records the durable subject this attachment belongs to, once it is known.
    /// A presence attempt starts before the subject is resolved.
    Attached {
        subject_id: SubjectId,
    },
    /// Holds a subject for a launch that has not presented yet. Not authority: it
    /// excludes competing attachments until this registration presents or is
    /// withdrawn.
    Reserved {
        subject_id: SubjectId,
        session: Option<String>,
    },
    /// A presence report on a claim with no subject yet, before anything authorizes
    /// it. Its own event so an interrupted decision leaves a readable attempt.
    Presence {
        session: String,
    },
    /// Authorizes this claim over its subject. The claim, not the subject record,
    /// is where an attachment becomes current.
    Granted {
        subject_id: SubjectId,
    },
    /// The session is already attached elsewhere, so this claim never authorizes.
    Duplicate {
        session: String,
    },
    /// Ends this claim's authority, keeping it as the record of what it was.
    Released,
    /// Ends an intended attachment that could not happen, recording why.
    Failed {
        reason: String,
    },
    OperationAdmitted(RequestRecord),
    OperationStarted(RequestRecord),
    OperationSettled(RequestRecord),
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

/// The record an event extends. Its id is the event's key on disk.
#[derive(Clone, Copy, Debug)]
pub enum Stream<'a> {
    /// An attachment of a subject to one process.
    Claim(&'a ClaimId),
    /// A process registration, recorded whether or not a claim holds it.
    Process(&'a RegistrationId),
    Container(&'a str),
}

impl Stream<'_> {
    fn key(&self) -> &str {
        match self {
            Self::Claim(id) => id.as_str(),
            Self::Process(id) => id.as_str(),
            Self::Container(id) => id,
        }
    }
}

/// One daemon-scoped FIFO, protected by the owning registry. Immutable files
/// reuse the registry's fsync/rename discipline; public pointer retention cannot
/// discard this authority.
#[derive(Debug, Default)]
struct SubjectAttachments {
    granted: Option<ClaimId>,
    reserved: Option<ClaimId>,
    last_granted: Option<ClaimId>,
}

#[derive(Debug)]
pub struct Log {
    directory: PathBuf,
    next_seq: u64,
    facts: BTreeMap<ClaimId, AttachmentFacts>,
    /// The sequence that last changed each claim, so "current", "reserved" and
    /// "previous grant" are read in recorded order rather than guessed from ids.
    claim_seq: BTreeMap<ClaimId, u64>,
    subjects: BTreeMap<SubjectId, SubjectAttachments>,
    registrations: BTreeMap<RegistrationId, ClaimId>,
    processes: BTreeMap<RegistrationId, ProcessFacts>,
    operations: BTreeMap<String, RequestRecord>,
    containers: BTreeMap<String, Container>,
    last_subject_event: BTreeMap<String, String>,
    last_operation_event: BTreeMap<String, String>,
    failed: bool,
}

impl Log {
    pub fn open(directory: &Path) -> Result<Self, String> {
        prepare_directory(directory)?;
        let mut files = Vec::new();
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| {
                format!(
                    "{UNREADABLE_STATE} {} has a file name that is not UTF-8",
                    entry.path().display()
                )
            })?;
            if name.starts_with(".tmp-") {
                continue;
            }
            let seq = name
                .strip_suffix(".json")
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or_else(|| {
                    format!(
                        "{UNREADABLE_STATE} {} is not a numbered journal entry",
                        entry.path().display()
                    )
                })?;
            files.push((seq, entry.path()));
        }
        files.sort_by_key(|(seq, _)| *seq);
        let mut log = Self {
            directory: directory.to_owned(),
            next_seq: 1,
            facts: BTreeMap::new(),
            claim_seq: BTreeMap::new(),
            subjects: BTreeMap::new(),
            registrations: BTreeMap::new(),
            processes: BTreeMap::new(),
            operations: BTreeMap::new(),
            containers: BTreeMap::new(),
            last_subject_event: BTreeMap::new(),
            last_operation_event: BTreeMap::new(),
            failed: false,
        };
        for (seq, path) in files {
            let bytes = read_record_file(&path, MAX_EVENT_BYTES)?;
            let event: Event = serde_json::from_slice(&bytes).map_err(|error| {
                format!(
                    "{UNREADABLE_STATE} {} is not a Radar event ({error})",
                    path.display()
                )
            })?;
            if event.version != VERSION {
                return Err(format!(
                    "{UNREADABLE_STATE} {} was written by another Radar build (event version {}, this build reads {VERSION})",
                    path.display(),
                    event.version
                ));
            }
            if seq != log.next_seq || event.seq != seq {
                return Err(format!(
                    "{UNREADABLE_STATE} {} breaks the journal sequence (entry {seq} of {}, event {})",
                    path.display(),
                    log.next_seq - 1,
                    event.seq
                ));
            }
            if let Some(id) = event.subject_id.as_deref()
                && SessionUuid::parse(id).is_none()
            {
                return Err(format!(
                    "{UNREADABLE_STATE} {} names subject `{id}`, which is not a session UUID",
                    path.display()
                ));
            }
            if SessionUuid::parse(&event.event_id).is_none() {
                return Err(format!(
                    "{UNREADABLE_STATE} {} has no session-UUID event id (`{}`)",
                    path.display(),
                    event.event_id
                ));
            }
            log.replay(&event).map_err(|error| {
                format!(
                    "{UNREADABLE_STATE} {} is inconsistent with the journal before it ({error})",
                    path.display()
                )
            })?;
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

    /// The attachment a claim records. The execution it names is read from the
    /// process stream, never copied here.
    pub fn facts(&self, claim_id: &ClaimId) -> Option<AttachmentFacts> {
        self.facts.get(claim_id).cloned()
    }
    pub fn process(&self, registration_id: &RegistrationId) -> Option<&ProcessFacts> {
        self.processes.get(registration_id)
    }

    /// The registration whose captured process is this one. Identities carry the
    /// boot and start time, so a recycled pid never matches an earlier launch.
    pub fn registration_of(&self, process: &ProcessIdentity) -> Option<&RegistrationId> {
        self.processes
            .iter()
            .find(|(_, facts)| facts.process.as_ref() == Some(process))
            .map(|(id, _)| id)
    }

    /// Every execution this daemon tracks, whether or not one subject claims it.
    pub fn processes(&self) -> impl Iterator<Item = (&RegistrationId, &ProcessFacts)> {
        self.processes.iter()
    }

    pub fn ids(&self) -> impl Iterator<Item = &ClaimId> {
        self.facts.keys()
    }

    /// Every claim on one subject, newest change first.
    pub fn claims_of_subject<'a>(
        &'a self,
        subject_id: &'a SubjectId,
    ) -> impl Iterator<Item = (ClaimId, &'a AttachmentFacts)> + 'a {
        let mut matched: Vec<(u64, ClaimId)> = self
            .facts
            .iter()
            .filter(|(_, facts)| facts.subject_id.as_ref() == Some(subject_id))
            .map(|(claim, _)| {
                (
                    self.claim_seq.get(claim).copied().unwrap_or(0),
                    claim.clone(),
                )
            })
            .collect();
        matched.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
        matched
            .into_iter()
            .filter_map(move |(_, claim)| self.facts.get(&claim).map(|facts| (claim, facts)))
    }

    /// Every claim one registration made, newest change first.
    pub fn claims_of_registration<'a>(
        &'a self,
        registration_id: &'a RegistrationId,
    ) -> impl Iterator<Item = (ClaimId, &'a AttachmentFacts)> + 'a {
        let mut matched: Vec<(u64, ClaimId)> = self
            .facts
            .iter()
            .filter(|(_, facts)| &facts.registration_id == registration_id)
            .map(|(claim, _)| {
                (
                    self.claim_seq.get(claim).copied().unwrap_or(0),
                    claim.clone(),
                )
            })
            .collect();
        matched.sort_by_key(|(seq, _)| std::cmp::Reverse(*seq));
        matched
            .into_iter()
            .filter_map(move |(_, claim)| self.facts.get(&claim).map(|facts| (claim, facts)))
    }

    /// The newest claim for one registration, whatever its standing.
    pub fn claim_of_registration(&self, registration_id: &RegistrationId) -> Option<ClaimId> {
        self.registrations.get(registration_id).cloned()
    }

    pub fn granted_claim(&self, subject: &SubjectId) -> Option<ClaimId> {
        self.subjects.get(subject)?.granted.clone()
    }

    pub fn reserved_claim(&self, subject: &SubjectId) -> Option<ClaimId> {
        self.subjects.get(subject)?.reserved.clone()
    }

    pub fn attachment(&self, subject: &SubjectId) -> Option<ClaimId> {
        let claims = self.subjects.get(subject)?;
        claims
            .granted
            .clone()
            .or_else(|| claims.reserved.clone())
            .or_else(|| claims.last_granted.clone())
    }

    fn validate_attachment(&self, id: &ClaimId, next: &AttachmentFacts) -> Result<(), String> {
        if let Some(previous) = self.facts.get(id)
            && (previous.registration_id != next.registration_id
                || previous
                    .subject_id
                    .as_ref()
                    .is_some_and(|subject| next.subject_id.as_ref() != Some(subject)))
        {
            return Err("a claim cannot change its registration or subject".into());
        }
        if !next.standing.is_terminal() {
            if let Some(other) = self.registrations.get(&next.registration_id)
                && other != id
                && !self.facts[other].standing.is_terminal()
            {
                return Err("the registration already holds an attachment".into());
            }
            if let Some(claims) = next
                .subject_id
                .as_ref()
                .and_then(|subject| self.subjects.get(subject))
            {
                if claims.reserved.as_ref().is_some_and(|other| other != id)
                    && claims.granted.as_ref() != Some(id)
                {
                    return Err("the subject already has a reserved attachment".into());
                }
                if next.standing.grants_authority()
                    && claims.granted.as_ref().is_some_and(|other| other != id)
                {
                    return Err("the subject already has a granted attachment".into());
                }
            }
        }
        Ok(())
    }

    fn index_attachment(&mut self, id: &ClaimId, next: &AttachmentFacts) {
        if let Some(subject) = &next.subject_id {
            let claims = self.subjects.entry(subject.clone()).or_default();
            if claims.granted.as_ref() == Some(id) {
                claims.granted = None;
            }
            if claims.reserved.as_ref() == Some(id) {
                claims.reserved = None;
            }
            if next.standing.grants_authority() {
                claims.granted = Some(id.clone());
                claims.last_granted = Some(id.clone());
            } else if next.standing.reserves_attachment() {
                claims.reserved = Some(id.clone());
            }
        }
        let current = self.registrations.get(&next.registration_id);
        if !next.standing.is_terminal()
            || current
                .is_none_or(|current| current == id || self.facts[current].standing.is_terminal())
        {
            self.registrations
                .insert(next.registration_id.clone(), id.clone());
        }
    }

    /// Materialized attachment views over the one process owner per registration.
    pub fn all_facts(&self) -> Vec<(ClaimId, AttachmentFacts)> {
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
        claim_id: &ClaimId,
        at: &str,
        source: Source,
        transition: Transition,
        expected: &AttachmentFacts,
    ) -> Result<(), String> {
        let stream = Stream::Claim(claim_id);
        let event = self.event(stream, at, source.clone(), transition.clone());
        let Recorded::Attachment(next) = self.reduce_event(stream, &event)? else {
            return Err("a claim transition produced an execution".into());
        };
        if next.as_ref() != expected {
            return Err("lifecycle transition changes unrelated facts".into());
        }
        self.append(stream, at, source, transition)
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
        let stream = Stream::Container(container_id);
        let event = self.event(stream, at, source.clone(), transition.clone());
        // `recorded_at` is the append's own bookkeeping, so the check is about
        // the facts a caller read rather than about when it read them.
        let mut expected = expected.clone();
        expected.recorded_at = at.to_owned();
        if reduce_container(self.containers.get(container_id), &event)? != expected {
            return Err("container transition changes unrelated facts".into());
        }
        self.append(stream, at, source, transition)
    }

    pub fn append(
        &mut self,
        stream: Stream<'_>,
        at: &str,
        source: Source,
        transition: Transition,
    ) -> Result<(), String> {
        if self.failed {
            return Err("event storage requires recovery".into());
        }
        let event = self.event(stream, at, source, transition);
        match (stream, &event.transition) {
            (
                Stream::Process(id),
                Transition::ProcessCreated {
                    registration_id, ..
                },
            ) => self.validate_process_creation(id, registration_id)?,
            (Stream::Container(id), _) => {
                reduce_container(self.containers.get(id), &event)?;
            }
            _ => {
                self.reduce_event(stream, &event)?;
            }
        }
        self.persist(&event)?;
        self.apply(stream, &event)
    }

    fn event(&self, stream: Stream<'_>, at: &str, source: Source, transition: Transition) -> Event {
        let key = stream.key();
        Event {
            version: VERSION,
            seq: self.next_seq,
            event_id: random_uuid(),
            subject_id: Some(key.to_owned()),
            operation_id: None,
            causation_id: self.last_subject_event.get(key).cloned(),
            at: at.to_owned(),
            source,
            transition,
        }
    }

    fn validate_process_creation(
        &self,
        key: &RegistrationId,
        registration_id: &RegistrationId,
    ) -> Result<(), String> {
        if key != registration_id {
            return Err("process event does not identify its registration".into());
        }
        if self.processes.contains_key(registration_id) {
            return Err("process registration is already recorded".into());
        }
        Ok(())
    }

    fn reduce_event(&self, stream: Stream<'_>, event: &Event) -> Result<Recorded, String> {
        match stream {
            Stream::Claim(id) => {
                let attachment = self.facts.get(id);
                let execution = attachment.and_then(|a| self.processes.get(&a.registration_id));
                let next = reduce_attachment(attachment, execution, event)?;
                self.validate_attachment(id, &next)?;
                Ok(Recorded::Attachment(Box::new(next)))
            }
            Stream::Process(id) => Ok(Recorded::Execution(Box::new(reduce_process(
                self.processes.get(id),
                event,
            )?))),
            Stream::Container(_) => Err("a container has no lifecycle facts".into()),
        }
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

    /// Replays a stored event onto the record its key names. The key on disk is
    /// untagged, so its kind is read from the transition and from what is already
    /// recorded: a claim's events extend its attachment, and any other lifecycle
    /// event extends a process no claim holds.
    fn replay(&mut self, event: &Event) -> Result<(), String> {
        if event.operation_id.is_some() {
            return self.replay_operation(event);
        }
        let key = event.subject_id.as_deref().ok_or("event has no subject")?;
        if event.transition.is_container() {
            return self.apply(Stream::Container(key), event);
        }
        let unnamed = || format!("event key `{key}` is not an identity");
        if !matches!(event.transition, Transition::ProcessCreated { .. }) {
            let claim = ClaimId::parse(key).ok_or_else(unnamed)?;
            if self.facts.contains_key(&claim)
                || matches!(
                    event.transition,
                    Transition::Created(_) | Transition::Enrolled(_)
                )
            {
                return self.apply(Stream::Claim(&claim), event);
            }
        }
        let registration = RegistrationId::parse(key).ok_or_else(unnamed)?;
        self.apply(Stream::Process(&registration), event)
    }

    fn apply(&mut self, stream: Stream<'_>, event: &Event) -> Result<(), String> {
        let key = stream.key();
        if event
            .causation_id
            .as_ref()
            .is_some_and(|id| self.last_subject_event.get(key) != Some(id))
        {
            return Err("event causation is invalid".into());
        }
        match (stream, &event.transition) {
            (
                Stream::Process(id),
                Transition::ProcessCreated {
                    registration_id,
                    facts,
                },
            ) => {
                self.validate_process_creation(id, registration_id)?;
                self.processes.insert(id.clone(), facts.as_ref().clone());
            }
            (Stream::Container(id), _) => {
                let container = reduce_container(self.containers.get(id), event)?;
                self.containers.insert(id.to_owned(), container);
            }
            _ => match self.reduce_event(stream, event)? {
                Recorded::Attachment(attachment) => {
                    if let Stream::Claim(claim) = stream {
                        self.index_attachment(claim, &attachment);
                        self.facts.insert(claim.clone(), *attachment);
                        self.claim_seq.insert(claim.clone(), event.seq);
                    }
                }
                Recorded::Execution(execution) => {
                    if let Stream::Process(id) = stream {
                        self.processes.insert(id.clone(), *execution);
                    }
                }
            },
        }
        self.last_subject_event
            .insert(key.to_owned(), event.event_id.clone());
        Ok(())
    }
}

/// Applies a claim transition. Standing, its subject and its session are the
/// claim's own facts; anything it asks about the process is read from the
/// execution record rather than kept in step here.
fn reduce_attachment(
    previous: Option<&AttachmentFacts>,
    execution: Option<&ProcessFacts>,
    event: &Event,
) -> Result<AttachmentFacts, String> {
    let mut facts = match (&event.transition, previous) {
        (Transition::Created(facts) | Transition::Enrolled(facts), None) => {
            return Ok(facts.as_ref().clone());
        }
        (_, Some(facts)) => facts.clone(),
        _ => return Err("lifecycle transition has no baseline".into()),
    };
    let ended = execution.is_none_or(|execution| {
        execution.exited_at.is_some() || execution.ended_externally_at.is_some()
    });
    match &event.transition {
        Transition::SessionRecorded { session } if facts.session.is_none() && !ended => {
            facts.session = Some(session.clone())
        }
        Transition::Attached { subject_id }
            if facts.subject_id.is_none() && !facts.standing.is_terminal() =>
        {
            facts.subject_id = Some(subject_id.clone());
        }
        Transition::Reserved {
            subject_id,
            session,
        } if !facts.standing.is_terminal() && !ended => {
            facts.subject_id = Some(subject_id.clone());
            facts.session = session.clone();
            facts.standing = Standing::Reserved;
        }
        Transition::Presence { session }
            if matches!(facts.standing, Standing::Reserved | Standing::Pending) && !ended =>
        {
            facts.session = Some(session.clone());
            facts.standing = Standing::Pending;
        }
        Transition::Granted { subject_id }
            if matches!(facts.standing, Standing::Reserved | Standing::Pending) && !ended =>
        {
            facts.subject_id = Some(subject_id.clone());
            facts.standing = Standing::Granted;
            facts.granted_at = Some(event.at.clone());
        }
        Transition::Duplicate { session } if !facts.standing.is_terminal() => {
            facts.session = Some(session.clone());
            facts.standing = Standing::Duplicate;
        }
        Transition::Released if !facts.standing.is_terminal() => {
            facts.standing = Standing::Released;
        }
        Transition::Failed { reason } if !facts.standing.is_terminal() => {
            facts.standing = Standing::Released;
            facts.failure = Some(reason.clone());
        }
        _ => return Err("invalid lifecycle transition".into()),
    }
    Ok(facts)
}

/// Applies an execution transition. Everything about the process — identity,
/// birth, binding, signal, exit, completion, placement — belongs here.
fn reduce_process(previous: Option<&ProcessFacts>, event: &Event) -> Result<ProcessFacts, String> {
    let mut facts = previous
        .cloned()
        .ok_or("lifecycle transition has no baseline")?;
    match &event.transition {
        Transition::Captured(process) if facts.state == State::Pending => {
            facts.state = State::Captured;
            facts.process = Some(process.clone());
        }
        Transition::Bound { registration_id } if facts.state == State::Captured => {
            facts.state = State::Bound;
            facts.bound_registration_id = Some(registration_id.clone());
        }
        Transition::Revoked
            if matches!(facts.state, State::Pending | State::Captured | State::Bound) =>
        {
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
        // Placement is where an execution runs, so it is recorded while it is
        // there and never for one that has ended.
        Transition::Placed(placement)
            if !matches!(facts.state, State::Revoked)
                && facts.exited_at.is_none()
                && facts.ended_externally_at.is_none()
                && facts.completion.is_none() =>
        {
            facts.placement = Some(placement.clone())
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
        // A launch that ended leaves a pane nobody owns: one revoked before it
        // bound, or one bound and verifiably exited. Its cleanup outcome is
        // recorded once, so a later operation on the same launch reports it
        // rather than closing anything twice.
        Transition::Completed(completion)
            if facts.process.is_some() && facts.completion.is_none() =>
        {
            facts.completion = Some(completion.clone())
        }
        Transition::PaneCleanup(outcome)
            if facts.pane_cleanup.is_none()
                && (facts.state == State::Revoked || facts.exited_at.is_some()) =>
        {
            facts.pane_cleanup = Some(outcome.clone())
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
