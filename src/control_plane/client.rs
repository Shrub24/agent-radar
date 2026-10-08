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

use serde::Serialize;
use serde_json::{Value, json};

use super::{MAX_LINE_BYTES, PROTOCOL_VERSION, random_uuid, socket_path, trusted_socket};
use crate::config::{RuntimeBackend, RuntimeConfig};
use crate::herdr::{HerdrConfig, HerdrRuntime};
use crate::lifecycle::{self, CloseRequest};
use crate::model::{FleetObservation, ForegroundEvidence};
use crate::runtime::{
    CloseOutcome, CloseTarget, CreateOutcome, CreateRequest, CreatedIdentity, CreatedKind,
    CreatedLocation, FocusOutcome, InputOutcome, InputRequest, OutputOutcome, OutputRead,
    OutputRequest, ReportOutcome, ReportRequest, RuntimeProvider, Target,
};

/// How long one exchange with the daemon may take before it is treated as not
/// answering.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one socket wait lasts before cancellation and the deadline are
/// checked again.
const WAIT_GRANULARITY: Duration = Duration::from_millis(10);

/// The label this caller's records carry, so an operator reading the store knows
/// who asked.
const REQUESTER: &str = "radar";

/// The capability names this protocol version defines.
///
/// A daemon's answer is interned against these, so a name this build cannot
/// implement is never credited to it.
const KNOWN_CAPABILITIES: &[&str] = &[
    "observe",
    "process_info",
    "focus",
    "close",
    "creation",
    "input",
    "output",
    "reporting",
];

/// The daemon as a [`RuntimeProvider`].
pub struct DaemonRuntime {
    socket: PathBuf,
    declared: &'static [&'static str],
}

impl DaemonRuntime {
    /// The capabilities Radar's dashboard cannot work without: it observes, asks
    /// for foreground evidence, focuses and closes.
    ///
    /// Anything beyond these is asked for and answered by the daemon: a backend
    /// that cannot create or report refuses that operation by name rather than
    /// making the whole daemon unusable.
    pub const REQUIRED: &'static [&'static str] = &["observe", "process_info", "focus", "close"];

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
        let answer =
            exchange_at(socket, "ping", json!({}), &cancel).map_err(|failure| failure.message)?;
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
        })
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
    fn capabilities(&self) -> &'static [&'static str] {
        self.declared
    }

    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        let result = self
            .call("observe", json!({}), cancel)
            .map_err(|failure| failure.message)?;
        let inventory = result.get("inventory").cloned().unwrap_or(Value::Null);
        serde_json::from_value(inventory)
            .map_err(|error| self.describe(format!("sent an unreadable inventory: {error}")))
    }

    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence {
        // Evidence that cannot be read is inconclusive, exactly as it is for the
        // direct adapter: nothing here invents a pane state.
        let Ok(result) = self.call("process_info", json!({"pane_id": pane_id}), cancel) else {
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
            .call("focus", params, cancel)
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
        match self.mutation("close", &request, cancel) {
            Ok(settled) => settled.close(),
            Err(failure) => failure.settled().close(),
        }
    }

    fn create(&self, request: &CreateRequest, cancel: &AtomicBool) -> CreateOutcome {
        match self.mutation("create", request, cancel) {
            Ok(settled) => settled.create(),
            Err(failure) => failure.settled().create(),
        }
    }

    fn input(&self, request: &InputRequest, cancel: &AtomicBool) -> InputOutcome {
        match self.mutation("input", request, cancel) {
            Ok(settled) => settled.input(),
            Err(failure) => failure.settled().input(),
        }
    }

    fn output(&self, request: &OutputRequest, cancel: &AtomicBool) -> OutputOutcome {
        let result = match self.call("output", json!(request), cancel) {
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
        match self.mutation("report", request, cancel) {
            Ok(settled) => settled.report(),
            Err(failure) => failure.settled().report(),
        }
    }
}

/// What Radar will talk to this run, and the one line to tell the operator when
/// the configured daemon was not the one used.
pub struct Selected {
    pub provider: Arc<dyn RuntimeProvider>,
    /// The backend in use, in the protocol's own words: `direct` or `daemon`.
    pub backend: &'static str,
    /// The single diagnostic to print, when a configured daemon could not be
    /// used. The provider is then the direct adapter.
    pub diagnostic: Option<String>,
}

/// Chooses the runtime before anything is collected or acted on.
///
/// The direct adapter is both the default and the fallback: the daemon is used
/// only when it answers, speaks this protocol version and declares the
/// capabilities the dashboard needs. The choice is made once — nothing here
/// switches later, and nothing retries an operation through the other backend,
/// because an operation the daemon may have accepted is unknown rather than
/// unperformed.
pub fn select(runtime: &RuntimeConfig, direct: HerdrConfig) -> Selected {
    let direct = || Selected {
        provider: Arc::new(HerdrRuntime::new(direct)),
        backend: "direct",
        diagnostic: None,
    };
    if runtime.backend == RuntimeBackend::Direct {
        return direct();
    }
    let socket = runtime.socket.clone().unwrap_or_else(socket_path);
    match DaemonRuntime::handshake(&socket) {
        Ok(daemon) => Selected {
            provider: Arc::new(daemon),
            backend: "daemon",
            diagnostic: None,
        },
        Err(failure) => Selected {
            diagnostic: Some(format!("{failure}; using the direct Herdr adapter")),
            ..direct()
        },
    }
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
fn exchange(
    socket: &Path,
    method: &str,
    params: Value,
    cancel: &AtomicBool,
) -> Result<Value, Failure> {
    let id = random_uuid();
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
    if answer.get("id").and_then(Value::as_str) != Some(id.as_str()) {
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
            // `internal` is deliberately absent: its own contract says the
            // request may or may not have had an effect, so it stays unknown.
            "bad_version"
            | "bad_request"
            | "unknown_method"
            | "bad_params"
            | "refused"
            | "busy"
            | "backend_unavailable"
            | "not_found" => Failure::refused(message),
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
    exchange(socket, method, params, cancel).map_err(|failure| Failure {
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
