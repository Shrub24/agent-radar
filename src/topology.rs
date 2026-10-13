//! Confirmed container controls: what the TUI asks the daemon to do to a
//! container it records, and what came of it.
//!
//! One control runs at a time, off the UI thread. Each carries the caller's own
//! request id, so an answer that never arrives leaves an operation the operator
//! can read back rather than an effect nobody can name. Nothing here retries: a
//! control the daemon may have accepted is unknown, never unperformed.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::control_plane::client::{
    ContainerAnswer, ContainerControls, ContainerCreate, ContainerDelete, ContainerReload,
};

/// One confirmed control, as the operator asked for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Control {
    /// Create a pane slot, splitting it from a pane, optionally running a command.
    Create(ContainerCreate),
    /// Delete a container, with the target the operator confirmed.
    Delete(ContainerDelete),
    /// Run a pane container's recorded recipe again.
    Reload(ContainerReload),
}

impl Control {
    /// The request id this control is dispatched and read back under.
    pub fn request_id(&self) -> &str {
        match self {
            Self::Create(request) => &request.request_id,
            Self::Delete(request) => &request.request_id,
            Self::Reload(request) => &request.request_id,
        }
    }

    /// The verb, as the operator sees it in the outcome line.
    pub fn verb(&self) -> &'static str {
        match self {
            Self::Create(_) => "create",
            Self::Delete(_) => "delete",
            Self::Reload(_) => "reload",
        }
    }
}

/// What one finished control reported: what the daemon answered, or why nothing
/// was asked of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finished {
    /// The verb the operator confirmed.
    pub verb: &'static str,
    /// The request id the control was dispatched under, so the operation can be
    /// read back whether or not an answer arrived.
    pub request_id: String,
    pub result: Result<ContainerAnswer, String>,
}

/// One control running on its own thread.
struct Request {
    cancel: Arc<AtomicBool>,
    outcome: Receiver<Result<ContainerAnswer, String>>,
    worker: JoinHandle<()>,
}

/// Runs one container control at a time off the calling thread.
pub struct ContainerControl {
    /// The daemon's controls, when this run talks to one. A direct adapter records
    /// no containers, so it has none and every control is refused.
    controls: Option<Arc<dyn ContainerControls>>,
    in_flight: Option<(Control, Request)>,
}

impl ContainerControl {
    /// A control worker that runs nothing until [`Self::start`].
    pub fn new(controls: Option<Arc<dyn ContainerControls>>) -> Self {
        Self {
            controls,
            in_flight: None,
        }
    }

    /// Whether a control is running. The TUI asks before it opens another
    /// confirmation: one control at a time is what keeps the outcome unambiguous.
    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Starts a confirmed control off the calling thread.
    ///
    /// One control runs at a time: a second is refused rather than started, because
    /// a control the daemon may already have accepted cannot be taken back, and its
    /// outcome would otherwise never reach the operator. A run without a daemon
    /// dispatches nothing and says so.
    pub fn start(&mut self, control: Control) -> Result<(), String> {
        if self.in_flight.is_some() {
            return Err("another container control is still running".to_string());
        }
        let Some(controls) = self.controls.clone() else {
            return Err("this run talks to a backend that records no containers".to_string());
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, outcome) = mpsc::channel();
        let worker = {
            let cancel = Arc::clone(&cancel);
            let dispatched = control.clone();
            thread::Builder::new()
                .name(format!("radar-topology-{}", control.verb()))
                .spawn(move || {
                    let _ = sender.send(run(controls.as_ref(), &dispatched, &cancel));
                })
                .expect("container control thread")
        };
        self.in_flight = Some((
            control,
            Request {
                cancel,
                outcome,
                worker,
            },
        ));
        Ok(())
    }

    /// The finished control's outcome, if one is waiting. Never blocks.
    pub fn poll(&mut self) -> Option<Finished> {
        let (_, request) = self.in_flight.as_mut()?;
        let result = match request.outcome.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            // The worker always hands over exactly one outcome before returning,
            // so this is only reachable through a panic in it.
            Err(TryRecvError::Disconnected) => {
                Err("the control request stopped unexpectedly".to_string())
            }
        };
        let (control, request) = self.in_flight.take().expect("checked above");
        let _ = request.worker.join();
        Some(Finished {
            verb: control.verb(),
            request_id: control.request_id().to_string(),
            result,
        })
    }

    /// Cancels an outstanding control and waits for its thread. Idempotent.
    ///
    /// Cancellation abandons a control still waiting on the daemon; one already
    /// dispatched is not retracted, and nothing here claims it was.
    pub fn shutdown(&mut self) {
        self.cancel_in_flight();
    }

    fn cancel_in_flight(&mut self) {
        if let Some((_, request)) = self.in_flight.take() {
            request.cancel.store(true, Ordering::SeqCst);
            let _ = request.worker.join();
        }
    }
}

/// One control, on the thread that runs it. The flag is the caller's own
/// cancellation: a control abandoned while it waits is cancelled, and one already
/// dispatched is not retracted.
fn run(
    controls: &dyn ContainerControls,
    control: &Control,
    cancel: &AtomicBool,
) -> Result<ContainerAnswer, String> {
    match control {
        Control::Create(request) => controls.create_container(request, cancel),
        Control::Delete(request) => controls.delete_container(request, cancel),
        Control::Reload(request) => controls.reload_container(request, cancel),
    }
}
