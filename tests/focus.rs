//! Focus behaviour against an in-memory runtime.
//!
//! The focuser's own job is running a request off the calling thread, handing
//! its outcome back without blocking and cancelling promptly at shutdown. Every
//! transport — the CLI invocation, socket discovery, the wire request, its
//! deadline and child cleanup — belongs to the runtime adapter and is tested
//! with a fake executable and a stub socket in `tests/runtime.rs`. Everything
//! here drives a small fake provider: a normalized target in, a one-line
//! message out, no CLI and no socket.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::model::{FleetObservation, ForegroundEvidence};
use agent_radar::{CloseTarget, FocusOutcome, Focuser, RuntimeProvider, Target};

/// How long one request may take before the test calls it stuck.
const PATIENCE: Duration = Duration::from_secs(10);

/// What a fake runtime answers to a focus request.
#[derive(Clone)]
enum Behavior {
    Succeed,
    Refuse(String),
    /// Holds the request open until it is cancelled.
    Block,
}

/// An in-memory runtime: records the normalized targets it is asked to focus
/// and answers with the staged outcome.
#[derive(Clone)]
struct FakeRuntime {
    seen: Arc<Mutex<Vec<Target>>>,
    behavior: Behavior,
}

impl FakeRuntime {
    fn new(behavior: Behavior) -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
            behavior,
        }
    }
}

impl RuntimeProvider for FakeRuntime {
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        unreachable!("the focuser reads no inventory")
    }

    fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        unreachable!("the focuser reads no foreground evidence")
    }

    fn focus(&self, target: &Target, cancel: &AtomicBool) -> Result<(), String> {
        self.focus_outcome(target, cancel)
            .diagnostic()
            .map_or(Ok(()), |message| Err(message.to_string()))
    }

    fn focus_outcome(&self, target: &Target, cancel: &AtomicBool) -> FocusOutcome {
        self.seen.lock().expect("seen").push(target.clone());
        match &self.behavior {
            Behavior::Succeed => FocusOutcome::Completed,
            Behavior::Refuse(message) => FocusOutcome::Refused(message.clone()),
            Behavior::Block => {
                while !cancel.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(5));
                }
                FocusOutcome::Unknown("focus cancelled".to_string())
            }
        }
    }

    fn close(&self, _target: &CloseTarget, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the focuser never closes anything")
    }
}

/// Runs one request to its outcome, never waiting on the runtime itself.
fn request(runtime: FakeRuntime, target: Target) -> FocusOutcome {
    let mut focuser = Focuser::new(runtime);
    focuser.start(target);
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(outcome) = focuser.poll() {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "the focus request never finished"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_workspace_target_reaches_the_runtime() {
    let runtime = FakeRuntime::new(Behavior::Succeed);
    let seen = Arc::clone(&runtime.seen);
    let outcome = request(runtime, Target::Workspace("wA".into()));
    assert_eq!(outcome, FocusOutcome::Completed);
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![Target::Workspace("wA".into())]
    );
}

#[test]
fn a_pane_target_reaches_the_runtime() {
    let runtime = FakeRuntime::new(Behavior::Succeed);
    let seen = Arc::clone(&runtime.seen);
    let outcome = request(runtime, Target::Pane("wA:p1".into()));
    assert_eq!(outcome, FocusOutcome::Completed);
    assert_eq!(
        *seen.lock().expect("seen"),
        vec![Target::Pane("wA:p1".into())]
    );
}

#[test]
fn a_refusal_is_reported_unchanged() {
    let runtime = FakeRuntime::new(Behavior::Refuse(
        "herdr refused to focus: pane wA:p1 not found".into(),
    ));
    let outcome = request(runtime, Target::Pane("wA:p1".into()));
    assert_eq!(
        outcome,
        FocusOutcome::Refused("herdr refused to focus: pane wA:p1 not found".into())
    );
}

#[test]
fn a_blocked_request_does_not_delay_shutdown() {
    let mut focuser = Focuser::new(FakeRuntime::new(Behavior::Block));
    focuser.start(Target::Workspace("wA".into()));
    thread::sleep(Duration::from_millis(50));

    let started = Instant::now();
    focuser.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown waited on the request instead of cancelling it"
    );
    assert!(
        focuser.poll().is_none(),
        "a cancelled request reports nothing"
    );
}
