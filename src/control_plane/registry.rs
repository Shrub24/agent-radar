//! The agent registry: durable agent identity, kept apart from mux inventory and
//! from the mutation records.
//!
//! A mutation record answers "what did this daemon do, and what is the state of
//! that request". A registry record answers something else: which agent
//! incarnations a publisher has registered, what is known about each, and what
//! private launch information is held for later lifecycle work. The two share
//! the daemon's private state root and nothing else — a registry write never
//! enters a mux mutation lane and dispatches no physical control.
//!
//! The record is a file before it is anything else. It is written into a
//! temporary sibling and hard-linked into place, so a reader only ever sees a
//! whole record and a write that did not land leaves nothing behind. Idempotency
//! is content-based: the same publisher incarnation asking again with identical
//! content is answered with the handle it already has, while different content
//! under one incarnation is refused rather than silently replacing identity.
//!
//! Only the fact that a process identity was *supplied* is public here. Whether
//! it is live is a separate answer that arrives with the process verifier; a
//! caller's claim is never promoted to verification by being stored.
//!
//! This module owns the private storage and codec foundation for the trusted
//! socket endpoints. Registration, publication snapshots and process verification
//! remain backend-independent and dispatch no physical control.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::control_plane::store::{format_millis, random_uuid};
use crate::model::{ProcessIdentity, SessionUuid};

/// The record shape this build writes and reads. A record naming another version
/// fails explicitly rather than being read as this one.
pub const REGISTRATION_VERSION: u32 = 1;

/// The largest registration this daemon reads or writes, so a planted file
/// cannot be read into memory whole.
pub const MAX_REGISTRATION_BYTES: usize = 64 * 1024;

/// One text field may be at most this many bytes. The registration and
/// publication channels both use it for display and producer facts.
pub const MAX_TEXT_BYTES: usize = 1024;

/// The most argv entries one launch specification may hold. The record bound
/// still applies on top of this.
pub const MAX_ARGV: usize = 256;

/// The most registrations one list page returns.
pub const MAX_LISTED: usize = 100;

/// The directory inside the state root that holds one file per agent.
const AGENTS: &str = "agents";

/// Where an agent's incarnation lives on one backend, in that backend's own
/// coordinates.
///
/// The backend name is free-form so a source Radar does not ship yet is
/// preserved rather than rejected; the rest are the coordinates as reported, and
/// each is optional because a backend need not name every level.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryLocation {
    pub backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
}

impl RegistryLocation {
    fn validate(&self) -> Result<(), String> {
        bounded_text("location.backend", &self.backend, false)?;
        for (field, value) in [
            ("location.instance", &self.instance),
            ("location.workspace", &self.workspace),
            ("location.tab", &self.tab),
            ("location.pane", &self.pane),
        ] {
            if let Some(value) = value {
                bounded_text(field, value, false)?;
            }
        }
        Ok(())
    }
}

/// The explicit session reference a launch specification resumes into.
///
/// Either form may be supplied, but not neither: a launch with no session is a
/// launch the executor would have to guess about, and guessing is exactly what
/// this record exists to avoid.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl LaunchSession {
    fn validate(&self) -> Result<(), String> {
        if self.uuid.is_none() && self.path.is_none() {
            return Err("`session` names a uuid or a path".to_string());
        }
        if let Some(uuid) = &self.uuid
            && SessionUuid::parse(uuid).is_none()
        {
            return Err("`session.uuid` must be a canonical UUID".to_string());
        }
        if let Some(path) = &self.path {
            absolute_path("session.path", path)?;
        }
        Ok(())
    }
}

/// The private launch/resume information held for later lifecycle execution.
///
/// Everything here is the producer's own words: an absolute executable, literal
/// arguments, an absolute working directory, a session reference and a revision.
/// No shell string, no captured environment and no inferred command — these
/// fields are stored inertly and nothing in this build executes them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchSpec {
    /// An absolute path to the program to launch.
    pub executable: String,
    /// The literal arguments, each passed as its own argument. An entry may be
    /// empty, which is a real argument an executor must pass as empty.
    pub argv: Vec<String>,
    /// An absolute working directory for the launch.
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<LaunchSession>,
    /// Who supplied this specification, for a reader deciding what to trust.
    pub provenance: String,
    /// The producer's revision of this specification.
    pub revision: String,
}

impl LaunchSpec {
    fn validate(&self) -> Result<(), String> {
        absolute_path("executable", &self.executable)?;
        absolute_path("cwd", &self.cwd)?;
        if self.argv.len() > MAX_ARGV {
            return Err(format!("`argv` names more than {MAX_ARGV} entries"));
        }
        for argument in &self.argv {
            bounded_text("argv entry", argument, true)?;
        }
        if let Some(session) = &self.session {
            session.validate()?;
        }
        bounded_text("provenance", &self.provenance, false)?;
        bounded_text("revision", &self.revision, false)?;
        Ok(())
    }
}

/// What a publisher asks the registry to register.
///
/// Session UUID is context, never the key: two live attaches to one session are
/// two incarnations and stay two records. The incarnation is what a publisher's
/// identity is bound to, and a process identity is an optional claim whose
/// verification is a separate answer.
///
/// Everything here is identity or configuration, fixed for the life of the
/// record. What a publisher can change over time — its state, waiting reason,
/// last outcome, advertised actions — belongs to the source-labelled publication
/// channels, not to the immutable registration: a mutable fact would make "the
/// same content, asked again" ambiguous, and an identical retry is what makes
/// registration idempotent. A `state` field is therefore unknown here and is
/// refused, and vocabulary preservation is validated where that vocabulary is
/// accepted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegistrationRequest {
    /// The publisher's own name for itself.
    pub source: String,
    /// The publisher incarnation UUID this record is bound to.
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<RegistryLocation>,
    /// An optional claimed process identity. Decoded strictly through
    /// [`deserialize_process_claim`].
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_process_claim"
    )]
    pub process: Option<ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<LaunchSpec>,
}

/// Decodes a claimed process identity strictly.
///
/// [`ProcessIdentity`] is shared with the observation path, which tolerates
/// unknown nested fields; a registry record instead refuses an identity this
/// build does not understand, because silently reading it as a weaker one would
/// make an unknown field look like a verified fact. The strictness lives on this
/// wire edge rather than in the shared model.
fn deserialize_process_claim<'de, D>(deserializer: D) -> Result<Option<ProcessIdentity>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Claim {
        boot_id: String,
        pid: i32,
        start_ticks: u64,
    }
    let claim = Option::<Claim>::deserialize(deserializer)?;
    Ok(claim.map(|claim| ProcessIdentity {
        boot_id: claim.boot_id,
        pid: claim.pid,
        start_ticks: claim.start_ticks,
    }))
}

impl RegistrationRequest {
    /// Whether this request is one the registry can store.
    ///
    /// Checked before anything is written: a refusal is the caller's mistake and
    /// leaves no record behind.
    pub fn validate(&self) -> Result<(), String> {
        bounded_text("source", &self.source, false)?;
        if SessionUuid::parse(&self.incarnation).is_none() {
            return Err("`incarnation` must be a canonical UUID".to_string());
        }
        if let Some(session) = &self.session
            && SessionUuid::parse(session).is_none()
        {
            return Err("`session` must be a canonical UUID".to_string());
        }
        for (field, value) in [("owner", &self.owner), ("run", &self.run)] {
            if let Some(value) = value {
                bounded_text(field, value, false)?;
            }
        }
        if let Some(label) = &self.label {
            bounded_text("label", label, true)?;
        }
        if let Some(location) = &self.location {
            location.validate()?;
        }
        if let Some(process) = &self.process {
            validate_process(process)?;
        }
        if let Some(launch) = &self.launch {
            launch.validate()?;
        }
        Ok(())
    }
}

/// One durable registration: the identity the daemon issued and the request it
/// was written from.
///
/// The request is kept whole, including private launch information, because it
/// is what an identical retry is compared against. Nothing here is ever returned
/// to a client as-is; [`Registration::public`] is the readable projection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub version: u32,
    /// The daemon-issued agent-record UUID and durable key.
    pub agent_id: String,
    pub registered_at: String,
    pub request: RegistrationRequest,
}

impl Registration {
    /// What may be read about this registration: identity and facts, with launch
    /// arguments, executable, working directory and session path reduced to an
    /// availability and a revision.
    pub fn public(&self) -> PublicRegistration {
        PublicRegistration {
            version: self.version,
            agent_id: self.agent_id.clone(),
            registered_at: self.registered_at.clone(),
            source: self.request.source.clone(),
            incarnation: self.request.incarnation.clone(),
            session: self.request.session.clone(),
            owner: self.request.owner.clone(),
            run: self.request.run.clone(),
            label: self.request.label.clone(),
            location: self.request.location.clone(),
            process_claimed: self.request.process.is_some(),
            launch: PublicLaunch {
                available: self.request.launch.is_some(),
                revision: self
                    .request
                    .launch
                    .as_ref()
                    .map(|spec| spec.revision.clone()),
            },
        }
    }
}

/// A registration as a client reads it. The private launch fields are absent by
/// construction, not by redaction: there is no field here to forget to strip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicRegistration {
    pub version: u32,
    pub agent_id: String,
    pub registered_at: String,
    pub source: String,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<RegistryLocation>,
    /// Whether a process identity was supplied. Its verification is a separate
    /// answer and is never asserted by this record.
    pub process_claimed: bool,
    pub launch: PublicLaunch,
}

/// What a public read says about private launch information: that something is
/// held, and which revision of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicLaunch {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// The daemon's registrations on disk.
#[derive(Debug)]
pub struct Registry {
    directory: PathBuf,
    /// Root that owns both identity and publication records.
    root: PathBuf,
    /// When this registry began serving: the process instant every freshness
    /// decision is relative to. A record received before this time is restored,
    /// not reported — even when its stored timestamps look recent — because no
    /// publisher has reported since this process started.
    served_since_ms: i64,
    /// Opaque serving epoch. A random UUID, not a wall-clock timestamp, so
    /// equal timestamps and backwards wall clocks cannot restore freshness.
    serving_epoch: String,
    /// One local lock serializes all registry transitions; this is not a
    /// cross-process/distributed lock. The state root has one owning daemon.
    admission: Mutex<()>,
}

/// Injectable local process birth-identity verification.
pub mod process;
/// Publication channels live alongside the registry; the details live in
/// [`publication`].
pub mod publication;

pub use process::{
    LocalProcfsVerifier, ProcessVerification, ProcessVerifier, verify_identity, verify_registration,
};
pub use publication::{
    AcceptedSnapshot, AcquireRequest, Channel, ChannelRecord, DEFAULT_LEASE_MS, ExpectedWriter,
    Freshness, MAX_ACTIONS, MAX_LEASE_MS, MAX_PUBLICATION_BYTES, MIN_LEASE_MS, Outcome,
    PUBLICATION_VERSION, PublicChannel, PublicChannelFacts, PublicSnapshot, PublicSnapshotFacts,
    PublicWriter, PublicWriterFacts, PublishRequest, PublisherIdentity, Snapshot, WriterBinding,
};

impl Registry {
    /// Opens the registry under `root`, creating `root` and its `agents` and
    /// `publications` directories if they are missing.
    ///
    /// The directory must be a real `0700` directory of this user. A registration
    /// names publisher incarnations and holds the private launch information
    /// those publishers supplied, so a path anyone else can write is not a
    /// registry.
    ///
    /// `now_ms` is the injected daemon wall clock used for lease-expiry arithmetic.
    /// A random serving epoch, independent of this time, makes restored records
    /// stale across opens even when the wall clock repeats or moves backwards.
    pub fn open(root: &Path) -> Result<Self, String> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| format!("system clock before Unix epoch: {error}"))?
            .as_millis() as i64;
        Self::open_at(root, now_ms)
    }

    /// When this registry began serving. This injected clock is used only for
    /// lease-expiry arithmetic, never to infer restart freshness.
    pub fn open_at(root: &Path, now_ms: i64) -> Result<Self, String> {
        prepare_directory(root)?;
        let directory = root.join(AGENTS);
        prepare_directory(&directory)?;
        let publications = root.join(publication::PUBLICATIONS);
        prepare_directory(&publications)?;
        Ok(Self {
            directory,
            root: root.to_path_buf(),
            served_since_ms: now_ms,
            serving_epoch: random_uuid(),
            admission: Mutex::new(()),
        })
    }

    /// The `agents` directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The `publications` directory.
    pub fn publications(&self) -> PathBuf {
        self.root.join(publication::PUBLICATIONS)
    }

    /// The opaque serving epoch of this registry instance.
    pub fn serving_epoch(&self) -> &str {
        &self.serving_epoch
    }

    /// When this registry began serving: every freshness decision's baseline.
    pub fn served_since_ms(&self) -> i64 {
        self.served_since_ms
    }

    /// Explicitly acquire a writer on a target channel. The target's `agent_id`
    /// remains unchanged across replacement; the opaque returned handle fences
    /// channel mutations. The publisher is an identity recorded as provenance,
    /// not a second agent registration: a publisher restart is a new incarnation
    /// UUID, and assignment may be reported by the named owner.
    pub fn acquire_writer(
        &self,
        request: &AcquireRequest,
        now_ms: i64,
    ) -> Result<WriterBinding, String> {
        publication::acquire_writer(self, request, now_ms)
    }

    /// Read one channel record, if it exists.
    pub fn channel(
        &self,
        agent_id: &str,
        channel: Channel,
    ) -> Result<Option<ChannelRecord>, String> {
        publication::channel(self, agent_id, channel)
    }

    /// Read the source-labelled public channel projection.
    pub fn published(
        &self,
        agent_id: &str,
        channel: Channel,
        now_ms: i64,
    ) -> Result<Option<PublicChannel>, String> {
        publication::published(self, agent_id, channel, now_ms)
    }

    /// Read a channel for a reader that is not its writer: same facts, no
    /// fencing handle.
    pub fn published_facts(
        &self,
        agent_id: &str,
        channel: Channel,
        now_ms: i64,
    ) -> Result<Option<PublicChannelFacts>, String> {
        publication::published_facts(self, agent_id, channel, now_ms)
    }

    /// Replace one channel's complete snapshot under its writer handle.
    pub fn publish(&self, request: &PublishRequest, now_ms: i64) -> Result<ChannelRecord, String> {
        publication::publish(self, request, now_ms)
    }

    /// Retire one channel writer without deleting its last snapshot.
    pub fn retire(
        &self,
        agent_id: &str,
        channel: Channel,
        writer_handle: &str,
        now_ms: i64,
    ) -> Result<ChannelRecord, String> {
        publication::retire(self, agent_id, channel, writer_handle, now_ms)
    }

    fn transition(&self) -> std::sync::MutexGuard<'_, ()> {
        self.admission
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads one registration.
    pub fn get(&self, agent_id: &str) -> Result<Option<Registration>, String> {
        validate_agent_id(agent_id)?;
        let path = self.path(agent_id);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("agent record {}: {error}", path.display())),
            Ok(_) => {}
        }
        let bytes = read_record_file(&path, MAX_REGISTRATION_BYTES)?;
        let registration = decode(&bytes).map_err(|error| format!("agent {agent_id}: {error}"))?;
        if registration.agent_id != agent_id {
            return Err(format!(
                "agent {agent_id}: record id does not match its filename"
            ));
        }
        Ok(Some(registration))
    }

    /// The registrations in stable agent-id order, at most `limit` of them and
    /// never more than [`MAX_LISTED`].
    ///
    /// The order is the durable key's, not a timestamp's: `registered_at` is
    /// when this daemon received a record, so a restore, a clock change or two
    /// records written in the same millisecond would all move a timestamp order
    /// around, while a page walk must be able to resume where it stopped. The
    /// caller that wants recency reads the timestamps it is given.
    ///
    /// Every record is read as a bounded regular file, never followed through a
    /// symlink. A malformed record fails the whole listing: a list must not
    /// forget an agent just because one file is unreadable.
    pub fn list(&self, limit: usize) -> Result<Vec<Registration>, String> {
        let mut registrations = self.all()?;
        registrations.truncate(limit.min(MAX_LISTED));
        Ok(registrations)
    }

    /// A cursor page of registrations in stable agent-id order.
    ///
    /// The cursor is the last key already returned; filtering uses strict lexical
    /// `>` so a page walk resumes exactly after that key and never skips an entry,
    /// even one written between pages. `limit` is the caller's page bound; a
    /// caller that asks for one extra record can tell whether another page exists.
    pub fn list_after(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Registration>, String> {
        if let Some(after) = after {
            validate_agent_id(after)?;
        }
        let records = self.all()?;
        Ok(records
            .into_iter()
            .filter(|record| after.is_none_or(|cursor| record.agent_id.as_str() > cursor))
            .take(limit)
            .collect())
    }

    /// Registers a publisher incarnation, or answers with the record it already
    /// has.
    ///
    /// The same source incarnation asking again with identical content is the
    /// same registration and returns its existing handle. Different content under
    /// one incarnation is refused: identity is not silently replaced. A different
    /// incarnation always registers, even when it shares a session UUID.
    ///
    /// Admission is serialized inside this `Registry`, so callers racing on one
    /// incarnation converge on one record. See [`Registry::admission`].
    pub fn register(
        &self,
        request: &RegistrationRequest,
        now_ms: i64,
    ) -> Result<Registration, String> {
        request.validate()?;
        let _transition = self.transition();
        if let Some(existing) = self.find_incarnation(&request.source, &request.incarnation)? {
            if existing.request == *request {
                return Ok(existing);
            }
            return Err(format!(
                "source `{}` incarnation {} already names different content",
                request.source, request.incarnation
            ));
        }
        let registration = Registration {
            version: REGISTRATION_VERSION,
            agent_id: random_uuid(),
            registered_at: format_millis(now_ms),
            request: request.clone(),
        };
        self.create(&registration)?;
        Ok(registration)
    }

    /// Every registration, in agent-id order.
    fn all(&self) -> Result<Vec<Registration>, String> {
        let entries = fs::read_dir(&self.directory).map_err(|error| format!("agents: {error}"))?;
        let mut registrations = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("agents: {error}"))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = read_record_file(&path, MAX_REGISTRATION_BYTES)?;
            let registration =
                decode(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
            if registration.agent_id
                != entry
                    .file_name()
                    .to_string_lossy()
                    .trim_end_matches(".json")
            {
                return Err(format!(
                    "{}: registration id does not match its filename",
                    path.display()
                ));
            }
            registrations.push(registration);
        }
        registrations.sort_by(|left, right| left.agent_id.cmp(&right.agent_id));
        Ok(registrations)
    }

    /// The record bound to one source incarnation, if any.
    fn find_incarnation(
        &self,
        source: &str,
        incarnation: &str,
    ) -> Result<Option<Registration>, String> {
        Ok(self.all()?.into_iter().find(|record| {
            record.request.source == source && record.request.incarnation == incarnation
        }))
    }

    /// Writes a new registration exactly once. An agent id is minted here, so a
    /// second write of the same id is a broken invariant rather than a retry.
    fn create(&self, registration: &Registration) -> Result<(), String> {
        validate_agent_id(&registration.agent_id)?;
        let bytes = serialize(registration)?;
        let temporary =
            self.directory
                .join(format!(".{}.tmp-{}", registration.agent_id, random_uuid()));
        let final_path = self.path(&registration.agent_id);
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::hard_link(&temporary, &final_path)?;
            fs::remove_file(&temporary)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(format!("agent {} already exists", registration.agent_id));
            }
            return Err(format!("agent {}: {error}", registration.agent_id));
        }
        Ok(())
    }

    fn path(&self, agent_id: &str) -> PathBuf {
        self.directory.join(format!("{agent_id}.json"))
    }
}

/// An agent id is a file name, so it has to be canonical before it can name one.
/// This rejects separators and traversal before any path is formed or read.
fn validate_agent_id(agent_id: &str) -> Result<(), String> {
    if SessionUuid::parse(agent_id).is_none() {
        return Err("agent id must be a canonical UUID".to_string());
    }
    Ok(())
}

/// An optional process identity is a claim, and the claim is what is checked:
/// it names a boot and a start stamp, and a pid is only meaningful with them.
fn validate_process(process: &ProcessIdentity) -> Result<(), String> {
    bounded_text("process.boot_id", &process.boot_id, false)?;
    if process.pid <= 0 {
        return Err("`process.pid` must be positive".to_string());
    }
    Ok(())
}

/// A bounded text field: at most [`MAX_TEXT_BYTES`], and carrying nothing a
/// terminal would act on.
fn bounded_text(field: &str, value: &str, allow_empty: bool) -> Result<(), String> {
    if !allow_empty && value.is_empty() {
        return Err(format!("`{field}` is required and must be non-empty"));
    }
    if value.len() > MAX_TEXT_BYTES {
        return Err(format!("`{field}` exceeds {MAX_TEXT_BYTES} bytes"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("`{field}` carries a control character"));
    }
    Ok(())
}

/// A path the daemon can hold for an executor: absolute, bounded, printable.
/// A relative path would resolve against whatever directory the executor
/// happened to run in, which is the ambiguity this record exists to remove.
fn absolute_path(field: &str, value: &str) -> Result<(), String> {
    bounded_text(field, value, false)?;
    if !value.starts_with('/') {
        return Err(format!("`{field}` must be an absolute path"));
    }
    Ok(())
}

/// Encodes one registration, bounded.
fn serialize(registration: &Registration) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec_pretty(registration)
        .map_err(|error| format!("agent {}: {error}", registration.agent_id))?;
    if bytes.len() > MAX_REGISTRATION_BYTES {
        return Err(format!(
            "agent {} exceeds {MAX_REGISTRATION_BYTES} bytes",
            registration.agent_id
        ));
    }
    Ok(bytes)
}

/// Decodes one registration, bounded and version-checked: a file this daemon
/// could not have written is not a record it will act on.
fn decode(bytes: &[u8]) -> Result<Registration, String> {
    if bytes.len() > MAX_REGISTRATION_BYTES {
        return Err(format!(
            "registration exceeds {MAX_REGISTRATION_BYTES} bytes"
        ));
    }
    let registration: Registration =
        serde_json::from_slice(bytes).map_err(|error| format!("not a registration: {error}"))?;
    if registration.version != REGISTRATION_VERSION {
        return Err(format!(
            "registration version {} is not served; this daemon writes {REGISTRATION_VERSION}",
            registration.version
        ));
    }
    Ok(registration)
}

/// Reads one record as a bounded regular file, never through a symlink.
///
/// The same rule the mutation store applies to its records, kept here so a
/// planted path cannot be read as a registry or publication record. Extracting
/// the two copies into one shared helper belongs with a change that may touch
/// the store.
fn read_record_file(path: &Path, bound: usize) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("record {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!("record {} is not a regular file", path.display()));
    }
    if metadata.len() > bound as u64 {
        return Err(format!("record {} exceeds {bound} bytes", path.display()));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("record {}: {error}", path.display()))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("record {}: {error}", path.display()))?;
    if !opened.is_file() || opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err(format!("record {} changed while opening", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((bound + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("record {}: {error}", path.display()))?;
    if bytes.len() > bound {
        return Err(format!("record {} exceeds {bound} bytes", path.display()));
    }
    Ok(bytes)
}

/// Creates `directory` `0700` where it is missing, then requires it to be a real
/// directory of this user with mode `0700`.
///
/// The mutation store applies the same rule to its own directory; the two remain
/// separate until a change may touch the store.
fn prepare_directory(directory: &Path) -> Result<(), String> {
    let diagnostic =
        |error: std::io::Error| format!("registry directory {}: {error}", directory.display());
    if !directory.exists() {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(diagnostic)?;
        // The umask can take bits off the creation mode, and the mode is part of
        // what makes a record trustworthy, so what is required is set explicitly.
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).map_err(diagnostic)?;
    }
    let metadata = fs::symlink_metadata(directory).map_err(diagnostic)?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "registry directory {} is a symlink",
            directory.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "registry directory {} is not a directory",
            directory.display()
        ));
    }
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err(format!(
            "registry directory {} is owned by uid {}",
            directory.display(),
            metadata.uid()
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(format!(
            "registry directory {} is not mode 0700",
            directory.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INCARNATION: &str = "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22";
    const SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

    fn request() -> RegistrationRequest {
        RegistrationRequest {
            source: "herdsman".into(),
            incarnation: INCARNATION.into(),
            session: Some(SESSION.into()),
            owner: None,
            run: None,
            label: Some("worker".into()),
            location: None,
            process: None,
            launch: None,
        }
    }

    fn launch() -> LaunchSpec {
        LaunchSpec {
            executable: "/usr/bin/pi".into(),
            argv: vec!["--resume".into(), "--session".into(), SESSION.into()],
            cwd: "/home/dev/proj".into(),
            session: Some(LaunchSession {
                uuid: Some(SESSION.into()),
                path: None,
            }),
            provenance: "herdsman".into(),
            revision: "7".into(),
        }
    }

    #[test]
    fn a_relative_executable_or_cwd_is_not_a_launch_the_daemon_will_hold() {
        let mut spec = launch();
        spec.executable = "pi".into();
        assert!(
            spec.validate()
                .expect_err("a relative executable")
                .contains("`executable` must be an absolute path")
        );
        let mut spec = launch();
        spec.cwd = "./proj".into();
        assert!(
            spec.validate()
                .expect_err("a relative cwd")
                .contains("`cwd` must be an absolute path")
        );
        let mut spec = launch();
        spec.argv = vec!["x".into(); MAX_ARGV + 1];
        assert!(
            spec.validate()
                .expect_err("too many arguments")
                .contains("`argv` names more than")
        );
    }

    #[test]
    fn a_launch_session_names_a_uuid_or_a_path() {
        let mut spec = launch();
        spec.session = Some(LaunchSession {
            uuid: None,
            path: None,
        });
        assert!(
            spec.validate()
                .expect_err("an empty session")
                .contains("`session` names a uuid or a path")
        );
        let mut spec = launch();
        spec.session = Some(LaunchSession {
            uuid: Some("not-a-uuid".into()),
            path: None,
        });
        assert!(
            spec.validate()
                .expect_err("a malformed uuid")
                .contains("canonical UUID")
        );
        let mut spec = launch();
        spec.session = Some(LaunchSession {
            uuid: None,
            path: Some("relative.session".into()),
        });
        assert!(
            spec.validate()
                .expect_err("a relative session path")
                .contains("`session.path` must be an absolute path")
        );
    }

    #[test]
    fn a_registration_validates_its_identities_and_bounds() {
        request().validate().expect("a valid request");

        let mut unset = request();
        unset.incarnation = "nope".into();
        assert!(
            unset
                .validate()
                .expect_err("a malformed incarnation")
                .contains("`incarnation` must be a canonical UUID")
        );

        let mut unset = request();
        unset.session = Some("nope".into());
        assert!(
            unset
                .validate()
                .expect_err("a malformed session")
                .contains("`session` must be a canonical UUID")
        );

        let mut unset = request();
        unset.source = String::new();
        assert!(
            unset
                .validate()
                .expect_err("an empty source")
                .contains("`source` is required")
        );

        let mut unbounded = request();
        unbounded.owner = Some("s".repeat(MAX_TEXT_BYTES + 1));
        assert!(
            unbounded
                .validate()
                .expect_err("an over-long owner")
                .contains("exceeds")
        );

        let mut controlled = request();
        controlled.label = Some("danger\u{7}".into());
        assert!(
            controlled
                .validate()
                .expect_err("a control character")
                .contains("control character")
        );

        let mut process = request();
        process.process = Some(ProcessIdentity {
            boot_id: String::new(),
            pid: 1,
            start_ticks: 1,
        });
        assert!(
            process
                .validate()
                .expect_err("an unnamed boot")
                .contains("`process.boot_id` is required")
        );

        let mut process = request();
        process.process = Some(ProcessIdentity {
            boot_id: "boot".into(),
            pid: 0,
            start_ticks: 1,
        });
        assert!(
            process
                .validate()
                .expect_err("a zero pid")
                .contains("`process.pid` must be positive")
        );
    }

    #[test]
    fn a_state_field_is_unknown_to_a_registration() {
        // State is a mutable publisher fact, and it belongs to the publication
        // channels: an immutable record whose content could change would make an
        // identical retry ambiguous. So it is not a registration field, and the
        // foundation refuses it like any other unknown field. Vocabulary
        // preservation is validated where that vocabulary is accepted.
        let json =
            format!(r#"{{"source":"herdsman","incarnation":"{INCARNATION}","state":"working"}}"#);
        let error = serde_json::from_str::<RegistrationRequest>(&json).expect_err("a refusal");
        assert!(error.to_string().contains("unknown field"), "{error}");
    }

    #[test]
    fn unknown_location_vocabulary_survives_the_codec() {
        let mut wanted = request();
        wanted.location = Some(RegistryLocation {
            backend: "some-future-mux".into(),
            instance: None,
            workspace: None,
            tab: None,
            pane: Some("wA:p1".into()),
        });
        let bytes = serde_json::to_vec(&wanted).expect("an encoding");
        let decoded: RegistrationRequest = serde_json::from_slice(&bytes).expect("a decoding");
        assert_eq!(decoded, wanted);
    }

    #[test]
    fn a_claimed_process_identity_is_strict_on_the_registration_wire() {
        let mut wanted = request();
        wanted.process = Some(ProcessIdentity {
            boot_id: "boot-abc".into(),
            pid: 4242,
            start_ticks: 99,
        });
        let bytes = serde_json::to_vec(&wanted).expect("an encoding");
        let decoded: RegistrationRequest = serde_json::from_slice(&bytes).expect("a decoding");
        assert_eq!(decoded, wanted, "a valid identity round-trips");

        // An identity this build does not understand is refused rather than read
        // as a weaker one. The shared model tolerates unknown nested fields for
        // the observation path; the registry wire does not.
        let json = format!(
            r#"{{"source":"herdsman","incarnation":"{INCARNATION}","process":{{"boot_id":"b","pid":1,"start_ticks":2,"verified":true}}}}"#
        );
        let error = serde_json::from_str::<RegistrationRequest>(&json).expect_err("a refusal");
        assert!(error.to_string().contains("unknown field"), "{error}");

        // A missing identity is an absent claim, not a malformed one.
        let json = format!(r#"{{"source":"herdsman","incarnation":"{INCARNATION}"}}"#);
        assert_eq!(
            serde_json::from_str::<RegistrationRequest>(&json)
                .expect("a decoding")
                .process,
            None
        );
    }

    #[test]
    fn a_request_with_an_unknown_field_is_refused_rather_than_read() {
        let json = format!(
            r#"{{"source":"herdsman","incarnation":"{INCARNATION}","session":null,"mode":"fast"}}"#
        );
        assert!(
            serde_json::from_str::<RegistrationRequest>(&json)
                .expect_err("an unknown field")
                .to_string()
                .contains("unknown field")
        );
    }

    #[test]
    fn a_registration_of_another_version_is_not_read_as_this_one() {
        let registration = Registration {
            version: 2,
            agent_id: random_uuid(),
            registered_at: format_millis(1_000),
            request: request(),
        };
        let bytes = serde_json::to_vec(&registration).expect("an encoding");
        assert!(
            decode(&bytes)
                .expect_err("a foreign version")
                .contains("is not served")
        );
    }
}
