//! Focus requests: the one action Radar asks a runtime to take.
//!
//! [`Focuser`] runs one request at a time on its own thread and never waits on
//! the runtime: [`Focuser::start`] hands the target over, [`Focuser::poll`]
//! picks up its outcome on a later loop iteration, and [`Focuser::shutdown`]
//! cancels and joins the worker, so a request outstanding at exit never delays
//! quitting and leaves no thread behind. How a request is transported — the
//! CLI invocation, socket discovery, the wire request, its deadline and its
//! child cleanup — belongs to the runtime adapter behind
//! [`RuntimeProvider`](crate::runtime::RuntimeProvider); this module holds none
//! of it and passes a normalized [`Target`] across the seam.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::observation::ObservationState;
use crate::runtime::{FocusOutcome, RuntimeProvider, Target};
use crate::tree::{FleetTree, RowId, RowKind, TreeNode};

/// A focus request running on its own thread.
struct Request {
    /// Set by the owner; the worker turns it into a kill or an abandoned wait.
    cancel: Arc<AtomicBool>,
    outcome: Receiver<FocusOutcome>,
    worker: JoinHandle<()>,
}

/// Runs focus requests off the calling thread, one at a time.
pub struct Focuser {
    provider: Arc<dyn RuntimeProvider>,
    in_flight: Option<Request>,
}

impl Focuser {
    /// A focuser that runs nothing until [`Self::start`].
    pub fn new(provider: impl RuntimeProvider + 'static) -> Self {
        Self {
            provider: Arc::new(provider),
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
            let provider = Arc::clone(&self.provider);
            let cancel = Arc::clone(&cancel);
            thread::Builder::new()
                .name("radar-focus".to_string())
                .spawn(move || {
                    let _ = sender.send(provider.focus_outcome(&target, &cancel));
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
    pub fn poll(&mut self) -> Option<FocusOutcome> {
        let request = self.in_flight.as_mut()?;
        let outcome = match request.outcome.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            // The worker always hands over exactly one outcome before
            // returning, so this is only reachable through a panic in it.
            Err(TryRecvError::Disconnected) => {
                FocusOutcome::Unknown("the focus request stopped unexpectedly".to_string())
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
