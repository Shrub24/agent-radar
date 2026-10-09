#![cfg(unix)]
//! Control-plane tests: a real daemon on a real socket in a private temporary
//! directory, driven line by line, plus the `radar daemon` subcommand as its own
//! process.
//!
//! Every wait is bounded, so a daemon that stops answering fails its test rather
//! than hanging it. Nothing here runs a mux command, and nothing could: this
//! build has no backend, and an operation's own record says nothing ran — its
//! effects are empty and its message names the capability that is missing.
//!
//! The records are the interesting surface. They are written before anything is
//! executed, they are readable through the protocol, and they are readable again
//! by a daemon bound over the same state root — which is what makes "did this
//! run" answerable after a restart.

use std::any::Any;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::Daemon;
use agent_radar::HerdrConfig;
use agent_radar::config::{RuntimeBackend, RuntimeConfig};
use agent_radar::control_plane::ops::Operation;
use agent_radar::control_plane::registry::{MAX_LISTED, MAX_TEXT_BYTES, ProcessVerifier};
use agent_radar::control_plane::{
    Category, DaemonRuntime, Derived, MAX_LINE_BYTES, MAX_VERIFICATION_JOBS, PROTOCOL_VERSION,
    RecordState, RequestOutcome, RequestRecord, Store, now_ms, random_uuid, select,
};
use agent_radar::lifecycle;
use agent_radar::model::{
    AgentObservation, BinaryFreshness, BinaryIdentity, FleetObservation, ForegroundEvidence,
    HerdsmanFacts, LocalFacts, Location, Pane, ProcessIdentity, RuntimeStatus, SemanticState, Tab,
    TerminalMode, Workspace,
};
use agent_radar::runtime::{
    CloseOutcome, CloseTarget, CreateOutcome, CreateRequest, CreatedLocation, FocusOutcome,
    InputOutcome, InputPayload, InputRequest, LaunchOutcome, LaunchRequest,
    MAX_REPORTED_TEXT_BYTES, OutputOutcome, OutputRead, OutputRequest, OutputSource, ReportOutcome,
    ReportRequest, ReportedState, RuntimeProvider, SplitDirection, StateReport, Target,
};
use serde_json::{Value, json};

/// What a fake backend answers to a focus request.
#[derive(Clone)]
enum FocusAnswer {
    Completed,
    Refused(String),
    Unknown(String),
}

struct FakeBackend {
    inventory: FleetObservation,
    focused: Mutex<Vec<Target>>,
    answer: FocusAnswer,
}

impl FakeBackend {
    fn new(inventory: FleetObservation, answer: FocusAnswer) -> Self {
        Self {
            inventory,
            focused: Mutex::new(Vec::new()),
            answer,
        }
    }
}

impl RuntimeProvider for FakeBackend {
    fn capabilities(&self) -> &'static [&'static str] {
        &["observe", "process_info", "focus"]
    }
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        Ok(self.inventory.clone())
    }
    fn foreground_evidence(&self, pane: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        if pane == "wA:p1" {
            ForegroundEvidence::command(200, Some("nvim".into()), Some("nvim file".into()))
        } else {
            ForegroundEvidence::Inconclusive
        }
    }
    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        self.focus_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }
    fn focus_outcome(&self, target: &Target, _cancel: &AtomicBool) -> FocusOutcome {
        self.focused
            .lock()
            .expect("focus list")
            .push(target.clone());
        match &self.answer {
            FocusAnswer::Completed => FocusOutcome::Completed,
            FocusAnswer::Refused(message) => FocusOutcome::Refused(message.clone()),
            FocusAnswer::Unknown(message) => FocusOutcome::Unknown(message.clone()),
        }
    }
    fn close(&self, _target: &CloseTarget, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!()
    }
}

/// How long one side of a test rendezvous waits for the other. Two threads in
/// one process meet in microseconds; the allowance is for a loaded machine.
const RENDEZVOUS_DEADLINE: Duration = Duration::from_secs(5);

/// A bounded rendezvous between a test and a backend call inside the daemon: the
/// backend announces it has entered the call and waits to be let go, and the test
/// waits for that announcement and later opens the gate.
///
/// This is what replaced `std::sync::Barrier`. A barrier that is never reached
/// parks its other side forever, so one missed handshake — or one thread that
/// panicked before arriving — took the whole `cargo test` run with it. Every wait
/// here has a deadline, so a miss fails the test with the message saying which
/// side never arrived.
struct Rendezvous {
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<()>,
    deadline: Duration,
}

/// The backend's side of a [`Rendezvous`], held by whichever fake is gated.
struct Gate {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    deadline: Duration,
}

impl Rendezvous {
    /// A gate for a backend, and the test's side of it.
    fn new() -> (Arc<Gate>, Self) {
        Self::with_deadline(RENDEZVOUS_DEADLINE)
    }

    /// The same rendezvous with another deadline, so the missed-rendezvous test
    /// reaches the failure without waiting out the production allowance.
    fn with_deadline(deadline: Duration) -> (Arc<Gate>, Self) {
        let (entered, arrival) = mpsc::channel();
        let (release, let_go) = mpsc::channel();
        (
            Arc::new(Gate {
                entered,
                release: Mutex::new(let_go),
                deadline,
            }),
            Self {
                entered: arrival,
                release,
                deadline,
            },
        )
    }

    /// Waits for the backend to be inside the call.
    fn entered(&self) {
        self.entered
            .recv_timeout(self.deadline)
            .expect("the backend never entered the gated call");
    }

    /// Lets a call the test is holding open finish.
    fn release(&self) {
        self.release
            .send(())
            .expect("the gated call is no longer waiting to be released");
    }
}

impl Gate {
    /// Announces that the gated call has begun. The channel is unbounded, so this
    /// never blocks: a test that is no longer listening has already failed.
    fn arrived(&self) {
        let _ = self.entered.send(());
    }

    /// Announces the call and waits to be let go.
    fn hold(&self) {
        self.arrived();
        self.release
            .lock()
            .expect("the release channel")
            .recv_timeout(self.deadline)
            .expect("the test never released the gated call");
    }
}

/// A backend whose focus holds dispatch open: the test rendezvouses on `gate`,
/// observes the daemon mid-request, then releases it. With `released: false` the
/// backend waits for the daemon to cancel it instead. Every wait is a rendezvous
/// or the daemon's own cancellation flag — never a test sleep.
struct GatedBackend {
    gate: Arc<Gate>,
    /// Whether the test lets the call go, or the daemon cancels it.
    released: bool,
    answer: FocusOutcome,
}

impl RuntimeProvider for GatedBackend {
    fn capabilities(&self) -> &'static [&'static str] {
        // `observe` because the daemon reads this backend while a focus is held.
        &["observe", "focus"]
    }
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        Ok(FleetObservation {
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes: Vec::new(),
            agents: Vec::new(),
        })
    }
    fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        ForegroundEvidence::Inconclusive
    }
    fn focus(&self, _target: &Target, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the gated backend is asked through focus_outcome")
    }
    fn focus_outcome(&self, _target: &Target, cancel: &AtomicBool) -> FocusOutcome {
        if self.released {
            self.gate.hold();
        } else {
            // No release: the daemon has to cancel this request, which is what a
            // stop of an in-flight focus looks like from the backend's side.
            self.gate.arrived();
            while !cancel.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
        }
        self.answer.clone()
    }
    fn close(&self, _target: &CloseTarget, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!()
    }
}

/// One workspace and one tab, with the listed panes and the agents the close
/// policy reads. `Some(managed)` is a Pi pane publishing (or not publishing)
/// owner metadata; `None` is a bare pane with no agent on it.
fn close_inventory(tab_id: &str, occupants: &[(&str, Option<bool>)]) -> FleetObservation {
    let location = |pane_id: &str| Location {
        workspace_id: "wA".into(),
        tab_id: tab_id.into(),
        pane_id: pane_id.into(),
    };
    FleetObservation {
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: None,
            number: Some(1),
        }],
        tabs: vec![Tab {
            tab_id: tab_id.into(),
            workspace_id: "wA".into(),
            label: None,
            number: Some(1),
        }],
        panes: occupants
            .iter()
            .map(|(pane_id, _)| Pane {
                location: location(pane_id),
                label: None,
                title: None,
            })
            .collect(),
        agents: occupants
            .iter()
            .filter_map(|(pane_id, occupant)| {
                occupant.map(|managed| AgentObservation {
                    location: location(pane_id),
                    name: Some("pi".into()),
                    label: None,
                    status: None,
                    session: None,
                    lineage: None,
                    facts: HerdsmanFacts {
                        managed_metadata: managed,
                        ..Default::default()
                    },
                })
            })
            .collect(),
    }
}

/// The params a client sends for a location it just observed: the target, and
/// the identity Radar froze from that same observation.
fn frozen_from(inventory: &FleetObservation, target: CloseTarget) -> Value {
    json!({
        "request": {
            "target": target,
            "identity": lifecycle::identity(inventory, &target),
        }
    })
}

/// What a fake backend answers to a close request.
#[derive(Clone)]
enum CloseAnswer {
    Completed,
    Refused(String),
    Unknown(String),
}

/// A fake backend for close: an inventory that may be unreadable, the targets it
/// was asked to close, and a staged answer. Nothing here is a mux.
struct CloseBackend {
    inventory: Result<FleetObservation, String>,
    closed: Mutex<Vec<CloseTarget>>,
    answer: CloseAnswer,
    /// Reached once the close has been entered, when the test wants to hold
    /// dispatch open. The wait that follows is the daemon's own cancel flag.
    entered: Option<Arc<Gate>>,
}

impl CloseBackend {
    fn new(inventory: Result<FleetObservation, String>, answer: CloseAnswer) -> Arc<Self> {
        Arc::new(Self {
            inventory,
            closed: Mutex::new(Vec::new()),
            answer,
            entered: None,
        })
    }

    /// The same inventory and answer, holding the close open until the daemon
    /// cancels it.
    fn gated(&self, entered: Arc<Gate>) -> Arc<Self> {
        Arc::new(Self {
            inventory: self.inventory.clone(),
            closed: Mutex::new(Vec::new()),
            answer: self.answer.clone(),
            entered: Some(entered),
        })
    }

    fn told_to_close(&self, target: &CloseTarget) -> bool {
        self.closed.lock().expect("the close list").contains(target)
    }
}

impl RuntimeProvider for CloseBackend {
    fn capabilities(&self) -> &'static [&'static str] {
        &["close"]
    }
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        self.inventory.clone()
    }
    fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        ForegroundEvidence::Inconclusive
    }
    fn focus(&self, _target: &Target, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the close backend is never asked to focus")
    }
    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        self.close_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }
    fn close_outcome(&self, target: &CloseTarget, cancel: &AtomicBool) -> CloseOutcome {
        self.closed
            .lock()
            .expect("the close list")
            .push(target.clone());
        if let Some(entered) = &self.entered {
            entered.arrived();
            while !cancel.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(1));
            }
        }
        match &self.answer {
            CloseAnswer::Completed => CloseOutcome::Completed,
            CloseAnswer::Refused(message) => CloseOutcome::Refused(message.clone()),
            CloseAnswer::Unknown(message) => CloseOutcome::Unknown(message.clone()),
        }
    }
}

/// Every capability the fake mux backend declares.
const MUX_CAPABILITIES: &[&str] = &[
    "observe",
    "process_info",
    "creation",
    "input",
    "output",
    "reporting",
];

/// The same, plus `launch`: a backend that can serve a spawn, because it can
/// both create a pane and run a command in it.
const SPAWN_CAPABILITIES: &[&str] = &[
    "observe",
    "process_info",
    "creation",
    "input",
    "output",
    "reporting",
    "launch",
    "close",
];

/// The daemon's own registry capability is advertised alongside whatever mux
/// capabilities the backend declares.
fn advertised(backend: &[&str]) -> Value {
    let mut capabilities: Vec<Value> = backend.iter().map(|name| json!(name)).collect();
    capabilities.push(json!("agent_registry"));
    Value::Array(capabilities)
}

/// What a fake backend answers to a report.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ReportAnswer {
    Completed,
    Refused(String),
    Unknown(String),
}

/// What a fake backend answers to a launch.
#[derive(Clone, Debug, PartialEq, Eq)]
enum LaunchAnswer {
    Completed,
    Refused(String),
    Unknown(String),
}

/// A fake backend for the mux primitives: what the test staged it answers, and
/// what the daemon asked it for. Nothing here is a mux.
struct MuxBackend {
    capabilities: &'static [&'static str],
    created: CreatedLocation,
    output: OutputRead,
    report: ReportAnswer,
    launch: LaunchAnswer,
    /// What this backend reports as its panes: what a topology read consults to
    /// confirm a recorded location.
    inventory: FleetObservation,
    foreground: ForegroundEvidence,
    closed: Mutex<Vec<CloseTarget>>,
    close_answer: CloseAnswer,
    created_requests: Mutex<Vec<CreateRequest>>,
    input_requests: Mutex<Vec<InputRequest>>,
    output_requests: Mutex<Vec<OutputRequest>>,
    report_requests: Mutex<Vec<ReportRequest>>,
    launch_requests: Mutex<Vec<LaunchRequest>>,
    /// When set, a create waits inside the gate, so a test can observe the
    /// daemon while the request is in flight. Every wait here is a rendezvous,
    /// never a sleep.
    gate: Option<Arc<Gate>>,
}

impl MuxBackend {
    fn new(capabilities: &'static [&'static str]) -> Arc<Self> {
        Self::staged(capabilities, None)
    }

    /// The same backend, answering a report with this outcome.
    fn reporting(&self, report: ReportAnswer) -> Arc<Self> {
        Self::staged(self.capabilities, Some(report))
    }

    fn staged(capabilities: &'static [&'static str], report: Option<ReportAnswer>) -> Arc<Self> {
        Arc::new(Self {
            capabilities,
            created: CreatedLocation {
                workspace_id: None,
                tab_id: None,
                pane_id: Some("wA:p2".into()),
            },
            output: OutputRead {
                text: "hello\n".into(),
                truncated: false,
                revision: Some(7),
            },
            report: report.unwrap_or(ReportAnswer::Completed),
            launch: LaunchAnswer::Completed,
            inventory: empty_inventory(),
            foreground: ForegroundEvidence::Inconclusive,
            closed: Mutex::new(Vec::new()),
            close_answer: CloseAnswer::Completed,
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    /// The same backend answering an output read with this snapshot.
    fn closing(self: &Arc<Self>, close: CloseAnswer) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            closed: Mutex::new(Vec::new()),
            close_answer: close,
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    fn with_foreground(self: &Arc<Self>, evidence: ForegroundEvidence) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: evidence,
            closed: Mutex::new(Vec::new()),
            close_answer: self.close_answer.clone(),
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    fn closed_targets(&self) -> Vec<CloseTarget> {
        self.closed.lock().expect("the close list").clone()
    }

    fn reading(self: &Arc<Self>, output: OutputRead) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created: self.created.clone(),
            output,
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            closed: Mutex::new(Vec::new()),
            close_answer: self.close_answer.clone(),
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    /// The same backend, answering a launch with this outcome.
    fn launching(self: &Arc<Self>, launch: LaunchAnswer) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch,
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            closed: Mutex::new(Vec::new()),
            close_answer: self.close_answer.clone(),
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    /// The same backend confirming a create with this answer's identities.
    fn naming(self: &Arc<Self>, created: CreatedLocation) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created,
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            closed: Mutex::new(Vec::new()),
            close_answer: self.close_answer.clone(),
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    /// The same backend, holding its first create open until the test releases it.
    fn gated(self: &Arc<Self>, gate: Arc<Gate>) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            closed: Mutex::new(Vec::new()),
            close_answer: self.close_answer.clone(),
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: Some(gate),
        })
    }

    /// The same backend, reporting exactly these panes as its inventory.
    fn observing(self: &Arc<Self>, panes: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: FleetObservation {
                workspaces: Vec::new(),
                tabs: Vec::new(),
                panes: panes
                    .iter()
                    .map(|pane_id| Pane {
                        location: Location {
                            workspace_id: "wA".into(),
                            tab_id: "wA:t1".into(),
                            pane_id: (*pane_id).into(),
                        },
                        label: None,
                        title: None,
                    })
                    .collect(),
                agents: Vec::new(),
            },
            foreground: self.foreground.clone(),
            closed: Mutex::new(Vec::new()),
            close_answer: self.close_answer.clone(),
            created_requests: Mutex::new(Vec::new()),
            input_requests: Mutex::new(Vec::new()),
            output_requests: Mutex::new(Vec::new()),
            report_requests: Mutex::new(Vec::new()),
            launch_requests: Mutex::new(Vec::new()),
            gate: None,
        })
    }

    /// How many reports this backend has been asked to forward.
    fn reports_seen(&self) -> usize {
        self.report_requests.lock().expect("the report list").len()
    }

    /// What this backend was asked to launch, in order.
    fn launches(&self) -> Vec<LaunchRequest> {
        self.launch_requests
            .lock()
            .expect("the launch list")
            .clone()
    }

    /// What this backend was asked to create, in order.
    fn creates(&self) -> Vec<CreateRequest> {
        self.created_requests
            .lock()
            .expect("the create list")
            .clone()
    }
}

impl RuntimeProvider for MuxBackend {
    fn capabilities(&self) -> &'static [&'static str] {
        self.capabilities
    }
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        Ok(self.inventory.clone())
    }
    fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        self.foreground.clone()
    }
    fn focus(&self, _target: &Target, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the mux backend is never asked to focus")
    }
    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        self.close_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }
    fn close_outcome(&self, target: &CloseTarget, _cancel: &AtomicBool) -> CloseOutcome {
        self.closed
            .lock()
            .expect("the close list")
            .push(target.clone());
        match &self.close_answer {
            CloseAnswer::Completed => CloseOutcome::Completed,
            CloseAnswer::Refused(message) => CloseOutcome::Refused(message.clone()),
            CloseAnswer::Unknown(message) => CloseOutcome::Unknown(message.clone()),
        }
    }
    fn create(&self, request: &CreateRequest, _cancel: &AtomicBool) -> CreateOutcome {
        self.created_requests
            .lock()
            .expect("the create list")
            .push(request.clone());
        if let Some(gate) = &self.gate {
            gate.hold();
        }
        CreateOutcome::Completed(self.created.clone())
    }
    fn input(&self, request: &InputRequest, _cancel: &AtomicBool) -> InputOutcome {
        self.input_requests
            .lock()
            .expect("the input list")
            .push(request.clone());
        InputOutcome::Completed
    }
    fn output(&self, request: &OutputRequest, _cancel: &AtomicBool) -> OutputOutcome {
        self.output_requests
            .lock()
            .expect("the output list")
            .push(request.clone());
        OutputOutcome::Completed(self.output.clone())
    }
    fn report(&self, request: &ReportRequest, _cancel: &AtomicBool) -> ReportOutcome {
        self.report_requests
            .lock()
            .expect("the report list")
            .push(request.clone());
        match &self.report {
            ReportAnswer::Completed => ReportOutcome::Completed,
            ReportAnswer::Refused(message) => ReportOutcome::Refused(message.clone()),
            ReportAnswer::Unknown(message) => ReportOutcome::Unknown(message.clone()),
        }
    }
    fn launch(&self, request: &LaunchRequest, _cancel: &AtomicBool) -> LaunchOutcome {
        self.launch_requests
            .lock()
            .expect("the launch list")
            .push(request.clone());
        match &self.launch {
            LaunchAnswer::Completed => LaunchOutcome::Completed,
            LaunchAnswer::Refused(message) => LaunchOutcome::Refused(message.clone()),
            LaunchAnswer::Unknown(message) => LaunchOutcome::Unknown(message.clone()),
        }
    }
}

/// One state report, as a publisher would send it.
fn state_report(state: &str) -> Value {
    json!({"request": {
        "kind": "state",
        "pane_id": "wA:p1",
        "source": "pi-herdsman",
        "agent": "worker",
        "state": state,
        "message": "2 tasks",
        "sequence": 4,
    }})
}

/// The reporting bridge carries one publisher's own facts. State, session and
/// display metadata reach the backend as the caller gave them, the record keeps
/// exactly those params, and a repeated id reports nothing a second time.
#[test]
fn the_reporting_bridge_is_served_and_recorded() {
    let sandbox = Sandbox::new("report");
    let backend = MuxBackend::new(MUX_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("ping", json!({}));
    assert!(
        ping["result"]["capabilities"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("reporting")),
        "{ping}"
    );

    let state_id = random_uuid();
    let state = client.call_id(&state_id, "report", state_report("working"));
    let record = result_record(&state);
    assert_eq!(record["outcome"], "completed");
    assert_eq!(record["target"], "wA:p1");
    assert_eq!(record["effects"], json!(["reported state working"]));
    // The record keeps what the caller reported — source, sequence and all — and
    // nothing was derived from it.
    assert_eq!(record["params"], state_report("working"));

    let session = client.call(
        "report",
        json!({"request": {
            "kind": "session",
            "pane_id": "wA:p1",
            "source": "pi-herdsman",
            "agent": "worker",
            "session_id": "9f1c",
            "session_path": "/tmp/pi/9f1c.jsonl",
            "session_start_source": "new",
            "sequence": 5,
        }}),
    );
    assert_eq!(result_record(&session)["outcome"], "completed");
    assert_eq!(
        result_record(&session)["effects"],
        json!(["reported session 9f1c"])
    );

    // Display metadata, as the extensions send it: tokens with one withdrawn,
    // the source the display applies to, state labels, a title, a clear flag, a
    // TTL and a sequence.
    let display = json!({"request": {
        "kind": "metadata",
        "target": {"kind": "pane", "pane_id": "wA:p1"},
        "source": "herdsman",
        "tokens": {"summary": "3 tasks", "title-suffix": null},
        "applies_to_source": "herdr:pi",
        "state_labels": {"working": "thinking"},
        "title": "",
        "clear_display_agent": true,
        "ttl_ms": 30000,
        "sequence": 6,
    }});
    let metadata = client.call("report", display.clone());
    assert_eq!(result_record(&metadata)["outcome"], "completed");
    assert_eq!(
        result_record(&metadata)["effects"],
        json!(["reported metadata"])
    );
    assert_eq!(result_record(&metadata)["params"], display);

    let workspace = client.call(
        "report",
        json!({"request": {
            "kind": "metadata",
            "target": {"kind": "workspace", "workspace_id": "wA"},
            "source": "herdsman",
            "tokens": {"role": "lead"},
        }}),
    );
    assert_eq!(result_record(&workspace)["outcome"], "completed");
    assert_eq!(result_record(&workspace)["target"], "wA");

    // What the backend was asked for is the normalized report, decoded: the
    // caller's values, keyed by the seam's own names.
    let seen = backend
        .report_requests
        .lock()
        .expect("the report list")
        .clone();
    assert_eq!(seen.len(), 4, "{seen:?}");
    match &seen[0] {
        ReportRequest::State(report) => {
            assert_eq!(report.source, "pi-herdsman");
            assert_eq!(report.agent, "worker");
            assert_eq!(report.state, ReportedState::Working);
            assert_eq!(report.sequence, Some(4));
        }
        other => panic!("{other:?}"),
    }
    match &seen[1] {
        ReportRequest::Session(report) => {
            assert_eq!(report.session_id.as_deref(), Some("9f1c"));
            assert_eq!(report.session_start_source.as_deref(), Some("new"));
        }
        other => panic!("{other:?}"),
    }
    match &seen[2] {
        ReportRequest::Metadata(report) => {
            assert_eq!(report.tokens["summary"].as_deref(), Some("3 tasks"));
            assert_eq!(report.tokens["title-suffix"], None);
            assert_eq!(report.applies_to_source.as_deref(), Some("herdr:pi"));
            assert_eq!(report.state_labels["working"], "thinking");
            assert!(report.clear_display_agent);
            assert_eq!(report.ttl_ms, Some(30000));
            assert_eq!(report.sequence, Some(6));
        }
        other => panic!("{other:?}"),
    }
    match &seen[3] {
        ReportRequest::Metadata(report) => {
            assert_eq!(report.target.id(), "wA");
            assert_eq!(report.tokens["role"].as_deref(), Some("lead"));
        }
        other => panic!("{other:?}"),
    }

    // A repeated id returns the record it wrote and reports nothing again; the
    // same id with different contents refuses.
    let replay = client.call_id(&state_id, "report", state_report("working"));
    assert_eq!(replay, state);
    assert_eq!(backend.reports_seen(), 4);
    let changed = client.call_id(&state_id, "report", state_report("idle"));
    assert_eq!(error_code(&changed), "refused");
    assert!(
        changed["error"]["message"]
            .as_str()
            .expect("the message")
            .contains("different contents"),
        "{changed}"
    );
    assert_eq!(backend.reports_seen(), 4);

    // A report leaves no record for a read to find: it is not read back here,
    // only the operation that reported it is.
    assert_eq!(
        client.call("request", json!({"id": state_id}))["result"]["request"]["method"],
        "report"
    );
    daemon.stop();
}

/// A backend that does not report refuses it explicitly and never advertises the
/// capability, rather than the daemon substituting another operation.
#[test]
fn a_backend_without_reporting_refuses_it_and_advertises_no_reporting() {
    let sandbox = Sandbox::new("report-unsupported");
    let backend = MuxBackend::new(&["observe"]);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("ping", json!({}));
    assert_eq!(ping["result"]["capabilities"], advertised(&["observe"]));

    let refused = client.call("report", state_report("working"));
    let record = result_record(&refused);
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], Category::BACKEND_UNAVAILABLE);
    assert!(
        record["message"]
            .as_str()
            .expect("the message")
            .contains("`reporting`"),
        "{record}"
    );
    assert_eq!(record["effects"], json!([]));
    assert_eq!(record["state"], "completed");
    assert_eq!(backend.reports_seen(), 0);
    daemon.stop();
}

/// A backend's own refusal is the report's answer: nothing was recorded on the
/// backend, so the record says refused and does not guess at an effect.
#[test]
fn a_backend_report_refusal_is_recorded_as_refused() {
    let sandbox = Sandbox::new("report-refused");
    let backend = MuxBackend::new(MUX_CAPABILITIES)
        .reporting(ReportAnswer::Refused("herdr refused the report".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let refused = client.call("report", state_report("working"));
    assert_eq!(result_record(&refused)["outcome"], "refused");
    assert_eq!(
        result_record(&refused)["category"],
        Category::BACKEND_REFUSED
    );
    assert_eq!(result_record(&refused)["effects"], json!([]));
    assert_eq!(backend.reports_seen(), 1);
    daemon.stop();
}

/// An uncertain report is not re-sent: the record stays unknown, the same id
/// returns it without a second report, and another request over the same pane is
/// refused as in flight because the first one may have landed.
#[test]
fn an_uncertain_report_stays_unknown_and_suppresses_the_target() {
    let sandbox = Sandbox::new("report-unknown");
    let backend = MuxBackend::new(MUX_CAPABILITIES)
        .reporting(ReportAnswer::Unknown("herdr timed out after 5s".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let first = client.call_id(&id, "report", state_report("working"));
    assert_eq!(result_record(&first)["outcome"], "unknown");
    assert_eq!(
        result_record(&first)["category"],
        Category::BACKEND_UNAVAILABLE
    );
    assert_eq!(result_record(&first)["effects"], json!([]));
    assert_eq!(backend.reports_seen(), 1);

    let replay = client.call_id(&id, "report", state_report("working"));
    assert_eq!(replay, first);
    assert_eq!(backend.reports_seen(), 1);

    let conflicting = client.call("report", state_report("idle"));
    assert_eq!(error_code(&conflicting), "refused");
    assert!(
        conflicting["error"]["message"]
            .as_str()
            .expect("the message")
            .contains("in flight"),
        "{conflicting}"
    );
    assert_eq!(backend.reports_seen(), 1);
    daemon.stop();
}

/// What a caller cannot mean is refused at the wire, before anything is recorded
/// or dispatched.
#[test]
fn the_wire_shapes_of_report_are_validated_before_dispatch() {
    let sandbox = Sandbox::new("report-validation");
    let backend = MuxBackend::new(MUX_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let many_tokens: serde_json::Map<String, Value> = (0..17)
        .map(|index| (format!("token-{index}"), json!("value")))
        .collect();
    let long = "x".repeat(MAX_REPORTED_TEXT_BYTES + 1);
    let cases = [
        json!({"request": {"kind": "nowhere", "pane_id": "wA:p1"}}),
        json!({"request": {"kind": "state", "pane_id": "", "source": "s", "agent": "a", "state": "idle"}}),
        json!({"request": {"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "busy"}}),
        json!({"request": {"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle", "resume_argv": ["pi"]}}),
        json!({"request": {"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle", "message": "a\u{1b}[31mred"}}),
        json!({"request": {"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle", "message": long}}),
        json!({"request": {"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": {"not a name": "v"}}}),
        json!({"request": {"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": many_tokens}}),
        json!({"request": {"kind": "metadata", "target": {"kind": "pane", "pane_id": "wA:p1"}, "source": "s", "tokens": {"ok": "v"}, "ttl_ms": 86400001}}),
        json!({"request": {"kind": "metadata", "target": {"kind": "workspace", "workspace_id": "wA"}, "source": "s", "tokens": {}}}),
        json!({"request": {"kind": "metadata", "target": {"kind": "workspace", "workspace_id": "wA"}, "source": "s", "tokens": {"ok": "v"}, "title": "no"}}),
        json!({"request": {"kind": "session", "pane_id": "wA:p1", "source": "s"}}),
        json!({"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle"}),
        json!({"request": {"kind": "state", "pane_id": "wA:p1", "source": "s", "agent": "a", "state": "idle"}, "extra": 1}),
    ];
    for params in cases {
        let response = client.call("report", params.clone());
        assert_eq!(error_code(&response), "bad_params", "{params}: {response}");
    }
    assert_eq!(backend.reports_seen(), 0);
    daemon.stop();
}

/// A populated normalized inventory and evidence, including states this build
/// preserves without naming.
fn populated_graph() -> (FleetObservation, ForegroundEvidence) {
    let location = Location {
        workspace_id: "wA".into(),
        tab_id: "wA:t1".into(),
        pane_id: "wA:p1".into(),
    };
    let inventory = FleetObservation {
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: Some("main".into()),
            number: Some(1),
        }],
        tabs: vec![Tab {
            tab_id: "wA:t1".into(),
            workspace_id: "wA".into(),
            label: None,
            number: Some(2),
        }],
        panes: vec![Pane {
            location: location.clone(),
            label: Some("editor".into()),
            title: Some("nvim".into()),
        }],
        agents: vec![AgentObservation {
            location,
            name: Some("pi".into()),
            label: Some("worker".into()),
            status: Some(RuntimeStatus::Other("dormant".into())),
            session: None,
            lineage: None,
            facts: HerdsmanFacts {
                state: Some(SemanticState::Other("grepping".into())),
                awaited: vec!["agent:peer".into()],
                ..HerdsmanFacts::default()
            },
        }],
    };
    let evidence = ForegroundEvidence::NonShell {
        pid: 200,
        name: Some("nvim".into()),
        command: None,
        local: LocalFacts {
            running_for: Some(Duration::from_millis(1500)),
            terminal: TerminalMode::FullScreen,
            binary: BinaryIdentity {
                freshness: BinaryFreshness::Current,
                running: Some("/nix/store/one".into()),
                installed: Some("/nix/store/two".into()),
                executable: None,
                unknown: None,
            },
            resources: None,
        },
    };
    (inventory, evidence)
}

/// How long a test waits for a line or for a process to go.
const DEADLINE: Duration = Duration::from_secs(10);

struct SocketBlockingVerifier {
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl ProcessVerifier for SocketBlockingVerifier {
    fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
        let _ = self.entered.send(());
        let _ = self.release.lock().expect("verifier release").recv();
        Ok(None)
    }
}

/// A verifier that blocks every call until it is released, counting calls that
/// started and calls that returned. The two counts tell a bounded daemon apart
/// from one that starts work it never account for.
#[derive(Default)]
struct GatedVerifier {
    started: AtomicUsize,
    finished: AtomicUsize,
    open: Mutex<bool>,
    released: Condvar,
}

impl GatedVerifier {
    fn new() -> Self {
        Self::default()
    }

    fn started(&self) -> usize {
        self.started.load(Ordering::SeqCst)
    }

    fn finished(&self) -> usize {
        self.finished.load(Ordering::SeqCst)
    }

    /// Lets every blocked call return from now on.
    fn release_all(&self) {
        *self.open.lock().expect("verifier gate") = true;
        self.released.notify_all();
    }

    /// Blocks newly arriving calls again, so a released verifier can be reused.
    fn block_again(&self) {
        *self.open.lock().expect("verifier gate") = false;
    }
}

struct MatchingVerifier {
    pid: i32,
    identity: ProcessIdentity,
}

impl ProcessVerifier for MatchingVerifier {
    fn inspect(&self, pid: i32) -> Result<Option<ProcessIdentity>, String> {
        (pid == self.pid)
            .then(|| self.identity.clone())
            .map(Some)
            .ok_or_else(|| "wrong pid".into())
    }
}

impl ProcessVerifier for GatedVerifier {
    fn inspect(&self, _pid: i32) -> Result<Option<ProcessIdentity>, String> {
        self.started.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock().expect("verifier gate");
        while !*open {
            open = self.released.wait(open).expect("verifier gate wait");
        }
        self.finished.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

/// Waits, bounded, for a condition a test observes from another thread.
fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + DEADLINE;
    while !ready() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// One `agent.get` on its own connection, with a daemon-ended connection
/// reported as an error rather than failing the read.
fn socket_agent_get(path: &Path, agent_id: &str) -> Value {
    let mut client = Client::dial(path);
    let id = random_uuid();
    client.send(&request_line(
        &id,
        "agent.get",
        json!({"agent_id":agent_id}),
    ));
    let mut line = String::new();
    let bytes = client.reader.read_line(&mut line).unwrap_or(0);
    if bytes == 0 {
        json!({"error":{"code":"connection_closed"}})
    } else {
        serde_json::from_str(&line).expect("daemon response")
    }
}

#[test]
fn socket_verification_does_not_block_publication_and_shutdown_is_bounded() {
    let sandbox = Sandbox::new("agent-verifier");
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (_release_tx, release_rx) = mpsc::sync_channel(1);
    let verifier = Arc::new(SocketBlockingVerifier {
        entered: entered_tx,
        release: Mutex::new(release_rx),
    });
    let mut daemon =
        Daemon::bind_with_backend_and_verifier(&sandbox.socket(), &sandbox.state(), None, verifier)
            .expect("bind with verifier");
    let mut setup = Client::dial(daemon.path());
    let process = ProcessIdentity {
        boot_id: "boot-test".into(),
        pid: 4242,
        start_ticks: 99,
    };
    let registered = setup.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", Some(process), None),
    );
    let target_id = agent_id(&registered);
    let verifier_path = daemon.path().to_path_buf();
    drop(setup);
    let getter = thread::spawn(move || {
        let mut client = Client::dial(&verifier_path);
        let id = random_uuid();
        client.send(&request_line(
            &id,
            "agent.get",
            json!({"agent_id":target_id}),
        ));
        let mut line = String::new();
        let bytes = client.reader.read_line(&mut line).unwrap_or(0);
        if bytes == 0 {
            json!({"error":{"code":"connection_closed"}})
        } else {
            serde_json::from_str(&line).expect("daemon response")
        }
    });
    entered_rx.recv_timeout(DEADLINE).expect("verifier entered");
    let mut writer = Client::dial(daemon.path());
    let written = writer.call(
        "agent.register",
        registration_body("1c2d3e4f-5678-4abc-9def-0123456789ab", None, None),
    );
    assert!(
        written["result"]["registration"]["agent_id"].is_string(),
        "{written}"
    );
    let started = Instant::now();
    daemon.stop();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown waited on verifier"
    );
    let answer = getter.join().unwrap();
    assert_eq!(error_code(&answer), "connection_closed");
}

#[test]
fn socket_verifier_capacity_is_bounded_daemon_wide_and_released_by_completion() {
    let sandbox = Sandbox::new("agent-verifier-cap");
    let verifier = Arc::new(GatedVerifier::new());
    let mut daemon = Daemon::bind_with_backend_and_verifier(
        &sandbox.socket(),
        &sandbox.state(),
        None,
        verifier.clone(),
    )
    .expect("bind with verifier");
    let process = ProcessIdentity {
        boot_id: "boot-test".into(),
        pid: 4242,
        start_ticks: 99,
    };
    let mut setup = Client::dial(daemon.path());
    let registered = setup.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", Some(process), None),
    );
    let target = agent_id(&registered);
    let path = daemon.path().to_path_buf();
    drop(setup);

    // Exactly the daemon-wide bound runs, however many requests arrive.
    let bound = MAX_VERIFICATION_JOBS;
    let getters = |count: usize| {
        (0..count)
            .map(|_| {
                let path = path.clone();
                let target = target.clone();
                thread::spawn(move || socket_agent_get(&path, &target))
            })
            .collect::<Vec<_>>()
    };
    let holders = getters(bound);
    wait_for("the verifier bound to be reached", || {
        verifier.started() == bound
    });

    // A request beyond the bound is answered without starting more work, even
    // while every running call is still blocked.
    for extra in getters(bound) {
        let answer = extra.join().unwrap();
        assert_eq!(
            answer["result"]["agent"]["process"]["verification"], "unavailable",
            "{answer}"
        );
    }
    assert_eq!(
        verifier.started(),
        bound,
        "requests beyond the bound started verifiers"
    );

    // Other registry work stays responsive while every slot is blocked.
    let mut publisher = Client::dial(&path);
    let written = publisher.call(
        "agent.register",
        registration_body("1c2d3e4f-5678-4abc-9def-0123456789ab", None, None),
    );
    assert!(
        written["result"]["registration"]["agent_id"].is_string(),
        "{written}"
    );

    // A verification that times out keeps its slot: the call ran, and nothing
    // it left behind is counted as capacity returned.
    for holder in holders {
        let answer = holder.join().unwrap();
        assert_eq!(
            answer["result"]["agent"]["process"]["verification"], "unavailable",
            "{answer}"
        );
    }
    for late in getters(bound) {
        let answer = late.join().unwrap();
        assert_eq!(
            answer["result"]["agent"]["process"]["verification"], "unavailable",
            "{answer}"
        );
    }
    assert_eq!(
        verifier.started(),
        bound,
        "a timed-out verification released its slot"
    );

    // Completion, and only completion, restores capacity: this verification
    // runs instead of being refused.
    verifier.release_all();
    wait_for("the blocked verifications to return", || {
        verifier.finished() == bound
    });
    let released = socket_agent_get(&path, &target);
    assert_eq!(
        released["result"]["agent"]["process"]["verification"], "absent",
        "{released}"
    );
    assert_eq!(verifier.started(), bound + 1, "capacity was not restored");

    // Shutdown does not wait for a verifier that is still blocked.
    verifier.block_again();
    let blocked = getters(1);
    wait_for("the blocked verification to start", || {
        verifier.started() == bound + 2
    });
    let started_at = Instant::now();
    daemon.stop();
    assert!(
        started_at.elapsed() < Duration::from_secs(1),
        "shutdown waited on a blocked verifier"
    );
    let answer = blocked.into_iter().next().unwrap().join().unwrap();
    assert_eq!(error_code(&answer), "connection_closed");
}

#[test]
fn socket_registration_acquire_publish_get_and_retire_are_private_and_independent() {
    let sandbox = Sandbox::new("agent-registry");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let launch_secret = "/private/SESSION-SECRET.json";
    let registration = registration_body(
        "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
        None,
        Some(
            json!({"executable":"/usr/bin/pi","argv":["--resume",launch_secret],"cwd":"/tmp","session":{"path":launch_secret},"provenance":"test","revision":"r1"}),
        ),
    );
    let registered = client.call("agent.register", registration);
    assert!(
        registered["result"]["registration"]["agent_id"].is_string(),
        "{registered}"
    );
    no_secret(&registered, launch_secret);
    let agent_id = agent_id(&registered);
    let acquired = client.call(
        "agent.acquire",
        json!({
            "agent_id":agent_id,"channel":"execution",
            "publisher":{"source":"child","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"}
        }),
    );
    assert!(
        acquired["result"]["writer"]["handle"].is_string(),
        "{acquired}"
    );
    let handle = acquired["result"]["writer"]["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let published = client.call(
        "agent.publish",
        json!({
            "agent_id":agent_id,"channel":"execution","writer_handle":handle,"sequence":1,
            "snapshot":channel_snapshot("idle")
        }),
    );
    assert_eq!(
        published["result"]["channel"]["snapshot"]["snapshot"]["activity"],
        "idle"
    );
    no_secret(&published, launch_secret);
    let replay = client.call(
        "agent.publish",
        json!({
            "agent_id":agent_id,"channel":"execution","writer_handle":handle,"sequence":1,
            "snapshot":channel_snapshot("idle")
        }),
    );
    assert!(replay["result"].is_object(), "equal replay: {replay}");
    assert_eq!(replay["result"]["channel"]["writer"]["handle"], handle);
    assert_eq!(replay["result"]["channel"]["snapshot"]["sequence"], 1);
    let get = client.call("agent.get", json!({"agent_id":agent_id}));
    assert_eq!(
        get["result"]["agent"]["registration"]["launch"]["available"],
        true
    );
    assert_eq!(
        get["result"]["agent"]["execution"]["snapshot"]["snapshot"]["activity"],
        "idle"
    );
    assert!(get["result"]["agent"]["execution"]["writer"]["handle"].is_null());
    assert!(get["result"]["agent"]["context"].is_null());
    no_secret(&get, launch_secret);
    let list = client.call("agent.list", json!({"limit":1}));
    assert_eq!(list["result"]["agents"].as_array().unwrap().len(), 1);
    no_secret(&list, launch_secret);
    assert!(list["result"]["agents"][0]["execution"]["writer"]["handle"].is_null());
    assert!(list["result"]["agents"][0]["context"].is_null());
    let retired = client.call(
        "agent.retire",
        json!({"agent_id":agent_id,"channel":"execution","writer_handle":handle}),
    );
    assert!(retired["result"].is_object(), "{retired}");
    let fenced = client.call(
        "agent.publish",
        json!({
            "agent_id":agent_id,"channel":"execution","writer_handle":handle,"sequence":2,
            "snapshot":channel_snapshot("working")
        }),
    );
    assert_eq!(error_code(&fenced), "refused");
    daemon.stop();
}

#[test]
fn agent_context_socket_publish_replace_replay_freshness_and_privacy() {
    const SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    const NEXT_SESSION: &str = "e7f8a9b0-c1d2-4e3f-8a4b-5c6d7e8f9a0b";
    let sandbox = Sandbox::new("agent-context");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let launch_secret = "/private/SESSION-CONTEXT-SECRET.json";
    let registered = client.call(
        "agent.register",
        registration_body(
            "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
            None,
            Some(json!({"executable":"/usr/bin/pi","argv":[launch_secret],"cwd":"/tmp","session":{"path":launch_secret},"provenance":"test","revision":"r1"})),
        ),
    );
    let id = agent_id(&registered);
    let absent = client.call("agent.get", json!({"agent_id":id}));
    assert!(absent["result"]["agent"]["context"].is_null(), "{absent}");
    let list_absent = client.call("agent.list", json!({"limit":1}));
    assert!(
        list_absent["result"]["agents"][0]["context"].is_null(),
        "{list_absent}"
    );

    // First report implicitly binds a generation-1 writer. The live response is
    // a credential-free projection and retains explicit null distinctly from
    // never-published read absence.
    let first = client.call(
        "agent.context",
        json!({"agent_id":id,"publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},"sequence":1,"lease_ms":10000,"context":{"session":SESSION}}),
    );
    assert!(first["result"]["context"].is_object(), "{first}");
    assert_eq!(first["result"]["context"]["context"]["session"], SESSION);
    assert_eq!(first["result"]["context"]["writer"]["generation"], 1);
    assert!(first["result"]["writer"]["handle"].is_string());
    assert!(first["result"]["warning"].is_null());
    no_secret(&first, launch_secret);

    let writer = first["result"]["writer"]["handle"]
        .as_str()
        .expect("first writer handle")
        .to_owned();

    let switched = client.call(
        "agent.context",
        json!({
            "agent_id":id,
            "publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},
            "writer_handle":writer,
            "sequence":2,
            "context":{"session":NEXT_SESSION}
        }),
    );
    assert_eq!(
        switched["result"]["context"]["context"]["session"],
        NEXT_SESSION
    );
    assert_eq!(switched["result"]["context"]["writer"]["generation"], 1);

    let replay = client.call(
        "agent.context",
        json!({
            "agent_id":id,
            "publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},
            "writer_handle":writer,
            "sequence":2,
            "context":{"session":NEXT_SESSION}
        }),
    );
    assert!(replay["result"]["warning"].as_str().is_some(), "{replay}");
    assert!(
        replay["result"]["warning"]
            .as_str()
            .unwrap()
            .contains("not refreshed")
    );

    // Explicit null means no current session; unlike an absent record it is an
    // object with `session: null`. The registration remains unchanged.
    let cleared = client.call(
        "agent.context",
        json!({
            "agent_id":id,
            "publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},
            "writer_handle":writer,
            "sequence":3,
            "context":{"session":null}
        }),
    );
    assert!(
        cleared["result"]["context"]["context"]["session"].is_null(),
        "{cleared}"
    );
    let get = client.call("agent.get", json!({"agent_id":id}));
    assert!(get["result"]["agent"]["context"].is_object(), "{get}");
    assert!(get["result"]["agent"]["context"]["context"]["session"].is_null());
    assert!(get["result"]["agent"]["context"]["context"]["freshness"] == "fresh");
    assert!(get["result"]["agent"]["context"]["writer"]["handle"].is_null());
    let list = client.call("agent.list", json!({"limit":1}));
    assert!(list["result"]["agents"][0]["context"].is_object(), "{list}");
    assert!(list["result"]["agents"][0]["context"]["context"]["session"].is_null());
    assert!(list["result"]["agents"][0]["context"]["writer"]["handle"].is_null());
    no_secret(&get, launch_secret);
    no_secret(&list, launch_secret);
    for response in [&get, &list, &cleared["result"]["context"]] {
        assert!(
            !response.to_string().contains(&writer),
            "writer handle leaked: {response}"
        );
        assert!(
            !response.to_string().contains("serving_epoch"),
            "{response}"
        );
    }

    // Restarting changes the serving epoch, making the restored context stale
    // while preserving the last explicit null.
    daemon.stop();
    drop(client);
    let mut daemon =
        Daemon::bind_without_backend(&sandbox.socket(), &sandbox.state()).expect("restart daemon");
    let mut client = Client::dial(daemon.path());
    let stale = client.call("agent.get", json!({"agent_id":id}));
    assert_eq!(
        stale["result"]["agent"]["context"]["context"]["freshness"],
        "stale"
    );
    assert!(
        stale["result"]["agent"]["context"]["context"]["restored"]
            .as_bool()
            .unwrap()
    );
    assert!(stale["result"]["agent"]["context"]["context"]["session"].is_null());
    no_secret(&stale, launch_secret);
    daemon.stop();
}

#[test]
fn agent_context_socket_expiry_and_replacement_keep_writer_fenced() {
    let sandbox = Sandbox::new("agent-context-replace");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let registered = client.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None, None),
    );
    let id = agent_id(&registered);
    let first = client.call(
        "agent.context",
        json!({
            "agent_id":id,
            "publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},
            "sequence":1,
            "lease_ms":1000,
            "context":{"session":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"}
        }),
    );
    assert!(first["result"]["context"].is_object(), "{first}");
    // Wait only for the deliberately minimum-length lease. Reads retain the
    // report but mark it stale; an explicit observed-generation replacement
    // advances the writer generation and fences the previous handle.
    thread::sleep(Duration::from_millis(1100));
    let list_stale = client.call("agent.list", json!({"limit":1}));
    assert_eq!(
        list_stale["result"]["agents"][0]["context"]["context"]["freshness"],
        "stale"
    );
    let stale = client.call("agent.get", json!({"agent_id":id}));
    assert_eq!(
        stale["result"]["agent"]["context"]["context"]["freshness"],
        "stale"
    );
    let old = first["result"]["writer"].clone();
    let replacement = client.call(
        "agent.context",
        json!({
            "agent_id":id,
            "publisher":{"source":"herdsman","incarnation":"4c2d3e4f-5678-4abc-9def-0123456789ab"},
            "replace":{"generation":old["generation"],"handle":old["handle"]},
            "sequence":1,
            "context":{"session":"e7f8a9b0-c1d2-4e3f-8a4b-5c6d7e8f9a0b"}
        }),
    );
    assert_eq!(replacement["result"]["writer"]["generation"], 2);
    assert_ne!(replacement["result"]["writer"]["handle"], old["handle"]);
    let fenced = client.call(
        "agent.context",
        json!({
            "agent_id":id,
            "publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},
            "writer_handle":old["handle"],
            "sequence":2,
            "context":{"session":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"}
        }),
    );
    assert_eq!(error_code(&fenced), "refused");
    let accepted = client.call("agent.get", json!({"agent_id":id}));
    assert_eq!(
        accepted["result"]["agent"]["context"]["writer"]["generation"],
        2
    );
    assert_eq!(
        accepted["result"]["agent"]["context"]["context"]["session"],
        "e7f8a9b0-c1d2-4e3f-8a4b-5c6d7e8f9a0b"
    );
    daemon.stop();
}

#[test]
fn socket_registry_replace_restart_and_reconnect_keep_writer_provenance() {
    let sandbox = Sandbox::new("agent-restart");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let registered = client.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None, None),
    );
    let id = agent_id(&registered);
    let first = client.call("agent.acquire", json!({"agent_id":id,"channel":"assignment","publisher":{"source":"owner","incarnation":"3c2d3e4f-5678-4abc-9def-0123456789ab","reporting_owner":"owner-A"}}));
    let old = first["result"]["writer"]["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let report = client.call("agent.publish", json!({"agent_id":id,"channel":"assignment","writer_handle":old,"sequence":1,"snapshot":channel_snapshot("unsettled")}));
    assert!(report["result"].is_object(), "{report}");
    let old_generation = first["result"]["writer"]["generation"].as_u64().unwrap();
    let retired = client.call(
        "agent.retire",
        json!({"agent_id":id,"channel":"assignment","writer_handle":old}),
    );
    assert!(retired["result"].is_object(), "{retired}");
    daemon.stop();
    drop(client);
    let mut daemon =
        Daemon::bind_without_backend(&sandbox.socket(), &sandbox.state()).expect("restart daemon");
    let mut client = Client::dial(daemon.path());
    let restored = client.call("agent.get", json!({"agent_id":id}));
    assert_eq!(
        restored["result"]["agent"]["assignment"]["snapshot"]["freshness"],
        "stale"
    );
    let replacement = client.call("agent.acquire", json!({
        "agent_id":id,"channel":"assignment",
        "publisher":{"source":"owner","incarnation":"4c2d3e4f-5678-4abc-9def-0123456789ab","reporting_owner":"owner-A"},
        "replace":{"generation":old_generation,"handle":old}
    }));
    let new_handle = replacement["result"]["writer"]["handle"]
        .as_str()
        .expect("replacement handle")
        .to_owned();
    assert_ne!(new_handle, old);
    assert_eq!(
        replacement["result"]["writer"]["generation"],
        old_generation + 1
    );
    let stale = client.call("agent.publish", json!({"agent_id":id,"channel":"assignment","writer_handle":old,"sequence":2,"snapshot":channel_snapshot("stale-writer")}));
    assert_eq!(error_code(&stale), "refused");
    let reconnect = client.call("agent.publish", json!({"agent_id":id,"channel":"assignment","writer_handle":new_handle,"sequence":1,"snapshot":channel_snapshot("owner-reconnected")}));
    assert!(reconnect["result"].is_object(), "{reconnect}");
    let accepted = client.call("agent.get", json!({"agent_id":id}));
    assert_eq!(
        accepted["result"]["agent"]["assignment"]["snapshot"]["snapshot"]["activity"],
        "owner-reconnected"
    );
    daemon.stop();
}

#[test]
fn registry_socket_rejects_unknown_fields_and_surfaces_corrupt_records() {
    let sandbox = Sandbox::new("agent-corrupt");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let bad_shape = client.call("agent.register", json!({"source":"test","incarnation":"8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22","unexpected":true}));
    assert_eq!(error_code(&bad_shape), "bad_params");
    let registered = client.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None, None),
    );
    let id = agent_id(&registered);
    daemon.stop();
    fs::write(
        sandbox.state().join("agents").join(format!("{id}.json")),
        b"not json",
    )
    .expect("corrupt record");
    let mut daemon = Daemon::bind_without_backend(&sandbox.socket(), &sandbox.state())
        .expect("bind corrupt store for explicit read failure");
    let mut client = Client::dial(daemon.path());
    let get = client.call("agent.get", json!({"agent_id":id}));
    assert_eq!(error_code(&get), "refused");
    let list = client.call("agent.list", json!({}));
    assert_eq!(error_code(&list), "refused");
    daemon.stop();
}

#[test]
fn registry_list_pages_are_cursor_stable_and_response_byte_bounded() {
    let sandbox = Sandbox::new("agent-pages");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    for n in 0..8 {
        let incarnation = format!("00000000-0000-4000-8000-{n:012x}");
        let mut request = registration_body(&incarnation, None, None);
        // Quotes and backslashes escape, so the response carries the escaped
        // text plus JSON framing, not the stored bytes.
        request["label"] = json!("\\\"".repeat(MAX_TEXT_BYTES / 2));
        let response = client.call("agent.register", request);
        assert!(
            response["result"]["registration"]["agent_id"].is_string(),
            "{response}"
        );
    }
    let mut cursor: Option<String> = None;
    let mut observed = Vec::new();
    loop {
        let response = client.call("agent.list", json!({"limit":3,"after":cursor}));
        // The whole line a client reads, envelope included, is what must fit.
        let line = format!("{response}\n");
        assert!(
            line.len() < MAX_LINE_BYTES,
            "oversized page response: {} bytes",
            line.len()
        );
        assert!(response["result"]["agents"].as_array().unwrap().len() <= 3);
        for entry in response["result"]["agents"].as_array().unwrap() {
            observed.push(
                entry["registration"]["agent_id"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        match response["result"]["next"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
    }
    let mut sorted = observed.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(observed, sorted);
    assert_eq!(observed.len(), 8);
    daemon.stop();
}

/// A private directory for one test's socket and state, removed with the test.
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "radar-control-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("mode 0700");
        Self { root: path }
    }

    fn socket(&self) -> PathBuf {
        self.root.join("control.sock")
    }

    fn state(&self) -> PathBuf {
        self.root.join("state")
    }

    fn daemon(&self) -> Daemon {
        Daemon::bind_without_backend(&self.socket(), &self.state())
            .expect("bind the control socket")
    }

    fn daemon_with(&self, backend: Arc<dyn RuntimeProvider>) -> Daemon {
        Daemon::bind_with_backend(&self.socket(), &self.state(), Some(backend))
            .expect("bind the control socket with backend")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// One connection, one line at a time.
struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    fn dial(path: &Path) -> Self {
        let stream = UnixStream::connect(path).expect("dial the control socket");
        stream
            .set_read_timeout(Some(DEADLINE))
            .expect("a read deadline");
        let writer = stream.try_clone().expect("a writer");
        Self {
            reader: BufReader::new(stream),
            writer,
        }
    }

    fn send(&mut self, line: &str) {
        self.writer
            .write_all(line.as_bytes())
            .and_then(|()| self.writer.write_all(b"\n"))
            .and_then(|()| self.writer.flush())
            .expect("write a request");
    }

    /// Reads one response. A connection the daemon ended fails here rather than
    /// waiting out the deadline, so an ended connection is reported as itself.
    fn read(&mut self) -> Value {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line).expect("read a response");
        assert!(
            read > 0,
            "the daemon ended the connection instead of answering"
        );
        serde_json::from_str(&line).unwrap_or_else(|error| panic!("{line:?}: {error}"))
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let id = random_uuid();
        self.call_id(&id, method, params)
    }

    fn call_id(&mut self, id: &str, method: &str, params: Value) -> Value {
        self.send(&request_line(id, method, params));
        self.read()
    }
}

fn request_line(id: &str, method: &str, params: Value) -> String {
    json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string()
}

/// A freeze of one empty pane: the exact close request a client sends, so older
/// no-backend tests stay about the daemon's records rather than the wire shape.
fn frozen_pane(pane_id: &str) -> Value {
    json!({
        "request": {
            "target": {"pane": pane_id},
            "identity": {"pane": {"pane_id": pane_id, "occupant": null}},
        }
    })
}

fn error_code(response: &Value) -> &str {
    response["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("no error code in {response}"))
}

fn no_secret(response: &Value, secret: &str) {
    assert!(
        !response.to_string().contains(secret),
        "private launch secret leaked: {response}"
    );
}

fn registration_body(
    incarnation: &str,
    process: Option<ProcessIdentity>,
    launch: Option<Value>,
) -> Value {
    let mut body =
        json!({"source":"test-publisher", "incarnation":incarnation, "label":"socket-agent"});
    if let Some(process) = process {
        body["process"] = serde_json::to_value(process).unwrap();
    }
    if let Some(launch) = launch {
        body["launch"] = launch;
    }
    body
}

/// Two canonical incarnations: a registration names its own incarnation as a
/// UUID, and a spawn names the record the daemon issued for it.
const PARENT_INCARNATION: &str = "3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b22";
const OTHER_INCARNATION: &str = "5b2e8c41-0a7d-4f36-b1d9-6c3a7e5f8b1d";

/// Registers one runtime subject that names where it lives, so a spawn can be
/// asked for under the pane it registered.
fn registered_parent(client: &mut Client, incarnation: &str, pane: &str) -> String {
    let mut body = registration_body(incarnation, None, None);
    body["location"] = json!({
        "backend": "herdr",
        "workspace": "wA",
        "tab": "wA:t1",
        "pane": pane,
    });
    let response = client.call("agent.register", body);
    agent_id(&response)
}

/// The spawn request one caller sends: a parent subject and the command it
/// resolved for the child.
fn spawn_params(parent: &str, executable: &str, argv: &[&str]) -> Value {
    json!({
        "request": {
            "parent": parent,
            "executable": executable,
            "argv": argv,
        },
        "requester": "test",
    })
}

/// The incarnation of a child process that registers with the token its launch
/// carried.
const CHILD_INCARNATION: &str = "9d8c7b6a-5f4e-4d3c-9b2a-1809f7e6d5c4";

/// Three request ids whose order is the durable key's, not the order they were
/// spawned in.
const FIRST_EDGE: &str = "11111111-1111-4111-8111-111111111111";
const SECOND_EDGE: &str = "22222222-2222-4222-8222-222222222222";
const THIRD_EDGE: &str = "33333333-3333-4333-8333-333333333333";

/// Registers a child that presents the token its launch carried, which is what
/// the launched process does when it starts.
fn child_close_params(spawn_request_id: &str, source: &str, incarnation: &str) -> Value {
    json!({
        "spawn_request_id": spawn_request_id,
        "source": source,
        "incarnation": incarnation,
        "intent": "cancel",
    })
}

fn register_close_child(client: &mut Client, token: &str, process: ProcessIdentity) -> Value {
    let mut body = registration_body(CHILD_INCARNATION, Some(process), None);
    body["spawn_token"] = json!(token);
    client.call("agent.register", body)
}

fn closes_directory(state: &Path) -> PathBuf {
    state.join("closes")
}

/// One recorded close, read back as the daemon wrote it.
fn close_record(state: &Path, close_id: &str) -> Value {
    let path = closes_directory(state).join(format!("{close_id}.json"));
    serde_json::from_str(&fs::read_to_string(path).expect("a close record")).expect("a record")
}

fn registering_with_token(client: &mut Client, incarnation: &str, token: &str) -> Value {
    let mut body = registration_body(incarnation, None, None);
    body["spawn_token"] = json!(token);
    client.call("agent.register", body)
}

/// Spawns one child of `parent` under an exact request id.
fn spawned_edge(client: &mut Client, id: &str, parent: &str) -> Value {
    let response = client.call_id(
        id,
        "spawn",
        spawn_params(parent, "/usr/bin/pi", &["--child"]),
    );
    assert_eq!(
        result_record(&response)["outcome"],
        "completed",
        "{response}"
    );
    response
}

/// The request ids one `spawn.list` page carries, in the order they arrived.
fn request_ids(response: &Value) -> Vec<String> {
    response["result"]["spawns"]
        .as_array()
        .expect("a page")
        .iter()
        .map(|edge| {
            edge["request_id"]
                .as_str()
                .expect("a public request id")
                .to_owned()
        })
        .collect()
}

/// One recorded spawn edge, as the daemon wrote it: the private token included,
/// because a test needs to know the secret that must not travel.
fn recorded_edge(state: &Path, request_id: &str) -> Value {
    serde_json::from_slice(&fs::read(edge_path(state, request_id)).expect("a recorded edge"))
        .expect("a spawn edge")
}

fn edge_path(state: &Path, request_id: &str) -> PathBuf {
    state.join("spawns").join(format!("{request_id}.json"))
}

fn edges_written(state: &Path) -> usize {
    match fs::read_dir(state.join("spawns")) {
        Ok(entries) => entries.count(),
        Err(_) => 0,
    }
}

fn channel_snapshot(activity: &str) -> Value {
    json!({"activity":activity,"waiting_reason":"waiting-for-owner","last_outcome":{"result":"failed-tool-timeout"},"actions":["retry"]})
}

fn agent_id(response: &Value) -> String {
    response["result"]["registration"]["agent_id"]
        .as_str()
        .expect("public agent id")
        .to_owned()
}

fn result_record(response: &Value) -> &Value {
    &response["result"]["request"]
}

/// The daemon answers liveness with what it is, so a client can tell a daemon
/// with no backend from a daemon of another version before asking it anything.
#[test]
fn ping_answers_the_version_backend_and_registry_capability_without_mux() {
    let sandbox = Sandbox::new("ping");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let response = client.call("ping", json!({}));
    assert_eq!(response["result"]["protocol"], PROTOCOL_VERSION);
    assert_eq!(response["result"]["backend"], "none");
    assert_eq!(response["result"]["capabilities"], advertised(&[]));
    let unsupported = client.call(
        "child.close",
        json!({
            "spawn_request_id": FIRST_EDGE,
            "source": "herdsman",
            "incarnation": CHILD_INCARNATION,
            "intent": "cancel",
        }),
    );
    assert_eq!(error_code(&unsupported), "refused");
    assert!(
        unsupported["error"]["message"]
            .as_str()
            .expect("a reason")
            .contains("no recorded edge")
    );
    assert!(
        !sandbox
            .state()
            .join("closes")
            .join(format!("{FIRST_EDGE}.json"))
            .exists(),
        "an unsupported managed close creates no accepted close record"
    );

    daemon.stop();
}

#[test]
fn real_socket_serves_normalized_observation_and_focus_once() {
    let sandbox = Sandbox::new("fake-backend");
    let backend = Arc::new(FakeBackend::new(
        serde_json::from_value(json!({"workspaces":[{"workspace_id":"wA","label":"main","number":1}],"tabs":[{"tab_id":"wA:t1","workspace_id":"wA","label":null,"number":1}],"panes":[{"location":{"workspace_id":"wA","tab_id":"wA:t1","pane_id":"wA:p1"},"label":null,"title":null}],"agents":[]})).expect("normalized fixture"),
        FocusAnswer::Completed,
    ));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("ping", json!({}));
    assert_eq!(ping["result"]["backend"], "runtime");
    assert_eq!(
        ping["result"]["capabilities"],
        advertised(&["observe", "process_info", "focus"])
    );
    let observed = client.call("observe", json!({}));
    assert_eq!(
        observed["result"]["inventory"]["workspaces"][0]["workspace_id"],
        "wA"
    );
    assert!(observed["result"]["inventory"].get("raw").is_none());
    let process = client.call("process_info", json!({"pane_id":"wA:p1"}));
    assert_eq!(process["result"]["evidence"]["kind"], "non_shell");
    assert_eq!(process["result"]["evidence"]["pid"], 200);

    let id = random_uuid();
    let request = json!({"target":"wA:p1","target_kind":"pane"});
    let first = client.call_id(&id, "focus", request.clone());
    assert!(first.get("result").is_some(), "{first}");
    assert_eq!(result_record(&first)["outcome"], "completed");
    let retry = client.call_id(&id, "focus", request.clone());
    assert_eq!(result_record(&retry)["id"], id);
    let changed = client.call_id(
        &id,
        "focus",
        json!({"target":"wA:p1","target_kind":"workspace"}),
    );
    assert_eq!(error_code(&changed), "refused");
    assert_eq!(
        backend.focused.lock().unwrap().as_slice(),
        &[Target::Pane("wA:p1".into())]
    );
    daemon.stop();
}

#[test]
fn an_explicit_backend_focus_refusal_is_recorded_as_refused() {
    let sandbox = Sandbox::new("focus-refused");
    let backend = Arc::new(FakeBackend::new(
        serde_json::from_value(json!({"workspaces":[],"tabs":[],"panes":[],"agents":[]}))
            .expect("empty normalized fixture"),
        FocusAnswer::Refused("target refused".into()),
    ));
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());
    let response = client.call("focus", json!({"target":"pane:1","target_kind":"pane"}));
    assert_eq!(result_record(&response)["outcome"], "refused");
    assert_eq!(
        result_record(&response)["category"],
        Category::BACKEND_REFUSED
    );
    daemon.stop();
}

#[test]
fn a_foreign_version_is_refused_and_the_connection_keeps_serving() {
    let sandbox = Sandbox::new("version");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let id = random_uuid();

    client.send(&json!({"version": 7, "id": id, "method": "ping", "params": {}}).to_string());
    let refused = client.read();
    assert_eq!(refused["id"], id);
    assert_eq!(error_code(&refused), "bad_version");
    assert!(
        refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("7"),
        "{refused}"
    );

    // A version that is missing rather than wrong is a malformed request, and
    // says so without pretending to know which version was meant.
    client.send(&json!({"id": id, "method": "ping"}).to_string());
    assert_eq!(error_code(&client.read()), "bad_request");

    // Neither refusal ended the connection.
    assert_eq!(client.call("ping", json!({}))["result"]["protocol"], 1);
    daemon.stop();
}

#[test]
fn an_unknown_method_is_refused_and_the_connection_keeps_serving() {
    let sandbox = Sandbox::new("unknown");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let refused = client.call("close_everything", json!({}));
    assert_eq!(error_code(&refused), "unknown_method");
    assert!(
        refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("close_everything")
    );
    assert_eq!(client.call("ping", json!({}))["result"]["protocol"], 1);
    daemon.stop();
}

/// A line the daemon cannot read is answered and forgotten: the connection is
/// still at a line boundary, so the client can correct itself on the same socket.
#[test]
fn malformed_and_oversized_lines_are_refused_without_ending_the_connection() {
    let sandbox = Sandbox::new("malformed");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());

    client.send("{this is not json");
    assert_eq!(error_code(&client.read()), "bad_request");

    client.send("");
    assert_eq!(error_code(&client.read()), "bad_request");

    // One byte over the cap: drained, refused, and the connection survives.
    let oversized = "x".repeat(MAX_LINE_BYTES + 1);
    client.send(&oversized);
    let refused = client.read();
    assert_eq!(error_code(&refused), "bad_request");
    assert!(
        refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains(&MAX_LINE_BYTES.to_string()),
        "{refused}"
    );

    assert_eq!(client.call("ping", json!({}))["result"]["protocol"], 1);
    daemon.stop();
}

/// The record is the point of the daemon: it is written before anything runs, it
/// answers through the protocol, and it outlives the daemon that wrote it.
#[test]
fn an_operation_is_recorded_before_it_is_refused_and_outlives_the_daemon() {
    let sandbox = Sandbox::new("recorded");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let mut params = frozen_pane("pane:3");
    params["requester"] = json!("a test");
    let response = client.call_id(&id, "close", params);
    let record = result_record(&response);
    assert_eq!(record["id"], id);
    assert_eq!(record["method"], "close");
    assert_eq!(record["requester"], "a test");
    assert_eq!(record["target"], "pane:3");
    assert_eq!(record["state"], "completed");
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], Category::BACKEND_UNAVAILABLE);
    assert!(
        record["message"]
            .as_str()
            .expect("a message")
            .contains("`close`"),
        "{record}"
    );
    // Nothing ran, and the record is the account of that.
    assert_eq!(record["effects"], json!([]));
    assert!(record["requested_at"].is_string());
    assert!(record["expires_at"].is_string());
    assert!(record["completed_at"].is_string());

    // Readable through the protocol, by id.
    let fetched = client.call("request", json!({"id": id}));
    assert_eq!(result_record(&fetched)["id"], id);
    let missing = client.call("request", json!({"id": random_uuid()}));
    assert_eq!(error_code(&missing), "not_found");
    let unnamed = client.call("request", json!({}));
    assert_eq!(error_code(&unnamed), "bad_params");

    // Readable on disk by a store that was not the daemon's...
    let store = Store::open(&sandbox.state()).expect("open the state");
    assert_eq!(
        store.get(&id).expect("a read").expect("the record").outcome,
        Some(RequestOutcome::Refused)
    );

    // ...and by a second daemon over the same state root, which is what a restart
    // looks like from a client's side.
    daemon.stop();
    let mut reopened = sandbox.daemon();
    let mut client = Client::dial(reopened.path());
    let fetched = client.call("request", json!({"id": id}));
    assert_eq!(result_record(&fetched)["outcome"], "refused");
    assert_eq!(
        result_record(&fetched)["completed_at"],
        record["completed_at"]
    );
    reopened.stop();
}

/// Reusing a protocol request id cannot overwrite the record or run a second
/// operation, even when its method and target differ.
#[test]
fn a_request_id_is_at_most_once_across_methods() {
    let sandbox = Sandbox::new("same-id-protocol");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let id = random_uuid();
    let first = client.call_id(&id, "close", frozen_pane("pane:3"));
    assert_eq!(result_record(&first)["method"], "close");
    let same = client.call_id(&id, "close", frozen_pane("pane:3"));
    assert_eq!(result_record(&same)["method"], "close");
    assert_eq!(result_record(&same)["params"], frozen_pane("pane:3"));
    let changed = client.call_id(
        &id,
        "focus",
        json!({"target": "pane:4", "target_kind":"pane"}),
    );
    assert_eq!(error_code(&changed), "refused");
    let stored = client.call("request", json!({"id": id}));
    assert_eq!(result_record(&stored)["method"], "close");
    assert_eq!(result_record(&stored)["target"], "pane:3");
    // Same id, same target, different contents: the occupant differs, so the
    // stored request does not describe this one.
    let mut changed_contents = frozen_pane("pane:3");
    changed_contents["request"]["identity"]["pane"]["occupant"] =
        json!({"name": "pi", "managed": false});
    let reordered = client.call_id(&id, "close", changed_contents);
    assert_eq!(error_code(&reordered), "refused");
    daemon.stop();
}

/// A target with a request already unresolved refuses the next one, and both
/// records are readable: a refusal is durable too.
#[test]
fn a_second_request_for_an_unresolved_target_is_refused_as_in_flight() {
    let sandbox = Sandbox::new("in-flight");
    let mut daemon = sandbox.daemon();
    let state = Store::open(&sandbox.state()).expect("open the state");
    // A first request over a target, recorded and not yet claimed — which is what
    // an operation waiting for its backend looks like from the store's side.
    let first = random_uuid();
    let held = RequestRecord::new(
        &first,
        "close",
        Some("radar".into()),
        Some("pane:9".into()),
        now_ms(),
    );
    assert_eq!(held.state, RecordState::Pending);
    assert_eq!(held.derive(now_ms()), Derived::Pending);
    state.create(&held).expect("create the first request");
    state.write(&held).expect("write the first request");

    let mut client = Client::dial(daemon.path());
    let second = random_uuid();
    let refused = client.call_id(
        &second,
        "focus",
        json!({"target": "pane:9", "target_kind":"pane"}),
    );
    assert_eq!(error_code(&refused), "refused");
    assert!(
        refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains(&first),
        "{refused}"
    );

    // Both records are readable, and the second says why it was refused.
    let held_record = client.call("request", json!({"id": first}));
    assert_eq!(result_record(&held_record)["state"], "pending");
    let refusal = client.call("request", json!({"id": second}));
    assert_eq!(result_record(&refusal)["outcome"], "refused");
    assert_eq!(result_record(&refusal)["category"], Category::IN_FLIGHT);

    // Another target is a different request, and is executed normally.
    let other = client.call_id(&random_uuid(), "close", frozen_pane("pane:10"));
    assert_eq!(
        result_record(&other)["category"],
        Category::BACKEND_UNAVAILABLE
    );
    daemon.stop();
}

/// The two mux reads are not operations: nothing is executed for them, so
/// nothing is recorded, and the caller is told which part of the backend is
/// missing rather than given an empty observation it might believe.
#[test]
fn the_mux_reads_are_errors_and_leave_no_record() {
    let sandbox = Sandbox::new("reads");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    for (method, params) in [
        ("observe", json!({})),
        ("process_info", json!({"pane_id":"pane:1"})),
    ] {
        let refused = client.call(method, params);
        assert_eq!(error_code(&refused), "backend_unavailable");
        assert!(
            refused["error"]["message"]
                .as_str()
                .expect("a message")
                .contains(method),
            "{refused}"
        );
    }
    let listed = client.call("requests", json!({}));
    assert_eq!(listed["result"]["requests"], json!([]));
    daemon.stop();
}

/// A registry and a resume are deliberately unsupported by this wrapper: a
/// request for lifecycle authority refuses as an unknown method rather than
/// taking a record, and the record list stays empty.
#[test]
fn registry_and_resume_are_explicitly_unsupported() {
    let sandbox = Sandbox::new("reports");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());

    let listed = client.call("registry", json!({}));
    assert_eq!(error_code(&listed), "unknown_method");
    let resume = client.call("resume", json!({}));
    assert_eq!(error_code(&resume), "unknown_method");
    let requests = client.call("requests", json!({}));
    assert_eq!(requests["result"]["requests"], json!([]));
    daemon.stop();
}

#[test]
fn requests_lists_the_newest_first_and_bounded() {
    let sandbox = Sandbox::new("listing");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let older = random_uuid();
    client.call_id(&older, "close", frozen_pane("pane:1"));
    // A later record: the clock has to move for the order to be observable, and
    // a millisecond is the record's resolution.
    thread::sleep(Duration::from_millis(5));
    let newer = random_uuid();
    client.call_id(
        &newer,
        "input",
        json!({"request": {"pane_id": "pane:2", "payload": {"kind": "text", "text": "x"}}}),
    );

    let listed = client.call("requests", json!({}));
    let records = listed["result"]["requests"].as_array().expect("a list");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["id"], newer);
    assert_eq!(records[1]["id"], older);

    let bounded = client.call("requests", json!({"limit": 1}));
    let records = bounded["result"]["requests"].as_array().expect("a list");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["id"], newer);

    let refused = client.call("requests", json!({"limit": "all of them"}));
    assert_eq!(error_code(&refused), "bad_params");
    daemon.stop();
}

#[test]
fn corrupt_or_unsafe_record_files_fail_closed_for_reads_and_suppression() {
    let sandbox = Sandbox::new("corrupt-record");
    let store = Store::open(&sandbox.state()).expect("a store");
    let id = random_uuid();
    let path = store.directory().join(format!("{id}.json"));

    fs::write(&path, b"{not-json").expect("malformed record");
    assert!(
        store
            .get(&id)
            .expect_err("malformed get refused")
            .contains("not a record")
    );
    assert!(
        store
            .recent(50)
            .expect_err("malformed list refused")
            .contains("not a record")
    );
    assert!(
        store
            .unresolved(Some("pane:1"), now_ms())
            .expect_err("suppression fails closed")
            .contains("not a record")
    );

    fs::write(&path, vec![b'x'; 65 * 1024]).expect("oversized record");
    assert!(
        store
            .recent(50)
            .expect_err("oversized list refused")
            .contains("exceeds")
    );

    fs::remove_file(&path).expect("remove oversized record");
    let outside = sandbox.root.join("outside");
    fs::write(&outside, b"{}").expect("outside record");
    std::os::unix::fs::symlink(&outside, &path).expect("symlink record");
    assert!(
        store
            .get(&id)
            .expect_err("symlink get refused")
            .contains("regular file")
    );
    assert!(
        store
            .unresolved(Some("pane:1"), now_ms())
            .expect_err("symlink blocks suppression")
            .contains("regular file")
    );
}

/// A socket file a dead daemon left behind is cleared; a socket a live daemon
/// answers on is reported, because a second listener on one path would mean two
/// daemons writing one store.
#[test]
fn a_stale_socket_file_is_cleared_and_a_live_one_is_reported() {
    let sandbox = Sandbox::new("stale");
    {
        // Dropping the listener closes the fd and leaves the file: exactly what a
        // killed daemon leaves.
        let _left = UnixListener::bind(sandbox.socket()).expect("a left socket");
    }
    assert!(sandbox.socket().exists());
    assert!(
        sandbox
            .socket()
            .metadata()
            .expect("metadata")
            .file_type()
            .is_socket()
    );

    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    assert_eq!(client.call("ping", json!({}))["result"]["protocol"], 1);

    let refusal = match Daemon::bind_at(&sandbox.socket(), &sandbox.state()) {
        Err(refusal) => refusal,
        Ok(_) => panic!("a second daemon bound one socket"),
    };
    assert!(refusal.contains("already running"), "{refusal}");

    let replacement = sandbox.root.join("replacement.sock");
    fs::rename(daemon.path(), &replacement).expect("move the daemon socket inode");
    UnixListener::bind(daemon.path()).expect("replacement pathname");
    daemon.stop();
    assert!(
        daemon.path().exists(),
        "stop leaves a replacement socket path"
    );
    assert!(
        replacement.exists(),
        "renamed original socket remains until removed explicitly"
    );
    let _ = fs::remove_file(&replacement);
}

#[test]
fn a_failed_state_initialization_removes_the_socket_it_bound() {
    let sandbox = Sandbox::new("failed-state");
    fs::create_dir_all(sandbox.state()).expect("state root");
    fs::set_permissions(sandbox.state(), fs::Permissions::from_mode(0o755))
        .expect("loose state mode");
    let error = match Daemon::bind_at(&sandbox.socket(), &sandbox.state()) {
        Err(error) => error,
        Ok(_) => panic!("daemon accepted an untrusted state directory"),
    };
    assert!(error.contains("not mode 0700"), "{error}");
    assert!(
        !sandbox.socket().exists(),
        "startup failure removed its socket"
    );
}

/// A state root that is not a private `0700` directory is refused: a record says
/// which panes were closed, and a path anyone else can write is not a record.
#[test]
fn a_state_root_that_is_not_ours_is_refused() {
    let sandbox = Sandbox::new("state-mode");
    fs::create_dir_all(sandbox.state()).expect("a directory");
    fs::set_permissions(sandbox.state(), fs::Permissions::from_mode(0o755)).expect("a mode");
    let refusal = match Store::open(&sandbox.state()) {
        Err(refusal) => refusal,
        Ok(_) => panic!("a state root that is not ours was accepted"),
    };
    assert!(refusal.contains("not mode 0700"), "{refusal}");
}

/// The subcommand is what an operator runs, so it is tested as a process: it
/// prints where it is serving, answers on that socket, and leaves neither a
/// socket file nor a lost record behind when it is signalled.
#[test]
fn the_daemon_subcommand_serves_prints_and_stops_cleanly_on_a_signal() {
    for (name, signal) in [("sigint", libc::SIGINT), ("sigterm", libc::SIGTERM)] {
        let sandbox = Sandbox::new(name);
        let socket = sandbox.socket();
        let state = sandbox.state();
        let mut child = DaemonChild::spawn(&socket, &state);
        let mut lines = BufReader::new(child.process.stdout.take().expect("stdout")).lines();
        let greeting = lines
            .next()
            .expect("a greeting line")
            .expect("a readable greeting");
        assert!(
            greeting.contains(&socket.display().to_string()),
            "{greeting}"
        );
        assert!(
            greeting.contains(&state.display().to_string()),
            "{greeting}"
        );

        // The socket answers by the time the path is printed.
        let mut client = Client::dial(&socket);
        assert_eq!(client.call("ping", json!({}))["result"]["protocol"], 1);
        // A record planted over one pane holds that target, so the input below is
        // refused in flight: the daemon writes a durable record under the state
        // path it was handed without asking any runtime to type anything.
        let pane = "wA:p1";
        let store = Store::open(&state).expect("the state");
        store
            .create(&RequestRecord::new(
                &random_uuid(),
                "input",
                None,
                Some(pane.into()),
                now_ms(),
            ))
            .expect("plant a held record");
        let id = random_uuid();
        let refused = client.call_id(
            &id,
            "input",
            json!({"request": {"pane_id": pane, "payload": {"kind": "text", "text": "x"}}}),
        );
        assert_eq!(error_code(&refused), "refused", "{name}: {refused}");
        drop(client);

        // SAFETY: `kill` takes a pid this process started and a signal number.
        let status = child.stop(signal, DEADLINE);
        assert!(status.success(), "{name}: {status:?}");
        assert!(!socket.exists(), "{name}: the socket file is gone");

        let store = Store::open(&state).expect("the state");
        assert_eq!(
            store.get(&id).expect("a read").expect("the record").outcome,
            Some(RequestOutcome::Refused),
            "{name}: the refusal survives the daemon"
        );
    }
}

/// An uncertain outcome is not a licence to retry: the record stays `unknown`,
/// a same-ID retry returns it without a second dispatch, and another request
/// over the same target is refused as in flight.
#[test]
fn an_uncertain_focus_stays_unknown_and_suppresses_the_target() {
    let sandbox = Sandbox::new("focus-unknown");
    let backend = Arc::new(FakeBackend::new(
        empty_inventory(),
        FocusAnswer::Unknown("herdr timed out after 5s".into()),
    ));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let request = json!({"target":"wA:p1","target_kind":"pane"});
    let first = client.call_id(&id, "focus", request.clone());
    assert_eq!(result_record(&first)["outcome"], "unknown");
    assert_eq!(result_record(&first)["state"], "completed");

    // The same id returns the same account; the backend is asked exactly once.
    let retry = client.call_id(&id, "focus", request);
    assert_eq!(result_record(&retry)["outcome"], "unknown");
    assert_eq!(
        result_record(&retry)["completed_at"],
        result_record(&first)["completed_at"]
    );

    // A different request over the same target cannot dispatch: the effect may
    // already have happened, so it is refused in flight and recorded as such.
    let other_id = random_uuid();
    let other = client.call_id(
        &other_id,
        "focus",
        json!({"target":"wA:p1","target_kind":"pane"}),
    );
    assert_eq!(error_code(&other), "refused");
    assert!(
        other["error"]["message"]
            .as_str()
            .expect("a message")
            .contains(&id),
        "{other}"
    );
    let refused = client.call("request", json!({"id": other_id}));
    assert_eq!(result_record(&refused)["category"], Category::IN_FLIGHT);
    assert_eq!(backend.focused.lock().unwrap().len(), 1);
    daemon.stop();
}

/// The record is written before dispatch, and the daemon keeps answering other
/// connections while the backend holds the request open. The rendezvous is
/// bounded: no test sleep decides when the request is mid-flight.
#[test]
fn a_started_record_is_visible_and_reads_stay_responsive_before_dispatch() {
    let sandbox = Sandbox::new("started");
    let (gate, rendezvous) = Rendezvous::new();
    let backend = Arc::new(GatedBackend {
        gate: Arc::clone(&gate),
        released: true,
        answer: FocusOutcome::Completed,
    });
    let mut daemon = sandbox.daemon_with(backend);
    let mut blocker = Client::dial(daemon.path());

    let id = random_uuid();
    blocker.send(&request_line(
        &id,
        "focus",
        json!({"target":"wA:p1","target_kind":"pane"}),
    ));
    // Blocks until the request has reached the backend, which is after the
    // started record was written.
    rendezvous.entered();

    // A second connection is served while the first waits on the backend.
    let mut reader = Client::dial(daemon.path());
    assert!(reader.call("observe", json!({})).get("result").is_some());
    let started = reader.call("request", json!({"id": id}));
    assert_eq!(result_record(&started)["state"], "started");
    assert!(result_record(&started)["outcome"].is_null());

    rendezvous.release();
    let done = blocker.read();
    assert_eq!(result_record(&done)["outcome"], "completed");
    daemon.stop();
}

/// A rendezvous whose other side never arrives fails on its deadline, saying
/// which side is missing, instead of parking a thread. That parking is what an
/// unbounded `Barrier::wait` did: one missed handshake — or one thread that
/// panicked before arriving — hung this whole suite for forty minutes. Both
/// directions are checked, with a short deadline so the check costs nothing.
#[test]
fn a_missed_rendezvous_fails_instead_of_parking() {
    let deadline = Duration::from_millis(50);

    let (gate, rendezvous) = Rendezvous::with_deadline(deadline);
    let started = Instant::now();
    let failed = panic::catch_unwind(AssertUnwindSafe(|| rendezvous.entered()));
    let message = panic_message(failed.expect_err("a backend that never arrived was not reported"));
    assert!(message.contains("never entered"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "a missed arrival waited {:?}",
        started.elapsed()
    );

    let started = Instant::now();
    let failed = panic::catch_unwind(AssertUnwindSafe(|| gate.hold()));
    let message = panic_message(failed.expect_err("a call nobody released was not reported"));
    assert!(message.contains("never released"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an unreleased call waited {:?}",
        started.elapsed()
    );
}

/// The message a caught panic carries, for asserting that a bounded wait failed
/// saying what was missing rather than silently.
fn panic_message(failed: Box<dyn Any + Send>) -> String {
    let failed = failed.as_ref();
    if let Some(message) = failed.downcast_ref::<String>() {
        return message.clone();
    }
    failed
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .unwrap_or_default()
}

/// Stopping the daemon cancels an in-flight focus promptly, and the record it
/// leaves behind is a durable unknown: the effect may have happened.
#[test]
fn stopping_the_daemon_cancels_an_in_flight_focus_into_a_durable_unknown() {
    let sandbox = Sandbox::new("stop-unknown");
    let (gate, rendezvous) = Rendezvous::new();
    let backend = Arc::new(GatedBackend {
        gate: Arc::clone(&gate),
        released: false,
        answer: FocusOutcome::Unknown("focus was cancelled before herdr answered".into()),
    });
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    client.send(&request_line(
        &id,
        "focus",
        json!({"target":"wA:p1","target_kind":"pane"}),
    ));
    rendezvous.entered();

    let started = Instant::now();
    daemon.stop();
    assert!(
        started.elapsed() < DEADLINE,
        "stop waited on the in-flight focus: {:?}",
        started.elapsed()
    );

    drop(client);
    let store = Store::open(&sandbox.state()).expect("the state");
    let record = store.get(&id).expect("a read").expect("the record");
    assert_eq!(record.state, RecordState::Completed);
    assert_eq!(record.outcome, Some(RequestOutcome::Unknown));
    assert!(record.completed_at.is_some());
}

/// The guard is the whole point of the daemon's close: a location its owner
/// manages is refused, and the backend is never asked.
#[test]
fn a_managed_pane_close_is_refused_before_the_backend_is_asked() {
    let sandbox = Sandbox::new("close-managed");
    let inventory = close_inventory("wA:t1", &[("wA:p1", Some(true))]);
    let backend = CloseBackend::new(Ok(inventory.clone()), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Pane("wA:p1".into());
    let response = client.call("close", frozen_from(&inventory, target.clone()));
    assert_eq!(result_record(&response)["outcome"], "refused");
    assert_eq!(
        result_record(&response)["category"],
        Category::BACKEND_REFUSED
    );
    assert_eq!(result_record(&response)["effects"], json!([]));
    assert!(
        result_record(&response)["message"]
            .as_str()
            .expect("a message")
            .contains("managed by its owner"),
        "{response}"
    );
    assert!(
        !backend.told_to_close(&target),
        "the backend was asked anyway"
    );
    daemon.stop();
}

/// One managed member is enough: a tab is refused whole rather than closed
/// around the pane its owner is using.
#[test]
fn a_tab_with_one_managed_member_is_refused_whole() {
    let sandbox = Sandbox::new("close-mixed-tab");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None), ("wA:p2", Some(true))]);
    let backend = CloseBackend::new(Ok(inventory.clone()), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Tab("wA:t1".into());
    let response = client.call("close", frozen_from(&inventory, target.clone()));
    assert_eq!(result_record(&response)["outcome"], "refused");
    assert_eq!(result_record(&response)["target"], "wA:t1");
    assert!(
        result_record(&response)["message"]
            .as_str()
            .expect("a message")
            .contains("managed by its owner"),
        "{response}"
    );
    assert!(
        !backend.told_to_close(&target),
        "the backend was asked anyway"
    );
    daemon.stop();
}

/// The operator confirmed what was in the pane; a different occupant by the time
/// the daemon looks is drift, and drift is never closed.
#[test]
fn a_replaced_occupant_is_refused_as_changed() {
    let sandbox = Sandbox::new("close-drifted");
    let confirmed = close_inventory("wA:t1", &[("wA:p1", Some(true))]);
    let now = close_inventory("wA:t1", &[("wA:p1", None)]);
    let backend = CloseBackend::new(Ok(now), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Pane("wA:p1".into());
    let response = client.call("close", frozen_from(&confirmed, target.clone()));
    assert_eq!(result_record(&response)["outcome"], "refused");
    assert_eq!(
        result_record(&response)["message"],
        "pane wA:p1 changed: nothing was closed"
    );
    assert!(
        !backend.told_to_close(&target),
        "the backend was asked anyway"
    );
    daemon.stop();
}

/// An inventory the backend cannot read proves nothing about ownership, so the
/// close is refused rather than attempted.
#[test]
fn an_unreadable_inventory_refuses_the_close() {
    let sandbox = Sandbox::new("close-unreadable");
    let backend = CloseBackend::new(
        Err("could not run herdr: no such file".into()),
        CloseAnswer::Completed,
    );
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Pane("wA:p1".into());
    let frozen = frozen_pane("wA:p1");
    let response = client.call("close", frozen);
    assert_eq!(result_record(&response)["outcome"], "refused");
    assert_eq!(
        result_record(&response)["category"],
        Category::BACKEND_REFUSED
    );
    assert!(
        !backend.told_to_close(&target),
        "the backend was asked anyway"
    );
    daemon.stop();
}

/// The happy path end to end: a bare pane is closed exactly once, and a retry of
/// the same request returns the same account rather than closing again.
#[test]
fn an_unchanged_unmanaged_pane_closes_once() {
    let sandbox = Sandbox::new("close-pane");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None), ("wA:p2", None)]);
    let backend = CloseBackend::new(Ok(inventory.clone()), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Pane("wA:p1".into());
    let frozen = frozen_from(&inventory, target.clone());
    let id = random_uuid();
    let first = client.call_id(&id, "close", frozen.clone());
    assert_eq!(result_record(&first)["outcome"], "completed");
    assert_eq!(result_record(&first)["effects"], json!(["close"]));
    assert_eq!(result_record(&first)["target"], "wA:p1");
    assert!(backend.told_to_close(&target), "the backend was not asked");

    let retry = client.call_id(&id, "close", frozen);
    assert_eq!(
        result_record(&retry)["completed_at"],
        result_record(&first)["completed_at"],
        "a retry must not close again"
    );
    daemon.stop();
}

/// A whole tab of bare panes is one close, and the record names the tab.
#[test]
fn an_unchanged_unmanaged_tab_closes_once() {
    let sandbox = Sandbox::new("close-tab");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None), ("wA:p2", None)]);
    let backend = CloseBackend::new(Ok(inventory.clone()), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Tab("wA:t1".into());
    let response = client.call("close", frozen_from(&inventory, target.clone()));
    assert_eq!(result_record(&response)["outcome"], "completed");
    assert_eq!(result_record(&response)["target"], "wA:t1");
    assert!(backend.told_to_close(&target), "the backend was not asked");
    daemon.stop();
}

/// An uncertain close is not a licence to retry: the record stays `unknown`, a
/// same-ID retry returns it without a second dispatch, and another request over
/// the same target is refused as in flight.
#[test]
fn an_uncertain_close_stays_unknown_and_suppresses_the_target() {
    let sandbox = Sandbox::new("close-unknown");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None)]);
    let backend = CloseBackend::new(
        Ok(inventory.clone()),
        CloseAnswer::Unknown("herdr timed out after 5s".into()),
    );
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let target = CloseTarget::Pane("wA:p1".into());
    let frozen = frozen_from(&inventory, target.clone());
    let id = random_uuid();
    let first = client.call_id(&id, "close", frozen.clone());
    assert_eq!(result_record(&first)["outcome"], "unknown");
    assert_eq!(result_record(&first)["state"], "completed");

    let retry = client.call_id(&id, "close", frozen);
    assert_eq!(
        result_record(&retry)["completed_at"],
        result_record(&first)["completed_at"]
    );

    let other_id = random_uuid();
    let other = client.call_id(&other_id, "close", frozen_from(&inventory, target.clone()));
    assert_eq!(error_code(&other), "refused");
    let refused = client.call("request", json!({"id": other_id}));
    assert_eq!(result_record(&refused)["category"], Category::IN_FLIGHT);
    assert_eq!(backend.closed.lock().unwrap().len(), 1);
    daemon.stop();
}

/// Stopping the daemon cancels an in-flight close promptly, and the record it
/// leaves behind is a durable unknown: the effect may have happened.
#[test]
fn stopping_the_daemon_cancels_an_in_flight_close_into_a_durable_unknown() {
    let sandbox = Sandbox::new("close-stop-unknown");
    let (gate, rendezvous) = Rendezvous::new();
    let inventory = close_inventory("wA:t1", &[("wA:p1", None)]);
    let backend = CloseBackend::new(
        Ok(inventory.clone()),
        CloseAnswer::Unknown("herdr did not answer before the daemon stopped".into()),
    )
    .gated(Arc::clone(&gate));
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    client.send(&request_line(
        &id,
        "close",
        frozen_from(&inventory, CloseTarget::Pane("wA:p1".into())),
    ));
    rendezvous.entered();

    let started = Instant::now();
    daemon.stop();
    assert!(
        started.elapsed() < DEADLINE,
        "stop waited on the in-flight close: {:?}",
        started.elapsed()
    );

    drop(client);
    let store = Store::open(&sandbox.state()).expect("the state");
    let record = store.get(&id).expect("a read").expect("the record");
    assert_eq!(record.state, RecordState::Completed);
    assert_eq!(record.outcome, Some(RequestOutcome::Unknown));
    assert!(record.completed_at.is_some());
}

/// One create, one input in each of its two shapes, and one bounded read, all
/// through the real socket: the backend is asked for the normalized request and
/// the operation records say what happened. The read leaves no record at all.
#[test]
fn launch_readiness_refuses_before_effect_dispatch_and_does_not_change_advertisement() {
    let sandbox = Sandbox::new("launch-readiness");
    let backend = MuxBackend::new(MUX_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("ping", json!({}));
    assert_eq!(
        ping["result"]["capabilities"],
        advertised(MUX_CAPABILITIES),
        "a backend that does not implement launch advertises only its actual capabilities"
    );
    let refusal = Operation::readiness("spawn", "launch", backend.capabilities)
        .expect_err("launch is not implemented by this backend");
    assert_eq!(
        refusal,
        "spawn needs the `launch` capability; this backend does not provide it"
    );
    assert!(
        backend
            .created_requests
            .lock()
            .expect("the create list")
            .is_empty(),
        "readiness must fail before any creation or dispatch"
    );

    // Other operations keep their own capability gates; they do not inherit
    // launch merely because a backend can create or accept terminal input.
    assert!(Operation::readiness("create", "creation", backend.capabilities).is_ok());
    assert!(Operation::readiness("input", "input", backend.capabilities).is_ok());
    let no_launch = ["creation", "input"];
    assert!(Operation::readiness("spawn", "launch", &no_launch).is_err());
    daemon.stop();
}

/// A spawn the backend can serve creates under the parent's own pane, launches
/// the command the caller resolved, and answers with the two records it wrote.
#[test]
fn a_spawn_creates_beside_its_parent_launches_the_command_and_reports_its_effects() {
    let sandbox = Sandbox::new("spawn-served");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let params = spawn_params(&parent, "/usr/bin/pi", &["--child", "row one"]);
    let response = client.call_id(&id, "spawn", params.clone());

    let record = result_record(&response);
    assert_eq!(record["method"], "spawn");
    assert_eq!(record["outcome"], "completed");
    // The record's target is the location the request changed: the pane the child
    // was created beside, so a competing mutation of it cannot run alongside.
    assert_eq!(record["target"], "wA:p1");
    assert_eq!(record["created"], json!({"kind": "pane", "id": "wA:p2"}));
    assert_eq!(
        record["effects"],
        json!(["created pane wA:p2", "launched child"])
    );

    // The edge is the second record: the three effects separately, and the parent
    // the child was created for.
    let spawn = &response["result"]["spawn"];
    assert_eq!(spawn["parent"], json!(parent));
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "completed");
    assert_eq!(
        spawn["location"],
        json!({"backend": "runtime", "pane": "wA:p2"})
    );
    assert!(
        spawn.get("bound").is_none(),
        "no child is claimed before one registers with the token"
    );

    // The child was created beside the parent's own pane and unfocused: a spawn
    // never lands wherever the multiplexer happens to have focus.
    let creates = backend.creates();
    assert_eq!(creates.len(), 1);
    assert_eq!(
        serde_json::to_value(&creates[0]).expect("the create"),
        json!({
            "kind": "pane_split",
            "pane_id": "wA:p1",
            "direction": "right",
            "focus": false,
        })
    );

    // The command launched is the caller's own, unchanged, in the pane the create
    // named, carrying the token the edge minted.
    let launches = backend.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].pane_id, "wA:p2");
    assert_eq!(launches[0].executable, "/usr/bin/pi");
    assert_eq!(
        launches[0].argv,
        vec!["--child".to_string(), "row one".into()]
    );
    let token = recorded_edge(&sandbox.state(), &id)["token"]
        .as_str()
        .expect("a minted token")
        .to_string();
    assert_eq!(launches[0].spawn_token, token);
    // The token is the only thing that binds the edge to its child, so it is a
    // credential: it travels to the pane and appears in no answer.
    no_secret(&response, &token);
    no_secret(&client.call("request", json!({"id": id})), &token);

    // A spawn types nothing of its own and touches no lifecycle: the launch is the
    // only thing written to the pane.
    assert!(
        backend
            .input_requests
            .lock()
            .expect("the input list")
            .is_empty(),
        "a spawn launches; it does not compose input"
    );
    // It writes two records and no other fact: one edge for one spawn, and no
    // publication, so no assignment or execution fact was authored.
    assert_eq!(edges_written(&sandbox.state()), 1);
    assert_eq!(
        fs::read_dir(sandbox.state().join("publications"))
            .expect("the publications directory")
            .count(),
        0,
        "a spawn authors no assignment fact"
    );
    daemon.stop();
}

/// The wire-level proof the launch capability gate was delivered for: a spawn
/// against a backend that cannot launch is refused before any effect.
#[test]
fn a_backend_without_launch_refuses_a_spawn_before_any_effect() {
    let sandbox = Sandbox::new("spawn-without-launch");
    let backend = MuxBackend::new(MUX_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let response = client.call_id(&id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
    // The record is the answer: an operation this backend cannot serve is
    // refused in a record a client can read, not answered as a bare error.
    let record = result_record(&response);
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], "backend_unavailable");
    assert!(
        record["message"]
            .as_str()
            .expect("a message")
            .contains("the `launch` capability"),
        "the refusal names what this backend cannot do: {response}"
    );
    assert_eq!(record["effects"], json!([]));
    assert!(record["created"].is_null());
    assert!(
        response["result"].get("spawn").is_none(),
        "a spawn this backend cannot serve authors no edge: {response}"
    );
    assert!(backend.creates().is_empty(), "nothing was created");
    assert!(backend.launches().is_empty(), "nothing was launched");
    assert_eq!(
        edges_written(&sandbox.state()),
        0,
        "a spawn this backend cannot serve records no intent and mints no token"
    );

    // The refusal outlives the answer, and still claims no effect.
    let recorded = client.call("request", json!({"id": id}));
    assert_eq!(result_record(&recorded)["outcome"], "refused");

    // Capability advertisement is unchanged: this backend implements what it
    // says it does, and its refusal adds nothing to it.
    let ping = client.call("ping", json!({}));
    assert_eq!(ping["result"]["capabilities"], advertised(MUX_CAPABILITIES));
    daemon.stop();
}

/// A replayed request id answers what the first attempt recorded, and performs
/// no second effect.
#[test]
fn a_replayed_spawn_request_id_answers_the_recorded_effects_once() {
    let sandbox = Sandbox::new("spawn-replay");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let params = spawn_params(&parent, "/usr/bin/pi", &["--child"]);
    let first = client.call_id(&id, "spawn", params.clone());
    let written = fs::read(edge_path(&sandbox.state(), &id)).expect("the edge file");

    let second = client.call_id(&id, "spawn", params.clone());
    assert_eq!(
        second, first,
        "a replay is answered from the records the first attempt wrote"
    );
    assert_eq!(backend.creates().len(), 1, "one create, once");
    assert_eq!(backend.launches().len(), 1, "one launch, once");
    assert_eq!(
        fs::read(edge_path(&sandbox.state(), &id)).expect("the edge file"),
        written,
        "a replay mints no second token and rewrites no effect"
    );

    // The same id with different contents is a different request, and is refused
    // rather than served as a second effect.
    let different = client.call_id(&id, "spawn", spawn_params(&parent, "/usr/bin/other", &[]));
    assert_eq!(error_code(&different), "refused");
    assert_eq!(backend.creates().len(), 1);
    assert_eq!(backend.launches().len(), 1);
    daemon.stop();
}

/// A refused launch leaves the pane the daemon created named, and removes
/// nothing.
#[test]
fn a_refused_spawn_launch_leaves_the_created_pane_named() {
    let sandbox = Sandbox::new("spawn-launch-refused");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES)
        .launching(LaunchAnswer::Refused("the pane refused the line".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let response = client.call_id(&id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
    let record = result_record(&response);
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], "backend_refused");
    assert_eq!(record["message"], "the pane refused the line");
    assert_eq!(
        record["created"],
        json!({"kind": "pane", "id": "wA:p2"}),
        "the pane it created is named even though the launch failed"
    );
    assert_eq!(record["effects"], json!(["created pane wA:p2"]));

    let spawn = &response["result"]["spawn"];
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "refused");
    assert_eq!(spawn["location"]["pane"], "wA:p2");
    // The daemon does not clean up a pane it can see and the operator may want:
    // the edge still names it, and the backend was asked to close nothing (the
    // harness panics if it ever is).
    assert_eq!(
        recorded_edge(&sandbox.state(), &id)["location"]["pane"],
        "wA:p2"
    );
    assert_eq!(backend.launches().len(), 1);
    daemon.stop();
}

/// An unconfirmed launch is unknown, is never reported as launched, and still
/// names the pane the create confirmed.
#[test]
fn an_unconfirmed_spawn_launch_reads_unknown_and_is_never_claimed_launched() {
    let sandbox = Sandbox::new("spawn-launch-unknown");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES)
        .launching(LaunchAnswer::Unknown("the pane never answered".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let response = client.call_id(&id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
    let record = result_record(&response);
    assert_eq!(record["outcome"], "unknown");
    assert_eq!(record["category"], "backend_unavailable");
    assert_eq!(record["message"], "the pane never answered");
    assert_eq!(record["created"], json!({"kind": "pane", "id": "wA:p2"}));
    assert_eq!(
        record["effects"],
        json!(["created pane wA:p2"]),
        "an unconfirmed launch is not an effect"
    );

    let spawn = &response["result"]["spawn"];
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "unknown");
    assert_eq!(spawn["location"]["pane"], "wA:p2");
    assert_eq!(
        recorded_edge(&sandbox.state(), &id)["launched"],
        "unknown",
        "the edge records the uncertainty rather than a launch"
    );
    daemon.stop();
}

/// A spawn under a parent the daemon has no runtime subject for, or one that
/// registered no pane, is refused before anything is created.
#[test]
fn a_spawn_without_a_parent_pane_is_refused_before_any_effect() {
    let sandbox = Sandbox::new("spawn-parent");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let unknown = client.call("spawn", spawn_params(&random_uuid(), "/usr/bin/pi", &[]));
    assert_eq!(error_code(&unknown), "not_found");
    assert!(
        unknown["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("is not a registered agent"),
        "{unknown}"
    );

    // A registered subject that named no pane is refused too: the daemon creates
    // under a pane it was told about, never under an inferred one.
    registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let unlocatable = client.call(
        "agent.register",
        registration_body(OTHER_INCARNATION, None, None),
    );
    let unlocatable = agent_id(&unlocatable);
    let no_pane = client.call("spawn", spawn_params(&unlocatable, "/usr/bin/pi", &[]));
    assert_eq!(error_code(&no_pane), "refused");
    assert!(
        no_pane["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("names no pane"),
        "{no_pane}"
    );

    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(edges_written(&sandbox.state()), 0);
    daemon.stop();
}

/// The wire shapes of a spawn are validated before anything is recorded.
#[test]
fn the_wire_shapes_of_spawn_are_validated_before_dispatch() {
    let sandbox = Sandbox::new("spawn-shapes");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let cases = [
        (
            "a command that is not absolute",
            spawn_params(&parent, "pi", &[]),
            "must be an absolute printable path",
        ),
        (
            "an argument a terminal would act on",
            spawn_params(&parent, "/usr/bin/pi", &["bell\u{7}"]),
            "cannot contain terminal control characters",
        ),
        (
            "a parent that is not a runtime subject id",
            spawn_params("parent", "/usr/bin/pi", &[]),
            "`parent` must be a canonical UUID",
        ),
        (
            "a field this request does not have",
            json!({"request": {
                "parent": &parent,
                "executable": "/usr/bin/pi",
                "command": "pi",
            }}),
            "unknown field",
        ),
        (
            "a request with no command",
            json!({"request": {"parent": &parent}}),
            "missing field `executable`",
        ),
        (
            "no request at all",
            json!({"requester": "test"}),
            "`request` is required",
        ),
    ];
    for (case, params, expected) in cases {
        let response = client.call("spawn", params);
        assert_eq!(error_code(&response), "bad_params", "{case}: {response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .expect("a message")
                .contains(expected),
            "{case}: {response}"
        );
    }
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(edges_written(&sandbox.state()), 0);
    daemon.stop();
}

#[test]
fn child_close_dispatches_only_the_verified_spawn_child_pane() {
    let sandbox = Sandbox::new("managed-close-success");
    let edge_id = random_uuid();
    let process = ProcessIdentity {
        boot_id: "boot-test".into(),
        pid: 200,
        start_ticks: 400,
    };
    let verifier: Arc<dyn ProcessVerifier> = Arc::new(MatchingVerifier {
        pid: 200,
        identity: process.clone(),
    });
    let base = MuxBackend::new(SPAWN_CAPABILITIES)
        .observing(&["wA:p2"])
        .with_foreground(ForegroundEvidence::command(200, Some("pi".into()), None));
    let backend = base.closing(CloseAnswer::Completed);
    let mut daemon = Daemon::bind_with_backend_and_verifier(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend.clone()),
        verifier,
    )
    .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    // Create the edge through the real socket path; then bind the child process claim.
    let spawned = client.call_id(&edge_id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
    assert_eq!(result_record(&spawned)["outcome"], "completed");
    let edge = recorded_edge(&sandbox.state(), &edge_id);
    let token = edge["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    let registered = register_close_child(&mut client, &token, process);
    assert!(
        registered["result"]["registration"]["agent_id"].is_string(),
        "{registered}"
    );
    let agent_id = registered["result"]["registration"]["agent_id"]
        .as_str()
        .expect("the child's registration")
        .to_owned();
    let registration = client.call("agent.get", json!({"agent_id": &agent_id}));
    let close_id = random_uuid();
    let first = client.call_id(
        &close_id,
        "child.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(first["result"]["close"]["outcome"], "completed", "{first}");
    assert_eq!(first["result"]["close"]["intent"], "cancel");
    assert_eq!(first["result"]["close"]["pane"], "wA:p2");
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Pane("wA:p2".into())]
    );
    // The pane result is durable, and it is the whole of what a close claims: the
    // recorded pane, the caller's intent, a confirmed mux closure, and no process
    // or assignment fact.
    let record = close_record(&sandbox.state(), &close_id);
    assert_eq!(record["outcome"], "completed", "{record}");
    assert_eq!(record["intent"], "cancel");
    assert_eq!(record["location"]["pane"], "wA:p2");
    assert!(record.get("message").is_none(), "{record}");
    assert!(
        !fs::read_to_string(closes_directory(&sandbox.state()).join(format!("{close_id}.json")))
            .expect("a reading")
            .contains(&token),
        "a close record never carries the spawn token"
    );

    // The same request id is answered from that record: the pane was closed once,
    // and the edge, its binding and the child's registration read as they did.
    let replay = client.call_id(
        &close_id,
        "child.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(
        replay["result"]["close"]["outcome"], "completed",
        "{replay}"
    );
    assert_eq!(replay["result"]["close"]["pane"], "wA:p2");
    assert_eq!(
        backend.closed_targets().len(),
        1,
        "a replay must not dispatch again"
    );
    let topology = client.call("spawn.get", json!({"request_id": &edge_id}));
    assert_eq!(topology["result"]["spawn"]["state"], "bound", "{topology}");
    assert_eq!(
        topology["result"]["spawn"]["bound"]["incarnation"],
        CHILD_INCARNATION
    );
    assert_eq!(topology["result"]["spawn"]["location"]["pane"], "wA:p2");
    assert_eq!(topology["result"]["spawn"]["created"], "completed");
    assert_eq!(topology["result"]["spawn"]["launched"], "completed");
    assert_eq!(
        client.call("agent.get", json!({"agent_id": &agent_id}))["result"]["agent"],
        registration["result"]["agent"],
        "a close rewrites no registration of the child it closed"
    );
    let ping = client.call("ping", json!({}));
    assert!(
        !ping["result"]["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("managed_child_close"))
    );
    daemon.stop();
}

/// A backend refusal is an outcome like any other: it is durable, it is replayed
/// without a second dispatch, and a conflicting use of its request id is refused.
#[test]
fn child_close_persists_a_backend_refusal_and_replays_it_without_dispatching() {
    let sandbox = Sandbox::new("managed-close-refused");
    let edge_id = random_uuid();
    let process = ProcessIdentity {
        boot_id: "boot-test".into(),
        pid: 200,
        start_ticks: 400,
    };
    let verifier: Arc<dyn ProcessVerifier> = Arc::new(MatchingVerifier {
        pid: 200,
        identity: process.clone(),
    });
    let backend = MuxBackend::new(SPAWN_CAPABILITIES)
        .observing(&["wA:p2"])
        .with_foreground(ForegroundEvidence::command(200, Some("pi".into()), None))
        .closing(CloseAnswer::Refused("no such pane".into()));
    let mut daemon = Daemon::bind_with_backend_and_verifier(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend.clone()),
        verifier,
    )
    .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let spawned = client.call_id(&edge_id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
    assert_eq!(result_record(&spawned)["outcome"], "completed");
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, process);

    let close_id = random_uuid();
    let refused = client.call_id(
        &close_id,
        "child.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&refused), "refused", "{refused}");
    assert_eq!(refused["error"]["message"], "no such pane", "{refused}");
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Pane("wA:p2".into())]
    );
    // The refusal is durable and carries the backend's own words, so a replay can
    // answer the same way after the backend is gone.
    let record = close_record(&sandbox.state(), &close_id);
    assert_eq!(record["outcome"], "refused", "{record}");
    assert_eq!(record["message"], "no such pane");
    let replay = client.call_id(
        &close_id,
        "child.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&replay), "refused", "{replay}");
    assert_eq!(replay["error"]["message"], "no such pane");
    assert_eq!(
        backend.closed_targets().len(),
        1,
        "a replay must not dispatch again"
    );

    // One request id names one close: the same id with different content is
    // refused, and the recorded refusal stands.
    let conflict = client.call_id(
        &close_id,
        "child.close",
        json!({
            "spawn_request_id": &edge_id,
            "source": "test-publisher",
            "incarnation": CHILD_INCARNATION,
            "intent": "complete",
        }),
    );
    assert_eq!(error_code(&conflict), "refused", "{conflict}");
    assert!(
        conflict["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("already names different contents"),
        "{conflict}"
    );
    assert_eq!(backend.closed_targets().len(), 1);
    assert_eq!(close_record(&sandbox.state(), &close_id), record);
    daemon.stop();
}

/// A close outlives the daemon that dispatched it: a restart reads the recorded
/// outcome, dispatches nothing, and infers no process exit from it.
#[test]
fn a_restarted_daemon_reads_the_recorded_close_outcome_without_dispatching() {
    let sandbox = Sandbox::new("managed-close-restart");
    let edge_id = random_uuid();
    let close_id = random_uuid();
    let process = ProcessIdentity {
        boot_id: "boot-test".into(),
        pid: 200,
        start_ticks: 400,
    };
    {
        let verifier: Arc<dyn ProcessVerifier> = Arc::new(MatchingVerifier {
            pid: 200,
            identity: process.clone(),
        });
        let backend = MuxBackend::new(SPAWN_CAPABILITIES)
            .observing(&["wA:p2"])
            .with_foreground(ForegroundEvidence::command(200, Some("pi".into()), None))
            .closing(CloseAnswer::Unknown("the reply was lost".into()));
        let mut daemon = Daemon::bind_with_backend_and_verifier(
            &sandbox.socket(),
            &sandbox.state(),
            Some(backend.clone()),
            verifier,
        )
        .expect("daemon");
        let mut client = Client::dial(daemon.path());
        let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
        let spawned = client.call_id(&edge_id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
        assert_eq!(result_record(&spawned)["outcome"], "completed");
        let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
            .as_str()
            .expect("private spawn token")
            .to_owned();
        register_close_child(&mut client, &token, process.clone());

        let lost = client.call_id(
            &close_id,
            "child.close",
            child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
        );
        assert_eq!(error_code(&lost), "backend_unavailable", "{lost}");
        assert_eq!(lost["error"]["message"], "the reply was lost", "{lost}");
        assert_eq!(
            backend.closed_targets(),
            vec![CloseTarget::Pane("wA:p2".into())]
        );
        daemon.stop();
    }

    // The restarted daemon has the same records and a backend it could dispatch
    // to. It answers from the record: no second close, and no reading of the pane
    // or the process behind it as gone.
    let backend = MuxBackend::new(SPAWN_CAPABILITIES)
        .observing(&["wA:p2"])
        .with_foreground(ForegroundEvidence::command(200, Some("pi".into()), None));
    let verifier: Arc<dyn ProcessVerifier> = Arc::new(MatchingVerifier {
        pid: 200,
        identity: process,
    });
    let mut restarted = Daemon::bind_with_backend_and_verifier(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend.clone()),
        verifier,
    )
    .expect("a restart");
    let mut client = Client::dial(restarted.path());
    let replay = client.call_id(
        &close_id,
        "child.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&replay), "backend_unavailable", "{replay}");
    assert_eq!(replay["error"]["message"], "the reply was lost", "{replay}");
    assert!(
        backend.closed_targets().is_empty(),
        "a restart re-dispatches nothing"
    );
    let topology = client.call("spawn.get", json!({"request_id": &edge_id}));
    assert_eq!(topology["result"]["spawn"]["state"], "bound", "{topology}");
    assert_eq!(topology["result"]["spawn"]["location"]["pane"], "wA:p2");
    restarted.stop();
}

#[test]
fn child_close_refuses_foreground_and_containment_mismatches_without_recording() {
    for (name, evidence, panes, verification) in [
        ("shell", ForegroundEvidence::Shell, vec!["wA:p2"], true),
        (
            "inconclusive",
            ForegroundEvidence::Inconclusive,
            vec!["wA:p2"],
            true,
        ),
        (
            "missing",
            ForegroundEvidence::command(200, Some("pi".into()), None),
            vec!["wA:p9"],
            true,
        ),
        (
            "different-pid",
            ForegroundEvidence::command(201, Some("pi".into()), None),
            vec!["wA:p2"],
            false,
        ),
        (
            "invalid-pid",
            ForegroundEvidence::command(0, Some("pi".into()), None),
            vec!["wA:p2"],
            true,
        ),
        (
            "mismatched-birth",
            ForegroundEvidence::command(200, Some("pi".into()), None),
            vec!["wA:p2"],
            false,
        ),
    ] {
        let sandbox = Sandbox::new(&format!("managed-close-{name}"));
        let id = random_uuid();
        let process = ProcessIdentity {
            boot_id: "boot-test".into(),
            pid: 200,
            start_ticks: 400,
        };
        let verifier: Arc<dyn ProcessVerifier> = Arc::new(MatchingVerifier {
            pid: if verification { 200 } else { 201 },
            identity: if verification {
                process.clone()
            } else if name == "mismatched-birth" {
                ProcessIdentity {
                    start_ticks: 401,
                    ..process.clone()
                }
            } else {
                ProcessIdentity {
                    pid: 201,
                    ..process.clone()
                }
            },
        });
        let backend = MuxBackend::new(SPAWN_CAPABILITIES)
            .observing(&panes)
            .with_foreground(evidence);
        let mut daemon = Daemon::bind_with_backend_and_verifier(
            &sandbox.socket(),
            &sandbox.state(),
            Some(backend.clone()),
            verifier,
        )
        .expect("daemon");
        let mut client = Client::dial(daemon.path());
        let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
        client.call_id(&id, "spawn", spawn_params(&parent, "/usr/bin/pi", &[]));
        let token = recorded_edge(&sandbox.state(), &id)["token"]
            .as_str()
            .unwrap()
            .to_owned();
        register_close_child(&mut client, &token, process);
        let close_id = random_uuid();
        let response = client.call_id(
            &close_id,
            "child.close",
            child_close_params(&id, "test-publisher", CHILD_INCARNATION),
        );
        assert!(
            response.get("error").is_some(),
            "a refusal must be an error, not an accepted close: {response}"
        );
        assert!(
            !closes_directory(&sandbox.state())
                .join(format!("{close_id}.json"))
                .exists()
        );
        assert!(backend.closed_targets().is_empty());
        daemon.stop();
    }
}

#[test]
fn the_topology_read_reports_the_edge_its_own_operation_recorded() {
    let sandbox = Sandbox::new("spawn-read");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES).observing(&["wA:p1", "wA:p2"]);
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let id = random_uuid();
    spawned_edge(&mut client, &id, &parent);

    let response = client.call("spawn.get", json!({"request_id": id}));
    let spawn = &response["result"]["spawn"];
    assert_eq!(spawn["request_id"], json!(id));
    assert_eq!(spawn["parent"], json!(parent));
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "completed");
    assert_eq!(spawn["location"]["pane"], "wA:p2");
    assert_eq!(
        spawn["state"], "unbound",
        "no child has presented the token"
    );
    assert_eq!(
        spawn["freshness"], "fresh",
        "the backend reports the recorded pane now"
    );
    assert!(spawn.get("bound").is_none());
    no_secret(
        &response,
        recorded_edge(&sandbox.state(), &id)["token"]
            .as_str()
            .expect("a minted token"),
    );

    // An edge the daemon never authored is refused rather than invented, and the
    // refusal names no other edge.
    let unknown = client.call("spawn.get", json!({"request_id": random_uuid()}));
    assert_eq!(error_code(&unknown), "not_found");
    daemon.stop();
}

#[test]
fn a_location_the_backend_no_longer_reports_reads_unresolved_without_revising_the_edge() {
    let sandbox = Sandbox::new("spawn-unresolved");
    // The backend reports the parent's pane and not the pane the child was
    // created in: the recorded location is gone, and the record says nothing
    // about what became of the child.
    let backend = MuxBackend::new(SPAWN_CAPABILITIES).observing(&["wA:p1"]);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let id = random_uuid();
    spawned_edge(&mut client, &id, &parent);
    let written = fs::read(edge_path(&sandbox.state(), &id)).expect("the edge file");

    let response = client.call("spawn.get", json!({"request_id": id}));
    let spawn = &response["result"]["spawn"];
    assert_eq!(spawn["state"], "unresolved");
    assert_eq!(spawn["freshness"], "stale");
    assert_eq!(
        spawn["location"]["pane"], "wA:p2",
        "the recorded location is reported, not dropped"
    );
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "completed");
    assert!(
        spawn.get("stopped").is_none(),
        "a missing pane is not a report that the child stopped"
    );
    assert_eq!(
        fs::read(edge_path(&sandbox.state(), &id)).expect("the edge file"),
        written,
        "a read re-verifies; it never revises the record"
    );
    // The read consulted the backend's own report and touched nothing else: this
    // fake is never asked to close or focus, and only the spawn created a pane.
    assert_eq!(backend.creates().len(), 1);
    daemon.stop();
}

#[test]
fn a_restarted_daemon_reads_its_edges_and_never_confirms_an_unchecked_location() {
    let sandbox = Sandbox::new("spawn-restart");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES).observing(&["wA:p2"]);
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let id = random_uuid();
    spawned_edge(&mut client, &id, &parent);
    // The child the launch started registers with the token it was given, so the
    // edge names an exact identity across the restart.
    let token = recorded_edge(&sandbox.state(), &id)["token"]
        .as_str()
        .expect("a minted token")
        .to_owned();
    registering_with_token(&mut client, CHILD_INCARNATION, &token);
    let live = client.call("spawn.get", json!({"request_id": &id}));
    assert_eq!(live["result"]["spawn"]["state"], "bound");
    assert_eq!(live["result"]["spawn"]["freshness"], "fresh");
    daemon.stop();

    // The restarted daemon has no backend to consult. The edges are its own
    // records, so the read is served and the location is left unconfirmed —
    // never refused, and never presented as current.
    let mut restarted = sandbox.daemon();
    let mut client = Client::dial(restarted.path());
    let response = client.call("spawn.get", json!({"request_id": &id}));
    let spawn = &response["result"]["spawn"];
    assert_eq!(
        spawn["state"], "bound",
        "a binding is a stored fact; a restart does not revise it"
    );
    assert_eq!(spawn["bound"]["incarnation"], CHILD_INCARNATION);
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "completed");
    assert_eq!(spawn["location"]["pane"], "wA:p2");
    assert_eq!(
        spawn["freshness"], "stale",
        "no live backend confirmed this location"
    );
    no_secret(&response, &token);
    restarted.stop();
}

#[test]
fn spawn_edges_page_in_key_order_and_a_client_can_read_one_it_lost_the_answer_for() {
    let sandbox = Sandbox::new("spawn-page");
    let backend = MuxBackend::new(SPAWN_CAPABILITIES).observing(&["wA:p2"]);
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    // Spawned out of order: a page's order is the durable key's.
    for id in [SECOND_EDGE, THIRD_EDGE, FIRST_EDGE] {
        spawned_edge(&mut client, id, &parent);
    }

    let first = client.call("spawn.list", json!({"limit": 2}));
    // The whole line a client reads, envelope included, is what must fit.
    assert!(format!("{first}\n").len() < MAX_LINE_BYTES);
    assert_eq!(request_ids(&first), vec![FIRST_EDGE, SECOND_EDGE]);
    assert_eq!(first["result"]["next"], json!(SECOND_EDGE));
    // Each entry is the shape `spawn.get` answers with, so a client reads one
    // edge the same way it reads a page of them.
    assert_eq!(first["result"]["spawns"][0]["state"], "unbound");
    assert_eq!(first["result"]["spawns"][0]["freshness"], "fresh");

    let second = client.call("spawn.list", json!({"limit": 2, "after": SECOND_EDGE}));
    assert_eq!(request_ids(&second), vec![THIRD_EDGE]);
    assert_eq!(second["result"]["next"], Value::Null);
    let done = client.call("spawn.list", json!({"after": THIRD_EDGE}));
    assert!(request_ids(&done).is_empty());
    assert_eq!(done["result"]["next"], Value::Null);

    // A page is bounded, so no answer can be grown by asking for one: no page is
    // unbounded, and a cursor that is not a key is refused rather than ignored.
    assert_eq!(
        error_code(&client.call("spawn.list", json!({"limit": 0}))),
        "bad_params"
    );
    assert_eq!(
        error_code(&client.call("spawn.list", json!({"limit": MAX_LISTED + 1}))),
        "bad_params"
    );
    assert_eq!(
        error_code(&client.call("spawn.list", json!({"after": "not-a-key"}))),
        "refused"
    );
    no_secret(
        &first,
        recorded_edge(&sandbox.state(), FIRST_EDGE)["token"]
            .as_str()
            .expect("a minted token"),
    );
    daemon.stop();
}

#[test]
fn an_agent_the_daemon_did_not_spawn_has_no_edge_and_is_not_an_error() {
    let sandbox = Sandbox::new("spawn-external");
    // The backend reports a pane the daemon never created: a read must not adopt
    // it into the topology from what it observes, and the second pane is the
    // decoy that would show up if it did.
    let backend = MuxBackend::new(SPAWN_CAPABILITIES).observing(&["wA:p2", "wA:p9"]);
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    // An empty topology is a page with nothing in it, not an error.
    let empty = client.call("spawn.list", json!({}));
    assert_eq!(empty["result"]["spawns"], json!([]));
    assert_eq!(empty["result"]["next"], Value::Null);

    let id = random_uuid();
    spawned_edge(&mut client, &id, &parent);
    // A child Herdsman launched through the backend directly: it registers, the
    // daemon authors no edge for it, and it never binds the pending one.
    let external = registered_parent(&mut client, OTHER_INCARNATION, "wA:p9");
    assert_eq!(
        edges_written(&sandbox.state()),
        1,
        "no edge is authored from observation"
    );

    let response = client.call("spawn.list", json!({}));
    assert_eq!(
        request_ids(&response),
        vec![id.clone()],
        "the topology holds the edge the daemon authored, and only it"
    );
    assert_eq!(response["result"]["spawns"][0]["state"], "unbound");
    assert!(!response.to_string().contains(&external));
    assert!(!response.to_string().contains(OTHER_INCARNATION));
    daemon.stop();
}

#[test]
fn the_mux_primitives_are_served_and_recorded() {
    let sandbox = Sandbox::new("mux-primitives");
    let backend = MuxBackend::new(MUX_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("ping", json!({}));
    assert_eq!(ping["result"]["capabilities"], advertised(MUX_CAPABILITIES));

    let create_id = random_uuid();
    let create_params = json!({
        "request": {
            "kind": "pane_split",
            "pane_id": "wA:p1",
            "direction": "right",
            "focus": false,
        },
        "requester": "test",
    });
    let create = client.call_id(&create_id, "create", create_params.clone());
    let record = result_record(&create);
    assert_eq!(record["outcome"], "completed");
    assert_eq!(record["target"], "wA:p1");
    // The identity the backend named travels as data, so a client acts on what it
    // created instead of parsing `effects`.
    assert_eq!(record["created"], json!({"kind": "pane", "id": "wA:p2"}));
    assert_eq!(record["effects"], json!(["created pane wA:p2"]));
    assert_eq!(
        backend.created_requests.lock().expect("the create list")[0],
        CreateRequest::PaneSplit {
            pane_id: "wA:p1".into(),
            direction: SplitDirection::Right,
            focus: false,
        }
    );

    // The same ID is answered from its record — identity included — and creates
    // nothing a second time.
    let replay = client.call_id(&create_id, "create", create_params);
    assert_eq!(result_record(&replay), record);
    assert_eq!(
        backend
            .created_requests
            .lock()
            .expect("the create list")
            .len(),
        1
    );

    let text = client.call(
        "input",
        json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "ls\n"}}}),
    );
    assert_eq!(result_record(&text)["outcome"], "completed");
    assert_eq!(result_record(&text)["target"], "wA:p1");
    assert_eq!(result_record(&text)["effects"], json!(["sent text"]));

    let keys = client.call(
        "input",
        json!({
            "request": {
                "pane_id": "wA:p1",
                "payload": {"kind": "keys", "keys": ["esc", "ctrl+c"]},
            }
        }),
    );
    assert_eq!(result_record(&keys)["effects"], json!(["sent keys"]));
    let sent = backend.input_requests.lock().expect("the input list");
    assert_eq!(sent.len(), 2, "the backend was asked for other input");
    assert_eq!(
        sent[1].payload,
        InputPayload::Keys {
            keys: vec!["esc".into(), "ctrl+c".into()]
        }
    );
    drop(sent);

    let id = random_uuid();
    let output = client.call_id(
        &id,
        "output",
        json!({"pane_id": "wA:p1", "source": "visible", "lines": 20}),
    );
    assert_eq!(output["result"]["output"]["text"], "hello\n");
    assert_eq!(output["result"]["output"]["revision"], 7);
    assert_eq!(
        backend.output_requests.lock().expect("the output list")[0],
        OutputRequest {
            pane_id: "wA:p1".into(),
            source: OutputSource::Visible,
            lines: Some(20),
            ansi: false,
        }
    );
    // A read is not an operation: nothing about it is a durable record.
    let missing = client.call("request", json!({"id": id}));
    assert_eq!(error_code(&missing), "not_found");
    daemon.stop();
}

/// A confirmation that named nothing carries no identity: the field is absent
/// rather than invented from the request, and the record says so in words.
#[test]
fn a_create_that_named_nothing_carries_no_identity() {
    let sandbox = Sandbox::new("mux-silent-create");
    let backend = MuxBackend::new(MUX_CAPABILITIES).naming(CreatedLocation::default());
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let create = client.call(
        "create",
        json!({"request": {"kind": "workspace", "focus": false}}),
    );
    let record = result_record(&create);
    assert_eq!(record["outcome"], "completed");
    assert!(record["created"].is_null(), "{record}");
    assert_eq!(record["effects"], json!([]));
    assert!(
        record["message"]
            .as_str()
            .expect("the message")
            .contains("without naming what it created"),
        "{record}"
    );
    daemon.stop();
}

/// A backend answer larger than one answer line carries is cut here, where the
/// bound is, rather than becoming an answer no client can read.
#[test]
fn an_oversized_output_read_is_cut_to_one_answer_line() {
    let sandbox = Sandbox::new("mux-output-bound");
    let long = "a".repeat(MAX_LINE_BYTES);
    let backend = MuxBackend::new(MUX_CAPABILITIES).reading(OutputRead {
        text: long.clone(),
        truncated: false,
        revision: None,
    });
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let read = client.call("output", json!({"pane_id": "wA:p1", "source": "recent"}));
    let text = read["result"]["output"]["text"]
        .as_str()
        .expect("the read text");
    assert!(text.len() < long.len(), "the answer was not bounded");
    assert!(long.starts_with(text), "the answer is not a prefix");
    assert_eq!(read["result"]["output"]["truncated"], true);
    assert!(text.len() <= MAX_LINE_BYTES);
    daemon.stop();
}

/// A backend that does not declare a primitive is never asked for it: the
/// operation refuses, names the capability, and its record claims no effect.
#[test]
fn an_unimplemented_primitive_is_refused_without_dispatch() {
    let sandbox = Sandbox::new("mux-unsupported");
    let backend = MuxBackend::new(&[]);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("ping", json!({}));
    assert_eq!(ping["result"]["capabilities"], advertised(&[]));

    // An operation this backend cannot serve leaves a settled refusal that names
    // the capability and claims no effect, not a request still claiming to be
    // started.
    let create = client.call(
        "create",
        json!({"request": {"kind": "workspace", "focus": false}}),
    );
    assert_eq!(result_record(&create)["outcome"], "refused");
    assert_eq!(
        result_record(&create)["category"],
        Category::BACKEND_UNAVAILABLE
    );
    assert!(
        result_record(&create)["message"]
            .as_str()
            .expect("the message")
            .contains("`creation`")
    );
    assert_eq!(result_record(&create)["effects"], json!([]));
    assert_eq!(result_record(&create)["state"], "completed");
    // A refusal names no identity: nothing was created.
    assert!(result_record(&create)["created"].is_null());

    let left = client.call(
        "input",
        json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "x"}}}),
    );
    assert_eq!(result_record(&left)["outcome"], "refused");
    assert!(
        result_record(&left)["message"]
            .as_str()
            .expect("the message")
            .contains("`input`")
    );
    // A read has no record, so its refusal is the whole answer.
    let read = client.call("output", json!({"pane_id": "wA:p1", "source": "visible"}));
    assert_eq!(error_code(&read), "backend_unavailable");
    assert!(
        read["error"]["message"]
            .as_str()
            .expect("the message")
            .contains("`output`")
    );
    assert!(backend.created_requests.lock().expect("a read").is_empty());
    assert!(backend.input_requests.lock().expect("a read").is_empty());
    assert!(backend.output_requests.lock().expect("a read").is_empty());
    daemon.stop();
}

/// What a caller cannot mean is refused at the wire, before anything is
/// recorded or dispatched.
#[test]
fn the_wire_shapes_of_create_and_input_are_validated_before_dispatch() {
    let sandbox = Sandbox::new("mux-validation");
    let backend = MuxBackend::new(MUX_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let many_keys: Vec<String> = (0..17).map(|_| "enter".to_string()).collect();
    let cases = [
        (
            "create",
            json!({"request": {"kind": "nowhere", "focus": false}}),
        ),
        (
            "create",
            json!({"request": {"kind": "pane_split", "pane_id": "", "direction": "right", "focus": false}}),
        ),
        (
            "create",
            json!({"request": {"kind": "workspace", "focus": false}, "extra": 1}),
        ),
        (
            "input",
            json!({"request": {"pane_id": "", "payload": {"kind": "text", "text": "ls\n"}}}),
        ),
        (
            "input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": ""}}}),
        ),
        (
            "input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "two\u{7}shapes"}}}),
        ),
        (
            "input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "keys", "keys": []}}}),
        ),
        (
            "input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "keys", "keys": many_keys}}}),
        ),
        (
            "input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "keys", "keys": ["esc\u{1b}"]}}}),
        ),
        (
            "output",
            json!({"pane_id": "wA:p1", "source": "visible", "lines": 0}),
        ),
    ];
    for (method, params) in cases {
        let response = client.call(method, params.clone());
        assert_eq!(error_code(&response), "bad_params", "{method} {params}");
    }
    assert!(backend.created_requests.lock().expect("a read").is_empty());
    assert!(backend.input_requests.lock().expect("a read").is_empty());
    assert!(backend.output_requests.lock().expect("a read").is_empty());
    daemon.stop();
}

/// A creation that names no location is a mutation of nothing in particular, so
/// it shares one lane with other unscoped mutations; a pane mutation is a
/// different location and runs beside it.
#[test]
fn an_unscoped_create_suppresses_another_unscoped_mutation_but_not_a_pane_one() {
    let sandbox = Sandbox::new("mux-lane");
    let (gate, rendezvous) = Rendezvous::new();
    let backend = MuxBackend::new(MUX_CAPABILITIES).gated(Arc::clone(&gate));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut held_client = Client::dial(daemon.path());
    let mut other = Client::dial(daemon.path());

    let held = random_uuid();
    held_client.send(&request_line(
        &held,
        "create",
        json!({"request": {"kind": "workspace", "focus": false}}),
    ));
    rendezvous.entered();

    let second = random_uuid();
    let refused = other.call_id(
        &second,
        "create",
        json!({"request": {"kind": "workspace", "focus": false}}),
    );
    assert_eq!(error_code(&refused), "refused");
    let record = other.call("request", json!({"id": second}));
    assert_eq!(result_record(&record)["category"], Category::IN_FLIGHT);

    let input = other.call(
        "input",
        json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "hi"}}}),
    );
    assert_eq!(
        result_record(&input)["outcome"],
        "completed",
        "a pane mutation was suppressed by an unscoped one"
    );

    rendezvous.release();
    let first = held_client.read();
    assert_eq!(result_record(&first)["outcome"], "completed");
    assert_eq!(result_record(&first)["target"], Value::Null);
    assert_eq!(backend.created_requests.lock().expect("the list").len(), 1);
    daemon.stop();
}

/// The wire form is the graph the runtime produces: a populated inventory and
/// evidence round-trip, unknown semantic states preserved as themselves.
#[test]
fn a_populated_normalized_graph_round_trips_including_unknown_states() {
    let (inventory, evidence) = populated_graph();
    let json = serde_json::to_value(&inventory).expect("serialize the inventory");
    assert_eq!(
        json["agents"][0]["facts"]["state"],
        json!({"other": "grepping"})
    );
    assert_eq!(json["agents"][0]["status"], json!({"other": "dormant"}));
    assert_eq!(json["agents"][0]["facts"]["awaited"], json!(["agent:peer"]));
    let back: FleetObservation = serde_json::from_value(json).expect("deserialize the inventory");
    assert_eq!(back, inventory);

    let json = serde_json::to_value(&evidence).expect("serialize the evidence");
    assert_eq!(json["kind"], "non_shell");
    assert_eq!(json["local"]["running_for"], 1500);
    assert!(json["command"].is_null());
    let back: ForegroundEvidence = serde_json::from_value(json).expect("deserialize the evidence");
    assert_eq!(back, evidence);
}

/// An empty normalized inventory, for backends that only exercise one path.
fn empty_inventory() -> FleetObservation {
    FleetObservation {
        workspaces: Vec::new(),
        tabs: Vec::new(),
        panes: Vec::new(),
        agents: Vec::new(),
    }
}

/// A `radar daemon` child one test started.
///
/// The child is killed and reaped on unwind, so an assertion that fails between
/// the spawn and the signal cannot leak a daemon serving on a temporary socket.
struct DaemonChild {
    process: Child,
    exited: bool,
}

impl DaemonChild {
    fn spawn(socket: &Path, state: &Path) -> Self {
        let process = Command::new(env!("CARGO_BIN_EXE_radar"))
            .arg("daemon")
            .env("RADAR_CONTROL_SOCKET", socket)
            .env("RADAR_CONTROL_STATE", state)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start radar daemon");
        Self {
            process,
            exited: false,
        }
    }

    /// Signals the daemon and waits for it to go, under a deadline.
    fn stop(&mut self, signal: i32, deadline: Duration) -> ExitStatus {
        // SAFETY: `kill` takes the pid of a child this process started and a
        // signal number.
        unsafe { libc::kill(self.process.id() as i32, signal) };
        let status = wait_for_exit(&mut self.process, deadline);
        self.exited = true;
        status
    }
}

impl Drop for DaemonChild {
    fn drop(&mut self) {
        // A child that already exited is reaped here rather than signalled, so a
        // recycled pid is never the target of this test's cleanup.
        if self.exited || matches!(self.process.try_wait(), Ok(Some(_))) {
            return;
        }
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// The guard is what keeps an aborted run from leaking a daemon, so the guard is
/// exercised rather than assumed: an assertion that fails between the spawn and
/// the signal still leaves no child behind.
#[test]
fn an_aborted_daemon_test_does_not_leak_its_child() {
    let sandbox = Sandbox::new("daemon-leak");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let leaked = Mutex::new(None);
    let aborted = panic::catch_unwind(AssertUnwindSafe(|| {
        let child = DaemonChild::spawn(&socket, &state);
        *leaked.lock().expect("the pid") = Some(child.process.id() as i32);
        panic!("this test aborts before it signals the daemon");
    }));
    let message = panic_message(aborted.expect_err("the abort"));
    assert!(message.contains("aborts before"), "{message}");

    let pid = leaked.lock().expect("the pid").expect("a pid");
    let until = Instant::now() + DEADLINE;
    while process_exists(pid) {
        assert!(
            Instant::now() < until,
            "the aborted test left pid {pid} behind"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

/// Whether a pid still exists. A killed but unreaped child is a zombie and still
/// exists, so a pid that is gone was also reaped.
fn process_exists(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("run kill -0")
        .success()
}

/// Waits for a child to go, with a deadline: a daemon that ignored its signal
/// fails its test rather than hanging it.
fn wait_for_exit(child: &mut Child, deadline: Duration) -> ExitStatus {
    let until = Instant::now() + deadline;
    loop {
        match child.try_wait().expect("a child status") {
            Some(status) => return status,
            None if Instant::now() >= until => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the daemon did not exit within {deadline:?}");
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}

// ---------------------------------------------------------------------------
// Radar's own client, and the choice between it and the direct adapter
// ---------------------------------------------------------------------------

/// Every capability Radar's client can ask a daemon for.
const CLIENT_CAPABILITIES: &[&str] = &[
    "observe",
    "process_info",
    "focus",
    "close",
    "creation",
    "input",
    "output",
    "reporting",
];

/// The configuration that selects the daemon at `socket`.
fn daemon_at(socket: &Path) -> RuntimeConfig {
    RuntimeConfig {
        backend: RuntimeBackend::Daemon,
        socket: Some(socket.to_path_buf()),
    }
}

/// A fake backend that serves everything a client can ask for, so one daemon
/// exercises the whole client surface. Nothing here is a mux.
struct ClientBackend {
    capabilities: &'static [&'static str],
    inventory: FleetObservation,
    evidence: ForegroundEvidence,
    close: CloseAnswer,
    closed: Mutex<Vec<CloseTarget>>,
}

impl ClientBackend {
    fn new(capabilities: &'static [&'static str]) -> Arc<Self> {
        let (inventory, evidence) = populated_graph();
        Arc::new(Self {
            capabilities,
            inventory,
            evidence,
            close: CloseAnswer::Completed,
            closed: Mutex::new(Vec::new()),
        })
    }

    /// The same backend observing this fleet instead.
    fn observing(self: &Arc<Self>, inventory: FleetObservation) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            inventory,
            evidence: self.evidence.clone(),
            close: self.close.clone(),
            closed: Mutex::new(Vec::new()),
        })
    }

    /// The same backend answering a close with this outcome.
    fn closing(self: &Arc<Self>, close: CloseAnswer) -> Arc<Self> {
        Arc::new(Self {
            capabilities: self.capabilities,
            inventory: self.inventory.clone(),
            evidence: self.evidence.clone(),
            close,
            closed: Mutex::new(Vec::new()),
        })
    }

    fn told_to_close(&self, target: &CloseTarget) -> bool {
        self.closed.lock().expect("the close list").contains(target)
    }

    fn closes_seen(&self) -> usize {
        self.closed.lock().expect("the close list").len()
    }
}

impl RuntimeProvider for ClientBackend {
    fn capabilities(&self) -> &'static [&'static str] {
        self.capabilities
    }
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        Ok(self.inventory.clone())
    }
    fn foreground_evidence(&self, pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        if pane_id == "wA:p1" {
            self.evidence.clone()
        } else {
            ForegroundEvidence::Inconclusive
        }
    }
    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        self.focus_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }
    fn focus_outcome(&self, _target: &Target, _cancel: &AtomicBool) -> FocusOutcome {
        FocusOutcome::Completed
    }
    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        self.close_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }
    fn close_outcome(&self, target: &CloseTarget, _cancel: &AtomicBool) -> CloseOutcome {
        self.closed
            .lock()
            .expect("the close list")
            .push(target.clone());
        match &self.close {
            CloseAnswer::Completed => CloseOutcome::Completed,
            CloseAnswer::Refused(message) => CloseOutcome::Refused(message.clone()),
            CloseAnswer::Unknown(message) => CloseOutcome::Unknown(message.clone()),
        }
    }
    fn create(&self, _request: &CreateRequest, _cancel: &AtomicBool) -> CreateOutcome {
        CreateOutcome::Completed(CreatedLocation {
            workspace_id: None,
            tab_id: None,
            pane_id: Some("wA:p2".into()),
        })
    }
    fn input(&self, _request: &InputRequest, _cancel: &AtomicBool) -> InputOutcome {
        InputOutcome::Completed
    }
    fn output(&self, _request: &OutputRequest, _cancel: &AtomicBool) -> OutputOutcome {
        OutputOutcome::Completed(OutputRead {
            text: "hello\n".into(),
            truncated: false,
            revision: Some(7),
        })
    }
    fn report(&self, _request: &ReportRequest, _cancel: &AtomicBool) -> ReportOutcome {
        ReportOutcome::Completed
    }
}

/// Where a [`recording_herdr`] writes what it was asked to serve.
fn asked_marker(root: &Path) -> PathBuf {
    root.join("asked")
}

/// A direct adapter that records being asked anything, so a test can prove it was
/// used — or never was. The script answers nothing: whether it ran at all is what
/// these tests assert.
fn recording_herdr(root: &Path) -> HerdrConfig {
    let executable = root.join("herdr");
    fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/asked\"\nexit 1\n",
    )
    .expect("write the recording adapter");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
        .expect("an executable adapter");
    HerdrConfig {
        executable,
        ..HerdrConfig::default()
    }
}

/// A socket that answers the requests a closure recognises and drops the
/// connection on the ones it does not, so the client's own mapping can be
/// exercised for answers a healthy daemon does not send.
fn scripted_socket(
    root: &Path,
    name: &str,
    answer: impl Fn(&Value) -> Option<Value> + Send + 'static,
) -> PathBuf {
    let path = root.join(name);
    let listener = UnixListener::bind(&path).expect("bind the scripted socket");
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let Ok(reader) = stream.try_clone() else {
                return;
            };
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let request: Value = serde_json::from_str(&line).expect("a request line");
            // No answer ends the connection, which is exactly the daemon that
            // accepted a request and never replied.
            let Some(response) = answer(&request) else {
                return;
            };
            let mut body = response.to_string();
            body.push('\n');
            let _ = stream.write_all(body.as_bytes());
            let _ = stream.flush();
        }
    });
    path
}

/// The ping answer a scripted socket gives, so a handshake can pass and the
/// scripted behaviour can be about the method that follows it.
fn scripted_ping(request: &Value) -> Value {
    json!({"id": request["id"], "result": {
        "protocol": PROTOCOL_VERSION,
        "backend": "runtime",
        "capabilities": CLIENT_CAPABILITIES,
    }})
}

fn scripted_method(request: &Value) -> &str {
    request["method"].as_str().unwrap_or("")
}

/// Radar's client speaks the protocol the daemon serves: every method the
/// dashboard can ask for is answered over a real socket, in the normalized
/// types the rest of Radar sees, and the effectful ones leave the daemon's own
/// records behind.
#[test]
fn the_daemon_client_serves_every_method_over_a_real_socket() {
    let sandbox = Sandbox::new("client-methods");
    // A fleet with a bare pane, so the close the client drives is one the daemon's
    // own containment check can prove unmanaged: a Pi pane publishing no owner
    // metadata is deliberately neither managed nor closable.
    let fleet = close_inventory("wA:t1", &[("wA:p1", None), ("wA:p2", None)]);
    let backend = ClientBackend::new(CLIENT_CAPABILITIES).observing(fleet.clone());
    let mut daemon = sandbox.daemon_with(backend.clone());
    let client = DaemonRuntime::handshake(daemon.path()).expect("the daemon is usable");
    let cancel = AtomicBool::new(false);
    assert_eq!(
        client.capabilities(),
        CLIENT_CAPABILITIES,
        "the client is what the daemon declared"
    );

    let (_, evidence) = populated_graph();
    assert_eq!(client.inventory(&cancel).expect("inventory"), fleet);
    assert_eq!(client.foreground_evidence("wA:p1", &cancel), evidence);
    assert_eq!(
        client.foreground_evidence("wA:p9", &cancel),
        ForegroundEvidence::Inconclusive
    );

    assert_eq!(
        client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel),
        FocusOutcome::Completed
    );
    let pane = CloseTarget::Pane("wA:p1".into());
    assert_eq!(
        client.close_outcome(&pane, &cancel),
        CloseOutcome::Completed
    );
    assert!(backend.told_to_close(&pane));
    assert_eq!(
        client.create(
            &CreateRequest::PaneSplit {
                pane_id: "wA:p1".into(),
                direction: SplitDirection::Right,
                focus: false,
            },
            &cancel
        ),
        CreateOutcome::Completed(CreatedLocation {
            workspace_id: None,
            tab_id: None,
            pane_id: Some("wA:p2".into()),
        })
    );
    assert_eq!(
        client.input(
            &InputRequest {
                pane_id: "wA:p1".into(),
                payload: InputPayload::Text {
                    text: "ls\n".into()
                },
            },
            &cancel
        ),
        InputOutcome::Completed
    );
    assert_eq!(
        client.output(
            &OutputRequest {
                pane_id: "wA:p1".into(),
                source: OutputSource::Visible,
                lines: Some(20),
                ansi: false,
            },
            &cancel
        ),
        OutputOutcome::Completed(OutputRead {
            text: "hello\n".into(),
            truncated: false,
            revision: Some(7),
        })
    );
    assert_eq!(
        client.report(
            &ReportRequest::State(StateReport {
                pane_id: "wA:p1".into(),
                source: "pi-herdsman".into(),
                agent: "worker".into(),
                state: ReportedState::Working,
                message: Some("2 tasks".into()),
                sequence: Some(4),
            }),
            &cancel
        ),
        ReportOutcome::Completed
    );

    // The reads left no record; the five effectful calls left one each, carrying
    // the params this client sent them with.
    let mut reader = Client::dial(daemon.path());
    let listed = reader.call("requests", json!({}));
    let records = listed["result"]["requests"].as_array().expect("a list");
    let methods: Vec<&str> = records
        .iter()
        .filter_map(|record| record["method"].as_str())
        .collect();
    assert_eq!(methods.len(), 5, "{records:?}");
    for recorded in ["focus", "close", "create", "input", "report"] {
        assert!(methods.contains(&recorded), "{recorded} was not recorded");
    }
    let close = records
        .iter()
        .find(|record| record["method"] == "close")
        .expect("the close record");
    assert_eq!(close["target"], "wA:p1");
    assert_eq!(
        close["params"]["request"]["target"],
        json!({"pane": "wA:p1"})
    );
    assert_eq!(close["params"]["requester"], "radar");
    daemon.stop();
}

/// A capability the daemon's backend lacks is that operation being refused by
/// name, not a silently different operation: the client passes the daemon's own
/// refusal on.
#[test]
fn a_backend_that_lacks_a_capability_refuses_that_operation_through_the_client() {
    let sandbox = Sandbox::new("client-missing-capability");
    let backend = ClientBackend::new(&["observe", "process_info", "focus", "close"]);
    let mut daemon = sandbox.daemon_with(backend);
    let client = DaemonRuntime::handshake(daemon.path()).expect("the dashboard's needs are met");
    let cancel = AtomicBool::new(false);

    let refused = client.create(&CreateRequest::Workspace { focus: false }, &cancel);
    match refused {
        CreateOutcome::Refused(message) => {
            assert!(message.contains("`creation`"), "{message}");
        }
        other => panic!("{other:?}"),
    }
    match client.input(
        &InputRequest {
            pane_id: "wA:p1".into(),
            payload: InputPayload::Keys {
                keys: vec!["esc".into()],
            },
        },
        &cancel,
    ) {
        InputOutcome::Refused(message) => assert!(message.contains("`input`"), "{message}"),
        other => panic!("{other:?}"),
    }
    match client.report(
        &ReportRequest::State(StateReport {
            pane_id: "wA:p1".into(),
            source: "pi-herdsman".into(),
            agent: "worker".into(),
            state: ReportedState::Idle,
            message: None,
            sequence: None,
        }),
        &cancel,
    ) {
        ReportOutcome::Refused(message) => assert!(message.contains("`reporting`"), "{message}"),
        other => panic!("{other:?}"),
    }
    match client.output(
        &OutputRequest {
            pane_id: "wA:p1".into(),
            source: OutputSource::Visible,
            lines: None,
            ansi: false,
        },
        &cancel,
    ) {
        OutputOutcome::Refused(message) => assert!(message.contains("`output`"), "{message}"),
        other => panic!("{other:?}"),
    }
    daemon.stop();
}

/// A daemon that cannot be reached leaves Radar on the direct adapter, with one
/// line saying so — and that adapter is the one that answers.
#[test]
fn an_unreachable_daemon_leaves_the_direct_adapter_in_use() {
    let sandbox = Sandbox::new("client-unreachable");
    let socket = sandbox.root.join("absent.sock");
    let selection = select(&daemon_at(&socket), recording_herdr(&sandbox.root));

    assert_eq!(selection.backend, "direct");
    let diagnostic = selection.diagnostic.expect("the fallback is reported once");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(
        diagnostic.contains("No such file or directory"),
        "{diagnostic}"
    );
    assert!(diagnostic.contains("direct Herdr"), "{diagnostic}");
    assert!(!diagnostic.contains('\n'), "{diagnostic}");

    // The selected provider is really the direct adapter: asking it anything runs
    // it, which the recording script proves.
    assert!(!asked_marker(&sandbox.root).exists());
    let cancel = AtomicBool::new(false);
    assert!(selection.provider.inventory(&cancel).is_err());
    assert!(
        asked_marker(&sandbox.root).exists(),
        "the direct adapter was not the one selected"
    );
}

/// A socket path Radar may not trust is refused *before* it is dialled: the
/// daemon could not have bound a socket the check rejects, so whoever did is
/// never taken as Radar's observation and control plane, and the fallback is the
/// direct adapter with one line naming the failing check.
#[test]
fn a_socket_path_radar_may_not_trust_is_refused_before_any_connection() {
    let sandbox = Sandbox::new("client-planted");
    let direct = recording_herdr(&sandbox.root);
    let cancel = AtomicBool::new(false);

    // A working fake daemon in a directory its owner left group-writable. This is
    // the socket-ahead-of-the-daemon case: it answers a handshake, and it is
    // refused anyway. The marker it writes on any request is what proves the
    // refusal happened before a connection rather than after it.
    let shared = sandbox.root.join("shared");
    fs::create_dir_all(&shared).expect("a shared directory");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).expect("a mode");
    let marker = sandbox.root.join("shared-was-asked");
    let planted = planted_daemon(&shared, "control.sock", marker.clone());
    let selection = select(&daemon_at(&planted), direct.clone());
    assert_eq!(selection.backend, "direct");
    let diagnostic = selection.diagnostic.expect("the fallback is reported once");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("not mode 0700"), "{diagnostic}");
    assert!(diagnostic.contains("direct Herdr"), "{diagnostic}");
    assert!(!marker.exists(), "Radar dialled a socket it may not trust");

    // The same live socket reached through a symlink at the name: the rule is
    // about the path, not about what the path points at.
    let linked = sandbox.root.join("linked.sock");
    std::os::unix::fs::symlink(&planted, &linked).expect("a symlink");
    let selection = select(&daemon_at(&linked), direct.clone());
    assert_eq!(selection.backend, "direct");
    let diagnostic = selection.diagnostic.expect("a diagnostic");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("is a symlink"), "{diagnostic}");
    assert!(!marker.exists(), "Radar dialled a socket it may not trust");

    // A regular file wearing a control socket's name.
    let file = sandbox.root.join("file.sock");
    fs::write(&file, b"not a socket").expect("a file");
    let selection = select(&daemon_at(&file), direct);
    assert_eq!(selection.backend, "direct");
    let diagnostic = selection.diagnostic.expect("a diagnostic");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("is not a socket"), "{diagnostic}");

    // The provider the refusal selected is really the direct adapter.
    assert!(!asked_marker(&sandbox.root).exists());
    assert!(selection.provider.inventory(&cancel).is_err());
    assert!(
        asked_marker(&sandbox.root).exists(),
        "the direct adapter was not the one selected"
    );
}

/// A working fake daemon planted at `root/name`, which records in `marker` that
/// anything at all was asked of it. A client that connected and sent its ping
/// would leave the file behind, so its absence is what proves nothing was dialled.
fn planted_daemon(root: &Path, name: &str, marker: PathBuf) -> PathBuf {
    scripted_socket(root, name, move |request| {
        let _ = fs::write(&marker, b"asked");
        (scripted_method(request) == "ping").then(|| scripted_ping(request))
    })
}

/// A daemon that is listening but does not provide what the dashboard needs is
/// not used either, and the line names what is missing.
#[test]
fn an_incompatible_daemon_is_reported_and_not_used() {
    let sandbox = Sandbox::new("client-incompatible");
    let backend = ClientBackend::new(&["observe", "process_info", "focus"]);
    let mut daemon = sandbox.daemon_with(backend);
    let selection = select(&daemon_at(daemon.path()), recording_herdr(&sandbox.root));

    assert_eq!(selection.backend, "direct");
    let diagnostic = selection.diagnostic.expect("the fallback is reported once");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("`close`"), "{diagnostic}");
    assert!(diagnostic.contains("direct Herdr"), "{diagnostic}");
    daemon.stop();
}

/// A daemon that provides what the dashboard needs is the one used, with nothing
/// printed: the fallback line is for the fallback.
#[test]
fn a_usable_daemon_is_selected_without_a_diagnostic() {
    let sandbox = Sandbox::new("client-selected");
    let backend = ClientBackend::new(CLIENT_CAPABILITIES);
    let mut daemon = sandbox.daemon_with(backend);
    let selection = select(&daemon_at(daemon.path()), recording_herdr(&sandbox.root));

    assert_eq!(selection.backend, "daemon");
    assert_eq!(selection.diagnostic, None);
    let cancel = AtomicBool::new(false);
    let (inventory, _) = populated_graph();
    assert_eq!(
        selection.provider.inventory(&cancel).expect("inventory"),
        inventory
    );
    assert!(
        !asked_marker(&sandbox.root).exists(),
        "the direct adapter was used under a selected daemon"
    );
    daemon.stop();
}

/// The client's own mapping is what keeps dispatch certainty: an answer the
/// daemon sends about a request that may have reached the backend is unknown, and
/// never a refusal a caller could read as "nothing happened".
#[test]
fn the_clients_error_mapping_keeps_dispatch_certainty() {
    let sandbox = Sandbox::new("client-mapping");
    let cancel = AtomicBool::new(false);

    // `internal` is the daemon's own word for "the request may or may not have
    // had an effect".
    let internal = scripted_socket(
        &sandbox.root,
        "internal.sock",
        |request| match scripted_method(request) {
            "ping" => Some(scripted_ping(request)),
            _ => Some(json!({"id": request["id"], "error": {
                "code": "internal",
                "message": "the store is unreadable",
            }})),
        },
    );
    let client = DaemonRuntime::handshake(&internal).expect("a scripted daemon");
    let outcome = client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel);
    assert!(matches!(outcome, FocusOutcome::Unknown(_)), "{outcome:?}");

    // A connection that ends without an answer may have carried the request.
    let silent = scripted_socket(
        &sandbox.root,
        "silent.sock",
        |request| match scripted_method(request) {
            "ping" => Some(scripted_ping(request)),
            _ => None,
        },
    );
    let client = DaemonRuntime::handshake(&silent).expect("a scripted daemon");
    let outcome = client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel);
    assert!(matches!(outcome, FocusOutcome::Unknown(_)), "{outcome:?}");

    // An accepted request whose record is not settled may still be executed, so
    // it is unknown rather than a refusal.
    let unresolved =
        scripted_socket(
            &sandbox.root,
            "unresolved.sock",
            |request| match scripted_method(request) {
                "ping" => Some(scripted_ping(request)),
                _ => Some(json!({"id": request["id"], "result": {"request": {
                    "id": request["id"],
                    "state": "started",
                }}})),
            },
        );
    let client = DaemonRuntime::handshake(&unresolved).expect("a scripted daemon");
    let outcome = client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel);
    assert!(matches!(outcome, FocusOutcome::Unknown(_)), "{outcome:?}");

    // A refusal the daemon states before dispatch stays a refusal.
    let refusing = scripted_socket(
        &sandbox.root,
        "refusing.sock",
        |request| match scripted_method(request) {
            "ping" => Some(scripted_ping(request)),
            _ => Some(json!({"id": request["id"], "error": {
                "code": "refused",
                "message": "focus for this target is already in flight",
            }})),
        },
    );
    let client = DaemonRuntime::handshake(&refusing).expect("a scripted daemon");
    let outcome = client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel);
    assert!(matches!(outcome, FocusOutcome::Refused(_)), "{outcome:?}");
}

/// The shipped binary's own handshake answer passes the check Radar makes at
/// startup, so a configured daemon is the one used — and the four capabilities
/// the dashboard needs are the runtime's own, not a fake's.
#[test]
fn the_real_daemon_passes_the_clients_handshake() {
    let sandbox = Sandbox::new("client-real-daemon");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let mut child = DaemonChild::spawn(&socket, &state);
    let mut lines = BufReader::new(child.process.stdout.take().expect("stdout")).lines();
    lines
        .next()
        .expect("a greeting line")
        .expect("a readable greeting");

    // The handshake alone: the daemon's own capabilities are static, so nothing
    // here asks the mux anything.
    let selection = select(&daemon_at(&socket), recording_herdr(&sandbox.root));
    assert_eq!(selection.backend, "daemon");
    assert_eq!(selection.diagnostic, None);
    for required in DaemonRuntime::REQUIRED {
        assert!(
            selection.provider.capabilities().contains(required),
            "a real daemon does not declare `{required}`"
        );
    }
    assert!(!asked_marker(&sandbox.root).exists());

    let status = child.stop(libc::SIGTERM, DEADLINE);
    assert!(status.success(), "{status:?}");
}

/// A daemon that stops answering is abandoned when the caller cancels, rather
/// than held for the exchange's own deadline: the dashboard must not block on a
/// mux that has stopped answering.
#[test]
fn a_stalled_daemon_is_abandoned_when_the_caller_cancels() {
    let sandbox = Sandbox::new("client-cancel");
    let stall = scripted_socket(&sandbox.root, "stall.sock", |request| {
        match scripted_method(request) {
            "ping" => Some(scripted_ping(request)),
            // Holds the connection open and answers nothing, which is the daemon
            // that is still alive and no longer replying.
            _ => {
                thread::sleep(Duration::from_secs(30));
                None
            }
        }
    });
    let client = DaemonRuntime::handshake(&stall).expect("a scripted daemon");
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let outcome = thread::scope(|scope| {
        let setter = Arc::clone(&cancel);
        scope.spawn(move || {
            thread::sleep(Duration::from_millis(50));
            setter.store(true, Ordering::SeqCst);
        });
        client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel)
    });
    assert!(matches!(outcome, FocusOutcome::Unknown(_)), "{outcome:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a cancelled request waited {:?}",
        started.elapsed()
    );
}

/// An uncertain daemon close is never re-run through the direct adapter: Radar's
/// own close path hands the operation over once, keeps its unknown answer, and
/// leaves the fallback adapter unused.
#[test]
fn an_uncertain_daemon_close_is_never_retried_through_direct_herdr() {
    let sandbox = Sandbox::new("client-no-retry");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None)]);
    let backend = ClientBackend::new(CLIENT_CAPABILITIES)
        .observing(inventory.clone())
        .closing(CloseAnswer::Unknown("the reply was lost".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let selection = select(&daemon_at(daemon.path()), recording_herdr(&sandbox.root));
    assert_eq!(selection.backend, "daemon");
    assert_eq!(selection.diagnostic, None);

    let target = CloseTarget::Pane("wA:p1".into());
    let request = lifecycle::CloseRequest {
        target: target.clone(),
        identity: lifecycle::identity(&inventory, &target),
    };
    let cancel = AtomicBool::new(false);
    let outcome = lifecycle::close_unmanaged(selection.provider.as_ref(), &request, &cancel);
    assert!(
        matches!(outcome, lifecycle::DirectClose::Unknown(_)),
        "{outcome:?}"
    );
    assert!(backend.told_to_close(&target));
    assert_eq!(backend.closes_seen(), 1, "the close was asked for twice");
    assert!(
        !asked_marker(&sandbox.root).exists(),
        "an uncertain daemon close was retried through the direct adapter"
    );
    daemon.stop();
}
