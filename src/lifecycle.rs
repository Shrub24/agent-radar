//! Lifecycle actions: closing a location Radar has positive evidence is
//! unmanaged.
//!
//! Containment is the whole policy. Pi Herdsman owns the panes it manages, so a
//! direct runtime close is only ever sent for a location with positive evidence
//! that the owner does not have it. Everything ambiguous refuses: an
//! unverified Pi pane, an agent kind Radar cannot place, a location an owner
//! manages, or a tab holding any of those. The classification is derived from
//! current evidence, never from a cached confirmation.
//!
//! [`Closer`] runs a confirmed close off the calling thread and takes its own
//! inventory immediately before acting. A direct mux close has no conditional
//! form, so a fresh containment check as close as the runtime allows is the
//! best guard against closing a location that became managed in the meantime;
//! nothing here promises atomicity across the inventory read and the close.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::control::{self, ControlRequest, ControlResult, NewRequest, OwnerControl, RequestState};
use crate::model::{
    AgentObservation, AgentState, FleetObservation, Lineage, SemanticState, SessionIdentity,
};
use crate::observation::RetainedAgent;
use crate::runtime::{CloseTarget, RuntimeProvider};
use crate::theme;

/// Whether a location is known to be outside Pi Herdsman's ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Containment {
    /// Positive evidence the location is unmanaged: no agent, or an agent kind
    /// Pi Herdsman never manages, with no retained association blocking it.
    Unmanaged,
    /// The owner's metadata says Pi Herdsman manages this location.
    Managed,
    /// Cannot be told: a Pi pane publishing no metadata, an agent kind Radar
    /// cannot place, or an agent whose kind is unknown.
    Uncertain,
}

impl Containment {
    /// Whether a direct runtime close may be sent for this location.
    pub fn is_unmanaged(self) -> bool {
        matches!(self, Self::Unmanaged)
    }
}

/// The pane's containment from current evidence.
pub fn pane(
    inventory: &FleetObservation,
    retained: &BTreeMap<String, RetainedAgent>,
    pane_id: &str,
) -> Containment {
    // A retained association is local continuity state: its pane reports no
    // agent now, but a managed one must still forbid a direct close. A non-Pi
    // association blocks nothing, so the live evidence below decides.
    if let Some(entry) = retained.get(pane_id) {
        let containment = agent_containment(&entry.observation);
        if !containment.is_unmanaged() {
            return containment;
        }
    }
    match inventory.agent_on_pane(pane_id) {
        Some(agent) => agent_containment(agent),
        // No agent and no blocking association: positive unmanaged evidence.
        None => Containment::Unmanaged,
    }
}

/// The tab's containment: managed or uncertain if any member pane is, so a
/// mixed tab is refused whole rather than closed around its managed pane.
pub fn tab(
    inventory: &FleetObservation,
    retained: &BTreeMap<String, RetainedAgent>,
    tab_id: &str,
) -> Containment {
    inventory
        .panes
        .iter()
        .filter(|member| member.location.tab_id == tab_id)
        .map(|member| pane(inventory, retained, &member.location.pane_id))
        .fold(Containment::Unmanaged, stricter)
}

/// The containment of a close target read from one inventory alone, as the
/// close worker has it: local retained associations are not visible off-thread,
/// and the confirmation already checked them against the full state.
pub fn target(inventory: &FleetObservation, target: &CloseTarget) -> Containment {
    let none = BTreeMap::new();
    match target {
        CloseTarget::Pane(pane_id) => pane(inventory, &none, pane_id),
        CloseTarget::Tab(tab_id) => tab(inventory, &none, tab_id),
    }
}

/// The containment of the agent recorded on a pane.
fn agent_containment(agent: &AgentObservation) -> Containment {
    // Any owner metadata makes the pane managed, whether or not this build can
    // read the key: presence is the signal, absence is unknown.
    if agent.facts.managed_metadata {
        return Containment::Managed;
    }
    match agent.name.as_deref() {
        // Pi without owner metadata is unverified, never unmanaged.
        Some(name) if name.eq_ignore_ascii_case("pi") => Containment::Uncertain,
        // A vendor Radar ships a mark for is a kind Pi Herdsman never manages.
        Some(name) if theme::shipped_agent(name) => Containment::Unmanaged,
        // A kind Radar cannot place is never guessed unmanaged.
        _ => Containment::Uncertain,
    }
}

/// The stricter of two answers: managed beats uncertain beats unmanaged.
fn stricter(a: Containment, b: Containment) -> Containment {
    match (a, b) {
        (Containment::Managed, _) | (_, Containment::Managed) => Containment::Managed,
        (Containment::Uncertain, _) | (_, Containment::Uncertain) => Containment::Uncertain,
        _ => Containment::Unmanaged,
    }
}

/// A confirmed direct close, frozen at confirmation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseRequest {
    pub target: CloseTarget,
    /// The observed identity frozen when the operator confirmed. The worker
    /// refuses when its own fresh inventory no longer matches it, so a target
    /// replaced between confirmation and the close is never acted on.
    pub identity: TargetIdentity,
}

/// What was observed on a pane, normalized for identity comparison.
///
/// Only identity-bearing fields: the agent kind, its session, its ownership
/// lineage and its owner-published label and run. Display titles are
/// deliberately absent, so a cosmetic retitle is not a new occupant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentity {
    pub name: Option<String>,
    pub session: Option<SessionIdentity>,
    pub lineage: Option<Lineage>,
    pub managed: bool,
    pub label: Option<String>,
    pub run: Option<String>,
}

/// One pane and what occupied it when the target was frozen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneIdentity {
    pub pane_id: String,
    pub occupant: Option<AgentIdentity>,
}

/// The observed identity of a close target, frozen at confirmation.
///
/// A pane target carries that pane's occupant; a tab target carries the whole
/// observed member set, so a member added, removed or replaced is drift even
/// when every member is individually unmanaged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetIdentity {
    Pane(PaneIdentity),
    Tab {
        tab_id: String,
        members: Vec<PaneIdentity>,
    },
}

impl TargetIdentity {
    /// The target this identity describes.
    pub fn target(&self) -> CloseTarget {
        match self {
            Self::Pane(pane) => CloseTarget::Pane(pane.pane_id.clone()),
            Self::Tab { tab_id, .. } => CloseTarget::Tab(tab_id.clone()),
        }
    }
}

/// The observed identity of `target` in this inventory.
pub fn identity(inventory: &FleetObservation, target: &CloseTarget) -> TargetIdentity {
    match target {
        CloseTarget::Pane(pane_id) => TargetIdentity::Pane(pane_identity(inventory, pane_id)),
        CloseTarget::Tab(tab_id) => {
            let mut members: Vec<PaneIdentity> = inventory
                .panes
                .iter()
                .filter(|pane| pane.location.tab_id == *tab_id)
                .map(|pane| pane_identity(inventory, &pane.location.pane_id))
                .collect();
            // Membership is a set: order in the source is not identity.
            members.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
            TargetIdentity::Tab {
                tab_id: tab_id.clone(),
                members,
            }
        }
    }
}

/// Whether a fresh inventory still holds the frozen identity unchanged.
pub fn matches(inventory: &FleetObservation, expected: &TargetIdentity) -> bool {
    identity(inventory, &expected.target()) == *expected
}

fn pane_identity(inventory: &FleetObservation, pane_id: &str) -> PaneIdentity {
    PaneIdentity {
        pane_id: pane_id.to_string(),
        occupant: inventory.agent_on_pane(pane_id).map(agent_identity),
    }
}

fn agent_identity(agent: &AgentObservation) -> AgentIdentity {
    AgentIdentity {
        name: agent.name.clone(),
        session: agent.session.clone(),
        lineage: agent.lineage.clone(),
        managed: agent.facts.managed_metadata,
        label: agent.facts.label.clone(),
        run: agent.facts.run.clone(),
    }
}

/// A close running on its own thread.
struct Request {
    cancel: Arc<AtomicBool>,
    outcome: Receiver<Result<String, String>>,
    worker: JoinHandle<()>,
}

/// Runs direct closes off the calling thread, one at a time.
pub struct Closer {
    provider: Arc<dyn RuntimeProvider>,
    in_flight: Option<Request>,
}

impl Closer {
    /// A closer that runs nothing until [`Self::start`].
    pub fn new(provider: impl RuntimeProvider + 'static) -> Self {
        Self {
            provider: Arc::new(provider),
            in_flight: None,
        }
    }

    /// Starts a confirmed close off the calling thread, replacing a request
    /// already in flight. Replacing waits only for that request's worker to
    /// notice the cancellation, never for its timeout.
    pub fn start(&mut self, request: CloseRequest) {
        self.cancel_in_flight();
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, outcome) = mpsc::channel();
        let worker = {
            let provider = Arc::clone(&self.provider);
            let cancel = Arc::clone(&cancel);
            thread::Builder::new()
                .name("radar-close".to_string())
                .spawn(move || {
                    let _ = sender.send(run(provider.as_ref(), &request, &cancel));
                })
                .expect("close thread")
        };
        self.in_flight = Some(Request {
            cancel,
            outcome,
            worker,
        });
    }

    /// The finished close's outcome, if one is waiting. Never blocks.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let request = self.in_flight.as_mut()?;
        let outcome = match request.outcome.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            // The worker always hands over exactly one outcome before
            // returning, so this is only reachable through a panic in it.
            Err(TryRecvError::Disconnected) => {
                Err("the close request stopped unexpectedly".to_string())
            }
        };
        let request = self.in_flight.take().expect("checked above");
        let _ = request.worker.join();
        Some(outcome)
    }

    /// Cancels an outstanding request and waits for its worker. Idempotent.
    ///
    /// Cancellation abandons a close still waiting on the runtime; a close
    /// already handed over is not retracted, and nothing here claims it was.
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

impl Drop for Closer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One confirmed close: take a fresh inventory, prove the target is still
/// positively unmanaged, then ask the runtime to close it.
fn run(
    provider: &dyn RuntimeProvider,
    request: &CloseRequest,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let inventory = provider.inventory(cancel)?;
    match &request.target {
        CloseTarget::Pane(pane_id) if inventory.pane(pane_id).is_none() => {
            return Err(format!("pane {pane_id} is gone: nothing to close"));
        }
        CloseTarget::Tab(tab_id) if !inventory.tabs.iter().any(|tab| tab.tab_id == *tab_id) => {
            return Err(format!("tab {tab_id} is gone: nothing to close"));
        }
        _ => {}
    }
    let described = request.target.description();
    if !matches(&inventory, &request.identity) {
        return Err(format!("{described} changed: nothing was closed"));
    }
    match target(&inventory, &request.target) {
        Containment::Unmanaged => provider
            .close(&request.target, cancel)
            .map(|()| format!("closed {described}")),
        Containment::Managed => Err(format!(
            "{described} is managed by its owner: direct close refused"
        )),
        Containment::Uncertain => Err(format!(
            "{described} cannot be verified as unmanaged: not closing"
        )),
    }
}

/// One exact managed target: close or restart of a worker Radar observes as
/// managed, routed to the owner session the worker publishes.
///
/// Everything the request needs is frozen here at confirmation and revalidated
/// before publication, so a worker replaced in the meantime is never named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedRequest {
    pub operation: control::Operation,
    pub target: CloseTarget,
    /// The observed identity frozen when the confirmation opened.
    pub identity: TargetIdentity,
    /// The exact owner: the parent session UUID the worker publishes.
    pub owner_session: String,
    pub label: String,
    pub run_id: String,
    pub pi_session_id: Option<String>,
    pub pi_session_path: Option<String>,
}

impl ManagedRequest {
    /// The exact target this names, for duplicate suppression.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.owner_session, self.label, self.run_id)
    }

    /// Builds the v1 request document for publication.
    fn document(
        &self,
        now: SystemTime,
        ttl: Duration,
    ) -> Result<ControlRequest, control::ControlError> {
        let pane_id = match &self.target {
            CloseTarget::Pane(pane_id) => Some(pane_id.clone()),
            CloseTarget::Tab(_) => None,
        };
        ControlRequest::with_ttl(
            NewRequest {
                operation: self.operation,
                label: self.label.clone(),
                run_id: self.run_id.clone(),
                pane_id,
                pi_session_id: self.pi_session_id.clone(),
                pi_session_path: self.pi_session_path.clone(),
            },
            now,
            ttl,
        )
    }
}

/// The exact owner-routed action for the currently observed managed agent on
/// `pane_id`, or why it cannot be built.
///
/// The identity is exact: a missing published label, run UUID or owner session
/// refuses, rather than a request inferred from metadata absence. Only a
/// currently observed worker may be named, so a continuity-retained history row
/// is never the target.
pub fn managed_request(
    inventory: &FleetObservation,
    pane_id: &str,
    operation: control::Operation,
) -> Result<ManagedRequest, String> {
    let agent = inventory
        .agent_on_pane(pane_id)
        .ok_or_else(|| format!("pane {pane_id} is managed but is not currently observed"))?;
    let lineage = agent
        .lineage
        .clone()
        .ok_or_else(|| format!("pane {pane_id} is managed but publishes no session lineage"))?;
    let owner_session = lineage
        .parent
        .as_ref()
        .ok_or_else(|| format!("pane {pane_id} is managed but has no owner session"))?
        .as_str()
        .to_string();
    let label = agent
        .facts
        .label
        .clone()
        .filter(|label| !label.is_empty())
        .ok_or_else(|| format!("pane {pane_id} is managed but publishes no label"))?;
    let run_id = agent
        .facts
        .run
        .clone()
        .filter(|run| !run.is_empty())
        .ok_or_else(|| format!("pane {pane_id} is managed but publishes no run id"))?;
    // The session is only used as an optional cross-check; a path-like reported
    // session is the persisted file the contract names, and anything else is
    // left out rather than guessed.
    let pi_session_path = match &agent.session {
        Some(SessionIdentity::Reported { value, .. }) if value.contains('/') => Some(value.clone()),
        _ => None,
    };
    let target = CloseTarget::Pane(pane_id.to_string());
    Ok(ManagedRequest {
        operation,
        identity: identity(inventory, &target),
        target,
        owner_session,
        label,
        run_id,
        pi_session_id: Some(lineage.session.as_str().to_string()),
        pi_session_path,
    })
}

/// Whether the owner advertises this worker as restartable right now, as far as
/// Radar can tell.
///
/// Restart exists only for an owner-advertised idle retained managed worker, so
/// a missing projection refuses rather than being assumed, and any derived
/// state other than idle contradicts the idle projection. A derived unknown is
/// not a licence to restart: it refuses like any other non-idle state, because
/// Radar cannot see that the worker is safely between runs. Local checks are
/// advisory; the owner repeats its full preflight and is authoritative.
pub fn restartable(agent: &AgentObservation) -> Result<(), String> {
    if !agent.facts.managed_metadata {
        return Err("not a managed worker".to_string());
    }
    if agent.facts.state != Some(SemanticState::Idle) {
        return Err("its owner does not report it idle".to_string());
    }
    match agent.activity_state() {
        AgentState::Idle => Ok(()),
        _ => Err("it is not idle now".to_string()),
    }
}

/// How often the worker re-reads the outstanding requests' files.
const POLL: Duration = Duration::from_millis(200);

/// A change to one managed action the UI should show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Update {
    /// What a later update about the same request refines. A publication that
    /// never minted a request id uses the exact target key instead.
    pub id: String,
    pub label: String,
    pub operation: control::Operation,
    pub kind: UpdateKind,
}

/// What happened to a managed action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateKind {
    /// Published; the owner has not answered yet.
    Submitted,
    /// Publication was refused; nothing was written.
    Failed(String),
    /// The owner claimed the request: execution started, outcome unknown.
    Started,
    /// Neither result nor claim, and the absolute expiry passed.
    NotExecuted,
    /// The owner answered.
    Answered(ControlResult),
    /// A file that exists could not be read as the contract requires.
    Invalid(String),
}

/// A command to the owner-control worker.
enum Command {
    Publish(Box<ManagedRequest>),
    Stop,
}

/// One published request the worker is still reading.
struct Tracked {
    key: String,
    label: String,
    request_id: String,
    operation: control::Operation,
    expires_at_ms: i64,
    control: OwnerControl,
    reported_started: bool,
    reported_invalid: bool,
    reported_unknown: bool,
}

/// Runs managed close and restart off the calling thread.
///
/// One worker thread owns every outstanding request: it publishes a confirmed
/// action, then reads the owner's files until a result settles it. Nothing here
/// retries a claim or an expiry, treats a local timeout as a verdict, or deletes
/// a published request. The UI only ever reads [`Self::poll`], so no path blocks
/// on the filesystem.
pub struct ManagedActions {
    commands: Sender<Command>,
    updates: Receiver<Update>,
    outstanding: Arc<Mutex<HashSet<String>>>,
    worker: Option<JoinHandle<()>>,
}

impl ManagedActions {
    /// A worker bound to a control root. It reads nothing until a request is
    /// published, and never creates a directory: an absent or untrusted root is
    /// the owner's transport being unavailable, not something to fix up.
    pub fn new(root: PathBuf) -> Self {
        Self::spawn(root, control::REQUEST_TTL)
    }

    /// A worker that publishes with a shorter absolute lifetime than the
    /// contract's, so a test can reach the expiry states without waiting.
    #[cfg(test)]
    fn with_ttl(root: PathBuf, ttl: Duration) -> Self {
        Self::spawn(root, ttl)
    }

    fn spawn(root: PathBuf, ttl: Duration) -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (update_tx, updates) = mpsc::channel();
        let outstanding = Arc::new(Mutex::new(HashSet::new()));
        let worker = {
            let outstanding = Arc::clone(&outstanding);
            thread::Builder::new()
                .name("radar-managed".to_string())
                .spawn(move || work(root, ttl, command_rx, update_tx, outstanding))
                .expect("managed thread")
        };
        Self {
            commands,
            updates,
            outstanding,
            worker: Some(worker),
        }
    }

    /// Publishes a confirmed action off the calling thread.
    ///
    /// Refuses synchronously when an outstanding request already names the same
    /// exact target: a pending, started or unknown request is never duplicated.
    pub fn start(&mut self, request: ManagedRequest) -> Result<(), String> {
        let key = request.key();
        let verb = match request.operation {
            control::Operation::Close => "close",
            control::Operation::Restart => "restart",
        };
        {
            let mut outstanding = self.outstanding.lock().expect("managed targets");
            if !outstanding.insert(key.clone()) {
                return Err(format!(
                    "a {verb} request for {} is already outstanding",
                    request.label
                ));
            }
        }
        if self
            .commands
            .send(Command::Publish(Box::new(request)))
            .is_err()
        {
            self.outstanding
                .lock()
                .expect("managed targets")
                .remove(&key);
            return Err("the owner-control worker is not running".to_string());
        }
        Ok(())
    }

    /// The next update, if any. Never blocks.
    pub fn poll(&mut self) -> Option<Update> {
        self.updates.try_recv().ok()
    }

    /// Stops the worker. A request that was already published is left in place:
    /// Radar does not delete or retract it, and does not wait for its expiry.
    pub fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.send(Command::Stop);
            let _ = worker.join();
        }
    }
}

impl Drop for ManagedActions {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The owner-control worker loop: drain commands, then re-read every tracked
/// request's files.
fn work(
    root: PathBuf,
    ttl: Duration,
    commands: Receiver<Command>,
    updates: Sender<Update>,
    outstanding: Arc<Mutex<HashSet<String>>>,
) {
    let mut tracked: Vec<Tracked> = Vec::new();
    loop {
        match commands.recv_timeout(POLL) {
            Ok(Command::Publish(request)) => {
                publish(&root, ttl, *request, &updates, &outstanding, &mut tracked)
            }
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        poll_tracked(&updates, &outstanding, &mut tracked);
    }
}

/// Publishes one request, or reports why it could not be.
fn publish(
    root: &Path,
    ttl: Duration,
    request: ManagedRequest,
    updates: &Sender<Update>,
    outstanding: &Mutex<HashSet<String>>,
    tracked: &mut Vec<Tracked>,
) {
    let key = request.key();
    let label = request.label.clone();
    let operation = request.operation;
    let control = match OwnerControl::open(root, &request.owner_session) {
        Ok(control) => control,
        Err(error) => {
            return fail(
                outstanding,
                updates,
                &key,
                &label,
                operation,
                error.to_string(),
            );
        }
    };
    let document = match request.document(SystemTime::now(), ttl) {
        Ok(document) => document,
        Err(error) => {
            return fail(
                outstanding,
                updates,
                &key,
                &label,
                operation,
                error.to_string(),
            );
        }
    };
    let expires_at_ms = match document.expires_at_ms() {
        Ok(ms) => ms,
        Err(error) => {
            return fail(
                outstanding,
                updates,
                &key,
                &label,
                operation,
                error.to_string(),
            );
        }
    };
    if let Err(error) = control.publish(&document) {
        return fail(
            outstanding,
            updates,
            &key,
            &label,
            operation,
            error.to_string(),
        );
    }
    let request_id = document.request_id.clone();
    tracked.push(Tracked {
        key,
        label: label.clone(),
        request_id: request_id.clone(),
        operation,
        expires_at_ms,
        control,
        reported_started: false,
        reported_invalid: false,
        reported_unknown: false,
    });
    let _ = updates.send(Update {
        id: request_id,
        label,
        operation,
        kind: UpdateKind::Submitted,
    });
}

/// Reports a publication that wrote nothing and releases the target so a later
/// attempt is not blocked by a request that does not exist.
fn fail(
    outstanding: &Mutex<HashSet<String>>,
    updates: &Sender<Update>,
    key: &str,
    label: &str,
    operation: control::Operation,
    message: String,
) {
    outstanding.lock().expect("managed targets").remove(key);
    let _ = updates.send(Update {
        id: key.to_string(),
        label: label.to_string(),
        operation,
        kind: UpdateKind::Failed(message),
    });
}

/// Re-reads every tracked request's files.
///
/// A known result or an unclaimed expiry settles the request and releases the
/// target. An `unknown` result is reported once and then kept: the owner
/// explicitly did not establish what happened, so the target stays suppressed
/// until a known result replaces it, and no dismissal or local deadline is
/// resolution. A claim is reported once and then kept: it is terminal evidence
/// that execution started, never a reason to retry, and a later result still
/// refines it. Unreadable evidence is reported once and kept, since the file
/// may yet be replaced by a valid one.
fn poll_tracked(
    updates: &Sender<Update>,
    outstanding: &Mutex<HashSet<String>>,
    tracked: &mut Vec<Tracked>,
) {
    let now = now_ms();
    let mut index = 0;
    while index < tracked.len() {
        let state = tracked[index].control.derive_state(
            &tracked[index].request_id,
            tracked[index].operation,
            tracked[index].expires_at_ms,
            now,
        );
        match state {
            Ok(RequestState::Result(result)) => {
                // An unknown outcome answers the request but not the question of
                // what happened to the target, so it must not release the
                // suppression that guards against re-requesting. Report it once
                // and keep tracking: a later known result still refines it.
                if result.outcome == control::Outcome::Unknown {
                    if !tracked[index].reported_unknown {
                        tracked[index].reported_unknown = true;
                        let entry = &tracked[index];
                        let _ = updates.send(Update {
                            id: entry.request_id.clone(),
                            label: entry.label.clone(),
                            operation: entry.operation,
                            kind: UpdateKind::Answered(result),
                        });
                    }
                    index += 1;
                    continue;
                }
                let entry = tracked.swap_remove(index);
                settle(outstanding, &entry.key);
                let _ = updates.send(Update {
                    id: entry.request_id,
                    label: entry.label,
                    operation: entry.operation,
                    kind: UpdateKind::Answered(result),
                });
            }
            Ok(RequestState::NotExecuted) => {
                let entry = tracked.swap_remove(index);
                settle(outstanding, &entry.key);
                let _ = updates.send(Update {
                    id: entry.request_id,
                    label: entry.label,
                    operation: entry.operation,
                    kind: UpdateKind::NotExecuted,
                });
            }
            Ok(RequestState::Started) => {
                if !tracked[index].reported_started {
                    tracked[index].reported_started = true;
                    let entry = &tracked[index];
                    let _ = updates.send(Update {
                        id: entry.request_id.clone(),
                        label: entry.label.clone(),
                        operation: entry.operation,
                        kind: UpdateKind::Started,
                    });
                }
                index += 1;
            }
            Ok(RequestState::Pending) => index += 1,
            Err(error) => {
                if !tracked[index].reported_invalid {
                    tracked[index].reported_invalid = true;
                    let entry = &tracked[index];
                    let _ = updates.send(Update {
                        id: entry.request_id.clone(),
                        label: entry.label.clone(),
                        operation: entry.operation,
                        kind: UpdateKind::Invalid(error.to_string()),
                    });
                }
                index += 1;
            }
        }
    }
}

fn settle(outstanding: &Mutex<HashSet<String>>, key: &str) {
    outstanding.lock().expect("managed targets").remove(key);
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentObservation, HerdsmanFacts, Location, Pane};
    use crate::observation::RetentionBasis;

    fn location(pane_id: &str) -> Location {
        Location {
            workspace_id: "wA".into(),
            tab_id: "wA:t1".into(),
            pane_id: pane_id.into(),
        }
    }

    fn pane_row(pane_id: &str) -> Pane {
        Pane {
            location: location(pane_id),
            label: None,
            title: None,
        }
    }

    fn agent(pane_id: &str, name: Option<&str>, managed: bool) -> AgentObservation {
        AgentObservation {
            location: location(pane_id),
            name: name.map(str::to_string),
            label: None,
            status: None,
            session: None,
            lineage: None,
            facts: HerdsmanFacts {
                managed_metadata: managed,
                ..HerdsmanFacts::default()
            },
        }
    }

    fn inventory(panes: &[&str], agents: Vec<AgentObservation>) -> FleetObservation {
        FleetObservation {
            workspaces: Vec::new(),
            tabs: vec![crate::model::Tab {
                tab_id: "wA:t1".into(),
                workspace_id: "wA".into(),
                label: None,
                number: None,
            }],
            panes: panes.iter().map(|id| pane_row(id)).collect(),
            agents,
        }
    }

    fn retained(agent: AgentObservation) -> BTreeMap<String, RetainedAgent> {
        let mut map = BTreeMap::new();
        map.insert(
            agent.location.pane_id.clone(),
            RetainedAgent {
                observation: agent,
                basis: RetentionBasis::Unverified,
            },
        );
        map
    }

    fn none() -> BTreeMap<String, RetainedAgent> {
        BTreeMap::new()
    }

    #[test]
    fn a_pane_with_no_agent_is_unmanaged() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Unmanaged);
    }

    #[test]
    fn a_non_pi_known_agent_is_unmanaged() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("claude"), false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Unmanaged);
    }

    #[test]
    fn a_pi_pane_without_metadata_is_uncertain() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("pi"), false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn pi_metadata_marks_a_pane_managed() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("pi"), true)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Managed);
    }

    #[test]
    fn an_unrecognized_agent_kind_is_never_unmanaged() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("mystery"), false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn an_agent_without_a_kind_is_uncertain() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", None, false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn a_retained_managed_association_forbids_a_direct_close() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        let retained = retained(agent("wA:p1", Some("pi"), true));
        assert_eq!(pane(&inventory, &retained, "wA:p1"), Containment::Managed);
    }

    #[test]
    fn a_retained_uncertain_association_forbids_a_direct_close() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        let retained = retained(agent("wA:p1", Some("pi"), false));
        assert_eq!(pane(&inventory, &retained, "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn a_mixed_tab_is_refused_whole() {
        let inventory = inventory(&["wA:p1", "wA:p2"], vec![agent("wA:p1", Some("pi"), true)]);
        assert_eq!(tab(&inventory, &none(), "wA:t1"), Containment::Managed);
    }

    #[test]
    fn an_all_unmanaged_tab_is_unmanaged() {
        let inventory = inventory(&["wA:p1", "wA:p2"], Vec::new());
        assert_eq!(tab(&inventory, &none(), "wA:t1"), Containment::Unmanaged);
    }

    #[test]
    fn target_reads_pane_and_tab_from_one_inventory() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        assert!(target(&inventory, &CloseTarget::Pane("wA:p1".into())).is_unmanaged());
        assert!(target(&inventory, &CloseTarget::Tab("wA:t1".into())).is_unmanaged());
    }

    const OWNER: &str = "01a10c77-8a6b-7035-8a0e-b1fa607bb507";
    const RUN: &str = "8f2b1c34-5d6e-4f70-8a91-2b3c4d5e6f71";

    /// One managed worker with a complete published identity.
    fn managed_inventory() -> FleetObservation {
        let session = crate::model::SessionUuid::parse(OWNER).expect("a UUID");
        let mut observation = inventory(&["wA:p1"], Vec::new());
        observation.agents.push(AgentObservation {
            location: location("wA:p1"),
            name: Some("pi".into()),
            label: None,
            status: Some(crate::model::RuntimeStatus::Idle),
            session: None,
            lineage: Some(Lineage {
                session: session.clone(),
                parent: Some(session),
            }),
            facts: HerdsmanFacts {
                managed_metadata: true,
                label: Some("implementer-1".into()),
                run: Some(RUN.into()),
                state: Some(SemanticState::Idle),
                ..HerdsmanFacts::default()
            },
        });
        observation
    }

    fn temp_owner_root() -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("radar-managed-unit-{}-{nanos}", std::process::id()));
        let owner = root.join(OWNER);
        for directory in [owner.clone(), owner.join("inbox"), owner.join("results")] {
            std::fs::create_dir_all(&directory).expect("create");
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .expect("private mode");
        }
        root
    }

    fn wait(actions: &mut ManagedActions) -> Update {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(update) = actions.poll() {
                return update;
            }
            assert!(std::time::Instant::now() < deadline, "no update arrived");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn request_files(root: &Path) -> usize {
        std::fs::read_dir(root.join(OWNER).join("inbox"))
            .expect("inbox")
            .filter(|entry| {
                entry
                    .as_ref()
                    .is_ok_and(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
            })
            .count()
    }

    #[test]
    fn an_unclaimed_request_expires_without_retry_or_duplication() {
        let root = temp_owner_root();
        let inventory = managed_inventory();
        let request = managed_request(&inventory, "wA:p1", control::Operation::Close)
            .expect("a complete managed identity");
        let mut actions = ManagedActions::with_ttl(root.clone(), Duration::from_millis(40));
        actions.start(request).expect("publish");

        let submitted = wait(&mut actions);
        assert!(
            matches!(submitted.kind, UpdateKind::Submitted),
            "{submitted:?}"
        );
        // Neither result nor claim: the absolute expiry is not executed.
        let expired = wait(&mut actions);
        assert!(
            matches!(expired.kind, UpdateKind::NotExecuted),
            "{expired:?}"
        );
        assert_eq!(request_files(&root), 1, "nothing was retried or duplicated");
        actions.shutdown();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_managed_request_needs_every_piece_of_exact_identity() {
        let mut observation = managed_inventory();
        observation.agents[0].facts.label = None;
        assert!(managed_request(&observation, "wA:p1", control::Operation::Close).is_err());

        let mut observation = managed_inventory();
        observation.agents[0].lineage = Some(Lineage {
            session: crate::model::SessionUuid::parse(OWNER).expect("a UUID"),
            parent: None,
        });
        assert!(managed_request(&observation, "wA:p1", control::Operation::Close).is_err());

        let observation = managed_inventory();
        let request =
            managed_request(&observation, "wA:p1", control::Operation::Close).expect("complete");
        assert_eq!(request.owner_session, OWNER);
        assert_eq!(request.label, "implementer-1");
        assert_eq!(request.run_id, RUN);
        assert_eq!(request.pi_session_id.as_deref(), Some(OWNER));
    }

    #[test]
    fn restart_needs_an_owner_advertised_idle_worker() {
        let mut observation = managed_inventory();
        assert!(restartable(&observation.agents[0]).is_ok());

        // A busy projection is not restartable even when Radar's own derivation
        // agrees it is working.
        observation.agents[0].facts.state = Some(SemanticState::Working);
        assert!(restartable(&observation.agents[0]).is_err());

        // An owner idle projection contradicted by a working pane refuses.
        let mut observation = managed_inventory();
        observation.agents[0].status = Some(crate::model::RuntimeStatus::Working);
        assert!(restartable(&observation.agents[0]).is_err());

        // An owner idle projection whose derivation is unknown also refuses:
        // unknown is not a licence to restart.
        let mut observation = managed_inventory();
        observation.agents[0].status = Some(crate::model::RuntimeStatus::Unknown);
        assert!(restartable(&observation.agents[0]).is_err());
    }
}
