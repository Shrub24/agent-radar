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
//! * **The daemon's capabilities are the client's.** A backend that cannot create
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
use super::registry::PublicSpawnTopology;
use super::{MAX_LINE_BYTES, PROTOCOL_VERSION, random_uuid, socket_path, trusted_socket};
use crate::lifecycle::{self, CloseRequest};
use crate::model::{FleetObservation, ForegroundEvidence};
use crate::registry_observation::{RegistryAgent, RegistryRead};
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
const REGISTRY_PAGE_LIMIT: usize = 100;

/// How long reading every registered agent may take in one refresh.
const REGISTRY_READ_DEADLINE: Duration = Duration::from_secs(10);

/// One `agent.list` answer.
#[derive(Deserialize)]
struct AgentPage {
    agents: Vec<RegistryAgent>,
    next: Option<String>,
}

/// One `spawn.list` answer.
#[derive(Deserialize)]
struct SpawnPage {
    spawns: Vec<PublicSpawnTopology>,
    next: Option<String>,
}

/// One `topology.list` answer.
#[derive(Deserialize)]
struct ContainerPage {
    containers: Vec<PublicContainer>,
    next: Option<String>,
}

/// The label this caller's records carry, so an operator reading the store knows
/// who asked.
const REQUESTER: &str = "radar";

/// The capability names this protocol version defines: the backend
/// capabilities a mux adapter declares, and the daemon contracts those can
/// serve.
///
/// A daemon's answer is interned against these, so a name this build cannot
/// implement is never credited to it.
const KNOWN_CAPABILITIES: &[&str] = &[
    ops::OBSERVE_CAPABILITY,
    ops::PROCESS_INFO_CAPABILITY,
    ops::FOCUS_CAPABILITY,
    ops::CLOSE_CAPABILITY,
    ops::CREATE_CAPABILITY,
    ops::INPUT_CAPABILITY,
    ops::OUTPUT_CAPABILITY,
    ops::REPORT_CAPABILITY,
    ops::LAUNCH_CAPABILITY,
    ops::LAUNCH_CONTEXT_CAPABILITY,
    ops::EXIT_OBSERVATION_CAPABILITY,
    ops::REGISTRY_CONTRACT,
    ops::REGISTRY_FACTS_CONTRACT,
    ops::EVENTS_CONTRACT,
    ops::TOPOLOGY_CREATE_CONTRACT,
    ops::TOPOLOGY_DELETE_CONTRACT,
    ops::TOPOLOGY_GET_CONTRACT,
    ops::TOPOLOGY_LIST_CONTRACT,
    ops::TOPOLOGY_RELOAD_CONTRACT,
    ops::TOPOLOGY_RECONCILE_CONTRACT,
    ops::SPAWN_CONTRACT,
    ops::SPAWN_CLOSE_CONTRACT,
    ops::SPAWN_LAUNCH_CONTEXT_CONTRACT,
    ops::SPAWN_EXIT_OBSERVATION_CONTRACT,
];

/// The daemon as a [`RuntimeProvider`].
pub struct DaemonRuntime {
    socket: PathBuf,
    declared: &'static [&'static str],
    identity: BackendIdentity,
}

impl DaemonRuntime {
    /// The capabilities Radar's dashboard cannot work without: it observes, asks
    /// for foreground evidence, focuses and closes.
    ///
    /// Anything beyond these is asked for and answered by the daemon: a backend
    /// that cannot create or report refuses that operation by name rather than
    /// making the whole daemon unusable.
    pub const REQUIRED: &'static [&'static str] = &[
        ops::OBSERVE_CAPABILITY,
        ops::PROCESS_INFO_CAPABILITY,
        ops::FOCUS_CAPABILITY,
        ops::CLOSE_CAPABILITY,
    ];

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
        let declared: Vec<&str> = answer
            .get("capabilities")
            .and_then(Value::as_array)
            .map(|names| names.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for required in Self::REQUIRED {
            if !declared.contains(required) {
                return Err(format!(
                    "the control daemon at {} does not provide `{required}`",
                    socket.display()
                ));
            }
        }
        Ok(Self {
            socket: socket.to_path_buf(),
            declared: intern(&declared),
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

    /// Every page of one cursor-paged read, to the end.
    ///
    /// A partial read is never returned as the whole: a page that fails, a
    /// cursor that does not advance, or a read that outlives its deadline fails
    /// the read, and so the refresh, which keeps the last inventory stale.
    fn read_all<T>(
        &self,
        method: &str,
        what: &str,
        deadline: Instant,
        cancel: &AtomicBool,
        decode: impl Fn(Value) -> Result<(Vec<T>, Option<String>), serde_json::Error>,
    ) -> Result<Vec<T>, String> {
        let mut items = Vec::new();
        let mut after: Option<String> = None;
        loop {
            if Instant::now() >= deadline {
                return Err(self.describe(format!(
                    "did not finish reading {what} within {REGISTRY_READ_DEADLINE:?}"
                )));
            }
            let mut params = json!({"limit": REGISTRY_PAGE_LIMIT});
            if let Some(cursor) = &after {
                params["after"] = json!(cursor);
            }
            let result = self
                .call(method, params, cancel)
                .map_err(|failure| failure.message)?;
            let (page, next) = decode(result).map_err(|error| {
                self.describe(format!("sent an unreadable {what} page: {error}"))
            })?;
            items.extend(page);
            match next {
                None => return Ok(items),
                Some(next) if after.as_deref() == Some(next.as_str()) => {
                    return Err(
                        self.describe(format!("repeated a {what} page cursor without progress"))
                    );
                }
                Some(next) => after = Some(next),
            }
        }
    }

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
}

impl RuntimeProvider for DaemonRuntime {
    /// The backend the daemon reports: this client asks nothing of the mux
    /// itself, so its identity is the one behind the socket.
    fn identity(&self) -> BackendIdentity {
        self.identity.clone()
    }

    fn capabilities(&self) -> &'static [&'static str] {
        self.declared
    }

    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        let result = self
            .call("mux.observe", json!({}), cancel)
            .map_err(|failure| failure.message)?;
        let inventory = result.get("inventory").cloned().unwrap_or(Value::Null);
        serde_json::from_value(inventory)
            .map_err(|error| self.describe(format!("sent an unreadable inventory: {error}")))
    }

    fn registry(&self, cancel: &AtomicBool) -> Result<Option<RegistryRead>, String> {
        // Without the registry there are no agent facts, and the inventory's own
        // agent records are not a substitute. Refreshing fails visibly instead.
        for contract in [ops::REGISTRY_CONTRACT, ops::SPAWN_CONTRACT] {
            if !self.declared.contains(&contract) {
                return Err(self.describe(format!("does not provide `{contract}`")));
            }
        }
        let deadline = Instant::now() + REGISTRY_READ_DEADLINE;
        let agents = self.read_all(
            "agent.list",
            "registered agents",
            deadline,
            cancel,
            |value| {
                let page: AgentPage = serde_json::from_value(value)?;
                Ok((page.agents, page.next))
            },
        )?;
        let topology =
            self.read_all("spawn.list", "spawn topology", deadline, cancel, |value| {
                let page: SpawnPage = serde_json::from_value(value)?;
                Ok((page.spawns, page.next))
            })?;
        // Containers are the daemon's own records, and a daemon that does not
        // serve them has none: a missing contract reads as no containers rather
        // than as a refresh that failed.
        let containers = if self.declared.contains(&ops::TOPOLOGY_LIST_CONTRACT) {
            self.read_all("topology.list", "containers", deadline, cancel, |value| {
                let page: ContainerPage = serde_json::from_value(value)?;
                Ok((page.containers, page.next))
            })?
        } else {
            Vec::new()
        };
        Ok(Some(RegistryRead {
            agents,
            topology,
            containers,
        }))
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
}

/// Connects to the daemon at its documented socket and proves it usable.
///
/// The failure names what was wrong. Nothing falls back to another source: an
/// unreachable daemon is the caller's to show, and the caller retries.
pub fn connect() -> Result<Selected, String> {
    connect_at(&socket_path())
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
    })
}

/// Interns a declared capability set against the protocol's own names.
///
/// [`RuntimeProvider::capabilities`] asks for a `'static` slice and the daemon's
/// answer arrives at runtime, so the subset is kept for the life of the process:
/// at most eight pointers, held for the one handshake this run performs. A
/// capability list the trait owned rather than borrowed would retire this.
fn intern(declared: &[&str]) -> &'static [&'static str] {
    let names: Vec<&'static str> = KNOWN_CAPABILITIES
        .iter()
        .copied()
        .filter(|known| declared.contains(known))
        .collect();
    Box::leak(names.into_boxed_slice())
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
    /// One container control, sent under the caller's own request id.
    ///
    /// The contract it needs is checked before the request is sent, so a caller
    /// refuses work this daemon does not serve instead of discovering it mid-effect.
    fn container_control(
        &self,
        contract: &str,
        request_id: &str,
        method: &str,
        params: Value,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        if !self.declared.contains(&contract) {
            return Err(self.describe(format!("does not provide `{contract}`")));
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

impl ContainerControls for DaemonRuntime {
    fn create_container(
        &self,
        request: &ContainerCreate,
        cancel: &AtomicBool,
    ) -> Result<ContainerAnswer, String> {
        let request_kind = match &request.kind {
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
            ops::TOPOLOGY_CREATE_CONTRACT,
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
            ops::TOPOLOGY_DELETE_CONTRACT,
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
            ops::TOPOLOGY_RELOAD_CONTRACT,
            &request.request_id,
            "topology.reload",
            params,
            cancel,
        )
    }
}
