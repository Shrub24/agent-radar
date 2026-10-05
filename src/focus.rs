//! Focus requests: the one action Radar asks Herdr to take.
//!
//! A workspace row is focused with `herdr workspace focus <id>`, which the CLI
//! takes by id. Pane focus has no CLI form by id (`herdr pane focus` moves by
//! direction), so a pane is focused with the socket API's `pane.focus` request:
//! one newline-delimited JSON object over the server's Unix socket. The socket
//! path comes from `herdr status --json`, the CLI's own resolution of the
//! session the collector reads, so Radar reaches the same server with no second
//! configuration. Focusing a pane raises its workspace and tab with it, so one
//! request covers all three levels.
//!
//! Every request runs on its own thread and is bounded by
//! [`FocusConfig::timeout`]: the calling loop never waits on Herdr, a CLI still
//! running at the deadline is killed and reaped by the collector's
//! [`crate::collector::run`], and a socket exchange past it is abandoned.
//! [`Focuser::shutdown`] cancels and joins the worker, so a request outstanding
//! at exit never delays quitting and leaves no child or thread behind.

use std::collections::{HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::collector::{self, CollectorConfig};
use crate::observation::ObservationState;
use crate::tree::{FleetTree, RowId, RowKind, TreeNode};

/// How often a socket wait re-checks for cancellation and the deadline. Small
/// enough that a cancelled request rejoins quickly, large enough not to spin.
const WAIT_GRANULARITY: Duration = Duration::from_millis(10);

/// Where `Enter` wants Herdr's focus moved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A pane row: its workspace, tab and pane in one request.
    Pane(String),
    /// A workspace row.
    Workspace(String),
}

/// Focus settings.
#[derive(Clone, Debug)]
pub struct FocusConfig {
    /// The `herdr` executable: a name resolved through `PATH`, or a path.
    pub executable: PathBuf,
    /// Hard bound on one request. Defaults to the collector's command timeout,
    /// because both run the same CLI against the same server.
    pub timeout: Duration,
}

impl Default for FocusConfig {
    fn default() -> Self {
        let collector = CollectorConfig::default();
        Self {
            executable: collector.executable,
            timeout: collector.command_timeout,
        }
    }
}

impl FocusConfig {
    /// The collector's command settings for the CLI path, so a focus command
    /// inherits its timeout and child-reaping rules rather than restating them.
    fn collector(&self) -> CollectorConfig {
        CollectorConfig {
            executable: self.executable.clone(),
            command_timeout: self.timeout,
            ..CollectorConfig::default()
        }
    }
}

/// A focus request running on its own thread.
struct Request {
    /// Set by the owner; the worker turns it into a kill or an abandoned wait.
    cancel: Arc<AtomicBool>,
    outcome: Receiver<Result<(), String>>,
    worker: JoinHandle<()>,
}

/// Runs focus requests off the calling thread, one at a time.
pub struct Focuser {
    config: FocusConfig,
    in_flight: Option<Request>,
}

impl Focuser {
    /// A focuser that runs nothing until [`Self::start`].
    pub fn new(config: FocusConfig) -> Self {
        Self {
            config,
            in_flight: None,
        }
    }

    /// Starts focusing `target` off the calling thread, replacing a request
    /// already in flight. Replacing waits only for that request's worker to
    /// notice the cancellation, never for its timeout.
    pub fn start(&mut self, target: Target) {
        self.cancel_in_flight();
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, outcome) = mpsc::channel();
        let worker = {
            let config = self.config.clone();
            let cancel = Arc::clone(&cancel);
            thread::Builder::new()
                .name("radar-focus".to_string())
                .spawn(move || {
                    let _ = sender.send(focus(&config, &target, &cancel));
                })
                .expect("focus thread")
        };
        self.in_flight = Some(Request {
            cancel,
            outcome,
            worker,
        });
    }

    /// The finished request's outcome, if one is waiting. Never blocks.
    pub fn poll(&mut self) -> Option<Result<(), String>> {
        let request = self.in_flight.as_mut()?;
        let outcome = match request.outcome.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            // The worker always hands over exactly one outcome before
            // returning, so this is only reachable through a panic in it.
            Err(TryRecvError::Disconnected) => {
                Err("the focus request stopped unexpectedly".to_string())
            }
        };
        let request = self.in_flight.take().expect("checked above");
        let _ = request.worker.join();
        Some(outcome)
    }

    /// Cancels an outstanding request and waits for its worker. Idempotent.
    pub fn shutdown(&mut self) {
        self.cancel_in_flight();
    }

    fn cancel_in_flight(&mut self) {
        if let Some(request) = self.in_flight.take() {
            request.cancel.store(true, Ordering::SeqCst);
            let _ = request.worker.join();
        }
    }
}

impl Drop for Focuser {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Runs one request to completion, cancellation or the deadline.
fn focus(config: &FocusConfig, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
    match target {
        Target::Workspace(workspace_id) => collector::run(
            &config.collector(),
            &["workspace", "focus", workspace_id],
            cancel,
        )
        .map(|_| ()),
        Target::Pane(pane_id) => focus_pane(config, pane_id, cancel),
    }
}

/// Focuses one pane: the CLI reports the socket, the schema's `pane.focus`
/// request does the rest.
fn focus_pane(config: &FocusConfig, pane_id: &str, cancel: &AtomicBool) -> Result<(), String> {
    let status = collector::run(&config.collector(), &["status", "--json"], cancel)?;
    let socket = socket_path(&status)?;
    let budget = Budget::of(config.timeout);
    let mut stream = UnixStream::connect(&socket)
        .map_err(|error| format!("could not reach herdr at {}: {error}", socket.display()))?;
    for setting in [
        stream.set_read_timeout(Some(WAIT_GRANULARITY)),
        stream.set_write_timeout(Some(WAIT_GRANULARITY)),
    ] {
        setting.map_err(|error| format!("herdr socket could not be read: {error}"))?;
    }
    // The schema's `pane.focus` params are `{"pane_id": "..."}`; focusing the
    // pane raises its workspace and tab, so one request does all three.
    let request = serde_json::json!({
        "id": "radar:focus",
        "method": "pane.focus",
        "params": { "pane_id": pane_id },
    });
    write_request(
        &mut stream,
        format!("{request}\n").as_bytes(),
        budget,
        cancel,
    )?;
    let answer = read_answer(&mut stream, budget, cancel)?;
    decode_answer(&answer)
}

/// The socket path from `herdr status --json`, which is the server the CLI
/// (and so the collector) is talking to.
fn socket_path(status: &str) -> Result<PathBuf, String> {
    let status: serde_json::Value = serde_json::from_str(status)
        .map_err(|error| format!("herdr status could not be read: {error}"))?;
    match status
        .pointer("/server/socket")
        .and_then(|path| path.as_str())
    {
        Some(path) => Ok(PathBuf::from(path)),
        None => Err("herdr is not running".to_string()),
    }
}

/// One exchange's deadline, so every wait reports the same timeout.
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

    /// Waits one granularity. `Err` once the request is cancelled or the
    /// deadline has passed.
    fn wait(&self, cancel: &AtomicBool) -> Result<(), String> {
        if cancel.load(Ordering::SeqCst) {
            return Err("focus cancelled".to_string());
        }
        if Instant::now() >= self.deadline {
            return Err(format!("herdr timed out after {:?}", self.timeout));
        }
        thread::sleep(WAIT_GRANULARITY);
        Ok(())
    }
}

/// Writes the whole request, waiting out a socket that is momentarily busy.
fn write_request(
    stream: &mut UnixStream,
    bytes: &[u8],
    budget: Budget,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut written = 0;
    while written < bytes.len() {
        match stream.write(&bytes[written..]) {
            Ok(0) => return Err("herdr closed the connection".to_string()),
            Ok(count) => written += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => budget.wait(cancel)?,
            Err(error) => return Err(format!("herdr could not be asked: {error}")),
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
            Ok(0) => return Err("herdr closed the connection".to_string()),
            Ok(count) => {
                answer.extend_from_slice(&chunk[..count]);
                if let Some(end) = answer.iter().position(|byte| *byte == b'\n') {
                    return Ok(String::from_utf8_lossy(&answer[..end]).trim().to_string());
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) if is_timeout(&error) => budget.wait(cancel)?,
            Err(error) => return Err(format!("herdr sent no answer: {error}")),
        }
    }
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// One answer: `result` is success, `error` is Herdr refusing the request.
fn decode_answer(answer: &str) -> Result<(), String> {
    let answer: serde_json::Value = serde_json::from_str(answer)
        .map_err(|error| format!("herdr sent an unreadable answer: {error}"))?;
    if let Some(error) = answer.get("error") {
        let reason = error
            .get("message")
            .and_then(|message| message.as_str())
            .or_else(|| error.get("code").and_then(|code| code.as_str()))
            .unwrap_or("no reason given");
        return Err(format!("herdr refused to focus: {reason}"));
    }
    answer
        .get("result")
        .map(|_| ())
        .ok_or_else(|| "herdr sent an unreadable answer".to_string())
}

/// The focus target of every row the current observation proves.
///
/// A row keeps its own location: agent and pane rows focus their pane,
/// workspace rows focus their workspace. A row whose location the observation
/// no longer places — a retained row whose pane is gone, a workspace only
/// retained facts name — gets no entry, which is what makes `Enter` refuse
/// instead of focusing something Radar cannot point at.
pub fn targets(tree: &FleetTree, state: &ObservationState) -> HashMap<RowId, Target> {
    let Some(inventory) = state.inventory() else {
        return HashMap::new();
    };
    let panes: HashSet<&str> = inventory
        .panes
        .iter()
        .map(|pane| pane.location.pane_id.as_str())
        .chain(
            inventory
                .agents
                .iter()
                .map(|agent| agent.location.pane_id.as_str()),
        )
        .collect();
    let workspaces: HashSet<&str> = inventory
        .workspaces
        .iter()
        .map(|workspace| workspace.workspace_id.as_str())
        .chain(
            inventory
                .panes
                .iter()
                .map(|pane| pane.location.workspace_id.as_str()),
        )
        .chain(
            inventory
                .agents
                .iter()
                .map(|agent| agent.location.workspace_id.as_str()),
        )
        .collect();

    fn walk(
        nodes: &[TreeNode],
        panes: &HashSet<&str>,
        workspaces: &HashSet<&str>,
        targets: &mut HashMap<RowId, Target>,
    ) {
        for node in nodes {
            let target = match &node.row.kind {
                RowKind::Workspace { workspace_id, .. } if workspaces.contains(&**workspace_id) => {
                    Some(Target::Workspace(workspace_id.clone()))
                }
                RowKind::Agent(agent) if panes.contains(&*agent.pane_id) => {
                    Some(Target::Pane(agent.pane_id.clone()))
                }
                RowKind::Pane(pane) if panes.contains(&*pane.pane_id) => {
                    Some(Target::Pane(pane.pane_id.clone()))
                }
                _ => None,
            };
            if let Some(target) = target {
                targets.insert(node.row.id.clone(), target);
            }
            walk(&node.children, panes, workspaces, targets);
        }
    }

    let mut targets = HashMap::new();
    walk(&tree.roots, &panes, &workspaces, &mut targets);
    targets
}
