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
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
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
use agent_radar::control_plane::ops::Operation;
use agent_radar::control_plane::registry::{MAX_LISTED, MAX_TEXT_BYTES, ProcessVerifier};
use agent_radar::control_plane::{
    Category, DaemonRuntime, Derived, MAX_LINE_BYTES, MAX_VERIFICATION_JOBS, PROTOCOL_VERSION,
    RecordState, RequestOutcome, RequestRecord, Store, connect_at, now_ms, random_uuid,
};
use agent_radar::lifecycle;
use agent_radar::model::{
    AgentObservation, BinaryFreshness, BinaryIdentity, FleetObservation, ForegroundEvidence,
    HerdsmanFacts, LocalFacts, Location, Pane, ProcessIdentity, RuntimeStatus, SemanticState, Tab,
    TerminalMode, Workspace,
};
use agent_radar::runtime::{
    BackendIdentity, CloseOutcome, CloseTarget, CreateOutcome, CreateRequest, CreatedLocation,
    FocusOutcome, InputOutcome, InputPayload, InputRequest, LaunchOutcome, LaunchRequest,
    MAX_REPORTED_TEXT_BYTES, OutputOutcome, OutputRead, OutputRequest, OutputSource,
    PaneExitObservation, Primitives, ReportOutcome, ReportRequest, ReportedState, RuntimeProvider,
    SplitDirection, StateReport, Target,
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
    fn identity(&self) -> BackendIdentity {
        BackendIdentity::named("fake")
    }

    fn primitives(&self) -> Primitives {
        Primitives::OBSERVE
            .union(Primitives::PROCESS_INFO)
            .union(Primitives::FOCUS)
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
    fn identity(&self) -> BackendIdentity {
        BackendIdentity::named("fake")
    }

    fn primitives(&self) -> Primitives {
        Primitives::OBSERVE.union(Primitives::FOCUS)
    }
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        Ok(FleetObservation {
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
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
        metadata_agents: Vec::new(),
        containers: Vec::new(),
        lifecycle_subjects: Vec::new(),
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: None,
            number: Some(1),
            session_path: None,
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
                current_path: None,
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
                    registry: None,
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
    fn identity(&self) -> BackendIdentity {
        BackendIdentity::named("fake")
    }

    fn primitives(&self) -> Primitives {
        Primitives::CLOSE
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

/// Backend primitive sets used by the fake runtime.
const MUX_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::CREATE)
    .union(Primitives::INPUT)
    .union(Primitives::OUTPUT)
    .union(Primitives::REPORT);
const SPAWN_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::FOCUS)
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::CREATE)
    .union(Primitives::INPUT)
    .union(Primitives::OUTPUT)
    .union(Primitives::REPORT)
    .union(Primitives::LAUNCH)
    .union(Primitives::CLOSE)
    .union(Primitives::LAUNCH_CONTEXT);
const TRACKED_SPAWN_PRIMITIVES: Primitives = SPAWN_PRIMITIVES.union(Primitives::LIFECYCLE_TRACKING);
const RECIPE_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::CREATE)
    .union(Primitives::LAUNCH)
    .union(Primitives::LAUNCH_CONTEXT)
    .union(Primitives::EXIT_OBSERVATION);
const DEAD_CLOSE_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::CREATE)
    .union(Primitives::INPUT)
    .union(Primitives::OUTPUT)
    .union(Primitives::REPORT)
    .union(Primitives::LAUNCH)
    .union(Primitives::CLOSE)
    .union(Primitives::EXIT_OBSERVATION);

/// Wire contracts derived from a fake backend's typed primitive set.
fn advertised(backend: Primitives) -> Value {
    Value::Array(
        agent_radar::control_plane::ops::advertised(backend)
            .into_iter()
            .map(|contract| json!(contract.as_str()))
            .collect(),
    )
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
    primitives: Primitives,
    created: CreatedLocation,
    output: OutputRead,
    report: ReportAnswer,
    launch: LaunchAnswer,
    /// What this backend reports as its panes: what a topology read consults to
    /// confirm a recorded location.
    inventory: FleetObservation,
    foreground: ForegroundEvidence,
    /// What this backend reports for a pane's exit state: a pane it was not told
    /// about stays unknown, which is what a backend without the capability says.
    exits: Mutex<BTreeMap<String, PaneExitObservation>>,
    /// The pid this backend claims serves it, when a test stages one: a daemon
    /// reads it as the fence for the panes this backend creates.
    server_pid: Mutex<Option<i32>>,
    /// When set, this backend's inventory read fails with these words.
    inventory_refusal: Mutex<Option<String>>,
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
    fn new(primitives: Primitives) -> Arc<Self> {
        Self::staged(primitives, None)
    }

    /// The same backend, answering a report with this outcome.
    fn reporting(&self, report: ReportAnswer) -> Arc<Self> {
        Self::staged(self.primitives, Some(report))
    }

    fn staged(primitives: Primitives, report: Option<ReportAnswer>) -> Arc<Self> {
        Arc::new(Self {
            primitives,
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
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: evidence,
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created: self.created.clone(),
            output,
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch,
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created,
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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

    /// The same backend reporting exactly this inventory.
    ///
    /// A container's frozen target has to be checkable against what the backend
    /// says is there, so a test that deletes one states what its backend serves.
    fn with_inventory(self: &Arc<Self>, inventory: FleetObservation) -> Arc<Self> {
        Arc::new(Self {
            primitives: self.primitives,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory,
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: self.inventory.clone(),
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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
            primitives: self.primitives,
            created: self.created.clone(),
            output: self.output.clone(),
            report: self.report.clone(),
            launch: self.launch.clone(),
            inventory: FleetObservation {
                metadata_agents: Vec::new(),
                containers: Vec::new(),
                lifecycle_subjects: Vec::new(),
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
                        current_path: None,
                    })
                    .collect(),
                agents: Vec::new(),
            },
            foreground: self.foreground.clone(),
            exits: Mutex::new(BTreeMap::new()),
            server_pid: Mutex::new(None),
            inventory_refusal: Mutex::new(None),
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

    /// Stages one pane as reported dead, or running, from now on.
    fn reporting_exit(&self, pane_id: &str, observation: PaneExitObservation) {
        self.exits
            .lock()
            .expect("the exit map")
            .insert(pane_id.to_string(), observation);
    }

    /// Stages this backend's inventory read as unavailable.
    fn refusing_inventory(&self, message: &str) {
        *self.inventory_refusal.lock().expect("the refusal") = Some(message.to_string());
    }

    /// Stages this backend as served by a process with this pid, so a daemon
    /// fences the panes it creates against one. Nothing here is a process: the
    /// verifier the test binds is what confirms or refuses the pid.
    fn served_by(&self, pid: i32) {
        *self.server_pid.lock().expect("the server pid") = Some(pid);
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
    fn identity(&self) -> BackendIdentity {
        BackendIdentity::named("fake")
    }

    fn primitives(&self) -> Primitives {
        self.primitives
    }

    fn server_process(&self, _cancel: &AtomicBool) -> Option<i32> {
        *self.server_pid.lock().expect("the server pid")
    }

    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        match self
            .inventory_refusal
            .lock()
            .expect("the refusal")
            .as_deref()
        {
            Some(message) => Err(message.to_string()),
            None => Ok(self.inventory.clone()),
        }
    }

    fn exit_observations(
        &self,
        pane_ids: &[String],
        _cancel: &AtomicBool,
    ) -> BTreeMap<String, PaneExitObservation> {
        let exits = self.exits.lock().expect("the exit map");
        pane_ids
            .iter()
            .map(|pane_id| {
                let observation = exits
                    .get(pane_id)
                    .cloned()
                    .unwrap_or(PaneExitObservation::Unknown);
                (pane_id.clone(), observation)
            })
            .collect()
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
    fn validate_launch(&self, request: &LaunchRequest) -> Result<(), String> {
        request.validate()
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
    let backend = MuxBackend::new(MUX_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("daemon.ping", json!({}));
    assert!(
        ping["result"]["contracts"]
            .as_array()
            .expect("contracts")
            .contains(&json!("registry.v1")),
        "{ping}"
    );

    let state_id = random_uuid();
    let state = client.call_id(&state_id, "mux.report", state_report("working"));
    let record = result_record(&state);
    assert_eq!(record["outcome"], "completed");
    assert_eq!(record["target"], "wA:p1");
    assert_eq!(record["effects"], json!(["reported state working"]));
    // The record keeps what the caller reported — source, sequence and all — and
    // nothing was derived from it.
    assert_eq!(record["params"], state_report("working"));

    let session = client.call(
        "mux.report",
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
    let metadata = client.call("mux.report", display.clone());
    assert_eq!(result_record(&metadata)["outcome"], "completed");
    assert_eq!(
        result_record(&metadata)["effects"],
        json!(["reported metadata"])
    );
    assert_eq!(result_record(&metadata)["params"], display);

    let workspace = client.call(
        "mux.report",
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
    let replay = client.call_id(&state_id, "mux.report", state_report("working"));
    assert_eq!(replay, state);
    assert_eq!(backend.reports_seen(), 4);
    let changed = client.call_id(&state_id, "mux.report", state_report("idle"));
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
        client.call("mux.request.get", json!({"id": state_id}))["result"]["request"]["method"],
        "mux.report"
    );
    daemon.stop();
}

/// A backend that does not report refuses it explicitly and never advertises the
/// capability, rather than the daemon substituting another operation.
#[test]
fn a_backend_without_reporting_refuses_it_and_advertises_no_reporting() {
    let sandbox = Sandbox::new("report-unsupported");
    let backend = MuxBackend::new(Primitives::OBSERVE);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["contracts"], advertised(Primitives::OBSERVE));

    let refused = client.call("mux.report", state_report("working"));
    let record = result_record(&refused);
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], Category::BACKEND_UNAVAILABLE);
    assert!(
        record["message"]
            .as_str()
            .expect("the message")
            .contains("`mux.report`"),
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
    let backend = MuxBackend::new(MUX_PRIMITIVES)
        .reporting(ReportAnswer::Refused("herdr refused the report".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let refused = client.call("mux.report", state_report("working"));
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
    let backend = MuxBackend::new(MUX_PRIMITIVES)
        .reporting(ReportAnswer::Unknown("herdr timed out after 5s".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let first = client.call_id(&id, "mux.report", state_report("working"));
    assert_eq!(result_record(&first)["outcome"], "unknown");
    assert_eq!(
        result_record(&first)["category"],
        Category::BACKEND_UNAVAILABLE
    );
    assert_eq!(result_record(&first)["effects"], json!([]));
    assert_eq!(backend.reports_seen(), 1);

    let replay = client.call_id(&id, "mux.report", state_report("working"));
    assert_eq!(replay, first);
    assert_eq!(backend.reports_seen(), 1);

    let conflicting = client.call("mux.report", state_report("idle"));
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
    let backend = MuxBackend::new(MUX_PRIMITIVES);
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
        let response = client.call("mux.report", params.clone());
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
        metadata_agents: Vec::new(),
        containers: Vec::new(),
        lifecycle_subjects: Vec::new(),
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: Some("main".into()),
            number: Some(1),
            session_path: None,
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
            current_path: None,
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
            registry: None,
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

/// The birth identity a test stages for one pid.
fn staged_identity(pid: i32, start_ticks: u64) -> ProcessIdentity {
    ProcessIdentity {
        boot_id: "boot-test".into(),
        pid,
        start_ticks,
    }
}

/// A verifier that confirms the identities it was handed and refuses every other
/// pid.
///
/// A close reads two processes — the child's own claim and the backend's serving
/// process — and staging them separately is what keeps one from answering for
/// the other. Handing it another identity for a pid it already knows is how a
/// test stages a process that has been replaced under a recycled pid.
#[derive(Default)]
struct StagedVerifier {
    identities: Mutex<BTreeMap<i32, ProcessIdentity>>,
}

impl StagedVerifier {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn staged(self: &Arc<Self>, identity: ProcessIdentity) {
        self.identities
            .lock()
            .expect("staged identities")
            .insert(identity.pid, identity);
    }
}

impl ProcessVerifier for StagedVerifier {
    fn inspect(&self, pid: i32) -> Result<Option<ProcessIdentity>, String> {
        match self.identities.lock().expect("staged identities").get(&pid) {
            Some(identity) => Ok(Some(identity.clone())),
            None => Err(format!("pid {pid} is not staged")),
        }
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
fn socket_agent_get(path: &Path, registration_id: &str) -> Value {
    let mut client = Client::dial(path);
    let id = random_uuid();
    client.send(&request_line(
        &id,
        "agent.get",
        json!({"registration_id":registration_id}),
    ));
    let mut line = String::new();
    let bytes = client.reader.read_line(&mut line).unwrap_or(0);
    if bytes == 0 {
        json!({"error":{"code":"connection_closed"}})
    } else {
        serde_json::from_str(&line).expect("daemon response")
    }
}

/// `agent.get` serves the claimed birth identity exactly, and no `claim` key at
/// all when nothing was claimed. Pane matching depends on the exact tuple.
#[test]
fn agent_get_serves_the_claimed_birth_identity_only_when_claimed() {
    let sandbox = Sandbox::new("agent-claim");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let process = ProcessIdentity {
        boot_id: "boot-claim".into(),
        pid: 5151,
        start_ticks: 77,
    };
    let claimed = client.call(
        "agent.register",
        registration_body(
            "6a2f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
            Some(process.clone()),
            None,
        ),
    );
    let claimed_id = registration_id(&claimed);
    let unclaimed = client.call(
        "agent.register",
        registration_body("7b3f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None, None),
    );
    let unclaimed_id = registration_id(&unclaimed);

    let served = client.call("agent.get", json!({"registration_id": claimed_id}));
    assert_eq!(
        served["result"]["agent"]["process"]["claim"],
        json!({"boot_id": "boot-claim", "pid": 5151, "start_ticks": 77}),
        "{served}"
    );
    let absent = client.call("agent.get", json!({"registration_id": unclaimed_id}));
    assert!(
        absent["result"]["agent"]["process"].get("claim").is_none(),
        "{absent}"
    );
    daemon.stop();
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
    let target_id = registration_id(&registered);
    let verifier_path = daemon.path().to_path_buf();
    drop(setup);
    let getter = thread::spawn(move || {
        let mut client = Client::dial(&verifier_path);
        let id = random_uuid();
        client.send(&request_line(
            &id,
            "agent.get",
            json!({"registration_id":target_id}),
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
        written["result"]["registration"]["registration_id"].is_string(),
        "{written}"
    );
    let started = Instant::now();
    daemon.stop();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown waited on verifier"
    );
    let answer = getter.join().unwrap();
    // An answer naming the reason beats a bare closed connection: the caller
    // learns the request was not dispatched and may ask again. What matters
    // here is that the stop did not wait for the blocked verification.
    assert_eq!(error_code(&answer), "busy", "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stopping"),
        "{answer}"
    );
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
    let target = registration_id(&registered);
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
        written["result"]["registration"]["registration_id"].is_string(),
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
    assert_eq!(error_code(&answer), "busy", "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stopping"),
        "{answer}"
    );
}

#[test]
fn events_read_long_poll_expiry_validation_and_shutdown() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let sandbox = Sandbox::new("events-read");
    let mut daemon = sandbox.daemon();
    let path = daemon.path().to_path_buf();
    let mut client = Client::dial(&path);
    let initial = client.call("events.read", json!({"after":0,"limit":10}));
    assert!(initial["result"]["events"].as_array().unwrap().is_empty());
    let empty_wait = client.call("events.read", json!({"after":0,"wait_ms":50}));
    assert!(
        empty_wait["result"]["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let (ready_tx, ready_rx) = mpsc::channel();
    let (answer_tx, answer_rx) = mpsc::channel();
    let reader_path = path.clone();
    let waiter = std::thread::spawn(move || {
        let mut client = Client::dial(&reader_path);
        client.send(&request_line(
            "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
            "events.read",
            json!({"after":0,"wait_ms":900}),
        ));
        ready_tx.send(()).unwrap();
        answer_tx.send(client.read()).unwrap();
    });
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("reader started");
    std::thread::sleep(Duration::from_millis(50));
    let registered = client.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None, None),
    );
    assert!(registered["result"]["registration"]["registration_id"].is_string());
    let answer = answer_rx
        .recv_timeout(Duration::from_millis(500))
        .expect("event wakes reader");
    assert_eq!(
        answer["result"]["events"][0]["kind"], "agent.registered",
        "{answer}"
    );
    waiter.join().unwrap();

    let epoch_mismatch = client.call("events.read", json!({"after":0,"epoch":"wrong"}));
    assert!(epoch_mismatch["result"]["cursor_expired"].is_object());
    let future_cursor = client.call("events.read", json!({"after":99}));
    assert!(future_cursor["result"]["cursor_expired"].is_object());
    let unknown = client.call("events.read", json!({"unexpected":true}));
    assert_eq!(error_code(&unknown), "bad_params");
    let bad_limit = client.call("events.read", json!({"limit":0}));
    assert_eq!(error_code(&bad_limit), "bad_params");

    let (ready_tx, ready_rx) = mpsc::channel();
    let shutdown_path = path.clone();
    let waiter = std::thread::spawn(move || {
        let mut client = Client::dial(&shutdown_path);
        client.send(&request_line(
            "9a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
            "events.read",
            json!({"after":1,"wait_ms":900}),
        ));
        ready_tx.send(()).unwrap();
        let start = Instant::now();
        let result = client.read();
        (result, start.elapsed())
    });
    ready_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("shutdown reader started");
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    daemon.stop();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "shutdown waited out the long poll"
    );
    // The stop ends the wait rather than letting it run to its own expiry, and
    // the waiter is answered rather than cut: an empty read is a normal result
    // (`more: false`), so a client loops and learns the daemon is stopping from
    // its next call — which this daemon answers `busy`.
    let (answer, elapsed) = waiter
        .join()
        .expect("the waiting read is answered, not left hanging");
    assert!(answer["result"]["events"].is_array(), "{answer}");
    assert_eq!(answer["result"]["more"], false, "{answer}");
    assert!(
        elapsed < Duration::from_millis(500),
        "the wait ran to its own expiry instead of ending with the stop: {elapsed:?}"
    );
}

/// A bounded prefix resumes where it stopped: the cursor names the last event
/// delivered, the head and the remaining count are separate facts, and nothing
/// is skipped on the way to the end.
#[test]
fn a_bounded_event_read_resumes_without_skipping_events() {
    let sandbox = Sandbox::new("events-cursor");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let incarnations = [
        "3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b01",
        "3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b02",
        "3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b03",
        "3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b04",
        "3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b05",
    ];
    for incarnation in incarnations {
        let registered = client.call("agent.register", registration_body(incarnation, None, None));
        assert!(registered["result"]["registration"]["registration_id"].is_string());
    }

    let mut seen: Vec<u64> = Vec::new();
    let mut after = 0;
    loop {
        let page = client.call("events.read", json!({"after": after, "limit": 2}));
        let events = page["result"]["events"].as_array().expect("events");
        assert!(
            !events.is_empty(),
            "the journal has more to deliver: {page}"
        );
        for event in events {
            seen.push(event["seq"].as_u64().expect("a sequence"));
        }
        let cursor = page["result"]["cursor"]["sequence"]
            .as_u64()
            .expect("a cursor");
        assert_eq!(
            cursor,
            events.last().expect("an event")["seq"].as_u64().unwrap(),
            "the cursor is the last event delivered: {page}"
        );
        assert_eq!(
            page["result"]["tail"].as_u64().unwrap() > cursor,
            page["result"]["more"] == json!(true),
            "{page}"
        );
        after = cursor;
        if page["result"]["more"] != json!(true) {
            break;
        }
    }
    assert_eq!(
        seen.len(),
        incarnations.len(),
        "every event was delivered once"
    );
    assert!(
        seen.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "a resumed read skipped nothing: {seen:?}"
    );

    // At the head there is nothing to resume from: an empty read says so rather
    // than reporting a position that does not exist.
    let done = client.call("events.read", json!({"after": after}));
    assert_eq!(done["result"]["events"], json!([]));
    assert_eq!(done["result"]["more"], json!(false), "{done}");
    assert_eq!(done["result"]["cursor"]["sequence"], json!(after));
    daemon.stop();
}

#[test]
fn daemon_journals_committed_registration_once_without_private_launch_data() {
    let sandbox = Sandbox::new("journal-registration");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let secret = "/private/journal-secret.json";
    let registered = client.call("agent.register", registration_body(
        "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None,
        Some(json!({"executable":"/usr/bin/pi","argv":[secret],"cwd":"/tmp","session":{"path":secret},"provenance":"test","revision":"r1"})),
    ));
    assert!(registered["result"]["registration"]["registration_id"].is_string());
    let journal: serde_json::Value = serde_json::from_slice(
        &fs::read(sandbox.state().join("events.json")).expect("journal file"),
    )
    .expect("journal JSON");
    let events = journal["events"].as_array().expect("events");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "agent.registered");
    assert_eq!(events[0]["registration_id"], registration_id(&registered));
    let handle = client.call(
        "agent.acquire",
        json!({
            "registration_id": registration_id(&registered), "channel": "execution",
            "publisher": {"source":"child", "incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"}
        }),
    )["result"]["writer"]["handle"]
        .as_str()
        .unwrap()
        .to_string();
    let publish = json!({
        "registration_id": registration_id(&registered), "channel":"execution", "writer_handle":handle,
        "sequence":1, "snapshot":channel_snapshot("idle")
    });
    assert!(client.call("agent.publish", publish.clone())["result"].is_object());
    assert!(client.call("agent.publish", publish)["result"].is_object());
    let serialized = serde_json::to_string(&journal).unwrap();
    assert!(!serialized.contains(secret));
    let after_publish: serde_json::Value = serde_json::from_slice(
        &fs::read(sandbox.state().join("events.json")).expect("journal file"),
    )
    .expect("journal JSON");
    let published_events = after_publish["events"].as_array().unwrap();
    assert_eq!(published_events.len(), 3);
    assert_eq!(published_events[1]["kind"], "publication.writer");
    assert_eq!(published_events[2]["kind"], "execution.published");
    let replay = client.call("agent.register", registration_body(
        "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None,
        Some(json!({"executable":"/usr/bin/pi","argv":[secret],"cwd":"/tmp","session":{"path":secret},"provenance":"test","revision":"r1"})),
    ));
    assert_eq!(
        replay["result"]["registration"]["registration_id"],
        events[0]["registration_id"]
    );
    let after_replay: serde_json::Value = serde_json::from_slice(
        &fs::read(sandbox.state().join("events.json")).expect("journal file"),
    )
    .expect("journal JSON");
    assert_eq!(after_replay["events"].as_array().unwrap().len(), 3);
    daemon.stop();
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
        registered["result"]["registration"]["registration_id"].is_string(),
        "{registered}"
    );
    no_secret(&registered, launch_secret);
    let registration_id = registration_id(&registered);
    let acquired = client.call(
        "agent.acquire",
        json!({
            "registration_id":registration_id,"channel":"execution",
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
            "registration_id":registration_id,"channel":"execution","writer_handle":handle,"sequence":1,
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
            "registration_id":registration_id,"channel":"execution","writer_handle":handle,"sequence":1,
            "snapshot":channel_snapshot("idle")
        }),
    );
    assert!(replay["result"].is_object(), "equal replay: {replay}");
    assert_eq!(replay["result"]["channel"]["writer"]["handle"], handle);
    assert_eq!(replay["result"]["channel"]["snapshot"]["sequence"], 1);
    let get = client.call("agent.get", json!({"registration_id":registration_id}));
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
        json!({"registration_id":registration_id,"channel":"execution","writer_handle":handle}),
    );
    assert!(retired["result"].is_object(), "{retired}");
    let fenced = client.call(
        "agent.publish",
        json!({
            "registration_id":registration_id,"channel":"execution","writer_handle":handle,"sequence":2,
            "snapshot":channel_snapshot("working")
        }),
    );
    assert_eq!(error_code(&fenced), "refused");
    daemon.stop();
}

#[test]
fn managed_parent_handoff_is_published_received_and_acknowledged() {
    // The handoff parties are real managed children: each is bound by the token its
    // launch carried, and its own Pi session is recorded when it first reports it.
    let Some(server) = TmuxServer::start("managed-parent-handoff") else {
        return;
    };
    let sandbox = Sandbox::new("managed-parent-handoff");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let backend = [
        ("RADAR_CONTROL_BACKEND", "tmux"),
        ("RADAR_TMUX_SERVER", server.name.as_str()),
    ];
    let mut daemon = DaemonChild::spawn_with(&socket, &state, &backend);
    let mut lines = BufReader::new(daemon.process.stdout.take().expect("stdout")).lines();
    lines.next().expect("greeting line").expect("greeting");
    let mut client = Client::dial(&socket);

    let observed = client.call("mux.observe", json!({}));
    let root_pane = observed["result"]["inventory"]["panes"][0]["location"]["pane_id"]
        .as_str()
        .expect("root pane")
        .to_string();
    let mut body = registration_body(PARENT_INCARNATION, None, None);
    body["location"] = json!({"backend":"tmux", "pane":root_pane});
    let root = registration_id(&client.call("agent.register", body));
    let root_session = random_uuid();
    publish_session_context(&mut client, &root, PARENT_INCARNATION, &root_session);

    let grant = |client: &mut Client,
                 agent: &str,
                 incarnation: &str,
                 session: &str,
                 claim: &str,
                 pane: &str| {
        let mut report =
            registration_body(incarnation, Some(captured_identity(&state, claim)), None);
        report["location"] =
            json!({"backend": "tmux", "pane": format!("tmux:{}:{pane}", server.name)});
        report["registration_id"] = json!(agent);
        report["session_id"] = json!(session);
        let claim_id = random_uuid();
        report["claim_id"] = json!(claim_id);
        let granted = client.call("agent.presence", report.clone());
        assert_eq!(
            granted["result"]["standing"], "granted",
            "{granted}; {report}"
        );
        claim_id
    };
    let (parent_subject, parent_agent, parent_pane) =
        spawned_live_tmux_child(&mut client, &state, &root, &root_session, Value::Null);
    let parent_session = random_uuid();
    grant(
        &mut client,
        &parent_agent,
        OTHER_INCARNATION,
        &parent_session,
        &parent_subject,
        &parent_pane,
    );

    let (worker_subject, worker_agent, worker_pane) = spawned_live_tmux_child(
        &mut client,
        &state,
        &parent_agent,
        &parent_session,
        json!({"parent_subject_id": parent_subject}),
    );
    let worker_session = random_uuid();
    let worker_claim = grant(
        &mut client,
        &worker_agent,
        CHILD_INCARNATION,
        &worker_session,
        &worker_subject,
        &worker_pane,
    );
    let attached = client.call("lifecycle.get", json!({"subject_id": worker_subject}));
    assert_eq!(
        attached["result"]["subject"]["session"],
        json!(worker_session),
        "{attached}"
    );

    let handoff_id = random_uuid();
    let publish = client.call_id(
        &handoff_id,
        "communication.publish",
        json!({
            "sender_registration_id": worker_agent,
            "recipient_subject_id": parent_subject,
            "correlation_id": "herdsman-run-17",
            "summary": "worker completed",
            "content": "full immutable handoff",
        }),
    );
    assert_eq!(publish["result"]["created"], true, "{publish}");
    // The sender names its attempt, not an ordinal position in a chain.
    assert_eq!(
        publish["result"]["handoff"]["sender"]["claim_id"],
        worker_claim
    );
    let replay = client.call_id(
        &handoff_id,
        "communication.publish",
        json!({
            "sender_registration_id": worker_agent,
            "recipient_subject_id": parent_subject,
            "correlation_id": "herdsman-run-17",
            "summary": "worker completed",
            "content": "full immutable handoff",
        }),
    );
    assert_eq!(replay["result"]["created"], false, "{replay}");
    let wrong_target = client.call_id(
        &random_uuid(),
        "communication.publish",
        json!({
            "sender_registration_id": worker_agent,
            "recipient_subject_id": worker_subject,
            "summary": "worker completed",
            "content": "must not be routed to self",
        }),
    );
    assert_eq!(wrong_target["error"]["code"], "refused", "{wrong_target}");
    let inbox = client.call(
        "communication.inbox",
        json!({"registration_id":parent_agent}),
    );
    assert_eq!(
        inbox["result"]["handoff_ids"].as_array().unwrap(),
        &[json!(handoff_id)]
    );
    let retrieved = client.call(
        "communication.get",
        json!({"registration_id":parent_agent,"handoff_id":handoff_id}),
    );
    assert_eq!(
        retrieved["result"]["handoff"]["content"],
        "full immutable handoff"
    );
    let ack = client.call(
        "communication.ack",
        json!({"registration_id":parent_agent,"handoff_id":handoff_id}),
    );
    assert_eq!(ack["result"]["created"], true, "{ack}");
    let read_after_ack = client.call(
        "communication.get",
        json!({"registration_id":parent_agent,"handoff_id":handoff_id}),
    );
    assert_eq!(
        read_after_ack["result"]["handoff"]["content"],
        "full immutable handoff"
    );
    assert!(
        read_after_ack["result"]["receipt"].is_object(),
        "{read_after_ack}"
    );
    let events = client.call("events.read", json!({"after":0,"limit":100}));
    let pointers = events["result"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|event| {
            event["kind"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("communication."))
        })
        .collect::<Vec<_>>();
    assert_eq!(pointers.len(), 2, "{events}");
    assert!(
        pointers
            .iter()
            .all(|event| event["request_id"] == handoff_id)
    );
    assert!(
        !events.to_string().contains("full immutable handoff"),
        "payload leaked into events: {events}"
    );
    let stopped = client.call_id(
        &random_uuid(),
        "lifecycle.stop",
        json!({"request": {
            "spawn_request_id": attached["result"]["subject"]["spawn_request_id"],
            "claim_id": worker_claim,
        }}),
    );
    assert_eq!(result_record(&stopped)["outcome"], "completed", "{stopped}");
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
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
    let id = registration_id(&registered);
    let absent = client.call("agent.get", json!({"registration_id":id}));
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
        json!({"registration_id":id,"publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},"sequence":1,"lease_ms":10000,"context":{"session":SESSION}}),
    );
    assert!(first["result"]["context"].is_object(), "{first}");
    assert_eq!(first["result"]["context"]["context"]["session"], SESSION);
    // No subject is attached to this registration, so Radar names none.
    assert_eq!(
        first["result"]["management"]["state"], "unmanaged",
        "{first}"
    );
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
            "registration_id":id,
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
            "registration_id":id,
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
            "registration_id":id,
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
    let get = client.call("agent.get", json!({"registration_id":id}));
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
    let stale = client.call("agent.get", json!({"registration_id":id}));
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
    let id = registration_id(&registered);
    let first = client.call(
        "agent.context",
        json!({
            "registration_id":id,
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
    let stale = client.call("agent.get", json!({"registration_id":id}));
    assert_eq!(
        stale["result"]["agent"]["context"]["context"]["freshness"],
        "stale"
    );
    let old = first["result"]["writer"].clone();
    let replacement = client.call(
        "agent.context",
        json!({
            "registration_id":id,
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
            "registration_id":id,
            "publisher":{"source":"herdsman","incarnation":"1c2d3e4f-5678-4abc-9def-0123456789ab"},
            "writer_handle":old["handle"],
            "sequence":2,
            "context":{"session":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"}
        }),
    );
    assert_eq!(error_code(&fenced), "refused");
    let accepted = client.call("agent.get", json!({"registration_id":id}));
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
    let id = registration_id(&registered);
    let first = client.call("agent.acquire", json!({"registration_id":id,"channel":"assignment","publisher":{"source":"owner","incarnation":"3c2d3e4f-5678-4abc-9def-0123456789ab","reporting_owner":"owner-A"}}));
    let old = first["result"]["writer"]["handle"]
        .as_str()
        .unwrap()
        .to_owned();
    let report = client.call("agent.publish", json!({"registration_id":id,"channel":"assignment","writer_handle":old,"sequence":1,"snapshot":channel_snapshot("unsettled")}));
    assert!(report["result"].is_object(), "{report}");
    let old_generation = first["result"]["writer"]["generation"].as_u64().unwrap();
    let retired = client.call(
        "agent.retire",
        json!({"registration_id":id,"channel":"assignment","writer_handle":old}),
    );
    assert!(retired["result"].is_object(), "{retired}");
    daemon.stop();
    drop(client);
    let mut daemon =
        Daemon::bind_without_backend(&sandbox.socket(), &sandbox.state()).expect("restart daemon");
    let mut client = Client::dial(daemon.path());
    let restored = client.call("agent.get", json!({"registration_id":id}));
    assert_eq!(
        restored["result"]["agent"]["assignment"]["snapshot"]["freshness"],
        "stale"
    );
    let replacement = client.call("agent.acquire", json!({
        "registration_id":id,"channel":"assignment",
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
    let stale = client.call("agent.publish", json!({"registration_id":id,"channel":"assignment","writer_handle":old,"sequence":2,"snapshot":channel_snapshot("stale-writer")}));
    assert_eq!(error_code(&stale), "refused");
    let reconnect = client.call("agent.publish", json!({"registration_id":id,"channel":"assignment","writer_handle":new_handle,"sequence":1,"snapshot":channel_snapshot("owner-reconnected")}));
    assert!(reconnect["result"].is_object(), "{reconnect}");
    let accepted = client.call("agent.get", json!({"registration_id":id}));
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
    let id = registration_id(&registered);
    daemon.stop();
    fs::write(
        sandbox.state().join("agents").join(format!("{id}.json")),
        b"not json",
    )
    .expect("corrupt record");
    let mut daemon = Daemon::bind_without_backend(&sandbox.socket(), &sandbox.state())
        .expect("bind corrupt store for explicit read failure");
    let mut client = Client::dial(daemon.path());
    let get = client.call("agent.get", json!({"registration_id":id}));
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
            response["result"]["registration"]["registration_id"].is_string(),
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
                entry["registration"]["registration_id"]
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
        stop_recorded_units(&self.root);
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Stops the scopes and slices a test's daemon recorded on the real user manager,
/// and clears the failed state a graceful stop leaves. The state directory names
/// every unit its launches created, so only those are touched.
fn stop_recorded_units(root: &Path) {
    const SUFFIX: &str = "-main.scope";
    let mut units = BTreeSet::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(text) = fs::read_to_string(&path) {
                for (at, _) in text.match_indices(SUFFIX) {
                    let name = &text[..at];
                    if let Some(start) = name.len().checked_sub(6 + 32)
                        && name.is_char_boundary(start)
                        && let id = &name[start + 6..]
                        && name[start..].starts_with("radar-")
                        && id.bytes().all(|b| b.is_ascii_hexdigit())
                    {
                        units.insert(format!("radar-{id}.slice"));
                        units.insert(format!("radar-{id}{SUFFIX}"));
                    }
                }
            }
        }
    }
    if units.is_empty() {
        return;
    }
    for verb in ["stop", "reset-failed"] {
        let _ = Command::new("systemctl")
            .args(["--user", verb])
            .args(&units)
            .output();
    }
}

/// The normalized fleet one fake backend serves, used by the observation tests.
fn observation_fleet() -> FleetObservation {
    serde_json::from_value(json!({
        "workspaces":[{"workspace_id":"wA","label":"main","number":1}],
        "tabs":[{"tab_id":"wA:t1","workspace_id":"wA","label":null,"number":1}],
        "panes":[{"location":{"workspace_id":"wA","tab_id":"wA:t1","pane_id":"wA:p1"},"label":null,"title":null}],
        "agents":[]
    }))
    .expect("normalized fixture")
}

/// Reads the snapshot the daemon has published, waiting for one to exist.
fn published_observation(client: &mut Client) -> Value {
    for _ in 0..40 {
        let answer = client.call(
            "observation.get",
            json!({"wait_ms": 2000, "classes": ["fleet", "processes"]}),
        );
        if let Some(snapshot) = answer["result"].get("snapshot") {
            return snapshot.clone();
        }
    }
    panic!("the daemon published no observation");
}

/// One observation read is a revision every reader shares, naming the revision a
/// caller already holds is answered as not modified, and a revision from another
/// daemon process is expired rather than answered as current.
#[test]
fn observation_reads_are_revisioned_and_never_append_to_the_journal() {
    let sandbox = Sandbox::new("observation-read");
    let backend = Arc::new(FakeBackend::new(
        observation_fleet(),
        FocusAnswer::Completed,
    ));
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let ping = client.call("daemon.ping", json!({}));
    assert!(
        ping["result"]["contracts"]
            .as_array()
            .expect("contracts")
            .contains(&json!("observation.get.v1")),
        "{ping}"
    );
    let head_before = client.call("events.read", json!({"after": 0}))["result"]["tail"].clone();

    let snapshot = published_observation(&mut client);
    let epoch = snapshot["epoch"].as_str().expect("an epoch").to_string();
    let revision = snapshot["revision"].as_u64().expect("a revision");
    assert!(revision >= 1, "{snapshot}");
    // The payload is the daemon's fleet projection, with the last-good inventory
    // under its own name rather than flattened into the envelope.
    assert_eq!(
        snapshot["observation"]["inventory"]["workspaces"][0]["workspace_id"],
        "wA"
    );
    // One source per observation input, each reporting for itself.
    let sources: Vec<&str> = snapshot["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .map(|source| source["name"].as_str().expect("a source name"))
        .collect();
    assert_eq!(sources, ["inventory", "registry", "processes", "tasks"]);
    assert_eq!(snapshot["sources"][0]["state"], "current");
    assert!(snapshot["sources"][0]["last_success_at"].is_string());

    // A reader that already holds this revision is told so rather than sent it
    // again.
    let same = client.call(
        "observation.get",
        json!({"epoch": epoch, "since_revision": revision, "classes": ["fleet"]}),
    );
    assert!(
        same["result"].get("not_modified").is_some(),
        "expected not_modified, got {same}"
    );

    // A revision from another process is not this daemon's revision 7.
    let other = client.call(
        "observation.get",
        json!({"epoch": "11111111-1111-4111-8111-111111111111", "since_revision": revision}),
    );
    assert!(
        other["result"].get("snapshot_expired").is_some(),
        "expected snapshot_expired, got {other}"
    );

    // Publishing revisions is not a durable event.
    let head_after = client.call("events.read", json!({"after": 0}))["result"]["tail"].clone();
    assert_eq!(
        head_before, head_after,
        "publishing a revision must not append to the durable journal"
    );
    daemon.stop();
}

/// Nothing is sampled while nobody asks, so a daemon with no display attached
/// collects nothing — and the first read cannot be handed a snapshot its own
/// demand caused.
#[test]
fn observation_samples_nothing_before_a_reader_asks() {
    let sandbox = Sandbox::new("observation-idle");
    let backend = Arc::new(FakeBackend::new(
        observation_fleet(),
        FocusAnswer::Completed,
    ));
    let mut daemon = sandbox.daemon_with(backend);
    // Long enough that a sampler collecting on its own would have published.
    std::thread::sleep(Duration::from_millis(400));
    let mut client = Client::dial(daemon.path());

    let first = client.call("observation.get", json!({"classes": ["fleet"]}));
    assert_eq!(
        first["result"]["not_modified"]["revision"], 0,
        "a daemon with no reader must not have sampled: {first}"
    );

    let snapshot = published_observation(&mut client);
    assert!(snapshot["revision"].as_u64().expect("a revision") >= 1);
    daemon.stop();
}

/// The read is bounded and its parameters are checked rather than guessed at.
#[test]
fn observation_refuses_unknown_classes_and_unbounded_waits() {
    let sandbox = Sandbox::new("observation-params");
    let backend = Arc::new(FakeBackend::new(
        observation_fleet(),
        FocusAnswer::Completed,
    ));
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let unknown = client.call("observation.get", json!({"classes": ["processors"]}));
    assert_eq!(error_code(&unknown), "bad_params", "{unknown}");
    assert!(
        unknown["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("processors")
    );

    let long = client.call("observation.get", json!({"wait_ms": 600_000}));
    assert_eq!(error_code(&long), "bad_params", "{long}");
    daemon.stop();
}

/// A daemon with no backend has nothing to observe, and says so rather than
/// publishing an empty fleet as a successful reading.
#[test]
fn observation_refuses_without_a_backend_instead_of_publishing_an_empty_fleet() {
    let sandbox = Sandbox::new("observation-no-backend");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());

    let answer = client.call("observation.get", json!({}));
    assert_eq!(error_code(&answer), "backend_unavailable", "{answer}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("nothing to publish")
    );
    daemon.stop();
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

    /// The first snapshot this daemon publishes, asking until it has one.
    ///
    /// A first read is demand alone: the revision it is answered with is the one
    /// that existed before it, so the snapshot its demand caused arrives on a
    /// later read. Nothing here invents a fleet to answer with.
    fn first_snapshot(&mut self) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let answer = self.call(
                "observation.get",
                json!({"since_revision": 0, "wait_ms": 1000}),
            );
            if let Some(snapshot) = answer["result"].get("snapshot") {
                return snapshot.clone();
            }
            assert!(
                Instant::now() < deadline,
                "no snapshot was published: {answer}"
            );
        }
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

fn process_identity_from_proc(pid: i32) -> ProcessIdentity {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).expect("process stat");
    let close = stat.rfind(')').expect("comm closing delimiter");
    let start_ticks = stat[close + 2..]
        .split_whitespace()
        .nth(19)
        .expect("stat starttime field")
        .parse()
        .expect("numeric starttime");
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .expect("boot id")
        .trim()
        .to_string();
    ProcessIdentity {
        boot_id,
        pid,
        start_ticks,
    }
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
    const SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let mut body = registration_body(incarnation, None, None);
    body["location"] = json!({
        "backend": "herdr",
        "workspace": "wA",
        "tab": "wA:t1",
        "pane": pane,
    });
    let response = client.call("agent.register", body);
    let id = registration_id(&response);
    let context = client.call("agent.context", json!({"registration_id":id,"publisher":{"source":"test-publisher","incarnation":incarnation},"sequence":1,"lease_ms":10000,"context":{"session":SESSION}}));
    assert!(
        context.get("result").is_some(),
        "context publish failed: {context}"
    );
    id
}

/// The spawn request one caller sends: a parent subject and the command it
/// resolved for the child.
fn spawn_params(parent: &str, executable: &str, argv: &[&str]) -> Value {
    json!({
        "request": {
            "parent": parent,
            "parent_session_id": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
            "executable": executable,
            "argv": argv,
        },
        "requester": "test",
    })
}

#[test]
fn a_caller_without_a_claim_reads_no_launches_and_names_nothing_else() {
    let sandbox = Sandbox::new("spawn-list-caller");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p2"]);
    let daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    spawned_edge(&mut client, FIRST_EDGE, &parent);

    // A registration that holds no claim has no authority to read launches, and
    // the parent is never a parameter it can supply.
    let refused = client.call(
        "spawn.list",
        json!({"registration_id": "0b000000-0000-4000-8000-000000000001"}),
    );
    assert_eq!(refused["error"]["code"], json!("refused"), "{refused}");

    let named = client.call(
        "spawn.list",
        json!({
            "registration_id": "0b000000-0000-4000-8000-000000000001",
            "parent_subject_id": "3f8c1d20-1111-4a2b-8c4d-000000000001",
        }),
    );
    assert_eq!(named["error"]["code"], json!("bad_params"), "{named}");

    // Without a caller the page is the unfiltered topology read it always was.
    let page = client.call("spawn.list", json!({}));
    assert_eq!(request_ids(&page), vec![FIRST_EDGE]);
}

#[test]
fn a_subject_filtered_page_holds_only_that_subject_and_filters_before_cutting() {
    use agent_radar::control_plane::registry::{Registry, SpawnRequest};

    let sandbox = Sandbox::new("spawn-subject-filter");
    let registry = Registry::open(&sandbox.state()).expect("a registry");
    let mine = "3f8c1d20-1111-4a2b-8c4d-000000000001";
    let other = "3f8c1d20-2222-4a2b-8c4d-000000000002";
    let session = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let parent = registry
        .register(
            &agent_radar::control_plane::registry::RegistrationRequest {
                source: "filter-test".into(),
                incarnation: "3f8c1d20-3333-4a2b-8c4d-000000000003".into(),
                session: Some(session.into()),
                owner: None,
                run: None,
                label: None,
                location: None,
                process: None,
                launch: None,
                spawn_token: None,
            },
            1,
        )
        .expect("a parent registration");
    registry
        .publish_context(
            &agent_radar::control_plane::registry::ContextRequest {
                registration_id: parent.registration_id.clone(),
                publisher: agent_radar::control_plane::registry::PublisherIdentity {
                    source: parent.request.source.clone(),
                    incarnation: parent.request.incarnation.clone(),
                    reporting_owner: None,
                },
                writer_handle: None,
                replace: None,
                sequence: 1,
                lease_ms: Some(60_000),
                observed_at: None,
                context: agent_radar::control_plane::registry::ContextValue {
                    session: Some(session.into()),
                    launch_identity: None,
                },
            },
            2,
        )
        .expect("a session context");
    let mut ids = Vec::new();
    for (request_id, subject) in [
        ("0a000001-0000-4000-8000-000000000001", Some(mine)),
        ("0a000002-0000-4000-8000-000000000002", Some(other)),
        ("0a000003-0000-4000-8000-000000000003", Some(mine)),
        ("0a000004-0000-4000-8000-000000000004", None),
    ] {
        let request = SpawnRequest {
            request_id: request_id.into(),
            parent: parent.registration_id.clone(),
            parent_session_id: session.into(),
            parent_subject_id: subject.map(str::to_string),
        };
        registry.record_spawn(&request, 1).expect("an edge");
        ids.push(request_id);
    }

    // The other subject's edge sits between this subject's two, so a filter that
    // ran after the page was cut would return a short or wrong page.
    let first = registry
        .spawn_edges_for_subject_after(mine, None, 1)
        .expect("first page");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].request_id, ids[0]);
    let second = registry
        .spawn_edges_for_subject_after(mine, Some(ids[0]), 10)
        .expect("second page");
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].request_id, ids[2]);
    assert!(
        registry
            .spawn_edges_for_subject_after(other, None, 10)
            .expect("other page")
            .iter()
            .all(|edge| edge.request_id == ids[1])
    );
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
        "spawn.create",
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

/// Waits until a tracked launch's bootstrap has been released to run its program.
///
/// A registration is accepted only from a launch the daemon captured, and the
/// capture is asynchronous to the spawn's own answer, so a test that registers
/// its child waits for that durable fact first.
fn await_captured_launch(state: &Path, request_id: &str) {
    let subject = recorded_edge(state, request_id)["claim_id"]
        .as_str()
        .expect("tracked launch subject")
        .to_string();
    for _ in 0..500 {
        if let Ok(registry) = agent_radar::control_plane::Registry::open(state)
            && agent_radar::control_plane::lifecycle::read(&registry, &subject).is_ok_and(
                |record| {
                    record.is_some_and(|subject| {
                        subject.process.state
                            == agent_radar::control_plane::lifecycle::State::Captured
                    })
                },
            )
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("launch {subject} was never captured");
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

fn registration_id(response: &Value) -> String {
    response["result"]["registration"]["registration_id"]
        .as_str()
        .expect("public agent id")
        .to_owned()
}

fn result_record(response: &Value) -> &Value {
    &response["result"]["request"]
}

/// The ids `mux.request.list` reports as unfinished: what the stop gate counts.
fn unfinished_ids(response: &Value) -> Vec<String> {
    response["result"]["requests"]
        .as_array()
        .expect("a list of records")
        .iter()
        .filter_map(|record| record["id"].as_str().map(str::to_owned))
        .collect()
}

/// The daemon answers liveness with what it is, so a client can tell a daemon
/// with no backend from a daemon of another version before asking it anything.
#[test]
fn ping_answers_the_revision_and_backend_contracts_without_a_mux() {
    let sandbox = Sandbox::new("ping");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let response = client.call("daemon.ping", json!({}));
    assert_eq!(response["result"]["protocol"], PROTOCOL_VERSION);
    assert_eq!(
        response["result"]["revision"],
        json!(agent_radar::control_plane::ops::PROTOCOL_REVISION)
    );
    assert_eq!(response["result"]["backend"], "none");
    assert_eq!(
        response["result"]["contracts"],
        json!([
            "registry.v1",
            "registry.facts.v1",
            "events.v1",
            "topology.get.v1",
            "topology.list.v1",
            // Abandonment and observation are daemon-owned and need no mux.
            "lifecycle.abandon.v1",
            "observation.get.v1",
            "communication.handoff.v1"
        ])
    );
    let unsupported = client.call(
        "spawn.close",
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

    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["backend"], "fake");
    assert_eq!(ping["result"]["instance"], Value::Null);
    assert_eq!(
        ping["result"]["contracts"],
        advertised(
            Primitives::OBSERVE
                .union(Primitives::PROCESS_INFO)
                .union(Primitives::FOCUS),
        )
    );
    let observed = client.call("mux.observe", json!({}));
    assert_eq!(
        observed["result"]["inventory"]["workspaces"][0]["workspace_id"],
        "wA"
    );
    assert!(observed["result"]["inventory"].get("raw").is_none());
    let process = client.call("mux.process_info", json!({"pane_id":"wA:p1"}));
    assert_eq!(process["result"]["evidence"]["kind"], "non_shell");
    assert_eq!(process["result"]["evidence"]["pid"], 200);

    let id = random_uuid();
    let request = json!({"target":"wA:p1","target_kind":"pane"});
    let first = client.call_id(&id, "mux.focus", request.clone());
    assert!(first.get("result").is_some(), "{first}");
    assert_eq!(result_record(&first)["outcome"], "completed");
    let retry = client.call_id(&id, "mux.focus", request.clone());
    assert_eq!(result_record(&retry)["id"], id);
    let changed = client.call_id(
        &id,
        "mux.focus",
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
    let response = client.call("mux.focus", json!({"target":"pane:1","target_kind":"pane"}));
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

    client
        .send(&json!({"version": 7, "id": id, "method": "daemon.ping", "params": {}}).to_string());
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
    client.send(&json!({"id": id, "method": "daemon.ping"}).to_string());
    assert_eq!(error_code(&client.read()), "bad_request");

    // Neither refusal ended the connection.
    assert_eq!(
        client.call("daemon.ping", json!({}))["result"]["protocol"],
        PROTOCOL_VERSION
    );
    daemon.stop();
}

/// The first wire is refused rather than aliased: a version-1 client is told the
/// version this daemon serves, and a name that version used performs nothing.
#[test]
fn the_version_one_wire_is_refused_without_aliases() {
    let sandbox = Sandbox::new("wire-v1");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    client.send(
        &json!({"version": 1, "id": id, "method": "focus",
            "params": {"target":"wA:p1","target_kind":"pane"}})
        .to_string(),
    );
    let refused = client.read();
    assert_eq!(refused["id"], id);
    assert_eq!(error_code(&refused), "bad_version");
    assert!(
        refused["error"]["message"]
            .as_str()
            .expect("a message")
            .contains("serves 2"),
        "the refusal names the version that is served: {refused}"
    );

    for removed in [
        "ping",
        "observe",
        "process_info",
        "output",
        "focus",
        "close",
        "create",
        "input",
        "report",
        "request",
        "requests",
        "spawn",
        "child.close",
    ] {
        let refused = client.call(removed, json!({}));
        assert_eq!(
            error_code(&refused),
            "unknown_method",
            "{removed}: {refused}"
        );
    }

    // No name above reached a backend, and none left a record to act on later.
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(backend.reports_seen(), 0);
    assert_eq!(edges_written(&sandbox.state()), 0);
    let store = Store::open(&sandbox.state()).expect("the state");
    assert!(
        store.recent(10).expect("the records").is_empty(),
        "a refused name is not an operation"
    );
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
    assert_eq!(
        client.call("daemon.ping", json!({}))["result"]["protocol"],
        PROTOCOL_VERSION
    );
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

    assert_eq!(
        client.call("daemon.ping", json!({}))["result"]["protocol"],
        PROTOCOL_VERSION
    );
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
    let response = client.call_id(&id, "mux.close", params);
    let record = result_record(&response);
    assert_eq!(record["id"], id);
    assert_eq!(record["method"], "mux.close");
    assert_eq!(record["requester"], "a test");
    assert_eq!(record["target"], "pane:3");
    assert_eq!(record["state"], "completed");
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], Category::BACKEND_UNAVAILABLE);
    assert!(
        record["message"]
            .as_str()
            .expect("a message")
            .contains("`mux.close`"),
        "{record}"
    );
    // Nothing ran, and the record is the account of that.
    assert_eq!(record["effects"], json!([]));
    assert!(record["requested_at"].is_string());
    assert!(record["expires_at"].is_string());
    assert!(record["completed_at"].is_string());

    // Readable through the protocol, by id.
    let fetched = client.call("mux.request.get", json!({"id": id}));
    assert_eq!(result_record(&fetched)["id"], id);
    let missing = client.call("mux.request.get", json!({"id": random_uuid()}));
    assert_eq!(error_code(&missing), "not_found");
    let unnamed = client.call("mux.request.get", json!({}));
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
    let fetched = client.call("mux.request.get", json!({"id": id}));
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
    let first = client.call_id(&id, "mux.close", frozen_pane("pane:3"));
    assert_eq!(result_record(&first)["method"], "mux.close");
    let same = client.call_id(&id, "mux.close", frozen_pane("pane:3"));
    assert_eq!(result_record(&same)["method"], "mux.close");
    assert_eq!(result_record(&same)["params"], frozen_pane("pane:3"));
    let changed = client.call_id(
        &id,
        "mux.focus",
        json!({"target": "pane:4", "target_kind":"pane"}),
    );
    assert_eq!(error_code(&changed), "refused");
    let stored = client.call("mux.request.get", json!({"id": id}));
    assert_eq!(result_record(&stored)["method"], "mux.close");
    assert_eq!(result_record(&stored)["target"], "pane:3");
    // Same id, same target, different contents: the occupant differs, so the
    // stored request does not describe this one.
    let mut changed_contents = frozen_pane("pane:3");
    changed_contents["request"]["identity"]["pane"]["occupant"] =
        json!({"name": "pi", "managed": false});
    let reordered = client.call_id(&id, "mux.close", changed_contents);
    assert_eq!(error_code(&reordered), "refused");
    daemon.stop();
}

/// An operator can end an unresolved record's recovery obligation. The target it
/// held becomes operable again, and the record still reports the uncertainty it
/// always did: abandoning never establishes what the operation did.
#[test]
fn an_operator_abandonment_releases_the_target_and_keeps_the_record_unresolved() {
    let sandbox = Sandbox::new("abandon");
    let mut daemon = sandbox.daemon();
    let state = Store::open(&sandbox.state()).expect("open the state");
    let held = random_uuid();
    let record = RequestRecord::new(
        &held,
        "mux.close",
        Some("radar".into()),
        Some("pane:9".into()),
        now_ms(),
    );
    state.create(&record).expect("create the held request");
    state
        .write(&record.claim().start())
        .expect("start the held request");

    let mut client = Client::dial(daemon.path());
    // While it is unresolved the target is held against a later mutation, and the
    // stop gate counts it as unfinished work.
    let refused = client.call_id(
        &random_uuid(),
        "mux.focus",
        json!({"target": "pane:9", "target_kind": "pane"}),
    );
    assert_eq!(error_code(&refused), "refused", "{refused}");
    let gate = client.call("mux.request.list", json!({"unfinished": true}));
    assert!(
        unfinished_ids(&gate).contains(&held),
        "the stop gate counts the unresolved record: {gate}"
    );

    let answer = client.call("lifecycle.abandon", json!({"request_id": held}));
    assert_eq!(
        result_record(&answer)["outcome"],
        "completed",
        "the abandonment itself is settled: {answer}"
    );

    // The record is still unresolved in every read, and says so with its own
    // uncertainty plus the abandonment.
    let after = client.call("mux.request.get", json!({"id": held}));
    assert_eq!(result_record(&after)["outcome"], "unknown");
    assert_eq!(result_record(&after)["state"], "completed");
    assert!(result_record(&after)["abandoned_at"].is_string(), "{after}");
    // The stop gate no longer counts it: its recovery obligation is over.
    let gate = client.call("mux.request.list", json!({"unfinished": true}));
    assert!(
        !unfinished_ids(&gate).contains(&held),
        "an abandoned record must not hold the stop gate: {gate}"
    );

    // And the target it held is operable again: the next request is no longer
    // refused for an in-flight record, it is answered on its own merits.
    let allowed = client.call_id(
        &random_uuid(),
        "mux.focus",
        json!({"target": "pane:9", "target_kind": "pane"}),
    );
    assert_eq!(
        result_record(&allowed)["category"],
        Category::BACKEND_UNAVAILABLE,
        "{allowed}"
    );
    daemon.stop();
}

/// Abandoning is for unresolved records only: a record nothing ran for, or one
/// with a definite outcome, has no recovery obligation to end.
#[test]
fn abandoning_a_record_that_never_ran_or_already_settled_is_refused() {
    let sandbox = Sandbox::new("abandon-refused");
    let mut daemon = sandbox.daemon();
    let state = Store::open(&sandbox.state()).expect("open the state");
    let pending = random_uuid();
    let record = RequestRecord::new(
        &pending,
        "mux.close",
        Some("radar".into()),
        Some("pane:9".into()),
        now_ms(),
    );
    state.create(&record).expect("create the pending request");

    let mut client = Client::dial(daemon.path());
    // Nothing is dispatched, so this is a definite refusal about the record and
    // the daemon answers with a settled record rather than an unresolved one.
    let refused = client.call("lifecycle.abandon", json!({"request_id": pending}));
    let record = result_record(&refused);
    assert_eq!(record["outcome"], "refused", "{refused}");
    assert_eq!(record["category"], Category::REFUSED, "{refused}");
    assert!(
        record["message"]
            .as_str()
            .expect("a message")
            .contains("never claimed"),
        "{refused}"
    );
    // The record it refused to abandon is untouched.
    let after = client.call("mux.request.get", json!({"id": pending}));
    assert_eq!(result_record(&after)["state"], "pending");
    assert!(result_record(&after)["abandoned_at"].is_null(), "{after}");

    // The same answer for a record with a definite outcome, and for one this
    // daemon has never heard of.
    let settled = random_uuid();
    let done = RequestRecord::new(
        &settled,
        "mux.close",
        Some("radar".into()),
        Some("pane:8".into()),
        now_ms(),
    )
    .complete(RequestOutcome::Completed, None, None, Vec::new(), now_ms());
    state.create(&done).expect("create the settled request");
    state.write(&done).expect("settle the request");
    let refused = client.call("lifecycle.abandon", json!({"request_id": settled}));
    assert_eq!(result_record(&refused)["outcome"], "refused", "{refused}");
    assert!(
        result_record(&refused)["message"]
            .as_str()
            .expect("a message")
            .contains("definite outcome"),
        "{refused}"
    );

    let missing = random_uuid();
    let refused = client.call("lifecycle.abandon", json!({"request_id": missing}));
    assert_eq!(result_record(&refused)["outcome"], "refused", "{refused}");
    assert!(
        result_record(&refused)["message"]
            .as_str()
            .expect("a message")
            .contains("no request"),
        "{refused}"
    );
    daemon.stop();
}

/// Reconciliation is an explicit pass over unresolved records. It settles only
/// what independent evidence establishes, and reports the rest with a reason
/// rather than guessing or replaying anything.
#[test]
fn reconciliation_reports_what_it_cannot_establish_and_settles_nothing_by_guesswork() {
    let sandbox = Sandbox::new("reconcile");
    let mut daemon = sandbox.daemon();
    let state = Store::open(&sandbox.state()).expect("open the state");
    // One record whose method has no independent evidence source.
    let unprovable = random_uuid();
    let record = RequestRecord::new(
        &unprovable,
        "mux.close",
        Some("radar".into()),
        Some("pane:9".into()),
        now_ms(),
    );
    state.create(&record).expect("create");
    state.write(&record.claim().start()).expect("start");
    // One lifecycle record naming a subject that is not recorded at all.
    let unknown_subject = random_uuid();
    let lifecycle = RequestRecord::new(
        &random_uuid(),
        "lifecycle.stop",
        Some("radar".into()),
        Some(unknown_subject.clone()),
        now_ms(),
    );
    state.create(&lifecycle).expect("create");
    state.write(&lifecycle.claim().start()).expect("start");

    let mut client = Client::dial(daemon.path());
    let answer = client.call("lifecycle.reconcile", json!({}));
    let outcome = &answer["result"]["reconciled"];
    assert_eq!(outcome["trigger"], "operator_request", "{answer}");
    assert_eq!(outcome["resolved"], 0, "{answer}");
    let reported = outcome["unresolved"].as_array().expect("a report");
    assert!(
        reported
            .iter()
            .any(|entry| entry["request_id"] == unprovable
                && entry["reason"]
                    .as_str()
                    .expect("a reason")
                    .contains("no independent evidence source")),
        "{answer}"
    );
    assert!(
        reported
            .iter()
            .any(|entry| entry["request_id"] == lifecycle.id
                && entry["reason"]
                    .as_str()
                    .expect("a reason")
                    .contains("is not recorded")),
        "{answer}"
    );

    // Neither record was settled, and neither target was released: a reason is
    // not an outcome.
    let after = client.call("mux.request.get", json!({"id": unprovable}));
    assert_eq!(result_record(&after)["state"], "started");
    assert!(result_record(&after)["outcome"].is_null(), "{after}");
    let held = client.call_id(
        &random_uuid(),
        "mux.focus",
        json!({"target": "pane:9", "target_kind": "pane"}),
    );
    assert_eq!(error_code(&held), "refused", "{held}");
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
        "mux.close",
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
        "mux.focus",
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
    let held_record = client.call("mux.request.get", json!({"id": first}));
    assert_eq!(result_record(&held_record)["state"], "pending");
    let refusal = client.call("mux.request.get", json!({"id": second}));
    assert_eq!(result_record(&refusal)["outcome"], "refused");
    assert_eq!(result_record(&refusal)["category"], Category::IN_FLIGHT);

    // Another target is a different request, and is executed normally.
    let other = client.call_id(&random_uuid(), "mux.close", frozen_pane("pane:10"));
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
        ("mux.observe", json!({})),
        ("mux.process_info", json!({"pane_id":"pane:1"})),
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
    let listed = client.call("mux.request.list", json!({}));
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
    let requests = client.call("mux.request.list", json!({}));
    assert_eq!(requests["result"]["requests"], json!([]));
    daemon.stop();
}

#[test]
fn requests_lists_the_newest_first_and_bounded() {
    let sandbox = Sandbox::new("listing");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let older = random_uuid();
    client.call_id(&older, "mux.close", frozen_pane("pane:1"));
    // A later record: the clock has to move for the order to be observable, and
    // a millisecond is the record's resolution.
    thread::sleep(Duration::from_millis(5));
    let newer = random_uuid();
    client.call_id(
        &newer,
        "mux.input",
        json!({"request": {"pane_id": "pane:2", "payload": {"kind": "text", "text": "x"}}}),
    );

    let listed = client.call("mux.request.list", json!({}));
    let records = listed["result"]["requests"].as_array().expect("a list");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["id"], newer);
    assert_eq!(records[1]["id"], older);

    let bounded = client.call("mux.request.list", json!({"limit": 1}));
    let records = bounded["result"]["requests"].as_array().expect("a list");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["id"], newer);

    let refused = client.call("mux.request.list", json!({"limit": "all of them"}));
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
    assert_eq!(
        client.call("daemon.ping", json!({}))["result"]["protocol"],
        PROTOCOL_VERSION
    );

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
        // Herdr starts without a systemd unit, so the serving path runs here.
        let mut child =
            DaemonChild::spawn_with(&socket, &state, &[("RADAR_CONTROL_BACKEND", "herdr")]);
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
        assert_eq!(
            client.call("daemon.ping", json!({}))["result"]["protocol"],
            PROTOCOL_VERSION
        );
        // A record planted over one pane holds that target, so the input below is
        // refused in flight: the daemon writes a durable record under the state
        // path it was handed without asking any runtime to type anything.
        let pane = "wA:p1";
        let store = Store::open(&state).expect("the state");
        store
            .create(&RequestRecord::new(
                &random_uuid(),
                "mux.input",
                None,
                Some(pane.into()),
                now_ms(),
            ))
            .expect("plant a held record");
        let id = random_uuid();
        let refused = client.call_id(
            &id,
            "mux.input",
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

/// Runs `radar daemon` with `environment` added to its own and asserts it refuses
/// to serve: it exits non-zero, reports no greeting, and binds no socket. The
/// operator's diagnostic, from stderr, is returned.
fn refused_startup(name: &str, environment: &[(&str, &str)]) -> String {
    let sandbox = Sandbox::new(name);
    let socket = sandbox.socket();
    let state = sandbox.state();
    let mut child = DaemonChild::spawn_with(&socket, &state, environment);
    let status = child.wait(DEADLINE);
    assert_eq!(status.code(), Some(1), "{status:?}");

    let mut greeting = String::new();
    child
        .process
        .stdout
        .take()
        .expect("stdout")
        .read_to_string(&mut greeting)
        .expect("the greeting");
    assert_eq!(greeting, "", "a refused daemon reported it was serving");

    let mut diagnostic = String::new();
    child
        .process
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut diagnostic)
        .expect("the diagnostic");
    assert!(diagnostic.starts_with("radar daemon: "), "{diagnostic}");
    assert!(!socket.exists(), "a refused daemon bound its socket");
    diagnostic
}

/// The daemon serves the physical backend its configuration names, and refuses
/// startup for a name that is no backend or a configuration its adapter cannot
/// serve. Each refusal says which backend is at fault and which are supported,
/// and none falls back: the process stops rather than serving through a backend
/// nobody configured.
#[test]
fn a_physical_backend_the_daemon_cannot_serve_refuses_startup() {
    let unknown = refused_startup("backend-unknown", &[("RADAR_CONTROL_BACKEND", "docker")]);
    assert!(unknown.contains("docker"), "{unknown}");
    for supported in ["herdr", "tmux"] {
        assert!(unknown.contains(supported), "{unknown}");
    }

    // The tmux adapter targets one named server and never tmux's default one,
    // so a configuration that names the default server refuses startup.
    let shared = refused_startup(
        "backend-tmux-default",
        &[
            ("RADAR_CONTROL_BACKEND", "tmux"),
            ("RADAR_TMUX_SERVER", "default"),
        ],
    );
    assert!(shared.contains("tmux"), "{shared}");
    assert!(shared.contains("default server"), "{shared}");

    // Two different mistakes, two different diagnostics: an operator who reads one
    // is told what to change, not handed the other one's problem.
    assert_ne!(unknown, shared);
}

/// The daemon serves the tmux server its own unit owns. A server whose unit cannot
/// start is a refusal at startup: nothing is served, nothing is created, and the
/// reason names the unit.
#[test]
fn a_tmux_daemon_whose_server_unit_cannot_start_refuses_startup() {
    // An installed template starts a server for any name, so the unit can start
    // and there is no refusal to observe on this machine.
    if tmux_unit_template_installed() {
        eprintln!("skipping an unstartable server unit: the unit template is installed");
        return;
    }
    let name = format!("radar-test-absent-{}", std::process::id());
    let socket_file = tmux_socket(&name);
    let sandbox = Sandbox::new("backend-tmux-refused");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let mut child = DaemonChild::spawn_with(
        &socket,
        &state,
        &[
            ("RADAR_CONTROL_BACKEND", "tmux"),
            ("RADAR_TMUX_SERVER", &name),
        ],
    );
    let status = child.wait(DEADLINE);
    let mut stderr = String::new();
    child
        .process
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("readable stderr");

    assert_eq!(status.code(), Some(1), "{status:?}");
    assert!(stderr.contains(&format!("radar-tmux@{name}")), "{stderr}");
    assert!(!socket.exists(), "a refused daemon bound its socket");
    assert!(
        !socket_file.exists(),
        "the daemon created a tmux server it does not own"
    );
}

/// A tmux server started outside its unit is foreign. The daemon refuses it and
/// does not adopt it, so a server the operator started by hand is never served.
#[test]
fn a_foreign_tmux_server_is_refused_and_not_adopted() {
    if !user_manager_available() {
        eprintln!("skipping a foreign tmux server: no systemd user manager");
        return;
    }
    let name = format!("radar-foreign-{}", std::process::id());
    let _ = Command::new("tmux")
        .args([
            "-f",
            "/dev/null",
            "-L",
            &name,
            "new-session",
            "-d",
            "-s",
            "main",
        ])
        .env_remove("TMUX")
        .status();
    let sandbox = Sandbox::new("backend-tmux-foreign");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let mut child = DaemonChild::spawn_with(
        &socket,
        &state,
        &[
            ("RADAR_CONTROL_BACKEND", "tmux"),
            ("RADAR_TMUX_SERVER", &name),
        ],
    );
    let status = child.wait(DEADLINE);
    let mut stderr = String::new();
    child
        .process
        .stderr
        .take()
        .expect("stderr")
        .read_to_string(&mut stderr)
        .expect("readable stderr");

    assert_eq!(status.code(), Some(1), "{status:?}");
    assert!(stderr.contains("did not start it"), "{stderr}");
    assert!(!socket.exists(), "a refused daemon bound its socket");
    let _ = Command::new("tmux")
        .args(["-L", &name, "kill-server"])
        .env_remove("TMUX")
        .status();
}

/// Whether this user's systemd already defines `radar-tmux@.service`, as the
/// installed Home Manager module does.
fn tmux_unit_template_installed() -> bool {
    Command::new("systemctl")
        .args(["--user", "cat", "radar-tmux@.service"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// A disposable tmux server for the tests that serve one through the daemon.
struct TmuxServer {
    name: String,
    socket: PathBuf,
}

impl TmuxServer {
    /// Starts one on a name of its own, or reports `None` on a machine without
    /// tmux.
    fn start(label: &str) -> Option<Self> {
        let available = Command::new("tmux")
            .arg("-V")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !available {
            eprintln!("skipping {label}: tmux is not installed");
            return None;
        }
        if !user_manager_available() {
            eprintln!("skipping {label}: no systemd user manager");
            return None;
        }
        let name = format!("radar-daemon-{}-{label}", std::process::id());
        // The server runs under the unit the daemon requires for its own servers,
        // so the daemon's ownership check sees the server it is meant to serve.
        // Where the real template is installed a transient unit of the same name
        // is refused, so the template itself starts it.
        let started = if tmux_unit_template_installed() {
            Command::new("systemctl")
                .args(["--user", "start", &format!("radar-tmux@{name}.service")])
                .status()
                .expect("run systemctl")
        } else {
            Command::new("systemd-run")
                .args(["--user", "--collect", "--quiet"])
                .arg(format!("--unit=radar-tmux@{name}"))
                .args([
                    "--property=Type=simple",
                    "--property=KillMode=control-group",
                    "tmux",
                    "-D",
                    "-L",
                    &name,
                    "-f",
                    "/dev/null",
                ])
                .status()
                .expect("run systemd-run")
        };
        assert!(started.success(), "the tmux unit did not start for {name}");
        let server = Self {
            socket: tmux_socket(&name),
            name,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while !server.answers() {
            assert!(
                Instant::now() < deadline,
                "tmux server {} never answered",
                server.name
            );
            thread::sleep(Duration::from_millis(50));
        }
        server.tmux(&["new-session", "-d", "-s", "main", "-x", "100", "-y", "30"]);
        Some(server)
    }

    /// Whether the server answers, without starting one: `show-options` never does.
    fn answers(&self) -> bool {
        Command::new("tmux")
            .arg("-L")
            .arg(&self.name)
            .args(["show-options", "-g"])
            .env_remove("TMUX")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// Runs one tmux command against this server, returning its stdout.
    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .arg("-f")
            .arg("/dev/null")
            .arg("-L")
            .arg(&self.name)
            .env_remove("TMUX")
            .args(args)
            .output()
            .expect("run tmux");
        assert!(
            output.status.success(),
            "tmux {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

/// Whether the test can run the server under a unit: a systemd user manager.
/// Configures the scope launcher for a daemon that runs inside this test process.
///
/// A tracked launch always enters a scope, so an in-process daemon resolves the
/// launcher from this process's environment. The value is the same one the child
/// daemons get, and setting it twice is the same write.
fn configure_scope_launcher() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // SAFETY: this is the only writer, it runs before any daemon in this
        // process reads the variable, and the value never changes after.
        unsafe { std::env::set_var("RADAR_SYSTEMD_RUN", scope_launcher()) };
    });
}

/// The scope launcher the test daemons are configured with: the real one when
/// this machine has it, so launches behave as they do in service. Units a test
/// creates are stopped when its sandbox is dropped.
fn scope_launcher() -> &'static str {
    static LAUNCHER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LAUNCHER.get_or_init(|| {
        std::env::var_os("PATH")
            .into_iter()
            .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
            .map(|directory| directory.join("systemd-run"))
            .find(|candidate| candidate.is_file())
            .and_then(|found| found.to_str().map(str::to_string))
            .unwrap_or_else(|| "/bin/true".to_string())
    })
}

fn user_manager_available() -> bool {
    Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

impl Drop for TmuxServer {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .arg("-L")
            .arg(&self.name)
            .env_remove("TMUX")
            .args(["kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = Command::new("systemctl")
            .args([
                "--user",
                "stop",
                &format!("radar-tmux@{}.service", self.name),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // A server that exited leaves its socket behind; only this name's own
        // file is ever removed.
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// Serves the daemon through the tmux adapter: what the client observes is the
/// disposable server's own objects, named the way this adapter names them, so a
/// daemon that served another backend could not answer this.
#[test]
fn a_tmux_backend_observes_the_server_it_was_given() {
    let Some(server) = TmuxServer::start("backend-tmux-observed") else {
        return;
    };
    let sandbox = Sandbox::new("backend-tmux-observed");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let mut child = DaemonChild::spawn_with(
        &socket,
        &state,
        &[
            ("RADAR_CONTROL_BACKEND", "tmux"),
            ("RADAR_TMUX_SERVER", &server.name),
        ],
    );
    let mut lines = BufReader::new(child.process.stdout.take().expect("stdout")).lines();
    lines.next().expect("a greeting line").expect("a greeting");

    let mut client = Client::dial(&socket);
    let observed = client.call("mux.observe", json!({}));
    let inventory = &observed["result"]["inventory"];
    assert_eq!(
        inventory["workspaces"][0]["workspace_id"],
        json!(format!("tmux:{}:$0", server.name)),
        "{observed}"
    );
    assert_eq!(
        inventory["workspaces"][0]["label"],
        json!("main"),
        "{observed}"
    );
    let pane = inventory["panes"][0]["location"]["pane_id"]
        .as_str()
        .unwrap_or_else(|| panic!("no pane in {observed}"));
    assert!(
        pane.starts_with(&format!("tmux:{}:%", server.name)),
        "{pane} is not a handle this adapter qualified"
    );
    assert_eq!(inventory["agents"], json!([]), "{observed}");

    let status = child.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A tracked launch is captured by its own bootstrap, and cancelling it signals
/// exactly the captured process and records its exit as verified.
#[test]
fn a_tracked_launch_is_cancelled_by_signalling_its_captured_process() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-cancel") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-cancel");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let backend = [
        ("RADAR_CONTROL_BACKEND", "tmux"),
        ("RADAR_TMUX_SERVER", server.name.as_str()),
    ];
    let mut daemon = DaemonChild::spawn_with(&socket, &state, &backend);
    let mut lines = BufReader::new(daemon.process.stdout.take().expect("stdout")).lines();
    lines.next().expect("greeting line").expect("greeting");

    let mut client = Client::dial(&socket);
    let observed = client.call("mux.observe", json!({}));
    let parent_pane = observed["result"]["inventory"]["panes"][0]["location"]["pane_id"]
        .as_str()
        .expect("parent pane")
        .to_string();
    let mut body = registration_body(PARENT_INCARNATION, None, None);
    body["location"] = json!({"backend":"tmux", "pane":parent_pane});
    let parent = registration_id(&client.call("agent.register", body));
    let session = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    client.call(
        "agent.context",
        json!({
            "registration_id": parent,
            "publisher": {"source":"test-publisher", "incarnation":PARENT_INCARNATION},
            "sequence": 1,
            "lease_ms": 10_000,
            "context": {"session": session}
        }),
    );

    let spawn_id = random_uuid();
    let spawned = client.call_id(
        &spawn_id,
        "spawn.create",
        json!({"request": {
            "parent": parent,
            "parent_session_id": session,
            "executable": "/bin/sh",
            "argv": ["-c", "exec sleep 300"]
        }}),
    );
    assert!(spawned["result"].is_object(), "{spawned}");
    await_captured_launch(&state, &spawn_id);
    let subject_id = recorded_edge(&state, &spawn_id)["claim_id"]
        .as_str()
        .expect("tracked launch subject")
        .to_string();
    let subject = subject_record(&state, &subject_id);
    let pid = subject["process"]["pid"].as_i64().expect("captured pid") as i32;

    let cancel_id = random_uuid();
    let selected =
        agent_radar::control_plane::client::connect_at(&socket).expect("dashboard client");
    let cancelled = selected
        .lifecycle
        .as_ref()
        .expect("daemon lifecycle controls")
        .cancel_subject(
            &agent_radar::control_plane::client::LifecycleControlRequest {
                request_id: cancel_id.clone(),
                claim_id: subject_id.clone(),
                subject_id: subject_id.clone(),
            },
            &AtomicBool::new(false),
        )
        .expect("lifecycle.cancel socket control");
    assert_eq!(cancelled.outcome(), "completed", "{cancelled:?}");
    assert_eq!(
        cancelled.request["effects"],
        json!([
            "launch revoked",
            "sigterm dispatched",
            "pane cleaned up",
            "process exit verified"
        ]),
        "{cancelled:?}"
    );
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "captured process {pid} still runs"
    );
    let replayed = selected
        .lifecycle
        .as_ref()
        .expect("daemon lifecycle controls")
        .cancel_subject(
            &agent_radar::control_plane::client::LifecycleControlRequest {
                request_id: cancel_id,
                claim_id: subject_id.clone(),
                subject_id,
            },
            &AtomicBool::new(false),
        )
        .expect("same-ID lifecycle.cancel replay");
    assert_eq!(
        replayed.request, cancelled.request,
        "a same-ID cancel replays without a second signal"
    );

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// The logical session a tracked parent publishes; tracked children launch under it.
const TRACKED_SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

/// Starts the daemon on a disposable tmux server and waits for its greeting.
fn tmux_daemon(socket: &Path, state: &Path, server: &TmuxServer) -> DaemonChild {
    let backend = [
        ("RADAR_CONTROL_BACKEND", "tmux"),
        ("RADAR_TMUX_SERVER", server.name.as_str()),
    ];
    let mut daemon = DaemonChild::spawn_with(socket, state, &backend);
    let mut reader = BufReader::new(daemon.process.stdout.take().expect("stdout"));
    let mut greeting = String::new();
    reader.read_line(&mut greeting).expect("greeting");
    daemon.process.stdout = Some(reader.into_inner());
    daemon
}

/// A registered parent in the server's own pane, publishing the tracked session.
fn tracked_parent(client: &mut Client) -> String {
    let observed = client.call("mux.observe", json!({}));
    let pane = observed["result"]["inventory"]["panes"][0]["location"]["pane_id"]
        .as_str()
        .expect("parent pane")
        .to_string();
    let mut body = registration_body(PARENT_INCARNATION, None, None);
    body["location"] = json!({"backend":"tmux", "pane":pane});
    let parent = registration_id(&client.call("agent.register", body));
    let context = client.call(
        "agent.context",
        json!({
            "registration_id": parent,
            "publisher": {"source":"test-publisher", "incarnation":PARENT_INCARNATION},
            "sequence": 1,
            "lease_ms": 10_000,
            "context": {"session": TRACKED_SESSION}
        }),
    );
    assert!(context["result"].is_object(), "{context}");
    parent
}

/// Spawns one tracked child under `parent` and waits until its bootstrap has been
/// released. Returns the spawn request id and its launch subject id.
fn tracked_spawn(client: &mut Client, state: &Path, parent: &str, argv: Value) -> (String, String) {
    let spawn_id = random_uuid();
    let spawned = client.call_id(
        &spawn_id,
        "spawn.create",
        json!({"request": {
            "parent": parent,
            "parent_session_id": TRACKED_SESSION,
            "executable": "/bin/sh",
            "argv": argv
        }}),
    );
    assert!(spawned["result"].is_object(), "{spawned}");
    await_captured_launch(state, &spawn_id);
    let subject = recorded_edge(state, &spawn_id)["claim_id"]
        .as_str()
        .expect("tracked launch subject")
        .to_string();
    (spawn_id, subject)
}

fn subject_record(state: &Path, subject_id: &str) -> Value {
    let registry =
        agent_radar::control_plane::Registry::open(state).expect("readable lifecycle event log");
    let subject = agent_radar::control_plane::lifecycle::read(&registry, subject_id)
        .expect("replayable subject")
        .expect("a launch subject");
    serde_json::to_value(subject).expect("a subject projection")
}

/// The program a launch claim will deliver: the daemon's own record of it.
fn recorded_launch(
    state: &Path,
    claim_id: &str,
) -> agent_radar::control_plane::lifecycle::LaunchContext {
    let registry =
        agent_radar::control_plane::Registry::open(state).expect("readable lifecycle event log");
    agent_radar::control_plane::lifecycle::read(&registry, claim_id)
        .expect("replayable claim")
        .expect("a launch claim")
        .process
        .launch
}

fn captured_identity(state: &Path, subject_id: &str) -> ProcessIdentity {
    serde_json::from_value(subject_record(state, subject_id)["process"].clone())
        .expect("a captured identity")
}

/// A child registration that presents the spawn's token and the captured identity.
fn child_registration_for(state: &Path, spawn_id: &str, subject_id: &str) -> Value {
    let mut body = registration_body(
        CHILD_INCARNATION,
        Some(captured_identity(state, subject_id)),
        None,
    );
    body["spawn_token"] = recorded_edge(state, spawn_id)["token"].clone();
    body
}

fn cancel_subject(client: &mut Client, request_id: &str, subject_id: &str) -> Value {
    client.call_id(
        request_id,
        "lifecycle.cancel",
        json!({"request": {"subject_id": subject_id, "claim_id": subject_id}}),
    )
}

fn await_gone(pid: i32) {
    for _ in 0..500 {
        if !process_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("process {pid} still runs");
}

/// Registration can commit before a cancel: the cancel then stops exactly the
/// process the registration bound, and the launch is bound rather than revoked.
#[test]
fn a_cancel_after_binding_stops_the_bound_process() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-bound") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-bound");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let pid = captured_identity(&state, &subject).pid;

    let registered = client.call(
        "agent.register",
        child_registration_for(&state, &spawn_id, &subject),
    );
    let child = registration_id(&registered);
    let cancelled = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&cancelled)["outcome"],
        "completed",
        "{cancelled}"
    );
    assert_eq!(
        result_record(&cancelled)["effects"],
        json!(["sigterm dispatched", "pane kept", "process exit verified"]),
        "{cancelled}"
    );
    assert!(!process_exists(pid), "the bound process {pid} still runs");
    assert_eq!(subject_record(&state, &subject)["state"], "bound");
    let spawn = client.call("spawn.get", json!({"request_id": spawn_id}));
    assert_eq!(
        spawn["result"]["spawn"]["bound"]["registration_id"],
        json!(child),
        "{spawn}"
    );
    assert_eq!(
        spawn["result"]["spawn"]["bound"]["subject_id"],
        json!(subject),
        "the bound edge names the agent, not only its process: {spawn}"
    );

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// Cancel can commit before registration: the late token presents the captured
/// identity and is refused, and the launch stays revoked.
#[test]
fn a_registration_after_cancel_is_refused_and_the_launch_stays_revoked() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-revoked") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-revoked");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let pid = captured_identity(&state, &subject).pid;

    let cancelled = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&cancelled)["effects"],
        json!([
            "launch revoked",
            "sigterm dispatched",
            "pane cleaned up",
            "process exit verified"
        ]),
        "{cancelled}"
    );
    assert!(!process_exists(pid), "the revoked process {pid} still runs");
    let late = client.call(
        "agent.register",
        child_registration_for(&state, &spawn_id, &subject),
    );
    assert_eq!(error_code(&late), "refused", "{late}");
    assert_eq!(subject_record(&state, &subject)["state"], "revoked");
    let spawn = client.call("spawn.get", json!({"request_id": spawn_id}));
    assert!(spawn["result"]["spawn"]["bound"].is_null(), "{spawn}");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// `lifecycle.get` reports a tracked launch's public facts and nothing private: no
/// launch context, env value, argv or correlation.
#[test]
fn lifecycle_get_reports_a_subject_without_its_private_launch_data() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-get") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-get");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (_, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let cancelled = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&cancelled)["outcome"],
        "completed",
        "{cancelled}"
    );

    let read = client.call("lifecycle.get", json!({"subject_id": subject}));
    let view = &read["result"]["subject"];
    assert_eq!(view["subject_id"], subject, "{read}");
    assert_eq!(view["state"], "revoked", "{read}");
    assert!(view["signalled_at"].is_string(), "{read}");
    assert!(view["exited_at"].is_string(), "{read}");
    let text = read.to_string();
    assert!(!text.contains("correlation"), "{read}");
    assert!(!text.contains("sleep 300"), "argv leaked: {read}");
    assert!(!text.contains("PI_RADAR_LAUNCH"), "{read}");
    assert!(!text.contains("launch\""), "{read}");

    let unknown = client.call("lifecycle.get", json!({"subject_id": random_uuid()}));
    assert_eq!(error_code(&unknown), "not_found", "{unknown}");
    let malformed = client.call("lifecycle.get", json!({"subject_id": "not-a-uuid"}));
    assert_eq!(error_code(&malformed), "bad_params", "{malformed}");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A revoked launch stays revoked, and a repeated cancel signals nothing, across a
/// daemon restart. A same-ID replay reads the stored outcome.
#[test]
fn a_revoked_launch_stays_revoked_and_unsignalled_across_a_daemon_restart() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-restart") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-restart");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let cancel_id = random_uuid();
    let first = cancel_subject(&mut client, &cancel_id, &subject);
    assert_eq!(result_record(&first)["outcome"], "completed", "{first}");
    let before = subject_record(&state, &subject);
    assert_eq!(daemon.stop(libc::SIGTERM, DEADLINE).code(), Some(0));

    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    assert_eq!(cancel_subject(&mut client, &cancel_id, &subject), first);
    let fresh = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&fresh)["effects"],
        json!(["pane cleaned up", "process exit verified"]),
        "{fresh}"
    );
    let after = subject_record(&state, &subject);
    assert_eq!(after["signalled_at"], before["signalled_at"]);
    assert_eq!(after["state"], "revoked");
    let late = client.call(
        "agent.register",
        child_registration_for(&state, &spawn_id, &subject),
    );
    assert_eq!(error_code(&late), "refused", "{late}");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A request id that names one cancel cannot name another launch's cancel, and
/// the refused replay signals nothing.
#[test]
fn a_cancel_request_id_cannot_be_reused_for_another_launch() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-conflict") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-conflict");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (_, first) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let (_, second) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let second_pid = captured_identity(&state, &second).pid;

    let request_id = random_uuid();
    let reply = cancel_subject(&mut client, &request_id, &first);
    assert_eq!(result_record(&reply)["outcome"], "completed", "{reply}");
    let conflicting = cancel_subject(&mut client, &request_id, &second);
    assert_eq!(error_code(&conflicting), "refused", "{conflicting}");
    assert!(process_exists(second_pid), "the other launch was signalled");
    assert_eq!(subject_record(&state, &second)["signalled_at"], Value::Null);
    assert_eq!(cancel_subject(&mut client, &request_id, &first), reply);

    let cleanup = cancel_subject(&mut client, &random_uuid(), &second);
    assert_eq!(result_record(&cleanup)["outcome"], "completed", "{cleanup}");
    await_gone(second_pid);

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A tracked launch whose bootstrap never reported has no process identity. Its
/// cancel revokes it, names the unresolved identity, and signals nothing.
#[test]
fn a_launch_whose_bootstrap_never_reported_is_cancelled_unresolved_without_a_signal() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-unresolved") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-unresolved");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    // The launch is recorded while no daemon runs, as a bootstrap that never
    // reported would leave it: pending, with no process identity.
    let registry = agent_radar::control_plane::registry::Registry::open(&state).expect("registry");
    let launch = agent_radar::control_plane::lifecycle::LaunchContext {
        executable: "/bin/sh".into(),
        argv: vec!["-c".into(), "exec sleep 300".into()],
        cwd: None,
        env: BTreeMap::new(),
    };
    let spawn_id = random_uuid();
    let ticket =
        agent_radar::control_plane::lifecycle::create(&registry, &spawn_id, launch, None, 1)
            .expect("pending launch");
    drop(registry);

    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let cancelled = cancel_subject(&mut client, &random_uuid(), &ticket.claim_id);
    assert_eq!(
        result_record(&cancelled)["outcome"],
        "completed",
        "{cancelled}"
    );
    assert_eq!(
        result_record(&cancelled)["effects"],
        json!(["launch revoked", "pane kept", "process identity unresolved"]),
        "{cancelled}"
    );
    let subject = subject_record(&state, &ticket.claim_id);
    assert_eq!(subject["state"], "revoked");
    assert_eq!(subject["signalled_at"], Value::Null, "{subject}");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A program that exits at once is captured and observed without any registration:
/// the cancel reports its exit and signals nothing.
#[test]
fn a_launch_that_exits_at_once_is_observable_without_a_registration() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-exit") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-exit");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(&mut client, &state, &parent, json!(["-c", "exit 0"]));
    let pid = captured_identity(&state, &subject).pid;
    await_gone(pid);

    let cancelled = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&cancelled)["effects"],
        json!(["launch revoked", "pane cleaned up", "process exit verified"]),
        "{cancelled}"
    );
    assert!(subject_record(&state, &subject)["exited_at"].is_string());
    let spawn = client.call("spawn.get", json!({"request_id": spawn_id}));
    assert!(spawn["result"]["spawn"]["bound"].is_null(), "{spawn}");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// The program runs with the requested argv, cwd and environment; the bootstrap
/// correlation is gone before exec and the registration token remains. Reads
/// expose none of the argv, env values, token or correlation.
#[test]
fn a_tracked_launch_runs_its_requested_program_and_keeps_its_secrets_private() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-context") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-context");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let launch_dir = sandbox.root.join("launch");
    fs::create_dir_all(&launch_dir).expect("launch directory");
    let spawn_id = random_uuid();
    let spawned = client.call_id(
        &spawn_id,
        "spawn.create",
        json!({"request": {
            "parent": parent,
            "parent_session_id": TRACKED_SESSION,
            "executable": "/bin/sh",
            "argv": ["-c", "env > effective.env; pwd > effective.cwd; exec sleep 300"],
            "cwd": launch_dir,
            "env": {"RADAR_TEST_MARK": "mark-value-7f3a"}
        }}),
    );
    assert!(spawned["result"].is_object(), "{spawned}");
    await_captured_launch(&state, &spawn_id);
    let subject = recorded_edge(&state, &spawn_id)["claim_id"]
        .as_str()
        .expect("tracked launch subject")
        .to_string();
    let pid = captured_identity(&state, &subject).pid;
    let token = recorded_edge(&state, &spawn_id)["token"]
        .as_str()
        .expect("spawn token")
        .to_string();

    // The program writes its environment before its directory, so the directory
    // file marks the end of both writes.
    for _ in 0..500 {
        if launch_dir.join("effective.cwd").exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let environment = fs::read_to_string(launch_dir.join("effective.env")).expect("environment");
    assert!(
        environment
            .lines()
            .any(|line| line == "RADAR_TEST_MARK=mark-value-7f3a")
    );
    assert!(
        environment
            .lines()
            .any(|line| line == format!("PI_RADAR_REGISTRATION_ID={token}"))
    );
    assert!(
        !environment.contains("PI_RADAR_LAUNCH_CORRELATION"),
        "{environment}"
    );
    assert!(
        !environment.contains("PI_RADAR_LAUNCH_SUBJECT"),
        "{environment}"
    );
    let directory = fs::read_to_string(launch_dir.join("effective.cwd")).expect("directory");
    assert_eq!(
        Path::new(directory.trim_end()),
        fs::canonicalize(&launch_dir).expect("canonical directory")
    );

    let record = client.call("mux.request.get", json!({"id": spawn_id}));
    let spawn = client.call("spawn.get", json!({"request_id": spawn_id}));
    for text in [record.to_string(), spawn.to_string()] {
        for secret in [
            "mark-value-7f3a",
            "exec sleep 300",
            "PI_RADAR_LAUNCH_CORRELATION",
        ] {
            assert!(!text.contains(secret), "{secret} is in {text}");
        }
    }

    let cleanup = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(result_record(&cleanup)["outcome"], "completed", "{cleanup}");
    await_gone(pid);

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A spawn whose arguments or environment have the wrong shape is refused without
/// echoing those values.
#[test]
fn a_refused_spawn_does_not_echo_its_arguments_or_environment() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-refusal") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-refusal");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    for (name, body) in [
        ("argv", json!({"argv": "argv-secret-9c1e"})),
        ("env", json!({"env": "env-secret-5d2a"})),
    ] {
        let mut request = json!({"parent": parent, "parent_session_id": TRACKED_SESSION, "executable": "/bin/sh"});
        request
            .as_object_mut()
            .expect("object")
            .extend(body.as_object().expect("object").clone());
        let refused = client.call("spawn.create", json!({"request": request}));
        assert!(refused["result"].is_null(), "{name}: {refused}");
        let text = refused.to_string();
        assert!(
            !text.contains("secret-9c1e") && !text.contains("secret-5d2a"),
            "{text}"
        );
    }

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn tmux_spawn_exit_persists_and_a_retained_child_pane_closes_once() {
    let Some(server) = TmuxServer::start("tmux-spawn-exit") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-spawn-exit");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let backend = [
        ("RADAR_CONTROL_BACKEND", "tmux"),
        ("RADAR_TMUX_SERVER", server.name.as_str()),
    ];
    let mut daemon = DaemonChild::spawn_with(&socket, &state, &backend);
    let mut lines = BufReader::new(daemon.process.stdout.take().expect("stdout")).lines();
    lines.next().expect("greeting line").expect("greeting");

    let mut client = Client::dial(&socket);
    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["backend"], "tmux", "{ping}");
    assert_eq!(ping["result"]["instance"], json!(server.name), "{ping}");
    assert!(
        ping["result"]["contracts"]
            .as_array()
            .is_some_and(|contracts| contracts.contains(&json!("spawn.exit_observation.v1"))),
        "{ping}"
    );
    let observed = client.call("mux.observe", json!({}));
    let parent_pane = observed["result"]["inventory"]["panes"][0]["location"]["pane_id"]
        .as_str()
        .expect("parent pane")
        .to_string();
    let incarnation = PARENT_INCARNATION;
    let session = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let mut body = registration_body(incarnation, None, None);
    body["location"] = json!({"backend":"tmux", "pane":parent_pane});
    let registration = client.call("agent.register", body);
    let parent = registration_id(&registration);
    let context = client.call(
        "agent.context",
        json!({
            "registration_id": parent,
            "publisher": {"source":"test-publisher", "incarnation":incarnation},
            "sequence": 1,
            "lease_ms": 10_000,
            "context": {"session": session}
        }),
    );
    assert!(context["result"].is_object(), "{context}");
    let facts_before = client.call("agent.get", json!({"registration_id":parent}));

    let (spawn_id, raw_pane, child_agent) = spawned_exiting_tmux_child(
        &mut client,
        &server,
        &state,
        &parent,
        session,
        CHILD_INCARNATION,
    );
    let edge = recorded_edge(&state, &spawn_id);
    // The recorded coordinates name the backend and the server instance that
    // produced them, so the same pane id elsewhere is a different pane.
    assert_eq!(edge["location"]["backend"], "tmux", "{edge}");
    assert_eq!(edge["location"]["instance"], json!(server.name), "{edge}");
    let child_pane = edge["location"]["pane"]
        .as_str()
        .expect("spawned pane")
        .to_string();
    let child_facts_before = client.call("agent.get", json!({"registration_id":child_agent}));
    let listed = client.call("spawn.list", json!({"limit":10}));
    assert_eq!(
        listed["result"]["spawns"][0]["exit_observation"]["code"],
        json!(23),
        "{listed}"
    );
    let output = client.call(
        "mux.output",
        json!({"pane_id":child_pane,"source":"recent"}),
    );
    assert!(
        output["result"]["output"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Pane is dead (status 23")),
        "{output}"
    );
    let facts_after = client.call("agent.get", json!({"registration_id":parent}));
    assert_eq!(
        facts_after["result"]["agent"]["execution"],
        facts_before["result"]["agent"]["execution"]
    );
    assert_eq!(
        facts_after["result"]["agent"]["assignment"],
        facts_before["result"]["agent"]["assignment"]
    );

    // A same-UID client can respawn a retained pane; while its command runs, there
    // is a live occupant, so nothing here closes it whatever the edge recorded.
    server.tmux(&["respawn-pane", "-k", "-t", &raw_pane, "sleep 300"]);
    let respawn_id = random_uuid();
    let respawned = client.call_id(
        &respawn_id,
        "spawn.close",
        child_close_params(&spawn_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&respawned), "refused", "{respawned}");
    assert!(
        !closes_directory(&state)
            .join(format!("{respawn_id}.json"))
            .exists(),
        "a live pane is not admitted for close on its recorded history"
    );
    assert!(
        pane_ids(&server).contains(&raw_pane.to_string()),
        "a refused close dispatches nothing"
    );

    // It exits again, so the pane is dead now while the edge still holds its
    // recorded exit: the retained pane is cleaned up once, and the answer says
    // exactly that — an observed pane-command exit and a pane cleanup, never a
    // process exit.
    server.tmux(&["respawn-pane", "-k", "-t", &raw_pane, "exit 23"]);
    wait_until_pane_dead(&server, &raw_pane);
    let close_id = random_uuid();
    let close = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&spawn_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(
        close["result"]["close"]["outcome"], "already_exited",
        "{close}"
    );
    assert_eq!(close["result"]["close"]["intent"], "cancel", "{close}");
    assert_eq!(
        close["result"]["close"]["exit_observation"]["state"], "exited",
        "{close}"
    );
    assert_eq!(
        close["result"]["close"]["exit_observation"]["code"],
        json!(23),
        "{close}"
    );
    assert!(
        !pane_ids(&server).contains(&raw_pane.to_string()),
        "the retained pane is gone: {:?}",
        pane_ids(&server)
    );
    let child_facts_after = client.call("agent.get", json!({"registration_id":child_agent}));
    assert_eq!(
        child_facts_after["result"]["agent"]["execution"],
        child_facts_before["result"]["agent"]["execution"],
        "a dead-pane close changes no execution fact"
    );
    assert_eq!(
        child_facts_after["result"]["agent"]["assignment"],
        child_facts_before["result"]["agent"]["assignment"],
        "a dead-pane close settles no assignment"
    );

    drop(client);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");

    let mut daemon = DaemonChild::spawn_with(&socket, &state, &backend);
    let mut lines = BufReader::new(daemon.process.stdout.take().expect("stdout")).lines();
    lines.next().expect("greeting line").expect("greeting");
    let mut client = Client::dial(&socket);
    // A restart replays the recorded close instead of closing anything again, and
    // the edge still holds the exit that close rested on.
    let replayed = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&spawn_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(replayed, close, "a restart re-dispatches no close");
    let restored = client.call("spawn.get", json!({"request_id":spawn_id}));
    assert_eq!(
        restored["result"]["spawn"]["exit_observation"]["code"],
        json!(23),
        "{restored}"
    );
    drop(client);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A tmux server restarted under the same name hands the same pane ids to panes
/// that were never this daemon's, so an edge whose pane belonged to the server
/// that has gone never cleans up the pane that now carries its id.
#[test]
fn a_tmux_pane_from_another_server_generation_is_never_closed() {
    let Some(server) = TmuxServer::start("tmux-generation") else {
        return;
    };
    let name = server.name.clone();
    let sandbox = Sandbox::new("tmux-generation");
    let socket = sandbox.socket();
    let state = sandbox.state();
    let backend = [
        ("RADAR_CONTROL_BACKEND", "tmux"),
        ("RADAR_TMUX_SERVER", name.as_str()),
    ];
    let mut daemon = DaemonChild::spawn_with(&socket, &state, &backend);
    let mut lines = BufReader::new(daemon.process.stdout.take().expect("stdout")).lines();
    lines.next().expect("greeting line").expect("greeting");

    let mut client = Client::dial(&socket);
    let observed = client.call("mux.observe", json!({}));
    let parent_pane = observed["result"]["inventory"]["panes"][0]["location"]["pane_id"]
        .as_str()
        .expect("parent pane")
        .to_string();
    let session = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let mut body = registration_body(PARENT_INCARNATION, None, None);
    body["location"] = json!({"backend":"tmux", "pane":parent_pane});
    let parent = registration_id(&client.call("agent.register", body));
    let context = client.call(
        "agent.context",
        json!({
            "registration_id": parent,
            "publisher": {"source":"test-publisher", "incarnation":PARENT_INCARNATION},
            "sequence": 1,
            "lease_ms": 10_000,
            "context": {"session": session}
        }),
    );
    assert!(context["result"].is_object(), "{context}");

    let (stale_spawn, stale_pane, _child) = spawned_exiting_tmux_child(
        &mut client,
        &server,
        &state,
        &parent,
        session,
        CHILD_INCARNATION,
    );
    let stale_edge = recorded_edge(&state, &stale_spawn);
    assert!(
        stale_edge["instance"].is_object(),
        "the edge records the instance that served its create: {stale_edge}"
    );
    assert_eq!(stale_edge["exit"]["code"], json!(23), "{stale_edge}");

    // The generation that created the pane ends. A fresh server answers to the
    // same name and hands out the same ids again.
    server.tmux(&["kill-server"]);
    drop(server);
    let Some(server) = TmuxServer::start("tmux-generation") else {
        return;
    };
    assert_eq!(server.name, name, "the replacement serves the same name");
    server.tmux(&["split-window", "-d", "-t", "%0"]);
    assert_eq!(
        pane_ids(&server),
        vec!["%0".to_string(), stale_pane.clone()],
        "the fresh server reuses the recorded pane id"
    );
    // The very coordinates the edge recorded, with a dead pane in them that the
    // daemon never created.
    server.tmux(&[
        "set-option",
        "-p",
        "-t",
        &stale_pane,
        "remain-on-exit",
        "on",
    ]);
    server.tmux(&["respawn-pane", "-k", "-t", &stale_pane, "exit 17"]);
    wait_until_pane_dead(&server, &stale_pane);

    let close_id = random_uuid();
    let refused = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&stale_spawn, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&refused), "refused", "{refused}");
    assert_eq!(
        refused["error"]["message"],
        "the backend instance serving now is not the one that created the recorded pane",
        "{refused}"
    );
    assert!(
        !closes_directory(&state)
            .join(format!("{close_id}.json"))
            .exists(),
        "a pane from another server generation is refused before admission"
    );
    assert!(
        pane_ids(&server).contains(&stale_pane),
        "a refused close dispatches nothing: {:?}",
        pane_ids(&server)
    );

    // A child of the server that is serving now is still a dead pane this daemon
    // may clean up, so the fence refuses another generation rather than every
    // dead pane.
    let (fresh_spawn, fresh_pane, _child) = spawned_exiting_tmux_child(
        &mut client,
        &server,
        &state,
        &parent,
        session,
        OTHER_INCARNATION,
    );
    let fresh_close = random_uuid();
    let closed = client.call_id(
        &fresh_close,
        "spawn.close",
        child_close_params(&fresh_spawn, "test-publisher", OTHER_INCARNATION),
    );
    assert_eq!(
        closed["result"]["close"]["outcome"], "already_exited",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["exit_observation"]["code"],
        json!(23),
        "{closed}"
    );
    assert!(
        !pane_ids(&server).contains(&fresh_pane),
        "the dead pane of the serving instance is cleaned up"
    );

    drop(client);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// The pane ids this disposable server reports.
fn pane_ids(server: &TmuxServer) -> Vec<String> {
    server
        .tmux(&["list-panes", "-a", "-F", "#{pane_id}"])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Publishes `session` as the current session of a registration, as the baseline
/// publisher does at session start.
fn publish_session_context(client: &mut Client, agent: &str, incarnation: &str, session: &str) {
    let context = client.call(
        "agent.context",
        json!({
            "registration_id": agent,
            "publisher": {"source":"test-publisher", "incarnation": incarnation},
            "sequence": 1,
            "lease_ms": 60_000,
            "context": {"session": session}
        }),
    );
    assert!(context["result"].is_object(), "{context}");
}

/// Spawns one managed child of `parent` under `managed` metadata in this daemon's
/// tmux server, binds the child's process with the token its launch carried, and
/// leaves it waiting, so it stays live. Returns the child's lifecycle subject, its
/// agent, and its raw tmux pane.
fn spawned_live_tmux_child(
    client: &mut Client,
    state: &Path,
    parent: &str,
    session: &str,
    managed: Value,
) -> (String, String, String) {
    let spawn_id = random_uuid();
    let mut request = json!({
        "parent": parent,
        "parent_session_id": session,
        "executable": "/bin/sh",
        "argv": ["-c", "exec sleep 300"],
    });
    if !managed.is_null() {
        request["managed"] = managed;
    }
    let spawned = client.call_id(&spawn_id, "spawn.create", json!({"request": request}));
    assert!(spawned["result"].is_object(), "{spawned}");
    await_captured_launch(state, &spawn_id);
    let edge = recorded_edge(state, &spawn_id);
    let token = edge["token"].as_str().expect("spawn token").to_string();
    let subject = edge["claim_id"]
        .as_str()
        .expect("child subject")
        .to_string();
    let raw_pane = edge["location"]["pane"]
        .as_str()
        .expect("spawned pane")
        .rsplit(':')
        .next()
        .expect("raw tmux pane")
        .to_string();
    (subject, token, raw_pane)
}

/// Spawns one managed child of `parent` in this daemon's tmux server, binds the
/// child's own process identity with the token its launch carried, releases the
/// child's waiting shell, and waits until its exit is a recorded fact of its
/// edge. Returns the spawn request, the raw tmux pane, and the bound child.
fn spawned_exiting_tmux_child(
    client: &mut Client,
    server: &TmuxServer,
    state: &Path,
    parent: &str,
    session: &str,
    incarnation: &str,
) -> (String, String, String) {
    let spawn_id = random_uuid();
    let spawned = client.call_id(
        &spawn_id,
        "spawn.create",
        json!({"request": {
            "parent": parent,
            "parent_session_id": session,
            "executable": "/bin/sh",
            "argv": ["-c", "read -r _; exit 23"]
        }}),
    );
    assert!(spawned["result"].is_object(), "{spawned}");
    await_captured_launch(state, &spawn_id);
    let edge = recorded_edge(state, &spawn_id);
    let token = edge["token"].as_str().expect("spawn token");
    let raw_pane = edge["location"]["pane"]
        .as_str()
        .expect("spawned pane")
        .rsplit(':')
        .next()
        .expect("raw tmux pane")
        .to_string();
    let pid = server
        .tmux(&["display-message", "-p", "-t", &raw_pane, "#{pane_pid}"])
        .trim()
        .parse::<i32>()
        .expect("child process pid");
    let mut child_registration =
        registration_body(incarnation, Some(process_identity_from_proc(pid)), None);
    child_registration["spawn_token"] = json!(token);
    let child_agent = registration_id(&client.call("agent.register", child_registration));
    let bound = client.call("spawn.get", json!({"request_id":spawn_id}));
    assert_eq!(
        bound["result"]["spawn"]["bound"]["registration_id"],
        json!(child_agent),
        "{bound}"
    );

    // The direct tmux client releases the child's waiting shell only after its
    // exact live process identity has been bound in the registry.
    server.tmux(&["send-keys", "-t", &raw_pane, "go", "Enter"]);
    let mut topology = Value::Null;
    for _ in 0..100 {
        topology = client.call("spawn.get", json!({"request_id":spawn_id}));
        if topology["result"]["spawn"]["exit_observation"]["code"] == json!(23) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        topology["result"]["spawn"]["exit_observation"]["state"],
        json!("exited"),
        "{topology}"
    );
    assert_eq!(
        topology["result"]["spawn"]["exit_observation"]["code"],
        json!(23),
        "{topology}"
    );
    (spawn_id, raw_pane, child_agent)
}

/// Waits, boundedly, until this server reports the pane dead, so a close is never
/// asked about a pane whose command is still starting or still running.
fn wait_until_pane_dead(server: &TmuxServer, raw_pane: &str) {
    for _ in 0..200 {
        let dead = server
            .tmux(&["list-panes", "-a", "-F", "#{pane_id} #{pane_dead}"])
            .lines()
            .any(|line| line.split_whitespace().collect::<Vec<_>>() == [raw_pane, "1"]);
        if dead {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("pane {raw_pane} never reported dead");
}

/// Where tmux keeps the socket of a server of `name`.
fn tmux_socket(name: &str) -> PathBuf {
    let parent = match std::env::var_os("TMUX_TMPDIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from("/tmp"),
    };
    // SAFETY: `getuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    parent.join(format!("tmux-{uid}")).join(name)
}

/// An uncertain outcome is not a licence to retry: the record stays `unknown` and
/// a same-ID retry returns it without a second dispatch. A focus changes nothing
/// durable, so unlike a mutation it never holds its target against another
/// request.
#[test]
fn an_uncertain_focus_stays_unknown_and_never_holds_its_target() {
    let sandbox = Sandbox::new("focus-unknown");
    let backend = Arc::new(FakeBackend::new(
        empty_inventory(),
        FocusAnswer::Unknown("herdr timed out after 5s".into()),
    ));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let request = json!({"target":"wA:p1","target_kind":"pane"});
    let first = client.call_id(&id, "mux.focus", request.clone());
    assert_eq!(result_record(&first)["outcome"], "unknown");
    assert_eq!(result_record(&first)["state"], "completed");

    // The same id returns the same account; the backend is asked exactly once.
    let retry = client.call_id(&id, "mux.focus", request);
    assert_eq!(result_record(&retry)["outcome"], "unknown");
    assert_eq!(
        result_record(&retry)["completed_at"],
        result_record(&first)["completed_at"]
    );

    // A focus changes nothing durable, so its unknown outcome protects nothing: a
    // different request over the same target is dispatched, not held in flight.
    let other_id = random_uuid();
    let other = client.call_id(
        &other_id,
        "mux.focus",
        json!({"target":"wA:p1","target_kind":"pane"}),
    );
    assert_eq!(result_record(&other)["outcome"], "unknown", "{other}");
    assert_ne!(
        result_record(&other)["category"],
        Category::IN_FLIGHT,
        "an unknown focus held its target"
    );
    assert_eq!(backend.focused.lock().unwrap().len(), 2);
    daemon.stop();
}

/// A stop is graceful: the request already being served answers, and a request
/// that arrives while stopping is refused instead of dropped. Without the drain
/// the first is a connection the daemon ends, which the client cannot tell from
/// a request that never arrived.
#[test]
fn a_stop_answers_the_request_in_flight_and_refuses_one_that_arrives_during_it() {
    let sandbox = Sandbox::new("graceful-stop");
    let (gate, rendezvous) = Rendezvous::new();
    let backend = Arc::new(GatedBackend {
        gate: Arc::clone(&gate),
        released: true,
        answer: FocusOutcome::Completed,
    });
    let mut daemon = sandbox.daemon_with(backend);
    let path = daemon.path().to_path_buf();

    let mut blocker = Client::dial(&path);
    let id = random_uuid();
    blocker.send(&request_line(
        &id,
        "mux.focus",
        json!({"target":"wA:p1","target_kind":"pane"}),
    ));
    // Blocks until the request has reached the backend, so the stop below is
    // asked to end a daemon that is genuinely mid-request.
    rendezvous.entered();

    // A connection dialled before the stop and left idle: what a client holds
    // between calls, and where a request can arrive while stopping.
    let mut later = Client::dial(&path);

    let stopped = Arc::new(AtomicBool::new(false));
    let finished = Arc::clone(&stopped);
    let stop = thread::spawn(move || {
        daemon.stop();
        finished.store(true, Ordering::SeqCst);
    });
    // Waiting for the stop to have begun, not for a request to be mid-flight:
    // the request above is held by the rendezvous, so the stop cannot finish
    // however long this waits.
    thread::sleep(Duration::from_millis(100));
    assert!(
        !stopped.load(Ordering::SeqCst),
        "the stop is waiting for the request the backend holds"
    );

    // A request arriving while stopping is answered before anything is
    // dispatched — which is what makes asking again safe.
    later.send(&request_line(&random_uuid(), "mux.observe", json!({})));
    let refusal = later.read();
    assert_eq!(refusal["error"]["code"], "busy", "{refusal}");
    assert!(
        refusal["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("stopping"),
        "{refusal}"
    );

    // Releasing the held request lets it answer. `Client::read` fails if the
    // daemon ended the connection instead of answering, so this is the drain.
    rendezvous.release();
    let done = blocker.read();
    assert_eq!(result_record(&done)["outcome"], "completed");
    stop.join().expect("the stop thread");
    assert!(stopped.load(Ordering::SeqCst));
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
        "mux.focus",
        json!({"target":"wA:p1","target_kind":"pane"}),
    ));
    // Blocks until the request has reached the backend, which is after the
    // started record was written.
    rendezvous.entered();

    // A second connection is served while the first waits on the backend.
    let mut reader = Client::dial(daemon.path());
    assert!(
        reader
            .call("mux.observe", json!({}))
            .get("result")
            .is_some()
    );
    let started = reader.call("mux.request.get", json!({"id": id}));
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
        "mux.focus",
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
    let response = client.call("mux.close", frozen_from(&inventory, target.clone()));
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

/// A registration made after the operator's snapshot and confirmation still
/// protects its pane and its tab: the daemon re-reads the registry before it
/// dispatches a generic close, and the backend is never asked.
#[test]
fn a_registration_made_after_confirmation_blocks_pane_and_tab_close() {
    let sandbox = Sandbox::new("close-registered");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None), ("wA:p2", None)]);
    let backend = CloseBackend::new(Ok(inventory.clone()), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    // The operator's frozen identities are taken before the registration exists.
    let pane = CloseTarget::Pane("wA:p1".into());
    let tab = CloseTarget::Tab("wA:t1".into());
    let frozen = [
        (pane.clone(), frozen_from(&inventory, pane.clone())),
        (tab.clone(), frozen_from(&inventory, tab.clone())),
    ];

    let mut body = registration_body(PARENT_INCARNATION, None, None);
    body["location"] = json!({
        "backend": "herdr",
        "workspace": "wA",
        "tab": "wA:t1",
        "pane": "wA:p1",
    });
    let registered = client.call("agent.register", body);
    assert!(registered.get("result").is_some(), "{registered}");

    for (target, request) in frozen {
        let response = client.call("mux.close", request);
        assert_eq!(result_record(&response)["outcome"], "refused", "{response}");
        assert!(
            !backend.told_to_close(&target),
            "{target:?} was dispatched past its registration"
        );
    }
    daemon.stop();
}

/// A registry that cannot be read refuses a generic close outright: with no
/// answer about registrations, the pane might be an agent's, so the backend is
/// never asked.
#[test]
fn an_unreadable_registry_refuses_a_generic_pane_close() {
    let sandbox = Sandbox::new("close-unreadable-registry");
    let inventory = close_inventory("wA:t1", &[("wA:p1", None)]);
    let backend = CloseBackend::new(Ok(inventory.clone()), CloseAnswer::Completed);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    // The registry directory is replaced by a file, so every registry read fails.
    let agents = sandbox.state().join("agents");
    fs::remove_dir_all(&agents).expect("remove the registry directory");
    fs::write(&agents, b"").expect("replace it with a file");

    let target = CloseTarget::Pane("wA:p1".into());
    let response = client.call("mux.close", frozen_from(&inventory, target.clone()));
    assert_eq!(result_record(&response)["outcome"], "refused", "{response}");
    assert!(
        !backend.told_to_close(&target),
        "the backend was asked despite an unreadable registry"
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
    let response = client.call("mux.close", frozen_from(&inventory, target.clone()));
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
    let response = client.call("mux.close", frozen_from(&confirmed, target.clone()));
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
    let response = client.call("mux.close", frozen);
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
    let first = client.call_id(&id, "mux.close", frozen.clone());
    assert_eq!(result_record(&first)["outcome"], "completed");
    assert_eq!(result_record(&first)["effects"], json!(["close"]));
    assert_eq!(result_record(&first)["target"], "wA:p1");
    assert!(backend.told_to_close(&target), "the backend was not asked");

    let retry = client.call_id(&id, "mux.close", frozen);
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
    let response = client.call("mux.close", frozen_from(&inventory, target.clone()));
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
    let first = client.call_id(&id, "mux.close", frozen.clone());
    assert_eq!(result_record(&first)["outcome"], "unknown");
    assert_eq!(result_record(&first)["state"], "completed");

    let retry = client.call_id(&id, "mux.close", frozen);
    assert_eq!(
        result_record(&retry)["completed_at"],
        result_record(&first)["completed_at"]
    );

    let other_id = random_uuid();
    let other = client.call_id(
        &other_id,
        "mux.close",
        frozen_from(&inventory, target.clone()),
    );
    assert_eq!(error_code(&other), "refused");
    let refused = client.call("mux.request.get", json!({"id": other_id}));
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
        "mux.close",
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
    let backend = MuxBackend::new(MUX_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let mut unsupported = spawn_params(&parent, "/usr/bin/pi", &[]);
    unsupported["request"]["cwd"] = json!("/private");
    assert_eq!(
        error_code(&client.call("spawn.create", unsupported)),
        "bad_params"
    );
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(edges_written(&sandbox.state()), 0);

    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(
        ping["result"]["contracts"],
        advertised(MUX_PRIMITIVES),
        "a backend that does not implement launch advertises only its actual capabilities"
    );
    let ping_again = client.call("daemon.ping", json!({}));
    assert_eq!(
        ping_again["result"]["contracts"], ping["result"]["contracts"],
        "a refused spawn changes nothing that is advertised"
    );
    assert!(
        !ping_again["result"]["contracts"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("spawn.launch_context.v1")),
        "a backend that cannot launch earns no launch-context contract: {ping_again}"
    );
    let refusal = Operation::Spawn
        .ready_for(backend.primitives)
        .expect_err("launch is not implemented by this backend");
    assert_eq!(
        refusal,
        "spawn.create needs the `mux.launch` capability; this backend does not provide it"
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
    assert!(Operation::Create.ready_for(backend.primitives).is_ok());
    assert!(Operation::Input.ready_for(backend.primitives).is_ok());
    let no_launch = Primitives::CREATE.union(Primitives::INPUT);
    assert!(Operation::Spawn.ready_for(no_launch).is_err());
    daemon.stop();
}

#[test]
fn spawn_refuses_durable_metadata_when_backend_cannot_track_lifecycle() {
    let sandbox = Sandbox::new("spawn-metadata-untracked");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let mut params = spawn_params(&parent, "/usr/bin/pi", &[]);
    params["request"]["managed"] = json!({"owner":"herdr"});

    let refused = client.call_id(&random_uuid(), "spawn.create", params);
    assert_eq!(error_code(&refused), "refused", "{refused}");
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(edges_written(&sandbox.state()), 0);
    daemon.stop();
}

#[test]
fn spawn_refuses_caller_supplied_radar_identity_environment_before_any_effect() {
    let sandbox = Sandbox::new("spawn-marker-env");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let mut params = spawn_params(&parent, "/usr/bin/pi", &[]);
    params["request"]["env"] = json!({
        "PI_RADAR_MANAGED_SUBJECT_ID": "11111111-1111-4111-8111-111111111111",
        "PI_RADAR_MANAGED_PARENT_SUBJECT_ID": "22222222-2222-4222-8222-222222222222"
    });

    let refused = client.call_id(&random_uuid(), "spawn.create", params);
    assert_eq!(error_code(&refused), "bad_params", "{refused}");
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(edges_written(&sandbox.state()), 0);
    daemon.stop();
}

#[test]
fn spawn_refuses_a_missing_managed_parent_subject_before_any_effect() {
    let sandbox = Sandbox::new("spawn-parent-subject");
    let backend = MuxBackend::new(TRACKED_SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let mut params = spawn_params(&parent, "/usr/bin/pi", &[]);
    params["request"]["managed"] = json!({
        "parent_subject_id": "01a10c77-8a6b-7035-8a0e-b1fa607bb507"
    });

    let refused = client.call_id(&random_uuid(), "spawn.create", params);
    assert_eq!(error_code(&refused), "not_found", "{refused}");
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    assert_eq!(edges_written(&sandbox.state()), 0);
    daemon.stop();
}

/// A spawn the backend can serve creates under the parent's own pane, launches
/// the command the caller resolved, and answers with the two records it wrote.
#[test]
fn a_spawn_creates_beside_its_parent_launches_the_command_and_reports_its_effects() {
    configure_scope_launcher();
    let sandbox = Sandbox::new("spawn-served");
    let backend = MuxBackend::new(TRACKED_SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let mut params = spawn_params(&parent, "/usr/bin/pi", &["--child", "row one"]);
    params["request"]["cwd"] = json!("/work dir");
    params["request"]["env"] = json!({"SECRET": "PRIVATE_ENV_SENTINEL_8812"});
    params["request"]["managed"] = json!({
        "owner": "herdr",
        "run": "run-17",
        "label": "reviewer",
        "role": "worker"
    });
    let response = client.call_id(&id, "spawn.create", params.clone());

    let record = result_record(&response);
    assert_eq!(record["method"], "spawn.create");
    assert_eq!(record["outcome"], "completed");
    let lifecycle = client.call("lifecycle.list", json!({"limit": 20}));
    let subject = lifecycle["result"]["subjects"]
        .as_array()
        .expect("subjects")
        .iter()
        .find(|subject| subject["spawn_request_id"] == id)
        .expect("the tracked launch is listed");
    assert_eq!(
        subject["managed"],
        json!({
            "owner": "herdr",
            "run": "run-17",
            "label": "reviewer",
            "role": "worker"
        })
    );
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
    assert_eq!(
        spawn["parent_session_id"],
        "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"
    );
    assert_eq!(spawn["created"], "completed");
    assert_eq!(spawn["launched"], "completed");
    assert_eq!(
        spawn["location"],
        json!({"backend": "fake", "pane": "wA:p2"})
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
    let response_text = response.to_string();
    assert!(!response_text.contains("PRIVATE_ENV_SENTINEL_8812"));
    let launches = backend.launches();
    assert_eq!(launches.len(), 1);
    assert_eq!(launches[0].pane_id, "wA:p2");
    assert_ne!(
        launches[0].executable, "/usr/bin/pi",
        "tracked launches start Radar's launch chain"
    );
    assert_eq!(&launches[0].argv[..3], ["run", "--launch", "--socket"]);
    // Identity is the attached execution, never an injected variable.
    for retired in [
        agent_radar::control_plane::lifecycle::MANAGED_SUBJECT_ENV,
        agent_radar::control_plane::lifecycle::MANAGED_LAUNCH_ENV,
        agent_radar::control_plane::lifecycle::MANAGED_GENERATION_ENV,
    ] {
        assert!(
            !launches[0].env.contains_key(retired),
            "{retired} is injected"
        );
    }
    assert_eq!(launches[0].cwd.as_deref(), Some("/work dir"));
    assert_eq!(launches[0].env["SECRET"], "PRIVATE_ENV_SENTINEL_8812");
    let token = recorded_edge(&sandbox.state(), &id)["token"]
        .as_str()
        .expect("a minted token")
        .to_string();
    assert_eq!(launches[0].spawn_token, Some(token.clone()));
    let launch_claim = recorded_edge(&sandbox.state(), &id)["claim_id"]
        .as_str()
        .expect("launch claim")
        .to_string();
    let delivered = recorded_launch(&sandbox.state(), &launch_claim);
    assert_eq!(delivered.executable, "/usr/bin/pi");
    assert_eq!(delivered.argv, ["--child", "row one"]);
    assert_ne!(launch_claim, token, "a claim does not identify the process");
    assert_eq!(
        subject_record(&sandbox.state(), &launch_claim)["registration_id"],
        token,
        "the launched process receives its admitted registration"
    );

    let secret = "PRIVATE_ENV_SENTINEL_8812";
    let get = client.call("mux.request.get", json!({"id": id}));
    let list = client.call("mux.request.list", json!({"limit": 20}));
    no_secret(&get, secret);
    no_secret(&list, secret);
    let replay = client.call_id(&id, "spawn.create", params.clone());
    no_secret(&replay, secret);
    assert_eq!(backend.creates().len(), 1);
    assert_eq!(backend.launches().len(), 1);

    let mut changed_env = params.clone();
    changed_env["request"]["env"]["SECRET"] = json!("DIFFERENT_PRIVATE_ENV");
    let changed_env_response = client.call_id(&id, "spawn.create", changed_env);
    assert_eq!(error_code(&changed_env_response), "refused");
    no_secret(&changed_env_response, "DIFFERENT_PRIVATE_ENV");
    assert_eq!(backend.creates().len(), 1);
    assert_eq!(backend.launches().len(), 1);

    let mut changed = params.clone();
    changed["request"]["cwd"] = json!("/different");
    assert_eq!(
        error_code(&client.call_id(&id, "spawn.create", changed)),
        "refused"
    );
    assert_eq!(backend.creates().len(), 1);
    assert_eq!(backend.launches().len(), 1);

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
        1,
        "only the parent's session context is published"
    );
    daemon.stop();
}

/// The wire-level proof the launch capability gate was delivered for: a spawn
/// against a backend that cannot launch is refused before any effect.
#[test]
fn a_backend_without_launch_refuses_a_spawn_before_any_effect() {
    let sandbox = Sandbox::new("spawn-without-launch");
    let backend = MuxBackend::new(MUX_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let response = client.call_id(
        &id,
        "spawn.create",
        spawn_params(&parent, "/usr/bin/pi", &[]),
    );
    // The record is the answer: an operation this backend cannot serve is
    // refused in a record a client can read, not answered as a bare error.
    let record = result_record(&response);
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], "backend_unavailable");
    assert!(
        record["message"]
            .as_str()
            .expect("a message")
            .contains("the `mux.launch` capability"),
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
    let recorded = client.call("mux.request.get", json!({"id": id}));
    assert_eq!(result_record(&recorded)["outcome"], "refused");

    // Capability advertisement is unchanged: this backend implements what it
    // says it does, and its refusal adds nothing to it.
    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["contracts"], advertised(MUX_PRIMITIVES));
    daemon.stop();
}

/// A replayed request id answers what the first attempt recorded, and performs
/// no second effect.
#[test]
fn a_replayed_spawn_request_id_answers_the_recorded_effects_once() {
    let sandbox = Sandbox::new("spawn-replay");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let params = spawn_params(&parent, "/usr/bin/pi", &["--child"]);
    let first = client.call_id(&id, "spawn.create", params.clone());
    let written = fs::read(edge_path(&sandbox.state(), &id)).expect("the edge file");

    let second = client.call_id(&id, "spawn.create", params.clone());
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
    let different = client.call_id(
        &id,
        "spawn.create",
        spawn_params(&parent, "/usr/bin/other", &[]),
    );
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).launching(LaunchAnswer::Refused(
        "echo SECRET_VALUE PI_RADAR_SPAWN_TOKEN=secret-token".into(),
    ));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let mut params = spawn_params(&parent, "/usr/bin/pi", &[]);
    params["request"]["env"] = json!({"SECRET": "SECRET_VALUE"});
    let response = client.call_id(&id, "spawn.create", params);
    let response_text = response.to_string();
    assert!(!response_text.contains("SECRET_VALUE"));
    assert!(!response_text.contains("secret-token"));
    let record = result_record(&response);
    assert_eq!(record["outcome"], "refused");
    assert_eq!(record["category"], "backend_refused");
    assert_eq!(record["message"], "backend refused child launch");
    let record_text = record.to_string();
    assert!(!record_text.contains("SECRET_VALUE"));
    assert!(!record_text.contains("secret-token"));
    let fetched = client.call("mux.request.get", json!({"id": id}));
    no_secret(&fetched, "SECRET_VALUE");
    no_secret(&fetched, "secret-token");
    let listed = client.call("mux.request.list", json!({"limit": 20}));
    no_secret(&listed, "SECRET_VALUE");
    no_secret(&listed, "secret-token");
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES)
        .launching(LaunchAnswer::Unknown("the pane never answered".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");

    let id = random_uuid();
    let response = client.call_id(
        &id,
        "spawn.create",
        spawn_params(&parent, "/usr/bin/pi", &[]),
    );
    let record = result_record(&response);
    assert_eq!(record["outcome"], "unknown");
    assert_eq!(record["category"], "backend_unavailable");
    assert_eq!(
        record["message"],
        "child launch outcome is unknown; it will not be retried"
    );
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let unknown = client.call(
        "spawn.create",
        spawn_params(&random_uuid(), "/usr/bin/pi", &[]),
    );
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
    let unlocatable = registration_id(&unlocatable);
    let context = client.call("agent.context", json!({"registration_id":unlocatable,"publisher":{"source":"test-publisher","incarnation":OTHER_INCARNATION},"sequence":1,"lease_ms":10000,"context":{"session":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"}}));
    assert!(context.get("result").is_some());
    let no_pane = client.call(
        "spawn.create",
        spawn_params(&unlocatable, "/usr/bin/pi", &[]),
    );
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

/// Missing, stale, null and mismatched context on the named agent must not create.
#[test]
fn spawn_refuses_missing_stale_null_and_mismatched_exact_parent_context_before_create() {
    const SESSION: &str = "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";
    let sandbox = Sandbox::new("spawn-context-refusal");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = client.call(
        "agent.register",
        registration_body(PARENT_INCARNATION, None, None),
    );
    let parent = registration_id(&parent);

    let missing = client.call("spawn.create", spawn_params(&parent, "/usr/bin/pi", &[]));
    assert_eq!(error_code(&missing), "refused");
    assert!(edges_written(&sandbox.state()) == 0);

    let mut writer_handle: Option<String> = None;
    let mut publish = |client: &mut Client, sequence, session: Value| {
        let mut params = json!({"registration_id":parent,"publisher":{"source":"test-publisher","incarnation":PARENT_INCARNATION},"sequence":sequence,"lease_ms":1000,"context":{"session":session}});
        if let Some(handle) = &writer_handle {
            params["writer_handle"] = json!(handle);
        }
        let response = client.call("agent.context", params);
        if let Some(handle) = response["result"]["writer"]["handle"].as_str() {
            writer_handle = Some(handle.to_string());
        }
        response
    };
    for (sequence, session, label) in [
        (1, json!(null), "null"),
        (2, json!("aa2b3c4d-e5f6-4a7b-8c9d-0e1f2a3b4c5d"), "mismatch"),
    ] {
        let published = publish(&mut client, sequence, session);
        assert!(
            published.get("result").is_some(),
            "context rejected: {published}"
        );
        let refused = client.call("spawn.create", spawn_params(&parent, "/usr/bin/pi", &[]));
        assert_eq!(error_code(&refused), "refused", "{label}: {refused}");
        assert_eq!(backend.creates().len(), 0);
        assert_eq!(edges_written(&sandbox.state()), 0);
    }
    publish(&mut client, 3, json!(SESSION));
    thread::sleep(Duration::from_millis(1100));
    let stale = client.call("spawn.create", spawn_params(&parent, "/usr/bin/pi", &[]));
    assert_eq!(error_code(&stale), "refused");
    assert_eq!(backend.creates().len(), 0);
    assert_eq!(edges_written(&sandbox.state()), 0);
    daemon.stop();
}

/// The wire shapes of a spawn are validated before anything is recorded.
#[test]
fn the_wire_shapes_of_spawn_are_validated_before_dispatch() {
    let sandbox = Sandbox::new("spawn-shapes");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
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
            "`request` is not a valid spawn request",
        ),
        (
            "a request with no command",
            json!({"request": {"parent": &parent, "parent_session_id":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d"}}),
            "`request` is not a valid spawn request",
        ),
        (
            "no request at all",
            json!({"requester": "test"}),
            "`request` is required",
        ),
        (
            "reserved launch token",
            {
                let mut value = spawn_params(&parent, "/usr/bin/pi", &[]);
                value["request"]["env"] =
                    json!({"PI_RADAR_REGISTRATION_ID":"WIRE_SECRET_SENTINEL"});
                value
            },
            "cannot set reserved",
        ),
        (
            "invalid environment value type",
            {
                let mut value = spawn_params(&parent, "/usr/bin/pi", &[]);
                value["request"]["env"] = json!({"SAFE":["WIRE_SECRET_SENTINEL"]});
                value
            },
            "`request` is not a valid spawn request",
        ),
    ];
    for (case, params, expected) in cases {
        let response = client.call("spawn.create", params);
        assert_eq!(error_code(&response), "bad_params", "{case}: {response}");
        assert!(
            !response.to_string().contains("WIRE_SECRET_SENTINEL"),
            "{case}: {response}"
        );
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
    let base = MuxBackend::new(SPAWN_PRIMITIVES)
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
    let spawned = client.call_id(
        &edge_id,
        "spawn.create",
        spawn_params(&parent, "/usr/bin/pi", &[]),
    );
    assert_eq!(result_record(&spawned)["outcome"], "completed");
    let edge = recorded_edge(&sandbox.state(), &edge_id);
    let token = edge["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    let registered = register_close_child(&mut client, &token, process);
    assert!(
        registered["result"]["registration"]["registration_id"].is_string(),
        "{registered}"
    );
    let registration_id = registered["result"]["registration"]["registration_id"]
        .as_str()
        .expect("the child's registration")
        .to_owned();
    let registration = client.call("agent.get", json!({"registration_id": &registration_id}));
    let close_id = random_uuid();
    let selected =
        agent_radar::control_plane::client::connect_at(daemon.path()).expect("dashboard client");
    let first = selected
        .lifecycle
        .as_ref()
        .expect("daemon lifecycle controls")
        .close_child(
            &close_id,
            &agent_radar::registry_observation::ChildCloseTarget {
                spawn_request_id: edge_id.clone(),
                source: "test-publisher".into(),
                incarnation: CHILD_INCARNATION.into(),
            },
            &AtomicBool::new(false),
        )
        .expect("spawn.close through dashboard client");
    assert_eq!(first.outcome(), "completed", "{first:?}");
    assert_eq!(first.request["intent"], "cancel");
    assert_eq!(first.request["pane"], "wA:p2");
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
        "spawn.close",
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
        client.call("agent.get", json!({"registration_id": &registration_id}))["result"]["agent"],
        registration["result"]["agent"],
        "a close rewrites no registration of the child it closed"
    );
    let ping = client.call("daemon.ping", json!({}));
    assert!(
        !ping["result"]["contracts"]
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES)
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
    let spawned = client.call_id(
        &edge_id,
        "spawn.create",
        spawn_params(&parent, "/usr/bin/pi", &[]),
    );
    assert_eq!(result_record(&spawned)["outcome"], "completed");
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, process);

    let close_id = random_uuid();
    let refused = client.call_id(
        &close_id,
        "spawn.close",
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
        "spawn.close",
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
        "spawn.close",
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
        let backend = MuxBackend::new(SPAWN_PRIMITIVES)
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
        let spawned = client.call_id(
            &edge_id,
            "spawn.create",
            spawn_params(&parent, "/usr/bin/pi", &[]),
        );
        assert_eq!(result_record(&spawned)["outcome"], "completed");
        let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
            .as_str()
            .expect("private spawn token")
            .to_owned();
        register_close_child(&mut client, &token, process.clone());

        let lost = client.call_id(
            &close_id,
            "spawn.close",
            child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
        );
        // An unconfirmed dispatch is its own word: `outcome_unknown`, never the
        // `backend_unavailable` a caller reads as "nothing was dispatched".
        assert_eq!(error_code(&lost), "outcome_unknown", "{lost}");
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES)
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
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&replay), "outcome_unknown", "{replay}");
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
        let backend = MuxBackend::new(SPAWN_PRIMITIVES)
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
        client.call_id(
            &id,
            "spawn.create",
            spawn_params(&parent, "/usr/bin/pi", &[]),
        );
        let token = recorded_edge(&sandbox.state(), &id)["token"]
            .as_str()
            .unwrap()
            .to_owned();
        register_close_child(&mut client, &token, process);
        let close_id = random_uuid();
        let response = client.call_id(
            &close_id,
            "spawn.close",
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

/// A retained pane is closed only from evidence read now *and* the exit this edge
/// recorded: a dead report alone is not a close, and a recorded exit alone is
/// history.
#[test]
fn a_dead_child_pane_closes_only_from_a_recorded_exit_and_a_dead_report_now() {
    let sandbox = Sandbox::new("dead-child-close");
    let backend = MuxBackend::new(DEAD_CLOSE_PRIMITIVES).observing(&["wA:p2"]);
    let verifier = stage_dead_pane(&backend, "wA:p2", Some(900));
    let mut daemon = daemon_with_staged_verifier(&sandbox, backend.clone(), verifier);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let edge_id = random_uuid();
    spawned_edge(&mut client, &edge_id, &parent);
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, staged_identity(200, 400));

    // The pane is dead now, but this edge never observed the exit: what happened
    // to a pane is not established by the pane's state alone.
    let unrecorded_id = random_uuid();
    let unrecorded = client.call_id(
        &unrecorded_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert!(unrecorded.get("error").is_some(), "{unrecorded}");
    assert!(
        !closes_directory(&sandbox.state())
            .join(format!("{unrecorded_id}.json"))
            .exists()
    );
    assert!(backend.closed_targets().is_empty());

    // The read that observes the exit records it on the edge. A pane running
    // again — a same-UID respawn, which tmux permits — is not a dead pane to
    // clean up however recorded its earlier exit is.
    let topology = client.call("spawn.get", json!({"request_id": edge_id}));
    assert_eq!(
        topology["result"]["spawn"]["exit_observation"]["code"],
        json!(23),
        "{topology}"
    );
    backend.reporting_exit("wA:p2", PaneExitObservation::Running);
    let running_id = random_uuid();
    let running = client.call_id(
        &running_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert!(running.get("error").is_some(), "{running}");
    assert!(backend.closed_targets().is_empty());

    // Dead now, with the recorded exit: the retained pane is cleaned up once. The
    // answer carries the exit the edge recorded, which a later report of the same
    // pane's state does not rewrite.
    backend.reporting_exit(
        "wA:p2",
        PaneExitObservation::Exited {
            code: Some(17),
            signal: None,
        },
    );
    let close_id = random_uuid();
    let closed = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(
        closed["result"]["close"]["outcome"], "already_exited",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["exit_observation"]["state"], "exited",
        "{closed}"
    );
    assert_eq!(
        closed["result"]["close"]["exit_observation"]["code"],
        json!(23),
        "{closed}"
    );
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Pane("wA:p2".into())]
    );

    // The same request id is answered from the record rather than closed twice.
    let replay = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(replay, closed);
    assert_eq!(backend.closed_targets().len(), 1);
    daemon.stop();
}

/// A pane the daemon cannot establish now is not established by dead history: an
/// unreadable inventory and a pane absent from it both refuse before recording.
#[test]
fn a_close_refuses_before_recording_when_the_pane_cannot_be_established_now() {
    let sandbox = Sandbox::new("dead-child-close-evidence");
    let process = ProcessIdentity {
        boot_id: "boot-test".into(),
        pid: 200,
        start_ticks: 400,
    };
    let backend = MuxBackend::new(DEAD_CLOSE_PRIMITIVES).observing(&["wA:p2"]);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let edge_id = random_uuid();
    spawned_edge(&mut client, &edge_id, &parent);
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, process);
    backend.reporting_exit(
        "wA:p2",
        PaneExitObservation::Exited {
            code: Some(23),
            signal: None,
        },
    );

    backend.refusing_inventory("tmux is gone");
    let unreadable_id = random_uuid();
    let unreadable = client.call_id(
        &unreadable_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(
        error_code(&unreadable),
        "backend_unavailable",
        "{unreadable}"
    );
    assert!(
        !closes_directory(&sandbox.state())
            .join(format!("{unreadable_id}.json"))
            .exists()
    );
    assert!(backend.closed_targets().is_empty());
    drop(client);
    daemon.stop();

    // The same recorded edge and child, read by a daemon whose inventory does not
    // hold the pane: the pane it would close is not there, so the dead-pane
    // evidence is never reached and nothing is dispatched.
    let backend = MuxBackend::new(DEAD_CLOSE_PRIMITIVES).observing(&["wA:p9"]);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    backend.reporting_exit(
        "wA:p2",
        PaneExitObservation::Exited {
            code: Some(23),
            signal: None,
        },
    );
    let absent_id = random_uuid();
    let absent = client.call_id(
        &absent_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&absent), "refused", "{absent}");
    assert!(
        absent["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("absent from fresh inventory")),
        "{absent}"
    );
    assert!(
        !closes_directory(&sandbox.state())
            .join(format!("{absent_id}.json"))
            .exists()
    );
    assert!(backend.closed_targets().is_empty());
    daemon.stop();
}

/// A dead pane whose close the backend refuses is a refusal, not a cleanup: the
/// record holds no observation, and only a confirmed close may claim one.
#[test]
fn a_dead_pane_close_the_backend_refuses_is_not_reported_as_cleanup() {
    let sandbox = Sandbox::new("dead-child-close-refused");
    let backend = MuxBackend::new(DEAD_CLOSE_PRIMITIVES)
        .observing(&["wA:p2"])
        .closing(CloseAnswer::Refused("no such pane".into()));
    let verifier = stage_dead_pane(&backend, "wA:p2", Some(900));
    let mut daemon = daemon_with_staged_verifier(&sandbox, backend.clone(), verifier);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let edge_id = random_uuid();
    spawned_edge(&mut client, &edge_id, &parent);
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, staged_identity(200, 400));
    let topology = client.call("spawn.get", json!({"request_id": edge_id}));
    assert_eq!(
        topology["result"]["spawn"]["exit_observation"]["state"], "exited",
        "{topology}"
    );

    let close_id = random_uuid();
    let refused = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&refused), "refused", "{refused}");
    assert_eq!(refused["error"]["message"], "no such pane", "{refused}");
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Pane("wA:p2".into())]
    );
    let record = close_record(&sandbox.state(), &close_id);
    assert_eq!(record["outcome"], "refused", "{record}");
    assert!(
        record.get("exit").is_none(),
        "a refused close observes nothing: {record}"
    );
    daemon.stop();
}

/// A pane id outlives the server that answered the create, so an edge that
/// cannot name the instance it was created in never cleans one up: a legacy or
/// unverifiable edge is unfenced, and the pane that now carries the id is not
/// Radar's to close.
#[test]
fn a_dead_pane_close_refuses_when_the_edge_records_no_instance() {
    let sandbox = Sandbox::new("dead-child-close-unfenced");
    let backend = MuxBackend::new(DEAD_CLOSE_PRIMITIVES).observing(&["wA:p2"]);
    // The backend names no serving process, so the create records no instance.
    let verifier = stage_dead_pane(&backend, "wA:p2", None);
    let mut daemon = daemon_with_staged_verifier(&sandbox, backend.clone(), verifier);
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let edge_id = random_uuid();
    spawned_edge(&mut client, &edge_id, &parent);
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, staged_identity(200, 400));
    let topology = client.call("spawn.get", json!({"request_id": edge_id}));
    assert_eq!(
        topology["result"]["spawn"]["exit_observation"]["code"],
        json!(23),
        "{topology}"
    );
    assert!(
        recorded_edge(&sandbox.state(), &edge_id)
            .get("instance")
            .is_none(),
        "no instance is inferred for an edge whose create recorded none"
    );

    let close_id = random_uuid();
    let unfenced = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&unfenced), "refused", "{unfenced}");
    assert_eq!(
        unfenced["error"]["message"],
        "the spawn edge records no verified backend instance for its pane",
        "{unfenced}"
    );
    assert!(
        !closes_directory(&sandbox.state())
            .join(format!("{close_id}.json"))
            .exists(),
        "an unfenced pane is refused before admission"
    );
    assert!(backend.closed_targets().is_empty());
    daemon.stop();
}

/// A server that has replaced the one that created a pane is not the instance
/// this edge was recorded against, whatever pane id it hands out: the close
/// refuses before it is admitted, and dispatches nothing.
#[test]
fn a_dead_pane_close_refuses_when_the_serving_instance_changed() {
    let sandbox = Sandbox::new("dead-child-close-refenced");
    let backend = MuxBackend::new(DEAD_CLOSE_PRIMITIVES).observing(&["wA:p2"]);
    let verifier = stage_dead_pane(&backend, "wA:p2", Some(900));
    let mut daemon = daemon_with_staged_verifier(&sandbox, backend.clone(), verifier.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    let edge_id = random_uuid();
    spawned_edge(&mut client, &edge_id, &parent);
    let token = recorded_edge(&sandbox.state(), &edge_id)["token"]
        .as_str()
        .expect("private spawn token")
        .to_owned();
    register_close_child(&mut client, &token, staged_identity(200, 400));
    let topology = client.call("spawn.get", json!({"request_id": edge_id}));
    assert_eq!(
        topology["result"]["spawn"]["exit_observation"]["code"],
        json!(23),
        "{topology}"
    );
    assert_eq!(
        recorded_edge(&sandbox.state(), &edge_id)["instance"],
        json!({"boot_id": "boot-test", "pid": 900, "start_ticks": 400}),
        "the edge records the verified instance its create was served by"
    );

    // The same pid now names a different process: a server restarted under the
    // name this daemon addresses reuses both the pid and the pane ids.
    verifier.staged(staged_identity(900, 900));
    let close_id = random_uuid();
    let refenced = client.call_id(
        &close_id,
        "spawn.close",
        child_close_params(&edge_id, "test-publisher", CHILD_INCARNATION),
    );
    assert_eq!(error_code(&refenced), "refused", "{refenced}");
    assert_eq!(
        refenced["error"]["message"],
        "the backend instance serving now is not the one that created the recorded pane",
        "{refenced}"
    );
    assert!(
        !closes_directory(&sandbox.state())
            .join(format!("{close_id}.json"))
            .exists(),
        "a pane from another instance is refused before admission"
    );
    assert!(backend.closed_targets().is_empty());
    daemon.stop();
}

/// A creation-time verifier that stalls is abandoned at the deadline, on the same
/// daemon-wide budget the close path uses: the spawn still answers, its edge
/// records no instance, and requests past the bound start no work of their own.
#[test]
fn a_stalled_creation_verifier_is_abandoned_on_the_daemons_verification_budget() {
    const STALL_BOUND: Duration = Duration::from_secs(6);
    let sandbox = Sandbox::new("spawn-verifier-stall");
    let verifier = Arc::new(GatedVerifier::new());
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p1", "wA:p2"]);
    // The backend names a serving process, so a create asks the verifier about it
    // — and this verifier never answers.
    backend.served_by(900);
    let mut daemon = Daemon::bind_with_backend_and_verifier(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend.clone()),
        verifier.clone(),
    )
    .expect("bind with a stalled verifier");
    let mut client = Client::dial(daemon.path());
    // One parent per stalled create: an operation on a target is refused while
    // another on that target is in flight, so separate subjects are what makes
    // these creates concurrent.
    let parents = (1..=(MAX_VERIFICATION_JOBS + 1))
        .map(|n| {
            let incarnation = format!("3f1c7a2e-9d4b-4c58-9c7b-{n:012x}");
            registered_parent(&mut client, &incarnation, &format!("wA:p{n}"))
        })
        .collect::<Vec<_>>();

    let path = daemon.path().to_path_buf();
    let stalls = parents[..MAX_VERIFICATION_JOBS]
        .iter()
        .map(|parent| {
            let path = path.clone();
            let parent = parent.clone();
            thread::spawn(move || {
                let mut client = Client::dial(&path);
                let request = random_uuid();
                let answer = client.call_id(
                    &request,
                    "spawn.create",
                    spawn_params(&parent, "/usr/bin/pi", &[]),
                );
                (request, answer)
            })
        })
        .collect::<Vec<_>>();
    wait_for("the creation verifiers to hold the daemon's budget", || {
        verifier.started() == MAX_VERIFICATION_JOBS
    });
    assert!(
        client.call("daemon.ping", json!({}))["result"].is_object(),
        "the daemon answers while every verifier slot is held"
    );

    // Every slot is held by a call that never returns, so a request past the bound
    // is answered without starting another: the budget is the daemon's, not a
    // per-request allowance, however many requests arrive.
    let extra_id = random_uuid();
    let extra_started = Instant::now();
    let extra = client.call_id(
        &extra_id,
        "spawn.create",
        spawn_params(&parents[MAX_VERIFICATION_JOBS], "/usr/bin/pi", &[]),
    );
    assert!(extra["result"].is_object(), "{extra}");
    assert!(
        extra_started.elapsed() < STALL_BOUND,
        "a request past the bound waited for a slot: {:?}",
        extra_started.elapsed()
    );
    assert_eq!(
        verifier.started(),
        MAX_VERIFICATION_JOBS,
        "a request past the bound started another verifier"
    );

    // The stalled calls answer at their deadline rather than with the verifier:
    // the create still runs, and the edge records no instance it could not verify.
    let mut requests = vec![extra_id];
    let abandoned = Instant::now();
    for stall in stalls {
        let (request, answer) = stall.join().expect("a stalled spawn answered");
        assert!(answer["result"].is_object(), "{answer}");
        requests.push(request);
    }
    assert!(
        abandoned.elapsed() < STALL_BOUND,
        "a stalled spawn was not abandoned at its deadline: {:?}",
        abandoned.elapsed()
    );
    for request in requests {
        let edge = recorded_edge(&sandbox.state(), &request);
        assert!(
            edge["location"]["pane"].is_string(),
            "the create ran after an unverifiable capture: {edge}"
        );
        assert!(
            edge.get("instance").is_none(),
            "an unverifiable instance is never recorded: {edge}"
        );
    }
    daemon.stop();
    verifier.release_all();
}

/// Binds a daemon over this sandbox that confirms only the identities `verifier`
/// was handed, so the processes a fence reads are staged one by one.
fn daemon_with_staged_verifier(
    sandbox: &Sandbox,
    backend: Arc<MuxBackend>,
    verifier: Arc<StagedVerifier>,
) -> Daemon {
    Daemon::bind_with_backend_and_verifier(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend),
        verifier,
    )
    .expect("bind the control socket with a staged verifier")
}

/// Stages the evidence a dead-pane close is fenced by: a reported exit for this
/// pane and, when the test names one, the pid its backend claims serves it and
/// the verifier that confirms it.
fn stage_dead_pane(
    backend: &Arc<MuxBackend>,
    pane: &str,
    server: Option<i32>,
) -> Arc<StagedVerifier> {
    backend.reporting_exit(
        pane,
        PaneExitObservation::Exited {
            code: Some(23),
            signal: None,
        },
    );
    let verifier = StagedVerifier::new();
    if let Some(pid) = server {
        backend.served_by(pid);
        verifier.staged(staged_identity(pid, 400));
    }
    verifier
}

#[test]
fn the_topology_read_reports_the_edge_its_own_operation_recorded() {
    let sandbox = Sandbox::new("spawn-read");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p1", "wA:p2"]);
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
    assert_eq!(spawn["exit_observation"]["state"], "unknown");
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p1"]);
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p2"]);
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p2"]);
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
    let backend = MuxBackend::new(SPAWN_PRIMITIVES).observing(&["wA:p2", "wA:p9"]);
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
    let backend = MuxBackend::new(MUX_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["contracts"], advertised(MUX_PRIMITIVES));

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
    let create = client.call_id(&create_id, "mux.create", create_params.clone());
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
    let replay = client.call_id(&create_id, "mux.create", create_params);
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
        "mux.input",
        json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "ls\n"}}}),
    );
    assert_eq!(result_record(&text)["outcome"], "completed");
    assert_eq!(result_record(&text)["target"], "wA:p1");
    assert_eq!(result_record(&text)["effects"], json!(["sent text"]));

    let keys = client.call(
        "mux.input",
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
        "mux.output",
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
    let missing = client.call("mux.request.get", json!({"id": id}));
    assert_eq!(error_code(&missing), "not_found");
    daemon.stop();
}

/// A confirmation that named nothing carries no identity: the field is absent
/// rather than invented from the request, and the record says so in words.
#[test]
fn a_create_that_named_nothing_carries_no_identity() {
    let sandbox = Sandbox::new("mux-silent-create");
    let backend = MuxBackend::new(MUX_PRIMITIVES).naming(CreatedLocation::default());
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let create = client.call(
        "mux.create",
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
    let backend = MuxBackend::new(MUX_PRIMITIVES).reading(OutputRead {
        text: long.clone(),
        truncated: false,
        revision: None,
    });
    let mut daemon = sandbox.daemon_with(backend);
    let mut client = Client::dial(daemon.path());

    let read = client.call(
        "mux.output",
        json!({"pane_id": "wA:p1", "source": "recent"}),
    );
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
    let backend = MuxBackend::new(Primitives::NONE);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["contracts"], advertised(Primitives::NONE));

    // An operation this backend cannot serve leaves a settled refusal that names
    // the capability and claims no effect, not a request still claiming to be
    // started.
    let create = client.call(
        "mux.create",
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
            .contains("`mux.create`")
    );
    assert_eq!(result_record(&create)["effects"], json!([]));
    assert_eq!(result_record(&create)["state"], "completed");
    // A refusal names no identity: nothing was created.
    assert!(result_record(&create)["created"].is_null());

    let left = client.call(
        "mux.input",
        json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "x"}}}),
    );
    assert_eq!(result_record(&left)["outcome"], "refused");
    assert!(
        result_record(&left)["message"]
            .as_str()
            .expect("the message")
            .contains("`mux.input`")
    );
    // A read has no record, so its refusal is the whole answer.
    let read = client.call(
        "mux.output",
        json!({"pane_id": "wA:p1", "source": "visible"}),
    );
    assert_eq!(error_code(&read), "backend_unavailable");
    assert!(
        read["error"]["message"]
            .as_str()
            .expect("the message")
            .contains("`mux.output`")
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
    let backend = MuxBackend::new(MUX_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let many_keys: Vec<String> = (0..17).map(|_| "enter".to_string()).collect();
    let cases = [
        (
            "mux.create",
            json!({"request": {"kind": "nowhere", "focus": false}}),
        ),
        (
            "mux.create",
            json!({"request": {"kind": "pane_split", "pane_id": "", "direction": "right", "focus": false}}),
        ),
        (
            "mux.create",
            json!({"request": {"kind": "workspace", "focus": false}, "extra": 1}),
        ),
        (
            "mux.input",
            json!({"request": {"pane_id": "", "payload": {"kind": "text", "text": "ls\n"}}}),
        ),
        (
            "mux.input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": ""}}}),
        ),
        (
            "mux.input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "text", "text": "two\u{7}shapes"}}}),
        ),
        (
            "mux.input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "keys", "keys": []}}}),
        ),
        (
            "mux.input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "keys", "keys": many_keys}}}),
        ),
        (
            "mux.input",
            json!({"request": {"pane_id": "wA:p1", "payload": {"kind": "keys", "keys": ["esc\u{1b}"]}}}),
        ),
        (
            "mux.output",
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
    let backend = MuxBackend::new(MUX_PRIMITIVES).gated(Arc::clone(&gate));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut held_client = Client::dial(daemon.path());
    let mut other = Client::dial(daemon.path());

    let held = random_uuid();
    held_client.send(&request_line(
        &held,
        "mux.create",
        json!({"request": {"kind": "workspace", "focus": false}}),
    ));
    rendezvous.entered();

    let second = random_uuid();
    let refused = other.call_id(
        &second,
        "mux.create",
        json!({"request": {"kind": "workspace", "focus": false}}),
    );
    assert_eq!(error_code(&refused), "refused");
    let record = other.call("mux.request.get", json!({"id": second}));
    assert_eq!(result_record(&record)["category"], Category::IN_FLIGHT);

    let input = other.call(
        "mux.input",
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
        metadata_agents: Vec::new(),
        containers: Vec::new(),
        lifecycle_subjects: Vec::new(),
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
        Self::spawn_with(socket, state, &[])
    }

    /// Starts `radar daemon` over `socket` and `state`, with `environment`
    /// added to the child's own.
    fn spawn_with(socket: &Path, state: &Path, environment: &[(&str, &str)]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_radar"));
        command
            .arg("daemon")
            .env("RADAR_CONTROL_SOCKET", socket)
            .env("RADAR_CONTROL_STATE", state)
            // Every tracked launch enters a scope, so the daemon needs the
            // launcher the way it needs its tmux. A sandbox without one still
            // names an absolute program, and a backend that never execs finds
            // nothing missing.
            .env("RADAR_SYSTEMD_RUN", scope_launcher())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in environment {
            command.env(key, value);
        }
        let process = command.spawn().expect("start radar daemon");
        Self {
            process,
            exited: false,
        }
    }

    /// Waits for a child that is expected to exit on its own, under a deadline.
    fn wait(&mut self, deadline: Duration) -> ExitStatus {
        let status = wait_for_exit(&mut self.process, deadline);
        self.exited = true;
        status
    }

    /// Signals the daemon and waits for it to go, under a deadline.
    fn stop(&mut self, signal: i32, deadline: Duration) -> ExitStatus {
        // SAFETY: `kill` takes the pid of a child this process started and a
        // signal number.
        unsafe { libc::kill(self.process.id() as i32, signal) };
        self.wait(deadline)
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

/// The contracts a scripted daemon advertises: none, so a scripted exchange is only
/// about the mux method it scripts.
const CLIENT_CONTRACTS: &[&str] = &[];

/// The client fake's complete typed primitive set.
const CLIENT_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::FOCUS)
    .union(Primitives::CLOSE)
    .union(Primitives::CREATE)
    .union(Primitives::INPUT)
    .union(Primitives::OUTPUT)
    .union(Primitives::REPORT);

/// A fake backend that serves everything a client can ask for, so one daemon
/// exercises the whole client surface. Nothing here is a mux.
struct ClientBackend {
    primitives: Primitives,
    inventory: FleetObservation,
    evidence: ForegroundEvidence,
    close: CloseAnswer,
    closed: Mutex<Vec<CloseTarget>>,
}

impl ClientBackend {
    fn new(primitives: Primitives) -> Arc<Self> {
        let (inventory, evidence) = populated_graph();
        Arc::new(Self {
            primitives,
            inventory,
            evidence,
            close: CloseAnswer::Completed,
            closed: Mutex::new(Vec::new()),
        })
    }

    /// The same backend observing this fleet instead.
    fn observing(self: &Arc<Self>, inventory: FleetObservation) -> Arc<Self> {
        Arc::new(Self {
            primitives: self.primitives,
            inventory,
            evidence: self.evidence.clone(),
            close: self.close.clone(),
            closed: Mutex::new(Vec::new()),
        })
    }

    /// The same backend answering a close with this outcome.
    fn closing(self: &Arc<Self>, close: CloseAnswer) -> Arc<Self> {
        Arc::new(Self {
            primitives: self.primitives,
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
    fn identity(&self) -> BackendIdentity {
        BackendIdentity::named("fake")
    }

    fn primitives(&self) -> Primitives {
        self.primitives
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
        "revision": agent_radar::control_plane::ops::PROTOCOL_REVISION,
        "backend": "herdr",
        "instance": null,
        "contracts": CLIENT_CONTRACTS,
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
    let backend = ClientBackend::new(CLIENT_PRIMITIVES).observing(fleet.clone());
    let mut daemon = sandbox.daemon_with(backend.clone());
    let client = DaemonRuntime::handshake(daemon.path()).expect("the daemon is usable");
    let cancel = AtomicBool::new(false);
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
    let listed = reader.call("mux.request.list", json!({}));
    let records = listed["result"]["requests"].as_array().expect("a list");
    let methods: Vec<&str> = records
        .iter()
        .filter_map(|record| record["method"].as_str())
        .collect();
    assert_eq!(methods.len(), 5, "{records:?}");
    for recorded in [
        "mux.focus",
        "mux.close",
        "mux.create",
        "mux.input",
        "mux.report",
    ] {
        assert!(methods.contains(&recorded), "{recorded} was not recorded");
    }
    let close = records
        .iter()
        .find(|record| record["method"] == "mux.close")
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
    let backend = ClientBackend::new(DaemonRuntime::REQUIRED);
    let mut daemon = sandbox.daemon_with(backend);
    let client = DaemonRuntime::handshake(daemon.path()).expect("the dashboard's needs are met");
    let cancel = AtomicBool::new(false);

    let refused = client.create(
        &CreateRequest::Workspace {
            focus: false,
            directory: None,
        },
        &cancel,
    );
    match refused {
        CreateOutcome::Refused(message) => {
            assert!(message.contains("`mux.create`"), "{message}");
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
        InputOutcome::Refused(message) => assert!(message.contains("`mux.input`"), "{message}"),
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
        ReportOutcome::Refused(message) => assert!(message.contains("`mux.report`"), "{message}"),
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
        OutputOutcome::Refused(message) => assert!(message.contains("`mux.output`"), "{message}"),
        other => panic!("{other:?}"),
    }
    daemon.stop();
}

/// A daemon that cannot be reached is a refusal that names why. Nothing is
/// substituted for it: the caller shows the reason and retries.
#[test]
fn an_unreachable_daemon_is_refused_with_its_reason() {
    let sandbox = Sandbox::new("client-unreachable");
    let socket = sandbox.root.join("absent.sock");
    let diagnostic = connect_at(&socket).err().expect("a refusal");

    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(
        diagnostic.contains("No such file or directory"),
        "{diagnostic}"
    );
}

/// A socket path Radar may not trust is refused *before* it is dialled: the
/// daemon could not have bound a socket the check rejects, so whoever did is
/// never taken as Radar's observation and control plane, and the fallback is the
/// direct adapter with one line naming the failing check.
#[test]
fn a_socket_path_radar_may_not_trust_is_refused_before_any_connection() {
    let sandbox = Sandbox::new("client-planted");

    // A working fake daemon in a directory its owner left group-writable. This is
    // the socket-ahead-of-the-daemon case: it answers a handshake, and it is
    // refused anyway. The marker it writes on any request is what proves the
    // refusal happened before a connection rather than after it.
    let shared = sandbox.root.join("shared");
    fs::create_dir_all(&shared).expect("a shared directory");
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o770)).expect("a mode");
    let marker = sandbox.root.join("shared-was-asked");
    let planted = planted_daemon(&shared, "control.sock", marker.clone());
    let diagnostic = connect_at(&planted).err().expect("a refusal");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("not mode 0700"), "{diagnostic}");
    assert!(!marker.exists(), "Radar dialled a socket it may not trust");

    // The same live socket reached through a symlink at the name: the rule is
    // about the path, not about what the path points at.
    let linked = sandbox.root.join("linked.sock");
    std::os::unix::fs::symlink(&planted, &linked).expect("a symlink");
    let diagnostic = connect_at(&linked).err().expect("a refusal");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("is a symlink"), "{diagnostic}");
    assert!(!marker.exists(), "Radar dialled a socket it may not trust");

    // A regular file wearing a control socket's name.
    let file = sandbox.root.join("file.sock");
    fs::write(&file, b"not a socket").expect("a file");
    let diagnostic = connect_at(&file).err().expect("a refusal");
    assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
    assert!(diagnostic.contains("is not a socket"), "{diagnostic}");
}

/// A working fake daemon planted at `root/name`, which records in `marker` that
/// anything at all was asked of it. A client that connected and sent its ping
/// would leave the file behind, so its absence is what proves nothing was dialled.
fn planted_daemon(root: &Path, name: &str, marker: PathBuf) -> PathBuf {
    scripted_socket(root, name, move |request| {
        let _ = fs::write(&marker, b"asked");
        (scripted_method(request) == "daemon.ping").then(|| scripted_ping(request))
    })
}

/// A daemon older than this build, which an install it outlived leaves behind, is
/// refused with one line that says how to replace it. A daemon that answers no
/// revision at all is older still.
#[test]
fn an_older_daemon_is_refused_and_told_to_restart() {
    let sandbox = Sandbox::new("client-incompatible");
    for answer_revision in [None, Some(0_u64)] {
        let name = format!("older-{}.sock", answer_revision.unwrap_or(99));
        let path = scripted_socket(&sandbox.root, &name, move |request| {
            let mut ping = scripted_ping(request);
            match answer_revision {
                Some(revision) => ping["result"]["revision"] = json!(revision),
                None => {
                    ping["result"].as_object_mut().unwrap().remove("revision");
                }
            }
            (scripted_method(request) == "daemon.ping").then_some(ping)
        });
        let diagnostic = connect_at(&path).err().expect("a refusal");
        assert_eq!(diagnostic.lines().count(), 1, "{diagnostic}");
        assert!(
            diagnostic.contains("is older than this build"),
            "{diagnostic}"
        );
        assert!(
            diagnostic.contains("systemctl --user restart radar-daemon"),
            "{diagnostic}"
        );
    }
}

/// A contract name this build does not know is tolerated and never used, so a
/// newer daemon does not make an older client refuse to start.
#[test]
fn an_unknown_contract_name_is_tolerated() {
    let sandbox = Sandbox::new("client-unknown-contract");
    let path = scripted_socket(&sandbox.root, "newer.sock", |request| {
        let mut ping = scripted_ping(request);
        ping["result"]["contracts"] = json!(["from-the-future.v9"]);
        (scripted_method(request) == "daemon.ping").then_some(ping)
    });
    assert!(connect_at(&path).is_ok());
}

/// A daemon that provides what the dashboard needs is the one used, with nothing
/// printed: the fallback line is for the fallback.
#[test]
fn a_usable_daemon_is_selected_without_a_diagnostic() {
    let sandbox = Sandbox::new("client-selected");
    let backend = ClientBackend::new(CLIENT_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend);
    let selection = connect_at(daemon.path()).expect("a usable daemon");

    let cancel = AtomicBool::new(false);
    let (inventory, _) = populated_graph();
    assert_eq!(
        selection.provider.inventory(&cancel).expect("inventory"),
        inventory
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
            "daemon.ping" => Some(scripted_ping(request)),
            _ => Some(json!({"id": request["id"], "error": {
                "code": "internal",
                "message": "the store is unreadable",
            }})),
        },
    );
    let client = DaemonRuntime::handshake(&internal).expect("a scripted daemon");
    let outcome = client.focus_outcome(&Target::Pane("wA:p1".into()), &cancel);
    assert!(matches!(outcome, FocusOutcome::Unknown(_)), "{outcome:?}");

    // `outcome_unknown` is the daemon's word for a dispatched request whose
    // result is not confirmed. On the close path that must reach the caller as
    // an unestablished effect: reading it as a refusal would license a retry of
    // a close that may already have happened.
    let inventory = serde_json::to_value(close_inventory("wA:t1", &[("wA:p1", None)]))
        .expect("an inventory shape");
    let uncertain =
        scripted_socket(
            &sandbox.root,
            "outcome-unknown.sock",
            move |request| match scripted_method(request) {
                "daemon.ping" => Some(scripted_ping(request)),
                "mux.observe" => Some(json!({
                    "id": request["id"],
                    "result": {"inventory": inventory.clone()},
                })),
                _ => Some(json!({"id": request["id"], "error": {
                    "code": "outcome_unknown",
                    "message": "managed child close outcome is unknown",
                }})),
            },
        );
    let client = DaemonRuntime::handshake(&uncertain).expect("a scripted daemon");
    let outcome = client.close_outcome(&CloseTarget::Pane("wA:p1".into()), &cancel);
    assert!(matches!(outcome, CloseOutcome::Unknown(_)), "{outcome:?}");

    // A connection that ends without an answer may have carried the request.
    let silent = scripted_socket(
        &sandbox.root,
        "silent.sock",
        |request| match scripted_method(request) {
            "daemon.ping" => Some(scripted_ping(request)),
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
                "daemon.ping" => Some(scripted_ping(request)),
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
            "daemon.ping" => Some(scripted_ping(request)),
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
    // Herdr starts without a systemd unit, so the handshake runs against it here.
    let mut child = DaemonChild::spawn_with(&socket, &state, &[("RADAR_CONTROL_BACKEND", "herdr")]);
    let mut lines = BufReader::new(child.process.stdout.take().expect("stdout")).lines();
    lines
        .next()
        .expect("a greeting line")
        .expect("a readable greeting");

    // The handshake alone: the daemon's own capabilities are static, so nothing
    // here asks the mux anything.
    let selection = connect_at(&socket).expect("a usable daemon");
    assert!(
        selection
            .provider
            .primitives()
            .contains(DaemonRuntime::REQUIRED)
    );

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
            "daemon.ping" => Some(scripted_ping(request)),
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
    let backend = ClientBackend::new(CLIENT_PRIMITIVES)
        .observing(inventory.clone())
        .closing(CloseAnswer::Unknown("the reply was lost".into()));
    let mut daemon = sandbox.daemon_with(backend.clone());
    let selection = connect_at(daemon.path()).expect("a usable daemon");

    let target = CloseTarget::Pane("wA:p1".into());
    let request = lifecycle::CloseRequest {
        target: target.clone(),
        identity: lifecycle::identity(&inventory, &target),
    };
    let cancel = AtomicBool::new(false);
    let outcome =
        lifecycle::close_unmanaged(selection.provider.as_ref(), &request, &cancel, || {
            Ok(Vec::new())
        });
    assert!(
        matches!(outcome, lifecycle::DirectClose::Unknown(_)),
        "{outcome:?}"
    );
    assert!(backend.told_to_close(&target));
    assert_eq!(backend.closes_seen(), 1, "the close was asked for twice");
    daemon.stop();
}

/// A backend that only says what it is, so the handshake's own words can be read
/// without a mux behind them.
struct IdentifiedBackend(BackendIdentity);

impl RuntimeProvider for IdentifiedBackend {
    fn identity(&self) -> BackendIdentity {
        self.0.clone()
    }

    fn primitives(&self) -> Primitives {
        DaemonRuntime::REQUIRED
    }

    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        Ok(empty_inventory())
    }

    fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        ForegroundEvidence::Inconclusive
    }

    fn focus(&self, _target: &Target, _cancel: &AtomicBool) -> Result<(), String> {
        Ok(())
    }

    fn close(&self, _target: &CloseTarget, _cancel: &AtomicBool) -> Result<(), String> {
        Ok(())
    }
}

/// The handshake names the backend that is actually wired, with the server
/// instance it targets, and Radar's client carries that identity through. A
/// daemon with no backend says so rather than naming a placeholder.
#[test]
fn the_handshake_names_the_wired_backend_and_its_instance() {
    let sandbox = Sandbox::new("backend-identity");

    let mut bare = sandbox.daemon();
    let ping = Client::dial(bare.path()).call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["backend"], "none", "{ping}");
    assert_eq!(ping["result"]["instance"], Value::Null, "{ping}");
    bare.stop();

    let daemon_backend = Arc::new(IdentifiedBackend(BackendIdentity::tmux("work")));
    let mut daemon = sandbox.daemon_with(daemon_backend);
    let ping = Client::dial(daemon.path()).call("daemon.ping", json!({}));
    assert_eq!(ping["result"]["backend"], "tmux", "{ping}");
    assert_eq!(ping["result"]["instance"], "work", "{ping}");

    let client = DaemonRuntime::handshake(daemon.path()).expect("the handshake passes");
    assert_eq!(client.identity(), BackendIdentity::tmux("work"));
    daemon.stop();
}

#[test]
fn socket_facts_and_owner_assignment_are_read_separately_with_identifier_only_pointers() {
    let sandbox = Sandbox::new("facts-socket");
    let mut daemon = sandbox.daemon();
    let mut client = Client::dial(daemon.path());
    let registered = client.call(
        "agent.register",
        registration_body("8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22", None, None),
    );
    let id = registration_id(&registered);
    let model = "private-model-name";
    let handle = client.call(
        "agent.acquire",
        json!({"registration_id":id,"channel":"facts","publisher":{"source":"pi","incarnation":"3c2d3e4f-5678-4abc-9def-0123456789ab"}}),
    )["result"]["writer"]["handle"]
        .as_str()
        .expect("facts writer handle")
        .to_owned();
    let published = client.call(
        "agent.publish",
        json!({"registration_id":id,"channel":"facts","writer_handle":handle,"sequence":1,
               "snapshot":{"model":model,"background_tasks":[{"id":"t1","phase":"running"}]}}),
    );
    assert!(published["result"].is_object(), "{published}");

    let got = client.call("agent.get", json!({"registration_id":id}));
    let facts = &got["result"]["agent"]["facts"];
    assert_eq!(facts["snapshot"]["freshness"], "fresh");
    assert_eq!(facts["snapshot"]["source"], "pi");
    assert_eq!(facts["snapshot"]["snapshot"]["model"], model);
    assert!(facts["writer"].get("handle").is_none(), "{facts}");
    assert!(facts["snapshot"].get("handle").is_none(), "{facts}");
    assert!(got["result"]["agent"]["execution"].is_null());

    let listed = client.call("agent.list", json!({}));
    let entry = listed["result"]["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|agent| agent["registration"]["registration_id"] == id)
        .expect("the registered agent in the list");
    assert_eq!(entry["facts"]["snapshot"]["snapshot"]["model"], model);
    daemon.stop();

    let journal: Value = serde_json::from_slice(
        &fs::read(sandbox.state().join("events.json")).expect("journal file"),
    )
    .expect("journal JSON");
    let event = journal["events"]
        .as_array()
        .expect("events")
        .last()
        .expect("an event");
    assert_eq!(event["kind"], "facts.published");
    assert_eq!(event["registration_id"], id);
    assert!(!serde_json::to_string(event).unwrap().contains(model));
}

fn stop_subject(client: &mut Client, request_id: &str, subject_id: &str) -> Value {
    client.call_id(
        request_id,
        "lifecycle.stop",
        json!({"request": {"subject_id": subject_id, "claim_id": subject_id}}),
    )
}

/// Waits until the daemon has recorded the launch's exit, through its pane-death
/// wake or a later reconciliation.
fn await_recorded_exit(state: &Path, subject_id: &str) {
    for _ in 0..500 {
        if !subject_record(state, subject_id)["exited_at"].is_null() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the launch's exit was never recorded");
}

/// Waits until no pane-died hook names the launch. The wake removes its slot
/// after it records the exit, so the record can be readable first.
fn await_hook_removed(server: &TmuxServer, subject_id: &str) -> String {
    for _ in 0..500 {
        let hooks = server.tmux(&["show-hooks", "-g", "pane-died"]);
        if !hooks.contains(subject_id) {
            return hooks;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the launch's pane-died wake was never removed");
}

/// Waits until the named tmux server has exited, so a replacement can take its
/// name.
fn await_server_exit(name: &str) {
    for _ in 0..500 {
        let alive = Command::new("tmux")
            .args(["-L", name, "has-session"])
            .env_remove("TMUX")
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the tmux server did not exit");
}

/// A stop signals the captured process once, replays by request id, and refuses
/// a conflicting reuse of that id without signalling the other launch.
#[test]
fn a_stop_signals_the_captured_process_once_and_replays_by_request_id() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-stop") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-stop");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (_, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let (_, other) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let pid = captured_identity(&state, &subject).pid;
    let other_pid = captured_identity(&state, &other).pid;

    let request_id = random_uuid();
    let stopped = stop_subject(&mut client, &request_id, &subject);
    assert_eq!(result_record(&stopped)["outcome"], "completed", "{stopped}");
    assert_eq!(
        result_record(&stopped)["effects"],
        json!(["sigterm dispatched", "process exit verified"]),
        "{stopped}"
    );
    await_gone(pid);
    let signalled = subject_record(&state, &subject)["signalled_at"].clone();
    assert!(!signalled.is_null());

    assert_eq!(stop_subject(&mut client, &request_id, &subject), stopped);
    let conflicting = stop_subject(&mut client, &request_id, &other);
    assert_eq!(error_code(&conflicting), "refused", "{conflicting}");
    assert!(process_exists(other_pid), "the other launch was signalled");
    assert_eq!(subject_record(&state, &other)["signalled_at"], Value::Null);

    let fresh = stop_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&fresh)["effects"],
        json!(["process exit verified"]),
        "{fresh}"
    );
    assert_eq!(subject_record(&state, &subject)["signalled_at"], signalled);

    let cleanup = cancel_subject(&mut client, &random_uuid(), &other);
    assert_eq!(result_record(&cleanup)["outcome"], "completed", "{cleanup}");
    await_gone(other_pid);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A process that ignores SIGTERM is reported uncertain. It is signalled once,
/// is not replaced, and a later stop does not signal it again.
#[test]
fn a_process_ignoring_sigterm_stops_uncertainly_and_is_not_signalled_again() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-ignored") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-ignored");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (_, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "trap '' TERM; exec sleep 300"]),
    );
    let pid = captured_identity(&state, &subject).pid;

    let first = stop_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(result_record(&first)["outcome"], "unknown", "{first}");
    assert_eq!(
        result_record(&first)["effects"],
        json!(["sigterm dispatched"]),
        "{first}"
    );
    // A managed stop names what is still running instead of claiming an exit.
    assert!(
        result_record(&first)["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("still running:")),
        "{first}"
    );
    assert!(process_exists(pid), "the process ignored SIGTERM");
    assert!(subject_record(&state, &subject)["exited_at"].is_null());
    assert_eq!(
        result_record(&first)["target"],
        subject,
        "the durable lane is the subject, not its pane"
    );
    let cancellation = client.call_id(
        &random_uuid(),
        "lifecycle.cancel",
        json!({"request": {"subject_id": subject, "claim_id": subject}}),
    );
    assert!(
        !result_record(&cancellation)["effects"]
            .to_string()
            .contains("sigterm dispatched"),
        "{cancellation}"
    );
    assert_eq!(
        subject_record(&state, &subject)["state"],
        "captured",
        "a conflicting cancel must not revoke the launch"
    );

    let second = stop_subject(&mut client, &random_uuid(), &subject);
    assert!(
        !result_record(&second)["effects"]
            .to_string()
            .contains("sigterm dispatched"),
        "{second}"
    );

    // SAFETY: the pid is this test's own process, the one it just launched.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    await_gone(pid);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// The raw tmux pane a tracked launch was dispatched into. A recorded location
/// qualifies the raw pane id the server lists.
fn launch_pane(client: &mut Client, spawn_id: &str) -> String {
    client.call("spawn.get", json!({"request_id": spawn_id}))["result"]["spawn"]["location"]["pane"]
        .as_str()
        .and_then(|recorded| recorded.rsplit(':').next())
        .expect("the launch's pane")
        .to_string()
}

/// A cancel of an unbound launch whose process is verified gone closes the pane
/// that launch left behind, records it as the launch's own fact, and never closes
/// anything a second time.
#[test]
fn an_unbound_launch_whose_process_exited_has_its_pane_cleaned_up_once() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-cleanup") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-cleanup");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(&mut client, &state, &parent, json!(["-c", "exit 0"]));
    let pane = launch_pane(&mut client, &spawn_id);
    assert!(
        pane_ids(&server).contains(&pane),
        "the launch's pane {pane} exists before the cancel: {:?}",
        pane_ids(&server)
    );

    let cancelled = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&cancelled)["outcome"],
        "completed",
        "{cancelled}"
    );
    assert!(
        result_record(&cancelled)["effects"]
            .to_string()
            .contains("pane cleaned up"),
        "{cancelled}"
    );
    assert!(
        !pane_ids(&server).contains(&pane),
        "the pane {pane} an abandoned launch left behind is gone: {:?}",
        pane_ids(&server)
    );
    assert_eq!(
        subject_record(&state, &subject)["pane_cleanup"]["outcome"],
        "cleaned",
        "the cleanup is a durable fact of the launch"
    );

    // A later cancel reports the recorded outcome and closes nothing again.
    let second = cancel_subject(&mut client, &random_uuid(), &subject);
    assert!(
        result_record(&second)["effects"]
            .to_string()
            .contains("pane cleaned up"),
        "{second}"
    );
    assert!(
        !pane_ids(&server).contains(&pane),
        "{second}: {:?}",
        pane_ids(&server)
    );

    drop(client);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A cancel whose signal is not verified gone keeps the pane: closing it would
/// end the process without a signal or an exit claim.
#[test]
fn an_unbound_launch_whose_process_may_still_be_live_keeps_its_pane() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-kept") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-kept");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "trap '' TERM; exec sleep 300"]),
    );
    let pid = captured_identity(&state, &subject).pid;
    let pane = launch_pane(&mut client, &spawn_id);

    let cancelled = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(
        result_record(&cancelled)["outcome"],
        "unknown",
        "{cancelled}"
    );
    assert!(
        result_record(&cancelled)["effects"]
            .to_string()
            .contains("pane kept"),
        "{cancelled}"
    );
    assert!(
        pane_ids(&server).contains(&pane),
        "the pane {pane} whose launch may still be alive is kept: {:?}",
        pane_ids(&server)
    );
    assert!(
        result_record(&cancelled)["message"]
            .as_str()
            .is_none_or(|message| message.contains("not") || message.starts_with("still running:")),
        "{cancelled}"
    );
    // A refusal dispatches nothing, so it records no cleanup: the launch's pane
    // is re-decided against whatever evidence a later cancel finds.
    assert_eq!(
        subject_record(&state, &subject)["pane_cleanup"],
        Value::Null,
        "a refusal is not a recorded cleanup effect"
    );
    assert!(process_exists(pid), "the process ignored SIGTERM");

    // SAFETY: the pid is this test's own process, the one it just launched.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    await_gone(pid);

    // The unresolved cancel keeps its target suppressed, so the process that
    // has since gone does not become a second cleanup of the same pane.
    let after = cancel_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(error_code(&after), "refused", "{after}");
    assert!(pane_ids(&server).contains(&pane), "{after}");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A pane whose process exits wakes the daemon, which records the exit without
/// a stop. The wake leaves the operator's own pane-died hook alone and removes
/// only its slot; a duplicate wake reads the same exit.
#[test]
fn a_pane_death_wakes_the_daemon_and_keeps_the_users_hook() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-wake") else {
        return;
    };
    server.tmux(&["set-hook", "-g", "pane-died", "display-message user-hook"]);
    let sandbox = Sandbox::new("tmux-lifecycle-wake");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(&mut client, &state, &parent, json!(["-c", "exit 0"]));
    await_recorded_exit(&state, &subject);

    let hooks = await_hook_removed(&server, &subject);
    assert!(hooks.contains("display-message user-hook"), "{hooks}");
    let instance = recorded_edge(&state, &spawn_id)["instance"].clone();
    let duplicate = client.call(
        "lifecycle.wake",
        json!({"subject_id": subject, "server": instance}),
    );
    assert_eq!(duplicate["result"]["exited"], json!(true), "{duplicate}");
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A wake naming a server generation that no longer serves the pane is refused,
/// and the replacement's pane is not recorded as this launch's exit.
#[test]
fn a_wake_from_a_replaced_server_generation_is_refused() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-generation") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-generation");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let instance = recorded_edge(&state, &spawn_id)["instance"].clone();
    let pid = captured_identity(&state, &subject).pid;

    server.tmux(&["kill-server"]);
    await_server_exit(&server.name);
    server.tmux(&["new-session", "-d", "-s", "main", "-x", "100", "-y", "30"]);
    let refused = client.call(
        "lifecycle.wake",
        json!({"subject_id": subject, "server": instance}),
    );
    assert_eq!(error_code(&refused), "refused", "{refused}");
    assert!(subject_record(&state, &subject)["exited_at"].is_null());

    await_gone(pid);
    let stopped = stop_subject(&mut client, &random_uuid(), &subject);
    assert_eq!(result_record(&stopped)["outcome"], "completed", "{stopped}");
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A revocation made before a crash is still in force after the daemon is killed
/// without a clean shutdown.
#[test]
fn a_revocation_survives_a_crash_without_a_clean_shutdown() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-crash") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-crash");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let (spawn_id, subject) = tracked_spawn(
        &mut client,
        &state,
        &parent,
        json!(["-c", "exec sleep 300"]),
    );
    let cancel_id = random_uuid();
    let first = cancel_subject(&mut client, &cancel_id, &subject);
    assert_eq!(result_record(&first)["outcome"], "completed", "{first}");
    let before = subject_record(&state, &subject);
    assert_eq!(daemon.stop(libc::SIGKILL, DEADLINE).code(), None);

    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    assert_eq!(cancel_subject(&mut client, &cancel_id, &subject), first);
    let late = client.call(
        "agent.register",
        child_registration_for(&state, &spawn_id, &subject),
    );
    assert_eq!(error_code(&late), "refused", "{late}");
    let after = subject_record(&state, &subject);
    assert_eq!(after["state"], "revoked");
    assert_eq!(after["signalled_at"], before["signalled_at"]);
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// A launch whose subject record would exceed its bound is refused before any
/// record or effect, rather than failing inside the daemon.
#[test]
fn a_launch_too_large_to_track_is_refused_before_any_effect() {
    let Some(server) = TmuxServer::start("tmux-lifecycle-oversize") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-lifecycle-oversize");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let entries = || {
        fs::read_dir(state.join("lifecycle"))
            .map(|dir| dir.count())
            .unwrap_or(0)
    };
    let before = entries();

    let spawn_id = random_uuid();
    let refused = client.call_id(
        &spawn_id,
        "spawn.create",
        json!({"request": {
            "parent": parent,
            "parent_session_id": TRACKED_SESSION,
            "executable": "/bin/sh",
            "argv": ["-c", "exec sleep 300"],
            "env": {"BULK": "x".repeat(70 * 1024)}
        }}),
    );
    assert_eq!(error_code(&refused), "bad_params", "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("too large to track")),
        "{refused}"
    );
    assert_eq!(entries(), before);
    assert!(!edge_path(&state, &spawn_id).exists());
    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

#[test]
fn a_confirmed_create_records_one_owned_container() {
    let sandbox = Sandbox::new("topology-create");
    let backend = MuxBackend::new(MUX_PRIMITIVES).naming(CreatedLocation {
        workspace_id: Some("wA".into()),
        tab_id: Some("wA:t2".into()),
        pane_id: Some("wA:p2".into()),
    });
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let ping = client.call("daemon.ping", json!({}));
    assert!(
        ping["result"]["contracts"]
            .as_array()
            .expect("capabilities")
            .contains(&json!("topology.get.v1")),
        "{ping}"
    );

    // An empty topology is a page with nothing in it, not an error.
    let empty = client.call("topology.list", json!({}));
    assert_eq!(empty["result"]["containers"], json!([]));
    assert_eq!(empty["result"]["next_after"], Value::Null);

    let workspace = client.call(
        "mux.create",
        json!({"request": {"kind": "workspace", "focus": false}, "requester": "test"}),
    );
    assert_eq!(result_record(&workspace)["outcome"], "completed");

    // A tab in that workspace: the tab is the container recorded, and the
    // workspace the runtime names is context, not a second container.
    let tab = client.call(
        "mux.create",
        json!({"request": {"kind": "tab", "workspace_id": "wA", "focus": false}, "requester": "test"}),
    );
    assert_eq!(result_record(&tab)["outcome"], "completed");

    let listed = client.call("topology.list", json!({}));
    let containers = listed["result"]["containers"]
        .as_array()
        .expect("containers")
        .clone();
    assert_eq!(
        containers.len(),
        2,
        "one container per creation, not one per named identity: {listed}"
    );
    let workspace_container = containers
        .iter()
        .find(|container| container["kind"] == json!("workspace"))
        .expect("the workspace container");
    let tab_container = containers
        .iter()
        .find(|container| container["kind"] == json!("tab"))
        .expect("the tab container");
    assert_eq!(workspace_container["provenance"], "created");
    assert_eq!(
        workspace_container["binding"],
        json!({"backend": "fake", "incarnation": Value::Null, "handle": "wA"})
    );
    assert_eq!(tab_container["binding"]["handle"], "wA:t2");
    assert_eq!(
        tab_container["parent"], workspace_container["container_id"],
        "a tab is recorded under the container its workspace handle names"
    );
    assert!(tab_container["observed_at"].is_null());

    // A pane split: the target pane was never recorded, so the parent stays
    // unresolved instead of being guessed.
    let pane = client.call(
        "mux.create",
        json!({"request": {"kind": "pane_split", "pane_id": "wA:p1", "direction": "right", "focus": false}, "requester": "test"}),
    );
    assert_eq!(result_record(&pane)["outcome"], "completed");
    let listed = client.call("topology.list", json!({}));
    let containers = listed["result"]["containers"]
        .as_array()
        .expect("containers")
        .clone();
    assert_eq!(containers.len(), 3);
    let pane_container = containers
        .iter()
        .find(|container| container["kind"] == json!("pane"))
        .expect("the pane container");
    assert_eq!(pane_container["binding"]["handle"], "wA:p2");
    assert_eq!(pane_container["parent"], Value::Null);

    // One container by id, and two reads that name nothing recorded.
    let id = pane_container["container_id"]
        .as_str()
        .expect("a container id")
        .to_string();
    let got = client.call("topology.get", json!({"container_id": id}));
    assert_eq!(got["result"]["container"], *pane_container);
    let missing = client.call("topology.get", json!({"container_id": random_uuid()}));
    assert_eq!(error_code(&missing), "not_found");
    let malformed = client.call("topology.get", json!({"container_id": "wA:p2"}));
    assert_eq!(error_code(&malformed), "bad_params");

    // A page of one tells the caller where to continue.
    let page = client.call("topology.list", json!({"limit": 1}));
    assert_eq!(page["result"]["containers"].as_array().unwrap().len(), 1);
    assert_eq!(page["result"]["has_more"], json!(true));
    let after = page["result"]["next_after"]
        .as_str()
        .expect("a continuation")
        .to_string();
    let next = client.call("topology.list", json!({"limit": 1, "after": after}));
    assert_eq!(next["result"]["containers"].as_array().unwrap().len(), 1);
    assert_ne!(
        next["result"]["containers"][0]["container_id"],
        page["result"]["containers"][0]["container_id"]
    );
    daemon.stop();
}

#[test]
fn reconciling_reports_drift_without_dispatching_or_deleting() {
    let sandbox = Sandbox::new("topology-reconcile");
    // The test backend names no serving instance, so its inventory cannot fence a
    // handle: a reconciliation says so rather than calling the pane present.
    let backend = MuxBackend::new(MUX_PRIMITIVES).observing(&["wA:p2"]);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let create = client.call(
        "mux.create",
        json!({"request": {"kind": "pane_split", "pane_id": "wA:p1", "direction": "right", "focus": false}, "requester": "test"}),
    );
    assert_eq!(result_record(&create)["outcome"], "completed");

    let reconciled = client.call("topology.reconcile", json!({}));
    assert_eq!(
        reconciled["result"]["reconciled"]["unavailable"],
        json!(1),
        "{reconciled}"
    );
    assert_eq!(
        backend.closed.lock().expect("the closed list").len(),
        0,
        "a reconciliation dispatches nothing"
    );

    // The record survives with the reason it could not be verified, so a caller
    // reads drift instead of a state nobody established.
    let listed = client.call("topology.list", json!({}));
    let container = &listed["result"]["containers"][0];
    assert!(
        container["unresolved"]
            .as_str()
            .is_some_and(|reason| reason.contains("could not be verified")),
        "{listed}"
    );
    assert!(container["observed_at"].is_string(), "{listed}");
    assert_eq!(container["removed_at"], Value::Null);
    assert_eq!(
        backend
            .created_requests
            .lock()
            .expect("the create list")
            .len(),
        1,
        "reconciling creates nothing"
    );

    let no_backend = {
        let sandbox = Sandbox::new("topology-reconcile-none");
        let mut daemon = sandbox.daemon();
        let mut client = Client::dial(daemon.path());
        let response = client.call("topology.reconcile", json!({}));
        daemon.stop();
        response
    };
    assert_eq!(error_code(&no_backend), "backend_unavailable");
    daemon.stop();
}

#[test]
fn a_spawn_records_the_pane_it_dispatched_into_as_a_container() {
    let sandbox = Sandbox::new("spawn-container");
    let backend = MuxBackend::new(SPAWN_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let parent = registered_parent(&mut client, PARENT_INCARNATION, "wA:p1");
    spawned_edge(&mut client, &random_uuid(), &parent);

    let listed = client.call("topology.list", json!({}));
    let containers = listed["result"]["containers"]
        .as_array()
        .expect("containers")
        .clone();
    assert_eq!(
        containers.len(),
        1,
        "the pane a spawn dispatched into is one container: {listed}"
    );
    assert_eq!(containers[0]["kind"], "pane");
    assert_eq!(containers[0]["provenance"], "created");
    assert_eq!(containers[0]["binding"]["handle"], "wA:p2");
    assert_eq!(
        containers[0]["parent"],
        Value::Null,
        "a registered parent pane has no recorded container to sit under"
    );
    daemon.stop();
}

#[test]
fn a_radar_creation_answers_with_the_container_it_recorded() {
    let sandbox = Sandbox::new("topology-create-verb");
    let backend = MuxBackend::new(MUX_PRIMITIVES).naming(CreatedLocation {
        workspace_id: Some("wA".into()),
        tab_id: Some("wA:t2".into()),
        pane_id: Some("wA:p2".into()),
    });
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let created = client.call(
        "topology.create",
        json!({"request": {"kind": "workspace", "focus": false}, "label": "work", "requester": "test"}),
    );
    assert_eq!(result_record(&created)["outcome"], "completed", "{created}");
    let container = created["result"]["container"].clone();
    assert_eq!(container["kind"], "workspace");
    assert_eq!(container["label"], "work");
    assert_eq!(container["provenance"], "created");
    assert_eq!(container["binding"]["handle"], "wA");

    // The answer names a record the daemon serves, so a caller acts on the
    // identity it made instead of joining a handle to the next tree it draws.
    let id = container["container_id"]
        .as_str()
        .expect("a container id")
        .to_string();
    let got = client.call("topology.get", json!({"container_id": id}));
    assert_eq!(got["result"]["container"], container);

    // A label is Radar's own name for a container, so it is bounded and carries
    // no control characters, and both are checked before the backend is asked.
    for label in [json!("two\nlines"), json!("x".repeat(129))] {
        let refused = client.call(
            "topology.create",
            json!({"request": {"kind": "workspace", "focus": false}, "label": label, "requester": "test"}),
        );
        assert_eq!(error_code(&refused), "bad_params", "{refused}");
        assert!(
            result_record(&refused)["outcome"].is_null(),
            "a refused parameter authors no record: {refused}"
        );
    }
    assert_eq!(
        backend.creates().len(),
        1,
        "only the admitted creation reached the backend"
    );
    daemon.stop();
}

#[test]
fn a_creation_this_backend_cannot_serve_is_refused_before_any_effect() {
    let sandbox = Sandbox::new("topology-create-unsupported");
    let backend = MuxBackend::new(
        Primitives::OBSERVE
            .union(Primitives::FOCUS)
            .union(Primitives::CLOSE),
    );
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let refused = client.call_id(
        &id,
        "topology.create",
        json!({"request": {"kind": "workspace", "focus": false}, "requester": "test"}),
    );
    let record = result_record(&refused);
    assert_eq!(record["outcome"], "refused");
    assert!(
        record["message"]
            .as_str()
            .expect("a message")
            .contains("the `mux.create` capability"),
        "the refusal names what this backend cannot do: {refused}"
    );
    assert!(backend.creates().is_empty(), "nothing was created");
    assert!(refused["result"].get("container").is_none());
    assert_eq!(
        client.call("daemon.ping", json!({}))["result"]["contracts"],
        advertised(
            Primitives::OBSERVE
                .union(Primitives::FOCUS)
                .union(Primitives::CLOSE)
        )
    );
    daemon.stop();
}

#[test]
fn a_replayed_creation_answers_with_the_same_container_and_creates_nothing() {
    let sandbox = Sandbox::new("topology-create-replay");
    let backend = MuxBackend::new(MUX_PRIMITIVES).naming(CreatedLocation {
        workspace_id: Some("wA".into()),
        tab_id: None,
        pane_id: None,
    });
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let id = random_uuid();
    let params = json!({
        "request": {"kind": "workspace", "focus": false},
        "label": "work",
        "requester": "test"
    });
    let first = client.call_id(&id, "topology.create", params.clone());
    let container = first["result"]["container"].clone();
    assert!(
        container["container_id"].is_string(),
        "the first answer names the container: {first}"
    );

    // The same request is the same creation: a caller recovering from a lost
    // answer reads the container it already made and reaches no backend.
    let again = client.call_id(&id, "topology.create", params);
    assert_eq!(again["result"]["container"], container, "{again}");
    assert_eq!(
        backend.creates().len(),
        1,
        "a replayed creation dispatches nothing"
    );

    // The same id naming different contents is refused rather than dispatched.
    let changed = client.call_id(
        &id,
        "topology.create",
        json!({
            "request": {"kind": "workspace", "focus": false},
            "label": "other",
            "requester": "test"
        }),
    );
    assert_eq!(error_code(&changed), "refused", "{changed}");
    assert_eq!(backend.creates().len(), 1);
    daemon.stop();
}

#[test]
fn the_snapshot_carries_the_containers_the_daemon_records() {
    let sandbox = Sandbox::new("client-containers");
    // The capabilities the dashboard client's handshake requires, plus launch:
    // its registry read requires the spawn contract, which a backend can only
    // serve when it can both create a pane and run a command in it.
    let backend = MuxBackend::new(
        CLIENT_PRIMITIVES
            .union(Primitives::CREATE)
            .union(Primitives::LAUNCH),
    )
    .naming(CreatedLocation {
        workspace_id: Some("wA".into()),
        tab_id: None,
        pane_id: None,
    });
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut raw = Client::dial(daemon.path());
    let created = raw.call(
        "topology.create",
        json!({"request": {"kind": "workspace", "focus": false}, "label": "work", "requester": "test"}),
    );
    assert!(
        created["result"]["container"]["container_id"].is_string(),
        "{created}"
    );

    // The daemon's own observation carries the containers it recorded: a display
    // reads them from the snapshot and from nowhere else, so a snapshot without
    // them leaves every row unactionable and every created place unnamed.
    let snapshot = raw.first_snapshot();
    let containers = snapshot["observation"]["inventory"]["containers"]
        .as_array()
        .unwrap_or_else(|| panic!("containers in the snapshot: {snapshot}"));
    assert_eq!(containers.len(), 1, "{snapshot}");
    assert_eq!(containers[0]["label"], "work", "{snapshot}");
    assert_eq!(containers[0]["binding"]["handle"], "wA", "{snapshot}");
    daemon.stop();
}

/// An inventory that reports one workspace and nothing inside it.
fn workspace_inventory(workspace_id: &str) -> FleetObservation {
    FleetObservation {
        metadata_agents: Vec::new(),
        containers: Vec::new(),
        lifecycle_subjects: Vec::new(),
        workspaces: vec![Workspace {
            workspace_id: workspace_id.into(),
            label: None,
            number: Some(1),
            session_path: None,
        }],
        tabs: Vec::new(),
        panes: Vec::new(),
        agents: Vec::new(),
    }
}

/// A freeze of one empty workspace: the target and identity a client sends when it
/// deletes a workspace it saw holding nothing.
fn frozen_workspace(workspace_id: &str) -> Value {
    json!({
        "target": {"workspace": workspace_id},
        "identity": {"workspace": {"workspace_id": workspace_id, "members": []}},
    })
}

/// A daemon whose fake backend can create and fence a workspace container.
///
/// The backend is served by this test process, so the container it creates records
/// a verified serving instance — the fence a deletion needs — and reports the
/// workspace, so a frozen target has a place to be checked against.
fn deletion_setup(name: &str, handle: &str) -> (Sandbox, Arc<MuxBackend>, Daemon, Client) {
    let sandbox = Sandbox::new(name);
    // A backend that can both create and close locations is what a container
    // deletion asks of it, so the test states both capabilities.
    let backend = MuxBackend::new(SPAWN_PRIMITIVES)
        .naming(CreatedLocation {
            workspace_id: Some(handle.into()),
            tab_id: None,
            pane_id: None,
        })
        .with_inventory(workspace_inventory(handle));
    backend.served_by(std::process::id() as i32);
    let daemon = sandbox.daemon_with(backend.clone());
    let client = Client::dial(daemon.path());
    (sandbox, backend, daemon, client)
}

/// A created workspace container's id and the delete request naming its place.
fn created_workspace(client: &mut Client, handle: &str) -> (String, Value) {
    let created = client.call(
        "topology.create",
        json!({"request": {"kind": "workspace", "focus": false}, "label": "work", "requester": "test"}),
    );
    let container = &created["result"]["container"];
    assert_eq!(container["binding"]["handle"], json!(handle), "{created}");
    let id = container["container_id"]
        .as_str()
        .expect("a container id")
        .to_string();
    let request = json!({
        "request": {"container_id": id, "frozen": frozen_workspace(handle)},
        "requester": "test",
    });
    (id, request)
}

#[test]
fn a_confirmed_deletion_closes_a_container_and_records_each_removal_once() {
    let (_sandbox, backend, mut daemon, mut client) = deletion_setup("topology-delete", "wA");
    let (id, request) = created_workspace(&mut client, "wA");

    let deletion_id = random_uuid();
    let deletion = client.call_id(&deletion_id, "topology.delete", request.clone());
    assert_eq!(
        result_record(&deletion)["outcome"],
        "completed",
        "{deletion}"
    );
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Workspace("wA".into())],
        "the container's own place is what is closed"
    );
    let removed = deletion["result"]["removed"]
        .as_array()
        .expect("the removed containers")
        .clone();
    assert_eq!(removed.len(), 1, "{deletion}");
    assert_eq!(removed[0]["container_id"], json!(id));
    assert!(removed[0]["removed_at"].is_string(), "{deletion}");
    // The outcome says what the runtime confirmed and what the daemon recorded,
    // and claims nothing about a process.
    let message = result_record(&deletion)["message"]
        .as_str()
        .expect("a note")
        .to_string();
    assert!(message.contains("closed by the runtime"), "{message}");
    assert!(message.contains("marked removed"), "{message}");
    assert!(!message.contains("exit"), "{message}");

    // The same request id answers the same record and closes nothing again.
    let replay = client.call_id(&deletion_id, "topology.delete", request);
    assert_eq!(result_record(&replay)["outcome"], "completed", "{replay}");
    assert_eq!(
        backend.closed_targets().len(),
        1,
        "a replayed deletion dispatches nothing"
    );

    // A fresh request id for the same container is refused: it is history now.
    let again = client.call(
        "topology.delete",
        json!({"request": {"container_id": id, "frozen": frozen_workspace("wA")}, "requester": "test"}),
    );
    assert_eq!(result_record(&again)["outcome"], "refused", "{again}");
    assert!(
        result_record(&again)["message"]
            .as_str()
            .is_some_and(|message| message.contains("was removed at")),
        "{again}"
    );
    assert_eq!(backend.closed_targets().len(), 1);
    daemon.stop();
}

#[test]
fn a_deletion_ends_the_place_whatever_is_registered_in_it() {
    let (_sandbox, backend, mut daemon, mut client) =
        deletion_setup("topology-delete-managed", "wA");
    let (id, request) = created_workspace(&mut client, "wA");

    // A worker registered into the workspace the deletion takes with it. The
    // operator confirmed the deletion, so the place is closed and the worker's
    // own record settles from the pane's loss rather than blocking the deletion.
    let mut body = registration_body("3f1c7a2e-9d4b-4c58-9c7b-2d0e1a9f4b22", None, None);
    body["location"] = json!({"backend": "fake", "workspace": "wA"});
    let registered = client.call("agent.register", body);
    let worker = registration_id(&registered).to_string();
    assert!(!worker.is_empty(), "{registered}");

    let deletion = client.call("topology.delete", request);
    assert_eq!(
        result_record(&deletion)["outcome"],
        "completed",
        "{deletion}"
    );
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Workspace("wA".to_string())],
        "the place is closed once"
    );
    let listed = client.call("topology.list", json!({}));
    let deleted = listed["result"]["containers"]
        .as_array()
        .expect("the containers")
        .iter()
        .find(|container| container["container_id"] == json!(id))
        .expect("the deleted container")
        .clone();
    assert!(
        deleted["removed_at"].is_string(),
        "the container is history now: {listed}"
    );
    daemon.stop();
}

#[test]
fn a_deletion_under_an_ended_serving_instance_forgets_the_records() {
    let (_sandbox, backend, mut daemon, mut client) =
        deletion_setup("topology-delete-replaced", "wA");
    let (id, request) = created_workspace(&mut client, "wA");

    // The handle is now served by another process: pid 1 is not this test's, so
    // the server the container was recorded under is provably gone and its places
    // went with it.
    backend.served_by(1);
    let deletion = client.call("topology.delete", request);
    assert_eq!(
        result_record(&deletion)["outcome"],
        "completed",
        "{deletion}"
    );
    let message = result_record(&deletion)["message"]
        .as_str()
        .expect("a note")
        .to_string();
    assert!(message.contains("no longer serves"), "{message}");
    assert!(message.contains("forgotten"), "{message}");
    assert!(
        !message.contains("closed by the runtime") && !message.contains("exit"),
        "nothing was closed and no exit is claimed: {message}"
    );
    let removed = deletion["result"]["removed"]
        .as_array()
        .expect("the removed containers")
        .clone();
    assert_eq!(removed.len(), 1, "{deletion}");
    assert_eq!(removed[0]["container_id"], json!(id), "{deletion}");
    assert!(
        backend.closed_targets().is_empty(),
        "a container recorded under an ended instance has nothing left to close"
    );
    daemon.stop();
}

#[test]
fn a_deletion_with_no_place_to_freeze_forgets_only_an_unserved_container() {
    let (_sandbox, backend, mut daemon, mut client) =
        deletion_setup("topology-delete-forget", "wA");
    let (id, request) = created_workspace(&mut client, "wA");

    // The container's place is still served, so a deletion that froze nothing
    // would orphan it: refused, and nothing is forgotten.
    let unfrozen = json!({"request": {"container_id": id}, "requester": "test"});
    let served = client.call("topology.delete", unfrozen.clone());
    assert_eq!(result_record(&served)["outcome"], "refused", "{served}");
    assert!(
        result_record(&served)["message"]
            .as_str()
            .is_some_and(|message| message.contains("still served")),
        "{served}"
    );

    // Its recorded serving instance is gone: there is no place left to freeze, and
    // the records are all that remain.
    backend.served_by(1);
    let forgotten = client.call("topology.delete", unfrozen);
    assert_eq!(
        result_record(&forgotten)["outcome"],
        "completed",
        "{forgotten}"
    );
    assert!(
        backend.closed_targets().is_empty(),
        "a forgotten container has no place to close"
    );
    let listed = client.call("topology.list", json!({}));
    let deleted = listed["result"]["containers"]
        .as_array()
        .expect("the containers")
        .iter()
        .find(|container| container["container_id"] == json!(id))
        .expect("the forgotten container")
        .clone();
    assert!(deleted["removed_at"].is_string(), "{listed}");
    let _ = request;
    daemon.stop();
}

#[test]
fn a_deletion_of_another_place_than_the_container_binds_is_refused() {
    let (_sandbox, backend, mut daemon, mut client) =
        deletion_setup("topology-delete-mismatch", "wA");
    let (id, _request) = created_workspace(&mut client, "wA");

    // The container is wA and the frozen target names a tab: the two describe
    // different places, so nothing is dispatched.
    let deletion = client.call(
        "topology.delete",
        json!({
            "request": {
                "container_id": id,
                "frozen": {
                    "target": {"tab": "wA:t1"},
                    "identity": {"tab": {"tab_id": "wA:t1", "members": []}},
                },
            },
            "requester": "test",
        }),
    );
    assert_eq!(result_record(&deletion)["outcome"], "refused", "{deletion}");
    assert!(
        result_record(&deletion)["message"]
            .as_str()
            .is_some_and(|message| message.contains("is not container")),
        "{deletion}"
    );
    assert!(backend.closed_targets().is_empty());
    daemon.stop();
}

/// A backend that can create a pane, run a command in it, and report what that
/// pane's command did: what a container recipe needs on both ends.
fn recipe_backend() -> Arc<MuxBackend> {
    // Served by this test process, so the container it creates records a verified
    // serving instance: the fence a reload checks before it replaces anything.
    let backend = MuxBackend::new(RECIPE_PRIMITIVES).naming(CreatedLocation {
        workspace_id: Some("wA".into()),
        tab_id: Some("wA:t1".into()),
        pane_id: Some("wA:p2".into()),
    });
    backend.served_by(std::process::id() as i32);
    backend
}

/// The create request one test uses: a pane split carrying a private command.
fn recipe_creation() -> Value {
    json!({
        "request": {
            "kind": "pane_split",
            "pane_id": "wA:p1",
            "direction": "right",
            "focus": false,
        },
        "label": "runner",
        "recipe": {
            "executable": "/bin/echo",
            "argv": ["sentinel-argv"],
            "cwd": "/tmp",
            "env": {"SENTINEL_ENV": "sentinel-value"},
        },
        "requester": "test",
    })
}

#[test]
fn a_topology_recipe_cannot_set_radar_identity_environment_variables() {
    let sandbox = Sandbox::new("topology-recipe-marker-env");
    let backend = recipe_backend();
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());
    let mut params = recipe_creation();
    params["recipe"]["env"]["PI_RADAR_MANAGED_SUBJECT_ID"] =
        json!("11111111-1111-4111-8111-111111111111");

    let refused = client.call("topology.create", params);
    assert_eq!(error_code(&refused), "bad_params", "{refused}");
    assert!(backend.creates().is_empty());
    assert!(backend.launches().is_empty());
    daemon.stop();
}

#[test]
fn a_recipe_creation_holds_the_command_privately_and_runs_it_in_its_pane() {
    let sandbox = Sandbox::new("topology-recipe");
    let backend = recipe_backend();
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let created = client.call("topology.create", recipe_creation());
    assert_eq!(result_record(&created)["outcome"], "completed", "{created}");
    let container = created["result"]["container"].clone();
    assert_eq!(container["recipe"], json!(true), "{created}");
    assert_eq!(container["label"], json!("runner"), "{created}");
    let id = container["container_id"]
        .as_str()
        .expect("a container id")
        .to_string();

    // The command runs in the pane the create named, launched by the daemon, and
    // carries no spawn token: it is not a spawn child.
    let launches = backend.launches();
    assert_eq!(launches.len(), 1, "{created}");
    assert_eq!(launches[0].pane_id, "wA:p2");
    assert_eq!(launches[0].executable, "/bin/echo");
    assert_eq!(launches[0].argv, vec!["sentinel-argv".to_string()]);
    assert_eq!(launches[0].spawn_token, None);

    // The recipe is held in the daemon's owner-only store, and no read carries it.
    use std::os::unix::fs::PermissionsExt;
    let held = sandbox
        .state()
        .join("lifecycle/private")
        .join(format!("container-{id}.json"));
    let bytes = fs::read_to_string(&held).expect("the private recipe");
    assert!(bytes.contains("sentinel-value"), "{bytes}");
    assert_eq!(
        fs::metadata(&held)
            .expect("recipe metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600,
        "a recipe is readable by its owner alone"
    );
    let read = client.call("topology.get", json!({"container_id": id}));
    assert_eq!(read["result"]["container"]["recipe"], json!(true), "{read}");
    assert!(!read.to_string().contains("sentinel-value"), "{read}");
    assert!(!read.to_string().contains("sentinel-argv"), "{read}");

    // A create that makes no pane of its own cannot carry a recipe, and one is
    // never held.
    let refused = client.call(
        "topology.create",
        json!({
            "request": {"kind": "workspace", "focus": false},
            "recipe": {"executable": "/bin/echo", "argv": [], "env": {}},
            "requester": "test",
        }),
    );
    assert_eq!(error_code(&refused), "bad_params", "{refused}");
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("a pane split or a tab"),
        "the refusal names what carries a recipe: {refused}"
    );
    assert_eq!(backend.creates().len(), 1, "nothing was created for it");
    daemon.stop();
}

/// A tab is made with a pane of its own, so a launch into a new window runs there:
/// the create carries the recipe, the daemon holds it, and the command runs in the
/// pane tmux reported for the tab. This is what `L` sends.
#[test]
fn a_tab_creation_runs_its_recipe_in_the_pane_the_tab_opened() {
    let sandbox = Sandbox::new("topology-recipe-tab");
    let backend = recipe_backend();
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let mut params = recipe_creation();
    params["request"] = json!({
        "kind": "tab",
        "workspace_id": "wA",
        "focus": false,
    });
    let created = client.call("topology.create", params);
    assert_eq!(result_record(&created)["outcome"], "completed", "{created}");
    assert_eq!(created["result"]["container"]["recipe"], json!(true));
    let launches = backend.launches();
    assert_eq!(launches.len(), 1, "{created}");
    assert_eq!(launches[0].pane_id, "wA:p2", "{created}");
    assert_eq!(launches[0].executable, "/bin/echo");
    assert_eq!(launches[0].argv, vec!["sentinel-argv".to_string()]);
    daemon.stop();
}

/// A server's own places are recorded as observed containers by the daemon's
/// own pass, with their parents, and a place that is not Radar's is never
/// reloaded from an invented recipe.
#[test]
fn a_servers_own_places_are_adopted_as_observed_containers() {
    let sandbox = Sandbox::new("topology-adopt");
    let inventory = FleetObservation {
        workspaces: vec![Workspace {
            workspace_id: "wA".into(),
            label: Some("by-hand".into()),
            number: None,
            session_path: None,
        }],
        tabs: vec![Tab {
            tab_id: "wA:t1".into(),
            workspace_id: "wA".into(),
            label: None,
            number: None,
        }],
        panes: vec![Pane {
            location: Location {
                workspace_id: "wA".into(),
                tab_id: "wA:t1".into(),
                pane_id: "wA:p1".into(),
            },
            label: None,
            title: None,
            current_path: None,
        }],
        ..FleetObservation::default()
    };
    let backend = MuxBackend::new(RECIPE_PRIMITIVES).with_inventory(inventory);
    // The daemon's own pid is a live process its verifier can confirm.
    backend.served_by(std::process::id() as i32);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let mut listed = Value::Null;
    for _ in 0..100 {
        listed = client.call("topology.list", json!({}));
        if listed["result"]["containers"]
            .as_array()
            .is_some_and(|containers| containers.len() == 3)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let containers = listed["result"]["containers"]
        .as_array()
        .unwrap_or_else(|| panic!("{listed}"));
    assert_eq!(containers.len(), 3, "{listed}");
    let by_kind = |kind: &str| {
        containers
            .iter()
            .find(|container| container["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} container in {listed}"))
    };
    for container in containers {
        assert_eq!(container["provenance"], "observed", "{container}");
        assert_eq!(container["recipe"], json!(false), "{container}");
        assert!(
            container["binding"]["incarnation"].is_object(),
            "{container}"
        );
    }
    assert_eq!(by_kind("workspace")["parent"], Value::Null);
    assert_eq!(
        by_kind("tab")["parent"],
        by_kind("workspace")["container_id"]
    );
    assert_eq!(by_kind("pane")["parent"], by_kind("tab")["container_id"]);
    assert!(
        backend.created_requests.lock().expect("creates").is_empty(),
        "adoption creates nothing"
    );
    assert!(
        backend.closed.lock().expect("closes").is_empty(),
        "adoption closes nothing"
    );

    // Another pass over the same server records nothing more.
    std::thread::sleep(Duration::from_millis(2_500));
    let again = client.call("topology.list", json!({}));
    assert_eq!(
        again["result"]["containers"],
        listed["result"]["containers"]
    );

    // An observed pane has no recipe, so a reload refuses by saying so.
    let reload = client.call(
        "topology.reload",
        json!({"request": {"container_id": by_kind("pane")["container_id"]}, "requester": "test"}),
    );
    assert_eq!(result_record(&reload)["outcome"], "refused", "{reload}");
    assert!(
        result_record(&reload)["message"]
            .as_str()
            .is_some_and(|message| message.contains("not one Radar created")),
        "{reload}"
    );
    daemon.stop();
}

#[test]
fn a_reload_runs_the_recorded_recipe_only_over_a_command_that_exited() {
    let sandbox = Sandbox::new("topology-reload");
    let backend = recipe_backend();
    let mut daemon = sandbox.daemon_with(backend.clone());
    let mut client = Client::dial(daemon.path());

    let created = client.call("topology.create", recipe_creation());
    let id = created["result"]["container"]["container_id"]
        .as_str()
        .expect("a container id")
        .to_string();
    assert_eq!(backend.launches().len(), 1, "{created}");
    let reload = json!({"request": {"container_id": id}, "requester": "test"});

    // Nothing establishes that the pane's command ended, so nothing is replaced.
    let unknown = client.call("topology.reload", reload.clone());
    assert_eq!(result_record(&unknown)["outcome"], "refused", "{unknown}");
    assert!(
        result_record(&unknown)["message"]
            .as_str()
            .is_some_and(|message| message.contains("does not report an exited command")),
        "{unknown}"
    );
    assert_eq!(
        backend.launches().len(),
        1,
        "no second launch was dispatched"
    );

    backend.reporting_exit("wA:p2", PaneExitObservation::Running);
    let running = client.call("topology.reload", reload.clone());
    assert_eq!(result_record(&running)["outcome"], "refused", "{running}");
    assert!(
        result_record(&running)["message"]
            .as_str()
            .is_some_and(|message| message.contains("still running its command")),
        "{running}"
    );
    assert_eq!(backend.launches().len(), 1);

    // A typed exit is the evidence a reload needs, and the recipe runs again in the
    // same pane.
    backend.reporting_exit(
        "wA:p2",
        PaneExitObservation::Exited {
            code: Some(0),
            signal: None,
        },
    );
    let reloaded = client.call("topology.reload", reload.clone());
    assert_eq!(
        result_record(&reloaded)["outcome"],
        "completed",
        "{reloaded}"
    );
    let launches = backend.launches();
    assert_eq!(launches.len(), 2, "{reloaded}");
    assert_eq!(launches[1].pane_id, "wA:p2");
    assert_eq!(launches[1].executable, "/bin/echo");
    assert_eq!(launches[1].spawn_token, None);
    // The container is the same one, and its recipe is still held: a reload is not
    // a new container.
    assert_eq!(reloaded["result"]["container"]["container_id"], json!(id));
    assert_eq!(
        reloaded["result"]["container"]["recipe"],
        json!(true),
        "{reloaded}"
    );

    // A container with no recipe refuses by name, and nothing is launched.
    let plain = client.call(
        "topology.create",
        json!({
            "request": {"kind": "pane_split", "pane_id": "wA:p1", "direction": "right", "focus": false},
            "label": "plain",
            "requester": "test",
        }),
    );
    let plain_id = plain["result"]["container"]["container_id"]
        .as_str()
        .expect("a container id")
        .to_string();
    let no_recipe = client.call(
        "topology.reload",
        json!({"request": {"container_id": plain_id}, "requester": "test"}),
    );
    assert_eq!(
        result_record(&no_recipe)["outcome"],
        "refused",
        "{no_recipe}"
    );
    assert!(
        result_record(&no_recipe)["message"]
            .as_str()
            .is_some_and(|message| message.contains("records no recipe")),
        "{no_recipe}"
    );
    assert_eq!(backend.launches().len(), 2);
    daemon.stop();
}

/// What the dashboard client needs from any daemon it dials: the reads and the
/// controls the TUI itself asks for.
const DASHBOARD_PRIMITIVES: Primitives = DaemonRuntime::REQUIRED;

/// What a backend must state to serve the container controls end to end: create a
/// place, run a command in it, report what that command did, and close it.
const CONTAINER_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::FOCUS)
    .union(Primitives::CREATE)
    .union(Primitives::CLOSE)
    .union(Primitives::LAUNCH)
    .union(Primitives::LAUNCH_CONTEXT)
    .union(Primitives::EXIT_OBSERVATION);

#[test]
fn the_dashboard_client_dispatches_container_controls_under_the_callers_id() {
    use agent_radar::control_plane::client::{
        ContainerControls, ContainerCreate, ContainerCreateKind, ContainerDelete, ContainerReload,
    };

    let sandbox = Sandbox::new("client-containers");
    let backend = MuxBackend::new(CONTAINER_PRIMITIVES)
        .naming(CreatedLocation {
            workspace_id: Some("wA".into()),
            tab_id: Some("wA:t1".into()),
            pane_id: Some("wA:p2".into()),
        })
        .observing(&["wA:p2"]);
    backend.served_by(std::process::id() as i32);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let controls = DaemonRuntime::handshake(daemon.path()).expect("the daemon is usable");

    // One creation, under the caller's own id.
    let create_id = random_uuid();
    let created = controls
        .create_container(
            &ContainerCreate {
                request_id: create_id.clone(),
                kind: ContainerCreateKind::PaneSplit {
                    pane_id: "wA:p1".into(),
                },
                label: Some("work".into()),
                recipe: None,
            },
            &AtomicBool::new(false),
        )
        .expect("the daemon serves a creation");
    assert_eq!(created.outcome(), "completed", "{created:?}");
    let container = created.container.clone().expect("the container it made");
    assert_eq!(container.label.as_deref(), Some("work"));
    assert_eq!(backend.creates().len(), 1);

    // The id is how a lost answer is read back: the same request answers the same
    // container and dispatches no second creation.
    let again = controls
        .create_container(
            &ContainerCreate {
                request_id: create_id.clone(),
                kind: ContainerCreateKind::PaneSplit {
                    pane_id: "wA:p1".into(),
                },
                label: Some("work".into()),
                recipe: None,
            },
            &AtomicBool::new(false),
        )
        .expect("the daemon still answers");
    assert_eq!(
        again.container.as_ref().map(|c| c.container_id.clone()),
        Some(container.container_id.clone())
    );
    assert_eq!(backend.creates().len(), 1, "a replay creates nothing");

    // A deletion carries the place and occupant the operator confirmed, and the
    // container records the removal.
    let delete_id = random_uuid();
    let deleted = controls
        .delete_container(
            &ContainerDelete {
                force: false,
                request_id: delete_id.clone(),
                container_id: container.container_id.clone(),
                frozen: json!({
                    "target": {"pane": "wA:p2"},
                    "identity": {"pane": {"pane_id": "wA:p2", "occupant": null}},
                }),
            },
            &AtomicBool::new(false),
        )
        .expect("the daemon serves a deletion");
    assert_eq!(deleted.outcome(), "completed", "{deleted:?}");
    assert_eq!(
        backend.closed_targets(),
        vec![CloseTarget::Pane("wA:p2".into())]
    );
    assert_eq!(deleted.removed.len(), 1, "{deleted:?}");
    assert!(
        deleted.removed[0].removed_at.is_some(),
        "the deletion records the removal: {deleted:?}"
    );
    assert_eq!(
        deleted.removed[0].container_id,
        container.container_id.clone(),
        "{deleted:?}"
    );

    // The replay answers the same record and closes nothing again.
    let replayed = controls
        .delete_container(
            &ContainerDelete {
                force: false,
                request_id: delete_id,
                container_id: container.container_id.clone(),
                frozen: json!({
                    "target": {"pane": "wA:p2"},
                    "identity": {"pane": {"pane_id": "wA:p2", "occupant": null}},
                }),
            },
            &AtomicBool::new(false),
        )
        .expect("the daemon still answers");
    assert_eq!(replayed.outcome(), "completed", "{replayed:?}");
    assert_eq!(backend.closed_targets().len(), 1, "a replay closes nothing");

    // A reload of the container that was just deleted is refused by name, and
    // nothing is launched.
    let reloaded = controls
        .reload_container(
            &ContainerReload {
                request_id: random_uuid(),
                container_id: container.container_id,
            },
            &AtomicBool::new(false),
        )
        .expect("the daemon answers a reload");
    assert_eq!(reloaded.outcome(), "refused", "{reloaded:?}");
    assert!(
        reloaded
            .message()
            .is_some_and(|message| message.contains("was removed at")),
        "{reloaded:?}"
    );
    assert!(backend.launches().is_empty(), "nothing was launched");
    daemon.stop();
}

#[test]
fn the_client_refuses_a_container_control_the_daemon_does_not_serve() {
    use agent_radar::control_plane::client::{
        ContainerControls, ContainerCreate, ContainerCreateKind,
    };

    let sandbox = Sandbox::new("client-containers-unsupported");
    // A backend that serves the dashboard's own reads and controls but cannot
    // create a place: this daemon serves no container creation.
    let backend = MuxBackend::new(DASHBOARD_PRIMITIVES);
    let mut daemon = sandbox.daemon_with(backend.clone());
    let controls = DaemonRuntime::handshake(daemon.path()).expect("the daemon is usable");
    let refused = controls.create_container(
        &ContainerCreate {
            request_id: random_uuid(),
            kind: ContainerCreateKind::PaneSplit {
                pane_id: "wA:p1".into(),
            },
            label: None,
            recipe: None,
        },
        &AtomicBool::new(false),
    );
    assert!(
        refused
            .as_ref()
            .is_err_and(|message| message.contains("topology.create.v1")),
        "{refused:?}"
    );
    assert!(
        backend.creates().is_empty(),
        "a contract the daemon does not serve is refused before it is sent"
    );
    daemon.stop();
}

/// What a fake backend serves for lead enrollment and restart: the spawn surface plus
/// the tracking capability those operations are admitted under.
const LEAD_PRIMITIVES: Primitives = Primitives::OBSERVE
    .union(Primitives::FOCUS)
    .union(Primitives::PROCESS_INFO)
    .union(Primitives::CREATE)
    .union(Primitives::INPUT)
    .union(Primitives::OUTPUT)
    .union(Primitives::REPORT)
    .union(Primitives::LAUNCH)
    .union(Primitives::CLOSE)
    .union(Primitives::LAUNCH_CONTEXT)
    .union(Primitives::LIFECYCLE_TRACKING);

/// A real process standing in for a running lead. It is a shell that waits on its
/// stdin and takes only flags, so its captured command line can be resumed. A reaper
/// owns it, so a stopped lead is a gone process rather than a zombie.
struct LeadProcess {
    pid: i32,
    identity: ProcessIdentity,
}

impl LeadProcess {
    fn start() -> Self {
        // A clean environment: the captured launch carries it into a restart, and the
        // test runner's own variables are not part of a lead's launch.
        let child = Command::new("/bin/sh")
            .args(["-s", "--"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a lead process");
        let pid = child.id() as i32;
        let identity = process_identity_from_proc(pid);
        // `Child::wait` would close the piped stdin first and end the lead at once,
        // so the reaper waits on the pid while it keeps the stdin open.
        thread::spawn(move || {
            let _stdin_kept_open = child;
            let mut status = 0;
            // SAFETY: reaping the pid this test spawned.
            unsafe {
                libc::waitpid(pid, &mut status, 0);
            }
        });
        Self { pid, identity }
    }

    fn kill(&self) {
        // SAFETY: signalling a pid this test started; a gone pid is refused by the kernel.
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
        }
    }
}

/// Registers a lead whose claimed process is `lead`, in `pane`, publishing `session`.
fn registered_lead(
    client: &mut Client,
    lead: &LeadProcess,
    pane: &str,
    session: &str,
) -> (String, String) {
    let mut body = registration_body(PARENT_INCARNATION, Some(lead.identity.clone()), None);
    body["location"] = json!({"backend":"tmux", "pane":pane});
    let agent = registration_id(&client.call("agent.register", body));
    let context = client.call(
        "agent.context",
        json!({
            "registration_id": agent,
            "publisher": {"source":"test-publisher", "incarnation":PARENT_INCARNATION},
            "sequence": 1,
            "lease_ms": 10_000,
            "context": {"session": session}
        }),
    );
    assert!(context["result"].is_object(), "{context}");
    let writer = context["result"]["writer"]["handle"]
        .as_str()
        .expect("context writer handle")
        .to_owned();
    (agent, writer)
}

fn presence_lead(
    client: &mut Client,
    lead: &LeadProcess,
    pane: &str,
    session: &str,
) -> (String, String) {
    let answer = client.call(
        "agent.presence",
        json!({
            "source":"herdsman-pi", "incarnation":PARENT_INCARNATION,
            "process":lead.identity.clone(), "session_id":session,
            "claim_id":random_uuid(),
            "location":{"backend":"tmux", "pane":pane}
        }),
    );
    assert_eq!(answer["result"]["standing"], "granted", "{answer}");
    (
        answer["result"]["registration_id"]
            .as_str()
            .expect("registration ID")
            .to_owned(),
        answer["result"]["subject_id"]
            .as_str()
            .expect("subject ID")
            .to_owned(),
    )
}

#[test]
fn presence_claims_survive_reconnect_and_old_releases_cannot_revoke_a_new_grant() {
    let sandbox = Sandbox::new("presence-claims");
    let lead = LeadProcess::start();
    let duplicate = LeadProcess::start();
    let backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1", "wA:p2"])
        .with_foreground(ForegroundEvidence::command(
            lead.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon = Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(backend))
        .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let claim = random_uuid();
    let report = json!({"source":"herdsman-pi", "incarnation":PARENT_INCARNATION,
        "process":lead.identity,"session_id":TRACKED_SESSION,"claim_id":claim,
        "location":{"backend":"tmux","pane":"wA:p1"}});
    let first = client.call("agent.presence", report.clone());
    assert_eq!(first["result"]["standing"], "granted", "{first}");
    let registration = first["result"]["registration_id"].clone();
    let subject = first["result"]["subject_id"].clone();
    let mut reassert = report.clone();
    reassert["registration_id"] = registration.clone();
    reassert["subject_id"] = subject.clone();
    let replay = client.call("agent.presence", reassert.clone());
    assert_eq!(replay["result"]["subject_id"], subject, "{replay}");
    reassert["location"]["pane"] = json!("wA:p2");
    let moved = client.call("agent.presence", reassert.clone());
    assert_eq!(moved["result"]["subject_id"], subject, "{moved}");
    let record = client.call("agent.get", json!({"registration_id": registration}));
    assert_eq!(
        record["result"]["agent"]["registration"]["location"]["pane"],
        "wA:p2"
    );
    let mut forged = reassert.clone();
    forged["subject_id"] = json!(random_uuid());
    assert!(client.call("agent.presence", forged)["error"].is_object());
    let duplicate_claim = random_uuid();
    let rejected_report = json!({"source":"herdsman-pi","incarnation":CHILD_INCARNATION,
        "process":duplicate.identity,"session_id":TRACKED_SESSION,"claim_id":duplicate_claim,
        "location":{"backend":"tmux","pane":"wA:p1"}});
    let rejected = client.call("agent.presence", rejected_report.clone());
    assert_eq!(rejected["result"]["standing"], "duplicate", "{rejected}");
    for method in ["lifecycle.cancel", "lifecycle.stop", "lifecycle.restart"] {
        let missing = client.call(method, json!({"request": {"subject_id": subject}}));
        assert_eq!(error_code(&missing), "bad_params", "{method}: {missing}");
    }
    let release = json!({"registration_id":registration,"claim_id":claim,"process":lead.identity});
    let released = client.call("agent.release", release.clone());
    assert_eq!(released["result"]["standing"], "released", "{released}");
    let stopped = client.call_id(
        &random_uuid(),
        "lifecycle.stop",
        json!({"request": {"subject_id": subject, "claim_id": subject}}),
    );
    assert_eq!(error_code(&stopped), "refused", "{stopped}");
    assert!(
        process_exists(lead.pid),
        "a released claim authorized a signal"
    );
    let inbox = client.call(
        "communication.inbox",
        json!({"registration_id": registration}),
    );
    assert!(
        inbox["error"].is_object(),
        "released claim retained inbox access: {inbox}"
    );
    assert_eq!(
        client.call("agent.presence", report.clone())["result"]["standing"],
        "released"
    );
    assert_eq!(
        client.call("agent.presence", rejected_report.clone())["result"]["standing"],
        "duplicate"
    );
    let mut fresh = report.clone();
    fresh["claim_id"] = json!(random_uuid());
    let granted = client.call("agent.presence", fresh.clone());
    assert_eq!(granted["result"]["standing"], "granted", "{granted}");
    assert_eq!(granted["result"]["subject_id"], subject);
    let active = client.call("lifecycle.get", json!({"subject_id": subject}));
    assert_eq!(
        active["result"]["subject"]["claim_id"], fresh["claim_id"],
        "{active}"
    );
    client.call("agent.release", release);
    assert_eq!(
        client.call("agent.presence", fresh.clone())["result"]["standing"],
        "granted"
    );
    daemon.stop();
    let backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1"])
        .with_foreground(ForegroundEvidence::command(
            lead.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon = Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(backend))
        .expect("reopen");
    let mut client = Client::dial(daemon.path());
    let restored = client.call("agent.presence", fresh.clone());
    assert_eq!(restored["result"]["subject_id"], subject, "{restored}");
    let released = client.call("agent.release", json!({
        "registration_id": registration, "claim_id": fresh["claim_id"], "process": lead.identity,
    }));
    assert_eq!(released["result"]["standing"], "released", "{released}");
    let history = client.call("lifecycle.get", json!({"subject_id": subject}));
    assert_eq!(
        history["result"]["subject"]["claim_id"], fresh["claim_id"],
        "{history}"
    );
    assert_eq!(
        client.call("agent.presence", rejected_report)["result"]["standing"],
        "duplicate"
    );
    daemon.stop();
    duplicate.kill();
    lead.kill();
}

#[test]
fn a_fresh_process_reclaims_the_existing_session_subject_after_release_and_restart() {
    let sandbox = Sandbox::new("presence-resume");
    let previous = LeadProcess::start();
    let first_backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1"])
        .with_foreground(ForegroundEvidence::command(
            previous.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon =
        Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(first_backend))
            .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let old_claim = random_uuid();
    let first = client.call(
        "agent.presence",
        json!({
            "source":"herdsman-pi", "incarnation":random_uuid(), "process":previous.identity,
            "session_id":TRACKED_SESSION, "claim_id":old_claim,
            "location":{"backend":"tmux","pane":"wA:p1"}
        }),
    );
    assert_eq!(first["result"]["standing"], "granted", "{first}");
    let subject = first["result"]["subject_id"].clone();
    let old_registration = first["result"]["registration_id"].clone();
    let released = client.call(
        "agent.release",
        json!({
            "registration_id":old_registration, "claim_id":old_claim, "process":previous.identity
        }),
    );
    assert_eq!(released["result"]["standing"], "released", "{released}");
    daemon.stop();

    let resumed = LeadProcess::start();
    let resumed_backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1"])
        .with_foreground(ForegroundEvidence::command(
            resumed.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon =
        Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(resumed_backend))
            .expect("reopened daemon");
    let mut client = Client::dial(daemon.path());
    let attached = client.call(
        "agent.presence",
        json!({
            "source":"herdsman-pi", "incarnation":random_uuid(), "process":resumed.identity,
            "session_id":TRACKED_SESSION, "claim_id":random_uuid(),
            "location":{"backend":"tmux","pane":"wA:p1"}
        }),
    );
    assert_eq!(attached["result"]["standing"], "granted", "{attached}");
    assert_eq!(attached["result"]["subject_id"], subject);
    assert_ne!(attached["result"]["registration_id"], old_registration);
    daemon.stop();
    resumed.kill();
    previous.kill();
}

/// A session switch moves a process to another subject and frees the first; a
/// duplicate stays a duplicate when its incumbent exits and when the daemon
/// restarts, and takes the subject only by presenting a fresh claim.
#[test]
fn a_session_switch_frees_its_subject_and_a_duplicate_waits_for_a_fresh_claim() {
    let sandbox = Sandbox::new("presence-switch");
    let backend = |lead: &LeadProcess, panes: &[&str]| {
        MuxBackend::new(LEAD_PRIMITIVES)
            .observing(panes)
            .with_foreground(ForegroundEvidence::command(
                lead.pid,
                Some("sh".into()),
                None,
            ))
    };
    let report = |lead: &LeadProcess, incarnation: &str, session: &str, claim: &str, pane: &str| {
        json!({"source":"herdsman-pi", "incarnation":incarnation, "process":lead.identity,
            "session_id":session, "claim_id":claim, "location":{"backend":"tmux","pane":pane}})
    };
    let other_session = random_uuid();

    // One process holds a session, then opens another: the same registration
    // moves to a different subject and the first session is free.
    let switcher = LeadProcess::start();
    let mut daemon = Daemon::bind_with_backend(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend(&switcher, &["wA:p1"])),
    )
    .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let held = client.call(
        "agent.presence",
        report(
            &switcher,
            PARENT_INCARNATION,
            TRACKED_SESSION,
            &random_uuid(),
            "wA:p1",
        ),
    );
    assert_eq!(held["result"]["standing"], "granted", "{held}");
    let subject = held["result"]["subject_id"].clone();
    let switched = client.call(
        "agent.presence",
        report(
            &switcher,
            PARENT_INCARNATION,
            &other_session,
            &random_uuid(),
            "wA:p1",
        ),
    );
    assert_eq!(switched["result"]["standing"], "granted", "{switched}");
    assert_eq!(
        switched["result"]["registration_id"], held["result"]["registration_id"],
        "a switch is the same process"
    );
    assert_ne!(switched["result"]["subject_id"], subject, "{switched}");
    daemon.stop();

    // A new process takes the freed session with the same subject; a second
    // process opening it is a duplicate.
    let incumbent = LeadProcess::start();
    let duplicate = LeadProcess::start();
    let mut daemon = Daemon::bind_with_backend(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend(&incumbent, &["wA:p1", "wA:p2"])),
    )
    .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let taken = client.call(
        "agent.presence",
        report(
            &incumbent,
            CHILD_INCARNATION,
            TRACKED_SESSION,
            &random_uuid(),
            "wA:p1",
        ),
    );
    assert_eq!(taken["result"]["subject_id"], subject, "{taken}");
    let stale_claim = random_uuid();
    let stale = report(
        &duplicate,
        &random_uuid(),
        TRACKED_SESSION,
        &stale_claim,
        "wA:p2",
    );
    assert_eq!(
        client.call("agent.presence", stale.clone())["result"]["standing"],
        "duplicate"
    );

    // The incumbent exits and the daemon restarts: the duplicate's loaded state
    // is still stale, so repeating its claim never promotes it.
    incumbent.kill();
    let gone = Instant::now() + DEADLINE;
    while process_exists(incumbent.pid) && Instant::now() < gone {
        thread::sleep(Duration::from_millis(10));
    }
    daemon.stop();
    let mut daemon = Daemon::bind_with_backend(
        &sandbox.socket(),
        &sandbox.state(),
        Some(backend(&duplicate, &["wA:p2"])),
    )
    .expect("daemon");
    let mut client = Client::dial(daemon.path());
    assert_eq!(
        client.call("agent.presence", stale.clone())["result"]["standing"],
        "duplicate"
    );
    // A fresh load presents a new claim, and only then takes the subject.
    let mut fresh = stale;
    fresh["claim_id"] = json!(random_uuid());
    let promoted = client.call("agent.presence", fresh);
    assert_eq!(promoted["result"]["standing"], "granted", "{promoted}");
    assert_eq!(promoted["result"]["subject_id"], subject, "{promoted}");
    daemon.stop();
    duplicate.kill();
    switcher.kill();
}

fn restart_lead_subject_with_id(client: &mut Client, request_id: &str, subject: &str) -> Value {
    client.call_id(
        request_id,
        "lifecycle.restart",
        json!({"request": {"subject_id": subject, "claim_id": subject}}),
    )
}

fn restart_lead_subject(client: &mut Client, subject: &str) -> Value {
    restart_lead_subject_with_id(client, &random_uuid(), subject)
}

#[test]
fn lifecycle_enrollment_without_a_presence_claim_is_refused() {
    let sandbox = Sandbox::new("lead-enroll-refusals");
    let lead = LeadProcess::start();
    let backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1"])
        .with_foreground(ForegroundEvidence::command(
            lead.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon =
        Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(backend.clone()))
            .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let (agent, _writer) = registered_lead(&mut client, &lead, "wA:p1", TRACKED_SESSION);

    let unclaimed = client.call_id(
        &random_uuid(),
        "lifecycle.enroll",
        json!({"request": {"registration_id": agent, "session": TRACKED_SESSION}}),
    );
    assert_eq!(
        result_record(&unclaimed)["outcome"],
        "refused",
        "{unclaimed}"
    );
    assert!(
        result_record(&unclaimed)["message"]
            .as_str()
            .is_some_and(|message| message.contains("presence claim")),
        "{unclaimed}"
    );

    let malformed = client.call(
        "lifecycle.enroll",
        json!({"request": {"registration_id": agent, "session": "not-a-uuid"}}),
    );
    assert_eq!(error_code(&malformed), "bad_params", "{malformed}");

    let listed = client.call("lifecycle.list", json!({}));
    assert_eq!(
        listed["result"]["subjects"].as_array().map(Vec::len),
        Some(0),
        "a refused enrollment recorded a subject: {listed}"
    );

    daemon.stop();
    lead.kill();
}

#[test]
fn restart_stops_the_verified_lead_then_launches_one_resumed_successor() {
    configure_scope_launcher();
    let sandbox = Sandbox::new("lead-restart");
    let lead = LeadProcess::start();
    let backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1"])
        .with_foreground(ForegroundEvidence::command(
            lead.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon =
        Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(backend.clone()))
            .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let (_agent, subject) = presence_lead(&mut client, &lead, "wA:p1", TRACKED_SESSION);

    let restart_id = random_uuid();
    let selected =
        agent_radar::control_plane::client::connect_at(daemon.path()).expect("dashboard client");
    let restarted = selected
        .lifecycle
        .as_ref()
        .expect("daemon lifecycle controls")
        .restart_subject(
            &agent_radar::control_plane::client::LifecycleControlRequest {
                request_id: restart_id.clone(),
                claim_id: subject.clone(),
                subject_id: subject.clone(),
            },
            &AtomicBool::new(false),
        )
        .expect("lifecycle.restart socket control");
    let record = &restarted.request;
    assert_eq!(record["outcome"], "completed", "{restarted:?}");
    let effects = record["effects"].as_array().expect("effects");
    assert_eq!(effects[0], "signalled previous process", "{restarted:?}");
    assert_eq!(effects[1], "previous process exited", "{restarted:?}");
    assert!(!process_exists(lead.pid), "the previous lead still runs");

    let launches = backend.launches();
    assert_eq!(launches.len(), 1, "one successor, got {launches:?}");
    let current = client.call("lifecycle.get", json!({"subject_id": subject}));
    assert_eq!(current["result"]["subject"]["subject_id"], subject);
    // The successor is a new claim of the same agent, so it is named by its own
    // claim id and no generation is published.
    assert_ne!(current["result"]["subject"]["claim_id"], subject);
    assert!(
        current["result"]["subject"]
            .get("execution_generation")
            .is_none(),
        "{current:?}"
    );
    let successor_id = current["result"]["subject"]["claim_id"]
        .as_str()
        .expect("successor claim id");
    let events = client.call("events.read", json!({"after": 0, "limit": 256}));
    let successor_events: Vec<_> = events["result"]["events"]
        .as_array()
        .expect("semantic events")
        .iter()
        .filter(|event| event["claim_id"] == successor_id)
        .collect();
    assert!(
        !successor_events.is_empty(),
        "successor lifecycle events: {events}"
    );
    assert!(
        successor_events
            .iter()
            .all(|event| { event["subject_id"] == subject && event["claim_id"] == successor_id })
    );
    let delivered = recorded_launch(&sandbox.state(), successor_id);
    assert!(
        delivered
            .argv
            .windows(2)
            .any(|pair| pair == ["--session", TRACKED_SESSION]),
        "the successor resumes the live session: {:?}",
        delivered.argv
    );

    // Same request ID replays its first answer even though the public subject ID
    // now resolves to the successor execution.
    let replay = selected
        .lifecycle
        .as_ref()
        .expect("daemon lifecycle controls")
        .restart_subject(
            &agent_radar::control_plane::client::LifecycleControlRequest {
                request_id: restart_id.clone(),
                claim_id: subject.clone(),
                subject_id: subject.clone(),
            },
            &AtomicBool::new(false),
        )
        .expect("same-ID lifecycle.restart replay");
    assert_eq!(
        replay.request, restarted.request,
        "same-ID replay returns the stored outcome"
    );
    assert_eq!(
        backend.launches().len(),
        1,
        "a replay launched a second successor"
    );

    // A distinct restart request resolves to the latest generation. Its Pi
    // process has not bound yet, so there is no recorded pane to restart from.
    let again = restart_lead_subject(&mut client, &subject);
    assert_eq!(again["error"]["code"], "refused", "{again}");
    assert_eq!(
        backend.launches().len(),
        1,
        "a second restart launched again"
    );

    daemon.stop();
    lead.kill();
}

#[test]
fn restart_refuses_before_any_signal_when_the_session_context_changes() {
    let sandbox = Sandbox::new("lead-restart-refusals");
    let lead = LeadProcess::start();
    let backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p1"])
        .with_foreground(ForegroundEvidence::command(
            lead.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon =
        Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(backend.clone()))
            .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let (agent, subject) = presence_lead(&mut client, &lead, "wA:p1", TRACKED_SESSION);

    let unknown = restart_lead_subject(&mut client, &random_uuid());
    assert_eq!(error_code(&unknown), "not_found", "{unknown}");
    let malformed = client.call(
        "lifecycle.restart",
        json!({"request": {"subject_id": "not-a-uuid"}}),
    );
    assert_eq!(error_code(&malformed), "bad_params", "{malformed}");

    // A fresh presence claim switches the same process to a different session.
    let moved = client.call(
        "agent.presence",
        json!({
            "source":"herdsman-pi", "incarnation":PARENT_INCARNATION,
            "registration_id":agent, "process":lead.identity.clone(),
            "session_id":random_uuid(), "claim_id":random_uuid(),
            "location":{"backend":"tmux", "pane":"wA:p1"}
        }),
    );
    assert_eq!(moved["result"]["standing"], "granted", "{moved}");
    let stale = restart_lead_subject(&mut client, &subject);
    assert_eq!(result_record(&stale)["outcome"], "refused", "{stale}");
    assert!(
        process_exists(lead.pid),
        "a session refusal signalled the lead"
    );
    assert!(backend.launches().is_empty());

    daemon.stop();
    lead.kill();
}

#[test]
fn presence_waits_without_a_current_pane_location() {
    let sandbox = Sandbox::new("presence-pane-pending");
    let lead = LeadProcess::start();
    let backend = MuxBackend::new(LEAD_PRIMITIVES)
        .observing(&["wA:p9"])
        .with_foreground(ForegroundEvidence::command(
            lead.pid,
            Some("sh".into()),
            None,
        ));
    let mut daemon = Daemon::bind_with_backend(&sandbox.socket(), &sandbox.state(), Some(backend))
        .expect("daemon");
    let mut client = Client::dial(daemon.path());
    let answer = client.call(
        "agent.presence",
        json!({
            "source":"herdsman-pi", "incarnation":PARENT_INCARNATION,
            "process":lead.identity.clone(), "session_id":TRACKED_SESSION,
            "claim_id":random_uuid()
        }),
    );
    assert_eq!(answer["result"]["standing"], "pending", "{answer}");
    let subjects = client.call("lifecycle.list", json!({}));
    assert_eq!(subjects["result"]["subjects"].as_array().unwrap().len(), 0);
    daemon.stop();
    lead.kill();
}

/// A managed launch end to end, from real output: the workload is a scope member
/// before it runs, scope state is never the outcome, a process that ignores TERM
/// is a survivor, and a daemon restart reattaches without signalling.
#[test]
fn a_managed_launch_is_scoped_before_it_runs_and_survives_a_restart() {
    let Some(server) = TmuxServer::start("tmux-scope-e2e") else {
        return;
    };
    let sandbox = Sandbox::new("tmux-scope-e2e");
    let (socket, state) = (sandbox.socket(), sandbox.state());
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let parent = tracked_parent(&mut client);
    let out = sandbox.root.join("out");
    fs::create_dir_all(&out).expect("output directory");

    // The program records its own cgroup before anything else: if the group did
    // not exist first, there is no file to read.
    // Both launches are admitted while the parent's published session is fresh;
    // its context is a lease, so a later publication is a separate concern.
    let stubborn = "trap '' TERM; exec sleep 300";
    let (_, stubborn_subject) =
        tracked_spawn(&mut client, &state, &parent, json!(["-c", stubborn]));
    let script = format!(
        "cat /proc/self/cgroup > {}/scope.cgroup; exec sleep 300",
        out.display()
    );
    let (_, subject) = tracked_spawn(&mut client, &state, &parent, json!(["-c", script]));
    // The program records its own cgroup before anything else: if the group did
    // not exist first, there is no file to read.
    let membership = wait_for_text(&out.join("scope.cgroup"));
    let recorded_registration = subject_record(&state, &subject)["registration_id"]
        .as_str()
        .expect("a registration")
        .to_string();
    let registration = recorded_registration.replace('-', "");
    assert!(
        membership.contains(&format!(
            "/radar.slice/radar-{registration}.slice/radar-{registration}-main.scope"
        )),
        "the workload is not in its own scope: {membership}"
    );

    // Scope state is not an outcome: it reads `success` while the unit is active,
    // and the typed tmux observation is what reports an exit.
    let unit = format!("radar-{registration}-main.scope");
    let shown = Command::new("systemctl")
        .args([
            "--user",
            "show",
            &unit,
            "--property=ActiveState",
            "--property=Result",
            "--value",
        ])
        .output()
        .expect("systemctl show");
    assert!(shown.status.success(), "systemctl show failed for {unit}");
    let state_text = String::from_utf8_lossy(&shown.stdout).to_lowercase();
    assert!(
        state_text.contains("active") && !state_text.contains("failed"),
        "{state_text}"
    );

    // One that ignores TERM is reported, not killed: no SIGKILL escalation.
    let stubborn_pid = captured_identity(&state, &stubborn_subject).pid;
    let cancelled = cancel_subject(&mut client, &random_uuid(), &stubborn_subject);
    let cancelled_record = result_record(&cancelled).clone();
    assert!(
        cancelled_record.to_string().contains("not verified")
            || cancelled_record.to_string().contains("still running"),
        "a TERM-ignoring process must be a survivor: {cancelled}"
    );
    assert!(process_exists(stubborn_pid), "the survivor was killed");
    let _ = Command::new("kill")
        .args(["-9", &stubborn_pid.to_string()])
        .status();

    // The record itself names the group: recovery needs it, and no public view
    // carries the launch's environment.
    let registry = agent_radar::control_plane::Registry::open(&state).expect("registry");
    let recorded =
        agent_radar::control_plane::lifecycle::read_process(&registry, &recorded_registration)
            .expect("read the process")
            .expect("the process is recorded");
    let scope = recorded
        .scope
        .expect("a launch Radar admitted runs in a scope of its own");
    assert_eq!(scope.unit, unit, "the record names the unit that exists");
    assert_eq!(
        scope.slice,
        format!("{}.slice", unit.trim_end_matches("-main.scope")),
        "the record names the slice that exists: {scope:?}"
    );
    // The other launch's process survived that group's stop: a stop of one
    // group is not a stop of every group.
    assert!(
        process_exists(captured_identity(&state, &subject).pid),
        "a sibling group's stop took this process"
    );

    // A restart re-reads the same launch and signals nothing.
    let pid = captured_identity(&state, &subject).pid;
    assert_eq!(daemon.stop(libc::SIGTERM, DEADLINE).code(), Some(0));
    let mut daemon = tmux_daemon(&socket, &state, &server);
    let mut client = Client::dial(&socket);
    let listed = client.call("lifecycle.get", json!({"subject_id": subject}));
    let subject_view = &listed["result"]["subject"];
    assert_eq!(
        subject_view["process"]["pid"],
        json!(pid),
        "the restart re-read the same process: {listed}"
    );
    assert_eq!(subject_view["state"], "captured", "{listed}");
    assert_eq!(subject_view["signalled_at"], json!(null), "{listed}");
    assert!(process_exists(pid), "the restart signalled the workload");

    let status = daemon.stop(libc::SIGTERM, DEADLINE);
    assert_eq!(status.code(), Some(0), "{status:?}");
}

/// Waits for a file a launched program writes, returning its text.
fn wait_for_text(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "{} was never written",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}
