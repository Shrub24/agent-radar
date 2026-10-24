//! Radar's own client for the control plane.
//!
//! Everything above the runtime seam keeps seeing normalized types; the line
//! protocol appears in this module and nowhere else. One request is one line and
//! one answer is one line, and the daemon's own record decides what an operation
//! established — `completed`, `refused` or `unknown` — never the mere fact that
//! a socket answered.
//!
//! Three things are deliberate:
//!
//! * **An uncertain operation is never retried elsewhere.** An operation the
//!   daemon may have accepted stays unknown, because a second attempt would be a
//!   second execution of something that may already have happened.
//! * **The daemon revision must match the client's.** A backend that cannot create
//!   or report is not answered by a different operation here; the daemon's own
//!   refusal is passed on as a refusal.
//! * **A socket is vouched for before it is dialled.** [`DaemonRuntime::handshake`]
//!   applies the daemon's own trust rule to the path, read-only, before any
//!   connection: a socket someone else planted — the fallback directory under
//!   `/tmp` is created by whoever gets there first — is refused rather than
//!   believed, so it never becomes Radar's observation and control plane.

mod controls;
mod exchange;

pub use controls::{
    ContainerAnswer, ContainerControls, ContainerCreate, ContainerCreateKind, ContainerDelete,
    ContainerReload, LifecycleAnswer, LifecycleControlRequest, LifecycleControls,
};
use exchange::*;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ops;
use super::{PROTOCOL_VERSION, socket_path, trusted_socket};
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
    /// the one line explaining why the daemon cannot be used.
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
        // Evidence that cannot be read is inconclusive: nothing here invents a
        // pane state.
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
