//! The durable request store: what the daemon knows about an operation.
//!
//! A lifecycle operation is a file before it is anything else. The record — the
//! request's identity, the target it froze, when it was asked for and when it
//! expires — is written before execution begins, and every state the operation
//! reaches is written into that same file. That ordering is the whole point: a
//! daemon that dies mid-operation leaves a record saying the outcome is unknown,
//! which is a different and more useful answer than silence, and a client that
//! times out can ask again instead of guessing whether its request ran.
//!
//! Records are one JSON document per request, written into a temporary sibling
//! and renamed into place, so a reader only ever sees a whole record. Nothing is
//! ever executed because a client asked twice: [`Store::unresolved`] is what a
//! second request for the same target and method is refused by, and only this
//! store's own records — never a client's clock — decide that a request is over.
//!
//! [`derive`] is the one place an outcome is read out of a record, because the
//! later slices that execute operations and the UI that reports them must agree
//! on what "unknown" means.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::control::timestamp_millis;
use crate::runtime::CreatedIdentity;

/// The directory inside the state root that holds one file per request.
const REQUESTS: &str = "requests";
const JOURNAL: &str = "events.json";
const JOURNAL_DIRTY: &str = "events.dirty";
const MAX_EVENTS: usize = 512;
const MAX_EVENT_BYTES: usize = 512 * 1024;
static JOURNAL_LOCKS: OnceLock<Mutex<std::collections::HashMap<PathBuf, Arc<Mutex<()>>>>> =
    OnceLock::new();
type EventWake = Arc<(Mutex<u64>, std::sync::Condvar)>;
type EventWakes = Mutex<std::collections::HashMap<PathBuf, EventWake>>;

static EVENT_WAKES: OnceLock<EventWakes> = OnceLock::new();

fn event_wake(root: &Path) -> EventWake {
    let wakes = EVENT_WAKES.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut wakes = wakes
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(
        wakes
            .entry(key)
            .or_insert_with(|| Arc::new((Mutex::new(0), std::sync::Condvar::new()))),
    )
}

fn journal_lock(root: &Path) -> Arc<Mutex<()>> {
    let locks = JOURNAL_LOCKS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut locks = locks
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Arc::clone(locks.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))))
}

/// A privacy-safe pointer to a committed daemon fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle_event_id: Option<String>,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Journal {
    epoch: String,
    next_seq: u64,
    #[serde(default)]
    source_seq: u64,
    events: Vec<Event>,
}

/// How long an operation's request stays executable, and how long a resume's
/// does. A resume stops an agent and starts it again — several waits long — so it
/// is given its own, longer life. Decided by method rather than by the caller: a
/// client that could name its own expiry could keep a stale request executable.
pub fn ttl_seconds(method: &str) -> u64 {
    if method == "resume" { 120 } else { 30 }
}

/// The categories a record's refusals are grouped under. These are the words the
/// UI and the later slices match on, so they live here rather than at each site
/// that writes one.
pub struct Category;

impl Category {
    /// The request was never claimed and its expiry passed: nothing ran.
    pub const NOT_EXECUTED: &'static str = "not_executed";
    /// A request for the same target and method was already unresolved.
    pub const IN_FLIGHT: &'static str = "in_flight";
    /// The operation needs a mux backend this daemon has not been given.
    pub const BACKEND_UNAVAILABLE: &'static str = "backend_unavailable";
    /// The backend gave an explicit refusal before reporting success.
    pub const BACKEND_REFUSED: &'static str = "backend_refused";
    /// The daemon refused the operation itself: an ineligible target, a frozen
    /// target that no longer matches, or an effect it will not dispatch.
    pub const REFUSED: &'static str = "refused";
}

/// What has happened to a request so far. Written in order; `Completed` is
/// terminal and carries the outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// Recorded, nothing claimed it yet.
    Pending,
    /// Claimed: the daemon is about to execute it, so it must never be retried.
    Claimed,
    /// Execution began. From here an outcome can only be recorded, never assumed.
    Started,
    /// Terminal: an outcome was recorded.
    Completed,
}

/// What a completed request did.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    /// The operation ran and its effects are in the record.
    Completed,
    /// The operation did not run, and the record says why.
    Refused,
    /// Execution began and the daemon cannot say what it did.
    Unknown,
}

/// One durable request.
///
/// `target` is the frozen identity's stable key, kept so a second request for the
/// same target and method can be refused while this one is unresolved. The
/// identity itself is a later slice's; what matters here is that a request names
/// what it is about, and that name does not change while it is in flight.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestRecord {
    pub id: String,
    pub method: String,
    /// The caller-supplied label of whoever asked, for a human reading records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Canonical params provide exact semantic equality on same-ID retries.
    /// Legacy fingerprint-only records remain readable but cannot prove a match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// RFC 3339 UTC with milliseconds, as [`now`] writes it.
    pub requested_at: String,
    pub expires_at: String,
    pub state: RecordState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<RequestOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default)]
    pub effects: Vec<String>,
    /// The identity a creation reported, in the runtime's own words: a kind and
    /// an id. Absent unless a create completed and its answer named the location
    /// — a refused or uncertain operation created nothing, and prose a client
    /// would have to parse is not an answer it can act on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<CreatedIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

impl RequestRecord {
    /// A new record, pending, expiring [`ttl_seconds`] after `now_ms`.
    ///
    /// Written before anything is executed; this is the file a crash leaves.
    pub fn new(
        id: &str,
        method: &str,
        requester: Option<String>,
        target: Option<String>,
        now_ms: i64,
    ) -> Self {
        Self {
            id: id.to_string(),
            method: method.to_string(),
            requester,
            target,
            params: None,
            fingerprint: None,
            requested_at: format_millis(now_ms),
            expires_at: format_millis(now_ms + (ttl_seconds(method) as i64) * 1000),
            state: RecordState::Pending,
            outcome: None,
            category: None,
            message: None,
            effects: Vec::new(),
            created: None,
            completed_at: None,
        }
    }

    /// Stores canonical request params for exact semantic retry comparison.
    pub fn with_params(mut self, params: serde_json::Value) -> Self {
        self.params = Some(params);
        self
    }

    /// Records the identity a creation reported, if its answer named one.
    pub fn created(mut self, created: Option<CreatedIdentity>) -> Self {
        self.created = created;
        self
    }

    /// Marks the request claimed: the daemon is about to execute it.
    pub fn claim(&self) -> Self {
        Self {
            state: RecordState::Claimed,
            ..self.clone()
        }
    }

    /// Marks execution as begun.
    pub fn start(&self) -> Self {
        Self {
            state: RecordState::Started,
            ..self.clone()
        }
    }

    /// Records the terminal outcome. A record that is already complete is
    /// returned unchanged: an outcome is written once, and a second writer — a
    /// restarting daemon, say — must not overwrite the first answer.
    pub fn complete(
        &self,
        outcome: RequestOutcome,
        category: Option<&str>,
        message: Option<&str>,
        effects: Vec<String>,
        now_ms: i64,
    ) -> Self {
        if self.state == RecordState::Completed {
            return self.clone();
        }
        Self {
            state: RecordState::Completed,
            outcome: Some(outcome),
            category: category.map(str::to_string),
            message: message.map(str::to_string),
            effects,
            completed_at: Some(format_millis(now_ms)),
            ..self.clone()
        }
    }
}

/// What a record means right now, in the order the later slices rely on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Derived {
    /// An outcome was recorded: terminal, whatever the clock says.
    Settled(RequestOutcome),
    /// A claim or a start without an outcome: the daemon cannot say what ran,
    /// and nothing may be retried.
    Unknown,
    /// Never claimed, and past its expiry: nothing ran, and nothing will.
    NotExecuted,
    /// Recorded, unclaimed, and still inside its expiry.
    Pending,
}

impl RequestRecord {
    /// What this record means at `now_ms`.
    ///
    /// A recorded outcome wins over everything, including the clock: it is the
    /// only thing that says what actually happened. A claim or a start is
    /// unknown, and an expired request that was never claimed is a refusal that
    /// names `not_executed` — not silence, and not a retry.
    pub fn derive(&self, now_ms: i64) -> Derived {
        if self.state == RecordState::Completed {
            return match self.outcome {
                Some(outcome) => Derived::Settled(outcome),
                // Completed without an outcome is a record this build cannot
                // read; it must not become a green light for a retry.
                None => Derived::Unknown,
            };
        }
        if matches!(self.state, RecordState::Claimed | RecordState::Started) {
            return Derived::Unknown;
        }
        match timestamp_millis(&self.expires_at) {
            Some(expires) if expires <= now_ms => Derived::NotExecuted,
            // An expiry this build cannot read is not a licence to execute.
            None => Derived::Unknown,
            Some(_) => Derived::Pending,
        }
    }
}

/// The daemon's records on disk.
///
/// Shared by every connection, so it is used behind one mutex and never by two
/// threads at once: the read-check-write that refuses a duplicate request is one
/// critical section, not three.
#[derive(Clone, Debug)]
pub struct Store {
    directory: PathBuf,
    root: PathBuf,
    journal_lock: Arc<Mutex<()>>,
    event_wake: Arc<(Mutex<u64>, std::sync::Condvar)>,
    events: Arc<Mutex<super::events::Log>>,
}

pub(crate) fn notify_events(root: &Path) {
    let wake = event_wake(root);
    *wake.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    wake.1.notify_all();
}

mod delivery;

impl Store {
    /// Opens the store under `root`, creating `root` and its `requests`
    /// directory if they are missing.
    ///
    /// The directory must be a real `0700` directory of this user: a record says
    /// which panes were closed and by whom, keeps the literal input a client sent
    /// and the metadata a publisher reported as it was given — these records are
    /// private, not redacted — and a path anyone else can write is not a record.
    pub fn open(root: &Path) -> Result<Self, String> {
        prepare_directory(root)?;
        let directory = root.join(REQUESTS);
        prepare_directory(&directory)?;
        let store = Self {
            directory,
            root: root.to_path_buf(),
            journal_lock: journal_lock(root),
            event_wake: event_wake(root),
            events: super::events::shared(&root.join("events"))?,
        };
        store.begin_journal_epoch()?;
        Ok(store)
    }

    fn begin_journal_epoch(&self) -> Result<(), String> {
        let _guard = self
            .journal_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dirty = self.root.join(JOURNAL_DIRTY);
        let unclean = dirty.exists();
        let journal = self.load_journal()?;
        let epoch = if unclean {
            random_uuid()
        } else {
            journal.epoch
        };
        let journal = Journal { epoch, ..journal };
        self.write_journal(&journal)?;
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&dirty)
            .and_then(|file| file.sync_all())
            .map_err(|error| format!("event dirty marker: {error}"))?;
        Ok(())
    }

    /// Marks a clean daemon stop, allowing the epoch to remain stable on reopen.
    pub fn finish_journal(&self) -> Result<(), String> {
        let _guard = self
            .journal_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match fs::remove_file(self.root.join(JOURNAL_DIRTY)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("event dirty marker: {error}")),
        }
    }

    fn load_journal(&self) -> Result<Journal, String> {
        let path = self.root.join(JOURNAL);
        match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|error| format!("event journal: {error}"))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal {
                epoch: random_uuid(),
                next_seq: 1,
                source_seq: 0,
                events: Vec::new(),
            }),
            Err(error) => Err(format!("event journal: {error}")),
        }
    }

    fn write_journal(&self, journal: &Journal) -> Result<(), String> {
        let bytes =
            serde_json::to_vec(journal).map_err(|error| format!("event journal: {error}"))?;
        let temporary = self.root.join(format!(".events.tmp-{}", random_uuid()));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.root.join(JOURNAL))?;
            fs::File::open(&self.root)?.sync_all()
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(temporary);
            return Err(format!("event journal: {error}"));
        }
        Ok(())
    }

    /// Appends a pointer event after its associated fact has become durable.
    ///
    /// Callers must treat failure as a lost event, not a failed fact: log/ignore
    /// the error and return the durable fact. An unclean restart changes the
    /// journal epoch so consumers know to resynchronise after such a gap.
    pub fn append_event(&self, mut event: Event) -> Result<Event, String> {
        if event.kind.is_empty()
            || event.kind.len() > 64
            || event.kind.chars().any(char::is_control)
        {
            return Err("event kind must be nonempty bounded text".into());
        }
        for identifier in [
            &event.agent_id,
            &event.spawn,
            &event.request_id,
            &event.subject_id,
            &event.lifecycle_event_id,
        ]
        .into_iter()
        .flatten()
        {
            if identifier.is_empty()
                || identifier.len() > 128
                || identifier.chars().any(char::is_control)
            {
                return Err("event identifiers must be nonempty bounded text".into());
            }
        }
        if event.at.is_empty() || event.at.len() > 64 || event.at.chars().any(char::is_control) {
            return Err("event time must be nonempty bounded text".into());
        }
        let _guard = self
            .journal_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut journal = self.load_journal()?;
        event.seq = journal.next_seq;
        let encoded = serde_json::to_vec(&event).map_err(|error| format!("event: {error}"))?;
        if encoded.len() > MAX_EVENT_BYTES {
            return Err("event exceeds journal byte limit".into());
        }
        journal.next_seq += 1;
        journal.events.push(event.clone());
        let mut journal_bytes =
            serde_json::to_vec(&journal).map_err(|error| format!("event journal: {error}"))?;
        while journal.events.len() > MAX_EVENTS || journal_bytes.len() > MAX_EVENT_BYTES {
            if journal.events.len() <= 1 {
                break;
            }
            journal.events.remove(0);
            journal_bytes =
                serde_json::to_vec(&journal).map_err(|error| format!("event journal: {error}"))?;
        }
        self.write_journal(&journal)?;
        let (generation, wake) = &*self.event_wake;
        *generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        wake.notify_all();
        Ok(event)
    }

    pub fn event_generation(&self) -> u64 {
        let (generation, _) = &*self.event_wake;
        *generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn wait_for_event(
        &self,
        observed: u64,
        duration: std::time::Duration,
    ) -> std::sync::WaitTimeoutResult {
        let (generation, wake) = &*self.event_wake;
        let generation = generation
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_guard, result) = wake
            .wait_timeout_while(generation, duration, |current| *current == observed)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        result
    }

    /// Retained events in sequence order, for internal verification and future reads.
    pub fn events(&self) -> Result<Vec<Event>, String> {
        Ok(self.event_snapshot()?.3)
    }

    /// Journal epoch, current cursor and earliest retained sequence.
    pub fn event_cursor(&self) -> Result<(String, u64, u64), String> {
        let (epoch, tail, earliest, _, _) = self.event_snapshot()?;
        Ok((epoch, tail, earliest))
    }

    /// The `requests` directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Reads one record.
    pub fn get(&self, id: &str) -> Result<Option<RequestRecord>, String> {
        validate_id(id)?;
        let path = self.path(id);
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self
                    .events
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .operation(id)
                    .is_some()
                {
                    return Err("event-backed operation is missing its private seed".into());
                }
                return Ok(None);
            }
            Err(error) => return Err(format!("request record {}: {error}", path.display())),
            Ok(_) => {}
        }
        let bytes = read_record_file(&path)?;
        let mut record = decode(&bytes).map_err(|error| format!("request {id}: {error}"))?;
        if record.id != id {
            return Err(format!(
                "request {id}: record id does not match its filename"
            ));
        }
        if let Some(projection) = self
            .events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .operation(id)
        {
            super::events::operations::apply(projection, &mut record)?;
        }
        Ok(Some(record))
    }

    /// Writes a new record exactly once. A second request reusing an id cannot
    /// replace the first record, even if the method or target differs.
    pub fn create(&self, record: &RequestRecord) -> Result<(), String> {
        validate_id(&record.id)?;
        if super::events::operations::managed(&record.method) {
            if self.get(&record.id)?.is_some() {
                return Err(format!("request {} already exists", record.id));
            }
            let mut log = self.events.lock().unwrap_or_else(|e| e.into_inner());
            if log.operation(&record.id).is_some() {
                return Err(format!("request {} already exists", record.id));
            }
            let bytes = serde_json::to_vec_pretty(record)
                .map_err(|_| "cannot encode private operation seed")?;
            super::registry::publication::write_atomic(
                &self.directory,
                &self.path(&record.id),
                &bytes,
                MAX_RECORD_BYTES,
            )?;
            return log.record_operation(record, false);
        }
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| format!("request {}: {error}", record.id))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(format!(
                "request {} exceeds {MAX_RECORD_BYTES} bytes",
                record.id
            ));
        }
        let temporary = self
            .directory
            .join(format!(".{}.tmp-{}", record.id, random_uuid()));
        let final_path = self.path(&record.id);
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
                return Err(format!("request {} already exists", record.id));
            }
            return Err(format!("request {}: {error}", record.id));
        }
        Ok(())
    }

    /// Replaces an existing record atomically for state transitions. The caller
    /// must have created it first; this is not an alternate way to accept a new
    /// request id.
    pub fn write(&self, record: &RequestRecord) -> Result<(), String> {
        validate_id(&record.id)?;
        let previous = self
            .get(&record.id)?
            .ok_or_else(|| format!("request {} does not exist", record.id))?;
        if super::events::operations::managed(&record.method) {
            if previous.params != record.params || previous.fingerprint != record.fingerprint {
                return Err("operation update changes private admitted parameters".into());
            }
            let mut log = self.events.lock().unwrap_or_else(|e| e.into_inner());
            if log.operation(&record.id).is_none() {
                log.record_operation(&previous, true)?;
            }
            return log.record_operation(record, false);
        }
        let bytes = serde_json::to_vec_pretty(record)
            .map_err(|error| format!("request {}: {error}", record.id))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(format!(
                "request {} exceeds {MAX_RECORD_BYTES} bytes",
                record.id
            ));
        }
        let temporary = self
            .directory
            .join(format!(".{}.tmp-{}", record.id, random_uuid()));
        let result = (|| -> std::io::Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, self.path(&record.id))
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(format!("request {}: {error}", record.id));
        }
        Ok(())
    }

    /// The newest records first, at most `limit` of them.
    ///
    /// Every JSON record is read as a bounded regular file, never followed
    /// through a symlink. A malformed record fails the whole listing: suppression
    /// must not forget an unknown request just because one file is unreadable.
    pub fn recent(&self, limit: usize) -> Result<Vec<RequestRecord>, String> {
        let entries =
            fs::read_dir(&self.directory).map_err(|error| format!("requests: {error}"))?;
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| format!("requests: {error}"))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let bytes = read_record_file(&path)?;
            let record = decode(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
            if record.id
                != entry
                    .file_name()
                    .to_string_lossy()
                    .trim_end_matches(".json")
            {
                return Err(format!(
                    "{}: record id does not match its filename",
                    path.display()
                ));
            }
            let mut record = record;
            if let Some(projection) = self
                .events
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .operation(&record.id)
            {
                super::events::operations::apply(projection, &mut record)?;
            }
            records.push(record);
        }
        let log = self.events.lock().unwrap_or_else(|e| e.into_inner());
        for id in log.operation_ids() {
            if !records.iter().any(|record| &record.id == id) {
                return Err("event-backed operation is missing its private seed".into());
            }
        }
        drop(log);
        records.sort_by(|left, right| {
            let left_at = timestamp_millis(&left.requested_at).unwrap_or(i64::MIN);
            let right_at = timestamp_millis(&right.requested_at).unwrap_or(i64::MIN);
            right_at.cmp(&left_at).then_with(|| right.id.cmp(&left.id))
        });
        records.truncate(limit);
        Ok(records)
    }

    /// Every record that is not yet completed, however old. `recent` is bounded
    /// by recency, so it cannot answer whether an operation is still pending.
    pub fn unfinished(&self) -> Result<Vec<RequestRecord>, String> {
        Ok(self
            .recent(usize::MAX)?
            .into_iter()
            .filter(|record| record.state != RecordState::Completed)
            .collect())
    }

    /// The newest unresolved mutation, regardless of method, over `target`.
    ///
    /// A request that has been claimed or started has no recorded outcome yet,
    /// and a request whose outcome is [`RequestOutcome::Unknown`] began without
    /// a trustworthy account of what it did. Both suppress another mutation over
    /// the same target: an unknown effect must not be replayed. `None` is a
    /// daemon-wide lane when the request did not name a target.
    pub fn unresolved(
        &self,
        target: Option<&str>,
        now_ms: i64,
    ) -> Result<Option<RequestRecord>, String> {
        let held = self.recent(usize::MAX)?;
        Ok(held.into_iter().find(|record| {
            record.target.as_deref() == target
                && matches!(
                    record.derive(now_ms),
                    Derived::Pending | Derived::Unknown | Derived::Settled(RequestOutcome::Unknown)
                )
        }))
    }

    fn path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("{id}.json"))
    }
}

/// A store path is built from request ids, so the id has to be canonical before
/// it can name a file. This rejects separators and traversal before any path is
/// formed or read.
fn validate_id(id: &str) -> Result<(), String> {
    if crate::model::SessionUuid::parse(id).is_none() {
        return Err("request id must be a canonical UUID".to_string());
    }
    Ok(())
}

fn read_record_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("request record {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(format!(
            "request record {} is not a regular file",
            path.display()
        ));
    }
    if metadata.len() > MAX_RECORD_BYTES as u64 {
        return Err(format!(
            "request record {} exceeds {MAX_RECORD_BYTES} bytes",
            path.display()
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| format!("request record {}: {error}", path.display()))?;
    let opened = file
        .metadata()
        .map_err(|error| format!("request record {}: {error}", path.display()))?;
    if !opened.is_file() || opened.dev() != metadata.dev() || opened.ino() != metadata.ino() {
        return Err(format!(
            "request record {} changed while opening",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_RECORD_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("request record {}: {error}", path.display()))?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(format!(
            "request record {} exceeds {MAX_RECORD_BYTES} bytes",
            path.display()
        ));
    }
    Ok(bytes)
}

/// Creates `directory` `0700` where it is missing, then requires it to be a real
/// directory of this user with mode `0700`.
fn prepare_directory(directory: &Path) -> Result<(), String> {
    let diagnostic =
        |error: std::io::Error| format!("control state directory {}: {error}", directory.display());
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
            "control state directory {} is a symlink",
            directory.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "control state directory {} is not a directory",
            directory.display()
        ));
    }
    if metadata.uid() != unsafe { libc::getuid() } {
        return Err(format!(
            "control state directory {} is owned by uid {}",
            directory.display(),
            metadata.uid()
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(format!(
            "control state directory {} is not mode 0700",
            directory.display()
        ));
    }
    Ok(())
}

/// Decodes one record, bounded: a file this daemon could not have written is not
/// a record it will act on.
fn decode(bytes: &[u8]) -> Result<RequestRecord, String> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(format!("record exceeds {MAX_RECORD_BYTES} bytes"));
    }
    serde_json::from_slice(bytes).map_err(|error| format!("not a record: {error}"))
}

/// The largest record this daemon reads or writes. Records carry a request's
/// identity and its effects, and a bound keeps a planted file from being read
/// into memory whole.
const MAX_RECORD_BYTES: usize = 64 * 1024;

/// A fresh version-4 UUID from the kernel's random source, for record ids and
/// temporary file names.
pub fn random_uuid() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut source) = File::open("/dev/urandom") {
        if source.read_exact(&mut bytes).is_err() {
            bytes = fallback_bytes();
        }
    } else {
        bytes = fallback_bytes();
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Randomness of last resort, when `/dev/urandom` cannot be read: the clock and
/// the process id, mixed so that two ids from one moment differ.
///
/// These ids only have to be unique within one machine's store; a caller that
/// needs unguessability is not the caller of this function.
fn fallback_bytes() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    bytes[..8].copy_from_slice(&(nanos as u64).to_le_bytes());
    bytes[8..12].copy_from_slice(&(std::process::id()).to_le_bytes());
    bytes[12..].copy_from_slice(&(nanos >> 64).to_le_bytes());
    bytes
}

/// Now, in the store's timestamp format, for a caller that has no clock of its
/// own to hand.
pub fn now() -> String {
    format_millis(unix_ms(std::time::SystemTime::now()))
}

/// The current time in the store's format, as Unix milliseconds.
pub fn now_ms() -> i64 {
    unix_ms(std::time::SystemTime::now())
}

fn unix_ms(now: std::time::SystemTime) -> i64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// Formats Unix milliseconds as this store's RFC 3339 UTC with milliseconds.
///
/// The format is fixed-width and always UTC, so two of these compare correctly
/// as strings as well as through [`timestamp_millis`].
pub fn format_millis(millis: i64) -> String {
    let seconds = millis.div_euclid(1000);
    let sub = millis.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3600;
    let minute = (second_of_day % 3600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{sub:03}Z")
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm, with
/// the epoch shifted to 0000-03-01 so leap days land at the end of an era).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22";

    fn record(method: &str, target: Option<&str>, now_ms: i64) -> RequestRecord {
        RequestRecord::new(
            ID,
            method,
            Some("radar".into()),
            target.map(str::to_string),
            now_ms,
        )
    }

    #[test]
    fn journal_is_ordered_bounded_and_persistent_and_has_safe_event_shape() {
        let directory = tempdir("journal");
        let store = Store::open(&directory).expect("store");
        let epoch = store.event_cursor().unwrap().0;
        for n in 0..MAX_EVENTS + 1 {
            store
                .append_event(Event {
                    subject_id: None,
                    lifecycle_event_id: None,
                    seq: 0,
                    kind: format!("k{n}"),
                    agent_id: Some("agent".into()),
                    spawn: None,
                    request_id: None,
                    at: "now".into(),
                })
                .unwrap();
        }
        let (same_epoch, cursor, earliest) = store.event_cursor().unwrap();
        assert_eq!(same_epoch, epoch);
        assert_eq!(cursor, (MAX_EVENTS + 1) as u64);
        assert_eq!(earliest, 2);
        let serialized = fs::read_to_string(directory.join(JOURNAL)).unwrap();
        assert!(!serialized.contains("environment"));
        assert!(!serialized.contains("token"));
        store.finish_journal().unwrap();
        let reopened = Store::open(&directory).unwrap();
        assert_eq!(reopened.event_cursor().unwrap().0, epoch);
        let event = reopened
            .append_event(Event {
                subject_id: None,
                lifecycle_event_id: None,
                seq: 0,
                kind: "publication".into(),
                agent_id: Some("a".into()),
                spawn: None,
                request_id: None,
                at: "later".into(),
            })
            .unwrap();
        assert_eq!(event.seq, (MAX_EVENTS + 2) as u64);
        drop(reopened);
        let unclean = Store::open(&directory).unwrap();
        assert_ne!(unclean.event_cursor().unwrap().0, epoch);
    }

    #[test]
    fn concurrent_journal_appends_are_serialized_across_store_handles() {
        let directory = tempdir("journal-concurrent");
        let store = Store::open(&directory).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|worker| {
                let store = Store::open(&directory).unwrap();
                std::thread::spawn(move || {
                    for item in 0..25 {
                        store
                            .append_event(Event {
                                subject_id: None,
                                lifecycle_event_id: None,
                                seq: 0,
                                kind: "concurrent".into(),
                                agent_id: Some(format!("{worker}-{item}")),
                                spawn: None,
                                request_id: None,
                                at: "now".into(),
                            })
                            .unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let (epoch, cursor, earliest) = store.event_cursor().unwrap();
        assert_eq!(cursor, 200);
        assert_eq!(earliest, 1);
        assert_eq!(store.event_cursor().unwrap().0, epoch);
        let journal = store.load_journal().unwrap();
        assert_eq!(
            journal
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            (1..=200).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_timestamp_is_utc_with_milliseconds_and_sorts_as_text() {
        let value = format_millis(1_767_225_600_123);
        assert_eq!(value, "2026-01-01T00:00:00.123Z");
        assert!(format_millis(1_000) < format_millis(2_000));
        assert_eq!(timestamp_millis(&value), Some(1_767_225_600_123));
        // A negative instant is before the epoch rather than an error.
        assert_eq!(format_millis(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn a_record_written_is_a_record_read() {
        let directory = tempdir("written");
        let store = Store::open(&directory).expect("a store");
        let mut wanted = record("close", Some("pane:3"), 1_000);
        wanted = wanted.complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_UNAVAILABLE),
            Some("no backend"),
            vec!["nothing_run".into()],
            1_100,
        );
        store.create(&wanted).expect("a create");
        store.write(&wanted).expect("a write");
        assert_eq!(store.get(&wanted.id).expect("a read"), Some(wanted.clone()));
        assert_eq!(store.recent(10).expect("a list"), vec![wanted]);
        assert_eq!(store.get(&random_uuid()).expect("a read"), None);
        assert!(
            store
                .write(&RequestRecord::new(
                    &random_uuid(),
                    "focus",
                    None,
                    None,
                    1_000
                ))
                .is_err()
        );
        drop_store(directory);
    }

    #[test]
    fn a_fresh_store_reads_what_another_wrote() {
        // The point of a record is that it outlives the process that wrote it.
        let directory = tempdir("reopened");
        let id = random_uuid();
        {
            let store = Store::open(&directory).expect("a store");
            let record = RequestRecord::new(&id, "close", None, Some("pane:1".into()), 5_000)
                .complete(
                    RequestOutcome::Completed,
                    None,
                    Some("closed"),
                    vec!["pane_closed".into()],
                    5_200,
                );
            store.create(&record).expect("a create");
            store.write(&record).expect("a write");
        }
        let reopened = Store::open(&directory).expect("a store");
        let record = reopened.get(&id).expect("a read").expect("the record");
        assert_eq!(record.outcome, Some(RequestOutcome::Completed));
        assert_eq!(record.effects, vec!["pane_closed".to_string()]);
        drop_store(directory);
    }

    #[test]
    fn an_expiry_is_decided_by_the_method() {
        let operation = record("close", None, 0);
        let resume = record("resume", None, 0);
        assert_eq!(timestamp_millis(&operation.expires_at), Some(30_000));
        assert_eq!(timestamp_millis(&resume.expires_at), Some(120_000));
    }

    fn tempdir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "radar-control-store-{name}-{}-{}",
            std::process::id(),
            random_uuid()
        ));
        fs::create_dir_all(&path).expect("a temporary directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("a mode");
        path
    }

    fn drop_store(directory: PathBuf) {
        let _ = fs::remove_dir_all(directory);
    }

    /// A request id is single-use. Even a completed request's id cannot be
    /// reused, because replacing its file would erase the only account of what
    /// the earlier request did.
    #[test]
    fn a_request_id_cannot_replace_an_existing_record() {
        let directory = tempdir("same-id");
        let store = Store::open(&directory).expect("a store");
        let first = record("close", Some("pane:1"), 1_000).complete(
            RequestOutcome::Refused,
            Some(Category::BACKEND_UNAVAILABLE),
            Some("no backend"),
            Vec::new(),
            1_100,
        );
        store.create(&first).expect("create the first record");
        let second = RequestRecord::new(
            ID,
            "focus",
            Some("other".into()),
            Some("pane:2".into()),
            2_000,
        );
        assert!(
            store
                .create(&second)
                .expect_err("same id refused")
                .contains("already exists")
        );
        assert_eq!(store.get(ID).expect("read").expect("first remains"), first);
        drop_store(directory);
    }

    #[test]
    fn an_unresolved_request_suppresses_mutations_across_methods() {
        let directory = tempdir("unresolved");
        let store = Store::open(&directory).expect("a store");
        let pending = record("close", Some("pane:1"), 1_000);
        store.create(&pending).expect("a create");
        store.write(&pending).expect("a write");
        assert!(
            store
                .unresolved(Some("pane:1"), 1_100)
                .expect("a search")
                .is_some()
        );
        assert!(
            store
                .unresolved(Some("pane:2"), 1_100)
                .expect("a search")
                .is_none()
        );
        // Mutations serialize across methods: input cannot race a close of the
        // same target just because it has a different method name.
        assert!(
            store
                .unresolved(Some("pane:1"), 1_100)
                .expect("a search")
                .is_some()
        );
        assert!(store.unresolved(None, 1_100).expect("a search").is_none());
        assert!(
            store
                .unresolved(Some("pane:1"), 40_000)
                .expect("a search")
                .is_none()
        );
        store.write(&pending.claim()).expect("a write");
        assert!(
            store
                .unresolved(Some("pane:1"), 40_000)
                .expect("a search")
                .is_some()
        );
        drop_store(directory);
    }

    #[test]
    fn a_record_derives_its_outcome_in_the_order_the_later_slices_need() {
        // Recorded at 1s, this request expires at 31s.
        let pending = record("close", Some("pane:1"), 1_000);
        assert_eq!(pending.derive(1_100), Derived::Pending);
        assert_eq!(pending.derive(30_999), Derived::Pending);
        assert_eq!(pending.derive(31_000), Derived::NotExecuted);
        // A claim means execution may have begun: it is unknown, not expired.
        assert_eq!(pending.claim().derive(31_000), Derived::Unknown);
        assert_eq!(pending.start().derive(1_100), Derived::Unknown);
        let refused = pending.complete(
            RequestOutcome::Refused,
            Some(Category::IN_FLIGHT),
            Some("already in flight"),
            Vec::new(),
            1_200,
        );
        assert_eq!(
            refused.derive(1_300),
            Derived::Settled(RequestOutcome::Refused)
        );
        // A recorded outcome wins over the clock, however long ago it was written.
        assert_eq!(
            refused.derive(999_999_999),
            Derived::Settled(RequestOutcome::Refused)
        );
        // A completed record with no outcome is unreadable, not a licence to run.
        let broken = RequestRecord {
            state: RecordState::Completed,
            ..pending.clone()
        };
        assert_eq!(broken.derive(1_300), Derived::Unknown);
        // So is an expiry this build cannot parse.
        let unreadable = RequestRecord {
            expires_at: "next tuesday".into(),
            ..pending.clone()
        };
        assert_eq!(unreadable.derive(1_300), Derived::Unknown);
    }

    #[test]
    fn unfinished_lists_an_old_pending_record_behind_newer_completed_ones() {
        let directory = tempdir("unfinished");
        let store = Store::open(&directory).expect("store");
        let old = RequestRecord::new(
            "00000000-0000-4000-8000-000000000000",
            "lifecycle.stop",
            Some("radar".into()),
            Some("subject".into()),
            1_000,
        );
        store.create(&old).expect("pending record");
        for n in 0..201 {
            let newer = RequestRecord::new(
                &format!("00000000-0000-4000-8000-{:012}", n + 1),
                "lifecycle.stop",
                Some("radar".into()),
                Some("subject".into()),
                2_000 + n,
            );
            store.create(&newer).expect("pending record");
            let done = newer.complete(
                RequestOutcome::Completed,
                None,
                Some("done"),
                Vec::new(),
                3_000,
            );
            store.write(&done).expect("completed record");
        }
        // The recency window is full of completed records, so only the unfinished
        // query can still reach the old pending one.
        let recent: Vec<String> = store
            .recent(200)
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect();
        assert!(!recent.contains(&old.id));
        let unfinished: Vec<String> = store
            .unfinished()
            .unwrap()
            .into_iter()
            .map(|record| record.id)
            .collect();
        assert_eq!(unfinished, vec![old.id.clone()]);
    }

    #[test]
    fn an_outcome_is_written_once() {
        let pending = record("close", None, 1_000);
        let refused =
            pending.complete(RequestOutcome::Refused, None, Some("no"), Vec::new(), 1_100);
        let again = refused.complete(
            RequestOutcome::Completed,
            None,
            Some("yes"),
            vec!["pane_closed".into()],
            1_200,
        );
        assert_eq!(again, refused);
        assert_eq!(
            again.completed_at.as_deref(),
            Some("1970-01-01T00:00:01.100Z")
        );
    }
}
