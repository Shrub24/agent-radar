//! Radar's own client for the control plane, and the choice between it and the
//! direct adapter.
//!
//! Everything above the runtime seam keeps seeing normalized types; the line
//! protocol appears in this module and nowhere else. One request is one line and
//! one answer is one line, and the daemon's own record decides what an operation
//! established — `completed`, `refused` or `unknown` — never the mere fact that
//! a socket answered.
//!
//! Two things are deliberate:
//!
//! * **One choice per run.** [`select`] picks the daemon or the direct adapter
//!   before anything is collected or acted on, and nothing switches afterwards.
//!   An operation the daemon may have accepted stays unknown: it is never retried
//!   through the direct adapter, because that would be a second execution of
//!   something that may already have happened.
//! * **The daemon revision must match the client's.** A backend that cannot create
//!   or report is not answered by a different operation here; the daemon's own
//!   refusal is passed on as a refusal.
//! * **A socket is vouched for before it is dialled.** [`DaemonRuntime::handshake`]
//!   applies the daemon's own trust rule to the path, read-only, before any
//!   connection: a socket someone else planted — the fallback directory under
//!   `/tmp` is created by whoever gets there first — is refused rather than
//!   believed, so it never becomes Radar's observation and control plane.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::containers::PublicContainer;
use super::ops;
use super::{MAX_LINE_BYTES, PROTOCOL_VERSION, random_uuid, socket_path, trusted_socket};
use crate::bus::{BusEvent, Task, TaskState};
use crate::lifecycle::{self, CloseRequest};
use crate::model::{FleetObservation, ForegroundEvidence};
use crate::runtime::{
    BackendIdentity, CloseOutcome, CloseTarget, CreateOutcome, CreateRequest, CreatedIdentity,
    CreatedKind, CreatedLocation, FocusOutcome, InputOutcome, InputRequest, OutputOutcome,
    OutputRead, OutputRequest, ReportOutcome, ReportRequest, RuntimeProvider, Target,
};

/// How long one exchange with the daemon may take before it is treated as not
/// answering.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one socket wait lasts before cancellation and the deadline are
/// checked again.
const WAIT_GRANULARITY: Duration = Duration::from_millis(10);

/// Agents asked for per `agent.list` page: the daemon's own maximum.
const TASKS_PAGE_LIMIT: usize = 100;

/// How long reading every page of one registry or task read may take.
const REGISTRY_READ_DEADLINE: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
pub struct TasksPage {
    pub revision: u64,
    pub sessions: Vec<TaskSession>,
    pub next_after: Option<String>,
}

#[derive(Deserialize)]
pub struct TaskSession {
    session: String,
    connected: bool,
    #[allow(dead_code, reason = "the wire carries it; no reader presents it yet")]
    last_updated_at: u64,
    tasks: Vec<TaskValue>,
}

#[derive(Deserialize)]
struct TaskValue {
    id: String,
    state: String,
    command: Option<String>,
    cwd: Option<String>,
    pid: Option<u32>,
    started_at: Option<u64>,
    last_output_at: Option<u64>,
    output_bytes: Option<u64>,
    exit_code: Option<i32>,
}

#[derive(Clone, Debug)]
pub struct TaskSnapshot {
    pub revision: u64,
    pub events: Vec<BusEvent>,
}

/// The label this caller's records carry, so an operator reading the store knows
/// who asked.
const REQUESTER: &str = "radar";

/// What one observation read returned.
#[derive(Clone, Debug)]
pub enum ObservationRead {
    /// The daemon's current revision.
    Snapshot(Box<crate::control_plane::observation::Snapshot>),
    /// The caller already holds the current revision.
    NotModified { epoch: String, revision: u64 },
    /// The caller named a revision or epoch this daemon does not hold; it must
    /// restart from the current revision rather than merge the two.
    Expired { epoch: String, revision: u64 },
    /// The snapshot is larger than one response may carry: the caller asked for
    /// more classes than it can receive in one read.
    TooLarge,
}

fn decode_observation_result(result: Value) -> Result<ObservationRead, serde_json::Error> {
    use crate::control_plane::public_contracts::observation::ObservationGetResponse;

    Ok(
        match serde_json::from_value::<ObservationGetResponse>(result)? {
            ObservationGetResponse::Snapshot(response) => {
                ObservationRead::Snapshot(Box::new(response.snapshot.into()))
            }
            ObservationGetResponse::NotModified(response) => ObservationRead::NotModified {
                epoch: response.not_modified.epoch,
                revision: response.not_modified.revision,
            },
            ObservationGetResponse::Expired(response) => ObservationRead::Expired {
                epoch: response.snapshot_expired.epoch,
                revision: response.snapshot_expired.revision,
            },
            ObservationGetResponse::TooLarge(_) => ObservationRead::TooLarge,
        },
    )
}

#[cfg(test)]
mod observation_result_tests {
    use super::*;

    #[test]
    fn not_modified_reads_the_nested_revision_from_the_public_response() {
        let result = decode_observation_result(json!({
            "not_modified": {"epoch": "epoch-1", "revision": 12}
        }))
        .expect("decode response");
        assert!(matches!(
            result,
            ObservationRead::NotModified { epoch, revision: 12 } if epoch == "epoch-1"
        ));
    }
}

/// The daemon as a [`RuntimeProvider`].
pub struct DaemonRuntime {
    socket: PathBuf,
    /// The contracts the daemon advertised, by name. A name this build does not
    /// know is kept and never used.
    declared: Vec<String>,
    identity: BackendIdentity,
    task_diagnostic: Option<String>,
}

impl DaemonRuntime {
    /// Whether the daemon advertised `contract`.
    fn serves(&self, contract: ops::Contract) -> bool {
        self.declared.iter().any(|name| name == contract.as_str())
    }

    /// The primitives the dashboard implements: observation, foreground evidence,
    /// focus and close.
    ///
    /// Anything beyond these is asked for and answered by the daemon: a backend
    /// that cannot create or report refuses that operation by name rather than
    /// making the whole daemon unusable.
    pub const REQUIRED: ops::Primitives = ops::Primitives::OBSERVE
        .union(ops::Primitives::PROCESS_INFO)
        .union(ops::Primitives::FOCUS)
        .union(ops::Primitives::CLOSE);

    /// Pings the daemon at `socket` and proves it is one Radar can use: the path
    /// is one Radar may trust, it answers, it speaks this protocol version, and it
    /// declares what the dashboard needs.
    ///
    /// The trust check is first and reads only: nothing is created, changed or
    /// even connected to before it passes, so a socket planted at the path by
    /// another process is never dialled. An operator-supplied socket is held to
    /// the same rule as the documented one, so a layout the daemon could not have
    /// bound surfaces as this diagnostic rather than as an unverified connection.
    ///
    /// It is checked once, here: the rule proves the directory is this user's own
    /// `0700`, which is exactly what stops anyone else from replacing the socket
    /// afterwards. A same-uid process could, and that is not a boundary this seam
    /// can hold.
    ///
    /// Every failure names what was wrong and where, so a caller can print it as
    /// the one line explaining why the direct adapter is being used.
    pub fn handshake(socket: &Path) -> Result<Self, String> {
        trusted_socket(socket)?;
        let cancel = AtomicBool::new(false);
        let answer = exchange_at(socket, "daemon.ping", json!({}), &cancel)
            .map_err(|failure| failure.message)?;
        let served = answer.get("protocol").and_then(Value::as_u64);
        if served != Some(PROTOCOL_VERSION as u64) {
            let served =
                served.map_or_else(|| "no version".to_string(), |v| format!("version {v}"));
            return Err(format!(
                "the control daemon at {} answers {served}; this build speaks protocol {}",
                socket.display(),
                PROTOCOL_VERSION
            ));
        }
        // The daemon and this build ship together; only a daemon that outlived an
        // install is older, and it is told apart by one number, not a name list.
        let revision = answer.get("revision").and_then(Value::as_u64);
        if revision.is_none_or(|revision| revision < u64::from(ops::PROTOCOL_REVISION)) {
            let revision =
                revision.map_or_else(|| "no revision".to_string(), |r| format!("revision {r}"));
            return Err(format!(
                "the control daemon at {} is older than this build ({revision}, needs {}); \
                 restart it with `systemctl --user restart radar-daemon`",
                socket.display(),
                ops::PROTOCOL_REVISION
            ));
        }
        let declared: Vec<String> = answer
            .get("contracts")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let task_diagnostic = answer
            .get("tasks")
            .filter(|tasks| tasks.get("available").and_then(Value::as_bool) == Some(false))
            .and_then(|tasks| tasks.get("error"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        Ok(Self {
            socket: socket.to_path_buf(),
            declared,
            task_diagnostic,
            identity: BackendIdentity {
                name: answer
                    .get("backend")
                    .and_then(Value::as_str)
                    .unwrap_or("none")
                    .to_string(),
                instance: answer
                    .get("instance")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            },
        })
    }

    /// Volatile publisher task state, fetched as one revision-consistent snapshot.
    pub fn task_snapshot(&self) -> Result<TaskSnapshot, String> {
        if !self.serves(ops::Contract::TasksList) {
            return Err(self.task_diagnostic.clone().unwrap_or_else(|| {
                "the control daemon does not provide `tasks.list.v1`".to_string()
            }));
        }
        let deadline = Instant::now() + REGISTRY_READ_DEADLINE;
        let cancel = AtomicBool::new(false);
        let mut after: Option<String> = None;
        let mut revision = None;
        let mut events = Vec::new();
        loop {
            if Instant::now() >= deadline {
                return Err(self.describe(format!(
                    "did not finish reading tasks within {REGISTRY_READ_DEADLINE:?}"
                )));
            }
            let mut params = json!({"limit": TASKS_PAGE_LIMIT});
            if let Some(cursor) = &after {
                params["after"] = json!(cursor);
            }
            let result = self
                .call("tasks.list", params, &cancel)
                .map_err(|failure| failure.message)?;
            let page: TasksPage = serde_json::from_value(result).map_err(|error| {
                self.describe(format!("sent an unreadable tasks page: {error}"))
            })?;
            if revision.is_some_and(|current| current != page.revision) {
                return Err(self.describe("task snapshot changed while paging".to_string()));
            }
            revision = Some(page.revision);
            events.extend(bus_events(page.sessions));
            match page.next_after {
                None => {
                    return Ok(TaskSnapshot {
                        revision: revision.unwrap_or_default(),
                        events,
                    });
                }
                Some(next) if after.as_deref() == Some(next.as_str()) => {
                    return Err(self.describe("task page cursor did not advance".to_string()));
                }
                Some(next) => after = Some(next),
            }
        }
    }

    pub fn task_diagnostic(&self) -> Option<&str> {
        self.task_diagnostic.as_deref()
    }

    /// One exchange, with every failure naming the daemon it happened to.
    /// One exchange, with every failure naming the daemon it happened to.
    fn call(&self, method: &str, params: Value, cancel: &AtomicBool) -> Result<Value, Failure> {
        exchange_at(&self.socket, method, params, cancel)
    }

    /// A prefix for a message about this daemon.
    fn describe(&self, message: String) -> String {
        format!("the control daemon at {} {message}", self.socket.display())
    }

    /// One effectful method, as the daemon's own record establishes it.
    fn mutation(
        &self,
        method: &str,
        request: &impl Serialize,
        cancel: &AtomicBool,
    ) -> Result<Settled, Failure> {
        let params = json!({"request": request, "requester": REQUESTER});
        self.call(method, params, cancel)
            .and_then(|result| Settled::of(&result))
    }

    /// Reads the daemon's shared observation snapshot.
    ///
    /// `since_revision` is the epoch-qualified revision the caller already
    /// holds: naming it lets the daemon answer `not_modified`, and `wait_ms`
    /// holds the read open for a newer one. The classes asked for are demanded
    /// of the sampler for as long as this reader keeps asking.
    pub fn observation_get(
        &self,
        epoch: Option<&str>,
        since_revision: Option<u64>,
        wait_ms: u64,
        classes: &[&str],
        cancel: &AtomicBool,
    ) -> Result<ObservationRead, String> {
        if !self.serves(ops::Contract::ObservationGet) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::ObservationGet.as_str()
            )));
        }
        let mut params = json!({"classes": classes});
        if let Some(epoch) = epoch {
            params["epoch"] = json!(epoch);
        }
        if let Some(revision) = since_revision {
            params["since_revision"] = json!(revision);
        }
        if wait_ms > 0 {
            params["wait_ms"] = json!(wait_ms);
        }
        let result = self
            .call("observation.get", params, cancel)
            .map_err(|failure| failure.message)?;
        decode_observation_result(result)
            .map_err(|error| self.describe(format!("sent an unreadable observation: {error}")))
    }
}

impl RuntimeProvider for DaemonRuntime {
    /// The backend the daemon reports: this client asks nothing of the mux
    /// itself, so its identity is the one behind the socket.
    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }

    fn primitives(&self) -> ops::Primitives {
        // What this client implements is fixed; the daemon's own backend
        // primitives are private and are never returned on the wire.
        Self::REQUIRED
    }

    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        let result = self
            .call("mux.observe", json!({}), cancel)
            .map_err(|failure| failure.message)?;
        let inventory = result.get("inventory").cloned().unwrap_or(Value::Null);
        serde_json::from_value(inventory)
            .map_err(|error| self.describe(format!("sent an unreadable inventory: {error}")))
    }

    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence {
        // Evidence that cannot be read is inconclusive, exactly as it is for the
        // direct adapter: nothing here invents a pane state.
        let Ok(result) = self.call("mux.process_info", json!({"pane_id": pane_id}), cancel) else {
            return ForegroundEvidence::Inconclusive;
        };
        let evidence = result.get("evidence").cloned().unwrap_or(Value::Null);
        serde_json::from_value(evidence).unwrap_or(ForegroundEvidence::Inconclusive)
    }

    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        match self.focus_outcome(target, cancel) {
            FocusOutcome::Completed => Ok(()),
            FocusOutcome::Refused(message) | FocusOutcome::Unknown(message) => Err(message),
        }
    }

    fn focus_outcome(&self, target: &Target, cancel: &AtomicBool) -> FocusOutcome {
        let (id, kind) = match target {
            Target::Pane(id) => (id, "pane"),
            Target::Workspace(id) => (id, "workspace"),
        };
        let params = json!({"target": id, "target_kind": kind, "requester": REQUESTER});
        match self
            .call("mux.focus", params, cancel)
            .and_then(|result| Settled::of(&result))
        {
            Ok(settled) => settled.focus(),
            Err(failure) => failure.settled().focus(),
        }
    }

    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        match self.close_outcome(target, cancel) {
            CloseOutcome::Completed => Ok(()),
            CloseOutcome::Refused(message) | CloseOutcome::Unknown(message) => Err(message),
        }
    }

    fn close_outcome(&self, target: &CloseTarget, cancel: &AtomicBool) -> CloseOutcome {
        // The daemon closes only against a frozen identity, and this is where the
        // request gets one: this client's own fresh observation. The daemon then
        // re-observes and applies its containment checks, so a target replaced in
        // between is refused there rather than acted on.
        let inventory = match self.inventory(cancel) {
            Ok(inventory) => inventory,
            Err(message) => return CloseOutcome::Refused(message),
        };
        let request = CloseRequest {
            target: target.clone(),
            identity: lifecycle::identity(&inventory, target),
        };
        match self.mutation("mux.close", &request, cancel) {
            Ok(settled) => settled.close(),
            Err(failure) => failure.settled().close(),
        }
    }

    fn create(&self, request: &CreateRequest, cancel: &AtomicBool) -> CreateOutcome {
        match self.mutation("mux.create", request, cancel) {
            Ok(settled) => settled.create(),
            Err(failure) => failure.settled().create(),
        }
    }

    fn input(&self, request: &InputRequest, cancel: &AtomicBool) -> InputOutcome {
        match self.mutation("mux.input", request, cancel) {
            Ok(settled) => settled.input(),
            Err(failure) => failure.settled().input(),
        }
    }

    fn output(&self, request: &OutputRequest, cancel: &AtomicBool) -> OutputOutcome {
        let result = match self.call("mux.output", json!(request), cancel) {
            Ok(result) => result,
            Err(failure) => return failure.into_output(),
        };
        let read = result.get("output").cloned().unwrap_or(Value::Null);
        match serde_json::from_value::<OutputRead>(read) {
            Ok(read) => OutputOutcome::Completed(read),
            Err(error) => OutputOutcome::Unknown(
                self.describe(format!("sent an unreadable output read: {error}")),
            ),
        }
    }

    fn report(&self, request: &ReportRequest, cancel: &AtomicBool) -> ReportOutcome {
        match self.mutation("mux.report", request, cancel) {
            Ok(settled) => settled.report(),
            Err(failure) => failure.settled().report(),
        }
    }
}

/// The daemon's runtime for this run: the reads and the container controls travel
/// through the same connection.
pub struct Selected {
    pub provider: Arc<dyn RuntimeProvider>,
    pub containers: Option<Arc<dyn ContainerControls>>,
    pub lifecycle: Option<Arc<dyn LifecycleControls>>,
    /// The daemon client itself, when this daemon serves the volatile task lane.
    pub tasks: Option<Arc<DaemonRuntime>>,
    /// The daemon client for the shared observation read. Present whenever the
    /// daemon serves `observation.get.v1`, which is what a display renders from.
    pub observation: Option<Arc<DaemonRuntime>>,
    pub task_diagnostic: Option<String>,
}

/// Connects to the daemon at its documented socket and proves it usable.
///
/// The failure names what was wrong. Nothing falls back to another source: an
/// unreachable daemon is the caller's to show, and the caller retries.
pub fn connect() -> Result<Selected, String> {
    connect_at(&socket_path())
}

/// One explicit reconciliation pass over the daemon's unresolved operations.
///
/// It dispatches nothing: it re-reads current evidence, settles only what that
/// evidence establishes, and reports the rest with a reason. It is not the
/// steady-state observation pass, which runs continuously whether or not anyone
/// asks for one.
pub fn reconcile(cancel: &AtomicBool) -> Result<Value, String> {
    let socket = socket_path();
    DaemonRuntime::handshake(&socket)?;
    let answer = exchange_at(&socket, "lifecycle.reconcile", json!({}), cancel)
        .map_err(|failure| failure.message)?;
    Ok(answer.get("reconciled").cloned().unwrap_or(Value::Null))
}

/// The ids of the daemon's records that are not yet completed, however old.
///
/// The caller must check that the socket unit is listening first: connecting to a
/// listening socket starts the daemon, which a stop must not do.
pub fn unfinished_requests() -> Result<Vec<String>, String> {
    let socket = socket_path();
    DaemonRuntime::handshake(&socket)?;
    let answer = exchange_at(
        &socket,
        "mux.request.list",
        json!({ "unfinished": true }),
        &AtomicBool::new(false),
    )
    .map_err(|failure| failure.message)?;
    Ok(answer
        .get("requests")
        .and_then(Value::as_array)
        .map(|records| {
            records
                .iter()
                .filter_map(|record| record.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default())
}

/// [`connect`] at a socket the caller names, held to the same trust rule.
pub fn connect_at(socket: &Path) -> Result<Selected, String> {
    let daemon = Arc::new(DaemonRuntime::handshake(socket)?);
    // The controls travel with the same client the reads come from: one handshake,
    // one socket, and no second transport for an effect.
    Ok(Selected {
        provider: Arc::clone(&daemon) as Arc<dyn RuntimeProvider>,
        containers: Some(Arc::clone(&daemon) as Arc<dyn ContainerControls>),
        lifecycle: Some(Arc::clone(&daemon) as Arc<dyn LifecycleControls>),
        tasks: daemon
            .serves(ops::Contract::TasksList)
            .then(|| Arc::clone(&daemon)),
        observation: daemon
            .serves(ops::Contract::ObservationGet)
            .then(|| Arc::clone(&daemon)),
        task_diagnostic: daemon.task_diagnostic.clone(),
    })
}

/// The dashboard's events for one page of volatile task sessions.
///
/// A session that is not connected is not presented: the ingest keeps a
/// disconnected session's last tasks for its own bookkeeping, and a display must
/// not show work it can no longer see.
fn bus_events(sessions: Vec<TaskSession>) -> Vec<BusEvent> {
    let mut events = Vec::new();
    for session in sessions {
        if !session.connected {
            continue;
        }
        let session_id = session.session;
        events.push(BusEvent::Connected {
            session: session_id.clone(),
            pane: None,
        });
        events.push(BusEvent::Tasks {
            session: session_id,
            tasks: session
                .tasks
                .into_iter()
                .map(|task| Task {
                    id: task.id,
                    state: match task.state.as_str() {
                        "running" => TaskState::Running,
                        "flushing" => TaskState::Flushing,
                        "review" => TaskState::Review,
                        state => TaskState::Unknown(state.to_string()),
                    },
                    command: task.command,
                    cwd: task.cwd,
                    pid: task.pid,
                    started_at: task.started_at,
                    last_output_at: task.last_output_at,
                    output_bytes: task.output_bytes,
                    exit_code: task.exit_code,
                })
                .collect(),
        });
    }
    events
}

/// The volatile task state a snapshot carried, as the dashboard's own events.
///
/// The snapshot's task source is the same `tasks.list` page the standalone read
/// returns, so both paths present one shape.
pub fn tasks_from_snapshot(tasks: &Value) -> Result<Vec<BusEvent>, String> {
    let page: TasksPage = serde_json::from_value(tasks.clone())
        .map_err(|error| format!("the snapshot's task source is unreadable: {error}"))?;
    Ok(bus_events(page.sessions))
}

/// How long one exchange has left, and where it may be interrupted.
#[derive(Clone, Copy)]
struct Budget {
    deadline: Instant,
    timeout: Duration,
}

impl Budget {
    fn of(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            timeout,
        }
    }

    /// Waits one granularity, or reports that the request was cancelled or is out
    /// of time.
    ///
    /// The message continues "the control daemon at <socket>", because that is
    /// where every caller's failure is reported from.
    fn wait(&self, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::SeqCst) {
            return Err("abandoned the request: it was cancelled".to_string());
        }
        if Instant::now() >= self.deadline {
            return Err(format!("did not complete within {:?}", self.timeout));
        }
        thread::sleep(WAIT_GRANULARITY);
        Ok(())
    }
}

/// One request that did not succeed, and what its failure establishes.
struct Failure {
    message: String,
    certainty: Certainty,
}

/// What a failed exchange proves about the operation.
#[derive(Clone, Copy)]
enum Certainty {
    /// Nothing was dispatched: the request never reached the daemon intact.
    Refused,
    /// The request may have been dispatched, so its effect is unestablished.
    Unknown,
}

impl Failure {
    fn refused(message: String) -> Self {
        Self {
            message,
            certainty: Certainty::Refused,
        }
    }

    fn unknown(message: String) -> Self {
        Self {
            message,
            certainty: Certainty::Unknown,
        }
    }

    /// A write that was refused outright, before one byte was accepted,
    /// dispatched nothing; a partial write may have.
    fn after_write(written: usize, message: String) -> Self {
        match written {
            0 => Self::refused(message),
            _ => Self::unknown(message),
        }
    }

    /// The same failure as an answer that establishes nothing.
    fn settled(self) -> Settled {
        match self.certainty {
            Certainty::Refused => Settled::Refused(self.message),
            Certainty::Unknown => Settled::Unknown(self.message),
        }
    }

    fn into_output(self) -> OutputOutcome {
        match self.certainty {
            Certainty::Refused => OutputOutcome::Refused(self.message),
            Certainty::Unknown => OutputOutcome::Unknown(self.message),
        }
    }
}

/// What one effectful operation's record establishes.
enum Settled {
    /// The operation ran. A creation's answer names what it created.
    Completed(Option<CreatedIdentity>),
    Refused(String),
    Unknown(String),
}

impl Settled {
    /// Reads the record the daemon answered with.
    ///
    /// A record that is not settled is unknown rather than refused: the daemon
    /// has accepted the request and may still execute it, so nothing here may be
    /// read as "it did not happen".
    fn of(result: &Value) -> Result<Self, Failure> {
        let Some(record) = result.get("request") else {
            return Err(Failure::unknown(
                "the daemon's answer carried no request record".to_string(),
            ));
        };
        let created = record
            .get("created")
            .and_then(|value| serde_json::from_value::<CreatedIdentity>(value.clone()).ok());
        let message = record
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string);
        let state = record.get("state").and_then(Value::as_str);
        let outcome = record.get("outcome").and_then(Value::as_str);
        match (state, outcome) {
            (Some("completed"), Some("completed")) => Ok(Self::Completed(created)),
            (Some("completed"), Some("refused")) => {
                Ok(Self::Refused(message.unwrap_or_else(|| {
                    "the daemon refused the operation".to_string()
                })))
            }
            (Some("completed"), Some("unknown")) => {
                Ok(Self::Unknown(message.unwrap_or_else(|| {
                    "the daemon cannot say what the operation did".to_string()
                })))
            }
            _ => {
                let id = record.get("id").and_then(Value::as_str).unwrap_or("it");
                Err(Failure::unknown(format!(
                    "{id} is still unresolved: the daemon may yet execute it"
                )))
            }
        }
    }

    fn focus(self) -> FocusOutcome {
        match self {
            Self::Completed(_) => FocusOutcome::Completed,
            Self::Refused(message) => FocusOutcome::Refused(message),
            Self::Unknown(message) => FocusOutcome::Unknown(message),
        }
    }

    fn close(self) -> CloseOutcome {
        match self {
            Self::Completed(_) => CloseOutcome::Completed,
            Self::Refused(message) => CloseOutcome::Refused(message),
            Self::Unknown(message) => CloseOutcome::Unknown(message),
        }
    }

    fn create(self) -> CreateOutcome {
        match self {
            Self::Completed(created) => {
                CreateOutcome::Completed(created.map(location_of).unwrap_or_default())
            }
            Self::Refused(message) => CreateOutcome::Refused(message),
            Self::Unknown(message) => CreateOutcome::Unknown(message),
        }
    }

    fn input(self) -> InputOutcome {
        match self {
            Self::Completed(_) => InputOutcome::Completed,
            Self::Refused(message) => InputOutcome::Refused(message),
            Self::Unknown(message) => InputOutcome::Unknown(message),
        }
    }

    fn report(self) -> ReportOutcome {
        match self {
            Self::Completed(_) => ReportOutcome::Completed,
            Self::Refused(message) => ReportOutcome::Refused(message),
            Self::Unknown(message) => ReportOutcome::Unknown(message),
        }
    }
}

/// The location one record's reported identity describes.
fn location_of(identity: CreatedIdentity) -> CreatedLocation {
    let mut location = CreatedLocation::default();
    match identity.kind {
        CreatedKind::Workspace => location.workspace_id = Some(identity.id),
        CreatedKind::Tab => location.tab_id = Some(identity.id),
        CreatedKind::Pane => location.pane_id = Some(identity.id),
    }
    location
}

/// One request, one answer, over the daemon's socket.
///
/// Certainty follows from how far the exchange got: a request that never reached
/// the daemon — no connection, or a write refused before one byte — dispatched
/// nothing and may be refused; anything after that is unknown, because the
/// daemon may have run the operation and lost only the answer.
///
/// Each exchange dials its own connection, which is what the daemon serves: one
/// connection carries lines until its reader fails, and a client that wants a
/// fresh request gets a fresh dial.
/// One request, under a request id the caller already has.
///
/// A confirmed container control carries the operator’s own id, so a lost answer
/// is read back under the id that was sent rather than sent again.
fn exchange_with_id(
    socket: &Path,
    id: &str,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    // The terminator is what makes it a line: the daemon answers one line per
    // request and reads until it has one.
    let mut line = json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
    .into_bytes();
    line.push(b'\n');
    let mut stream = UnixStream::connect(socket)
        .map_err(|error| Failure::refused(format!("could not be reached: {error}")))?;
    let granularity = Some(WAIT_GRANULARITY);
    let _ = stream.set_read_timeout(granularity);
    let _ = stream.set_write_timeout(granularity);
    let budget = Budget::of(EXCHANGE_TIMEOUT);
    write_request(&mut stream, &line, budget, cancel)?;
    let answer = read_answer(&mut stream, budget, cancel).map_err(Failure::unknown)?;
    let answer: Value = serde_json::from_str(&answer)
        .map_err(|error| Failure::unknown(format!("sent an unreadable answer: {error}")))?;
    if answer.get("id").and_then(Value::as_str) != Some(id) {
        return Err(Failure::unknown("answered a different request".to_string()));
    }
    if let Some(error) = answer.get("error") {
        let code = error
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("no reason given");
        let message = format!("refused `{method}` ({code}): {message}");
        return Err(match code {
            // The codes the daemon answers before anything is dispatched.
            "bad_version"
            | "bad_request"
            | "unknown_method"
            | "bad_params"
            | "refused"
            | "busy"
            | "backend_unavailable"
            | "not_found" => Failure::refused(message),
            // A dispatched request whose result was not confirmed is never a
            // refusal: `outcome_unknown` says the daemon may have acted and
            // `internal` says it failed while serving.
            "outcome_unknown" | "internal" => Failure::unknown(message),
            // A code this build does not know cannot be read as "nothing
            // happened", so it stays unknown too.
            _ => Failure::unknown(message),
        });
    }
    match answer.get("result") {
        Some(result) => Ok(result.clone()),
        None => Err(Failure::unknown(format!(
            "answered `{method}` without a result"
        ))),
    }
}

/// [`exchange`], with every failure naming the daemon it happened to.
fn exchange_at(
    socket: &Path,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    exchange_with_id_at(socket, &random_uuid(), method, params, cancel)
}

/// The id-carrying form of the exchange above, with every failure naming the
/// daemon it happened to.
fn exchange_with_id_at(
    socket: &Path,
    id: &str,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    exchange_with_id(socket, id, method, params, cancel).map_err(|failure| Failure {
        message: format!(
            "the control daemon at {} {}",
            socket.display(),
            failure.message
        ),
        certainty: failure.certainty,
    })
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Writes the whole request line, waiting out a socket that is momentarily busy.
fn write_request(
    stream: &mut UnixStream,
    bytes: &[u8],
    budget: Budget,
    cancel: &AtomicBool,
) -> Result<(), Failure> {
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => {
                return Err(Failure::after_write(
                    written,
                    "closed the connection while the request was being written".to_string(),
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => {
                budget
                    .wait(cancel)
                    .map_err(|message| Failure::after_write(written, message))?;
            }
            Err(error) => {
                return Err(Failure::after_write(
                    written,
                    format!("could not be asked: {error}"),
                ));
            }
        }
    }
    Ok(())
}

/// Reads one answer line, waiting out a socket that is momentarily empty.
fn read_answer(
    stream: &mut UnixStream,
    budget: Budget,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let mut answer = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Err("ended the connection without answering".to_string()),
            Ok(count) => {
                answer.extend_from_slice(&chunk[..count]);
                if let Some(end) = answer.iter().position(|byte| *byte == b'\n') {
                    return Ok(String::from_utf8_lossy(&answer[..end]).trim().to_string());
                }
                if answer.len() > MAX_LINE_BYTES {
                    return Err(format!("answered more than {MAX_LINE_BYTES} bytes"));
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => budget.wait(cancel)?,
            Err(error) => return Err(format!("sent no answer: {error}")),
        }
    }
}

/// The container controls a daemon serves, for the TUI to request and read back.
///
/// Every call carries the caller's own request id, because the answer to a
/// confirmed control can be lost: the operator reads that id back rather than the
/// client dispatching a second effect. A control the daemon does not serve is
/// refused before it is sent, and nothing here falls back to another transport.
pub trait LifecycleControls: Send + Sync {
    fn supports_child_close(&self) -> bool;
    fn read_operation(
        &self,
        request_id: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<LifecycleAnswer>, String>;
    fn close_child(
        &self,
        request_id: &str,
        target: &crate::registry_observation::ChildCloseTarget,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    fn cancel_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    fn stop_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    fn restart_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String>;
    /// Enrolls a registered lead the daemon did not launch as a lifecycle subject.
    fn enroll_agent(
        &self,
        _request_id: &str,
        _agent_id: &str,
        _session: &str,
        _cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        Err("this run cannot enroll a lead".to_string())
    }

    /// Ends an unresolved record's recovery obligation, releasing only the
    /// administrative block it holds.
    ///
    /// The record is named rather than targeted: it is the one holding the target,
    /// so it cannot be reached through the target. Nothing is dispatched, and no
    /// outcome is established — the recorded uncertainty stands.
    fn abandon_record(
        &self,
        _request_id: &str,
        _held_request_id: &str,
        _cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        Err("this run cannot abandon a record".to_string())
    }
}

/// One subject-targeted request; pane ids are not lifecycle targets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleControlRequest {
    pub request_id: String,
    pub subject_id: String,
}

/// The operation record the daemon committed, without private launch data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifecycleAnswer {
    pub request: Value,
}

impl LifecycleAnswer {
    pub fn outcome(&self) -> &str {
        self.request
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    }

    pub fn message(&self) -> Option<&str> {
        self.request.get("message").and_then(Value::as_str)
    }
}

pub trait ContainerControls: Send + Sync {
    /// Splits a pane slot from a pane, running a command in it when one is given.
    fn create_container(
        &self,
        request: &ContainerCreate,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String>;
    /// Deletes one container Radar records, as the operator confirmed it.
    fn delete_container(
        &self,
        request: &ContainerDelete,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String>;
    /// Runs a pane container's recorded recipe again in its own slot.
    fn reload_container(
        &self,
        request: &ContainerReload,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String>;
}

/// What one container creation makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerCreateKind {
    /// A new workspace, which needs no existing place to start from, starting in
    /// `directory` when the operator chose one.
    Workspace { directory: Option<String> },
    /// A tab in this workspace.
    Tab { workspace_id: String },
    /// A pane slot split from this pane.
    PaneSplit { pane_id: String },
}

/// A container creation, with Radar’s own name for the slot and the command it
/// runs.
///
/// No `Debug`: a recipe is private launch data, and printing one would put a
/// command and its environment in a diagnostic.
#[derive(Clone, PartialEq, Eq)]
pub struct ContainerCreate {
    /// The caller’s own id: the durable key of this operation.
    pub request_id: String,
    /// What the creation makes, and where.
    pub kind: ContainerCreateKind,
    pub label: Option<String>,
    /// The command the slot runs, in the daemon's own recipe shape. It is held
    /// privately by the daemon and never read back.
    pub recipe: Option<super::containers::Recipe>,
}

/// A container deletion: the container, and the target the operator confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerDelete {
    pub request_id: String,
    pub container_id: String,
    /// The frozen target and occupant, as the daemon's close contract takes it:
    /// the daemon refuses the deletion when that place no longer holds it.
    pub frozen: Value,
}

/// A container reload: the pane container whose recorded recipe runs again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerReload {
    pub request_id: String,
    pub container_id: String,
}

/// What the daemon answered for one container control: its own record, the
/// container it made or acted on, and the containers a deletion removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerAnswer {
    pub request: Value,
    /// The container a creation made, or a reload acted on, unchanged.
    pub container: Option<PublicContainer>,
    /// The containers a deletion marked removed, in id order.
    pub removed: Vec<PublicContainer>,
}

impl std::fmt::Debug for ContainerCreate {
    /// Names what the control is, and never what it runs: a recipe is private
    /// launch data, so it is reported as held rather than printed.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ContainerCreate")
            .field("request_id", &self.request_id)
            .field("kind", &self.kind)
            .field("label", &self.label)
            .field("recipe", &self.recipe.as_ref().map(|_| "held"))
            .finish()
    }
}

impl ContainerAnswer {
    /// The outcome the record carries: `completed`, `refused` or `unknown`.
    pub fn outcome(&self) -> &str {
        self.request
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    }

    /// The record's own message, when it wrote one.
    pub fn message(&self) -> Option<&str> {
        self.request.get("message").and_then(Value::as_str)
    }
}

impl DaemonRuntime {
    fn lifecycle_control(
        &self,
        contract: ops::Contract,
        method: &str,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(contract) {
            return Err(self.describe(format!("does not provide `{}`", contract.as_str())));
        }
        let result = exchange_with_id_at(
            &self.socket,
            &request.request_id,
            method,
            json!({"request": {"subject_id": request.subject_id}, "requester": REQUESTER}),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
        })
    }

    /// One container control, sent under the caller's own request id.
    ///
    /// The contract it needs is checked before the request is sent, so a caller
    /// refuses work this daemon does not serve instead of discovering it mid-effect.
    fn container_control(
        &self,
        contract: ops::Contract,
        request_id: &str,
        method: &str,
        params: Value,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        if !self.serves(contract) {
            return Err(self.describe(format!("does not provide `{}`", contract.as_str())));
        }
        let result = exchange_with_id_at(&self.socket, request_id, method, params, cancel)
            .map_err(|failure| failure.message)?;
        let container = self.public_container(result.get("container"))?;
        let removed = self.public_container_list(result.get("removed"))?;
        Ok(ContainerAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
            container,
            removed,
        })
    }
}

impl DaemonRuntime {
    /// One container the daemon named, when it named one.
    fn public_container(&self, value: Option<&Value>) -> Result<Option<PublicContainer>, String> {
        value
            .filter(|value| !value.is_null())
            .map(|value| serde_json::from_value(value.clone()))
            .transpose()
            .map_err(|error| self.describe(format!("answered an unreadable container: {error}")))
    }

    /// The containers the daemon named, in the order it named them.
    fn public_container_list(&self, value: Option<&Value>) -> Result<Vec<PublicContainer>, String> {
        let Some(values) = value.and_then(Value::as_array) else {
            return Ok(Vec::new());
        };
        values
            .iter()
            .map(|value| serde_json::from_value(value.clone()))
            .collect::<Result<Vec<PublicContainer>, _>>()
            .map_err(|error| self.describe(format!("answered an unreadable container: {error}")))
    }
}

impl LifecycleControls for DaemonRuntime {
    fn supports_child_close(&self) -> bool {
        self.serves(ops::Contract::SpawnClose)
    }

    fn read_operation(
        &self,
        request_id: &str,
        cancel: &AtomicBool,
    ) -> Result<Option<LifecycleAnswer>, String> {
        let result = self
            .call("mux.request.get", json!({"id": request_id}), cancel)
            .map_err(|failure| failure.message)?;
        Ok(result
            .get("request")
            .cloned()
            .map(|request| LifecycleAnswer { request }))
    }

    fn close_child(
        &self,
        request_id: &str,
        target: &crate::registry_observation::ChildCloseTarget,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(ops::Contract::SpawnClose) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::SpawnClose.as_str()
            )));
        }
        let result = exchange_with_id_at(
            &self.socket,
            request_id,
            "spawn.close",
            json!({
                "spawn_request_id": target.spawn_request_id,
                "source": target.source,
                "incarnation": target.incarnation,
                "intent": "cancel",
            }),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("close").cloned().unwrap_or(Value::Null),
        })
    }

    fn cancel_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.lifecycle_control(
            ops::Contract::LifecycleCancel,
            "lifecycle.cancel",
            request,
            cancel,
        )
    }

    fn stop_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.lifecycle_control(
            ops::Contract::LifecycleStop,
            "lifecycle.stop",
            request,
            cancel,
        )
    }

    fn restart_subject(
        &self,
        request: &LifecycleControlRequest,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.lifecycle_control(
            ops::Contract::LifecycleRestart,
            "lifecycle.restart",
            request,
            cancel,
        )
    }

    fn enroll_agent(
        &self,
        request_id: &str,
        agent_id: &str,
        session: &str,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        self.enroll_registered(request_id, agent_id, session, cancel)
    }

    fn abandon_record(
        &self,
        request_id: &str,
        held_request_id: &str,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(ops::Contract::LifecycleAbandon) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::LifecycleAbandon.as_str()
            )));
        }
        let result = exchange_with_id_at(
            &self.socket,
            request_id,
            "lifecycle.abandon",
            json!({"request_id": held_request_id, "requester": REQUESTER}),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
        })
    }
}

impl DaemonRuntime {
    /// One enrollment, sent under the caller's own request id.
    fn enroll_registered(
        &self,
        request_id: &str,
        agent_id: &str,
        session: &str,
        cancel: &AtomicBool,
    ) -> Result<LifecycleAnswer, String> {
        if !self.serves(ops::Contract::LifecycleEnroll) {
            return Err(self.describe(format!(
                "does not provide `{}`",
                ops::Contract::LifecycleEnroll.as_str()
            )));
        }
        let result = exchange_with_id_at(
            &self.socket,
            request_id,
            "lifecycle.enroll",
            json!({"request": {"agent_id": agent_id, "session": session}, "requester": REQUESTER}),
            cancel,
        )
        .map_err(|failure| failure.message)?;
        Ok(LifecycleAnswer {
            request: result.get("request").cloned().unwrap_or(Value::Null),
        })
    }
}

impl ContainerControls for DaemonRuntime {
    fn create_container(
        &self,
        request: &ContainerCreate,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let request_kind = match &request.kind {
            ContainerCreateKind::Workspace { directory } => {
                let mut request = json!({"kind": "workspace", "focus": false});
                if let Some(directory) = directory {
                    request["directory"] = json!(directory);
                }
                request
            }
            ContainerCreateKind::Tab { workspace_id } => json!({
                "kind": "tab",
                "workspace_id": workspace_id,
                "focus": false,
            }),
            ContainerCreateKind::PaneSplit { pane_id } => json!({
                "kind": "pane_split",
                "pane_id": pane_id,
                "direction": "right",
                "focus": false,
            }),
        };
        let mut params = json!({
            "request": request_kind,
            "requester": REQUESTER,
        });
        if let Some(label) = &request.label {
            params["label"] = json!(label);
        }
        if let Some(recipe) = &request.recipe {
            params["recipe"] = serde_json::to_value(recipe)
                .map_err(|error| self.describe(format!("cannot carry the recipe: {error}")))?;
        }
        self.container_control(
            ops::Contract::TopologyCreate,
            &request.request_id,
            "topology.create",
            params,
            cancel,
        )
    }

    fn delete_container(
        &self,
        request: &ContainerDelete,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let params = json!({
            "request": {
                "container_id": request.container_id,
                "frozen": request.frozen,
            },
            "requester": REQUESTER,
        });
        self.container_control(
            ops::Contract::TopologyDelete,
            &request.request_id,
            "topology.delete",
            params,
            cancel,
        )
    }

    fn reload_container(
        &self,
        request: &ContainerReload,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let params = json!({
            "request": {"container_id": request.container_id},
            "requester": REQUESTER,
        });
        self.container_control(
            ops::Contract::TopologyReload,
            &request.request_id,
            "topology.reload",
            params,
            cancel,
        )
    }
}
