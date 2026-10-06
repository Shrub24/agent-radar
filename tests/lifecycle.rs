//! The direct-close worker against an in-memory runtime.
//!
//! [`Closer`]'s own job is taking a fresh inventory, refusing anything that is
//! not positively unmanaged, running the close off the calling thread and
//! cancelling promptly at shutdown. Every transport — the CLI invocation, its
//! deadline and child cleanup — belongs to the adapter and is tested against a
//! fake `herdr` in `tests/runtime.rs`. Everything here drives a small fake
//! provider: a normalized target in, a one-line message out, no runtime.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::lifecycle::{TargetIdentity, identity};
use agent_radar::model::{
    AgentObservation, FleetObservation, ForegroundEvidence, HerdsmanFacts, Location, Pane,
    SessionIdentity, Tab,
};
use agent_radar::{CloseRequest, CloseTarget, Closer, RuntimeProvider};

/// How long one request may take before the test calls it stuck.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the fake answers when it is asked for an inventory.
#[derive(Clone)]
enum Answer {
    /// One unmanaged pane, with no agent.
    Unmanaged,
    /// One pane carrying a `pi` agent; `managed` selects whether it publishes
    /// any owner metadata.
    Agent { name: &'static str, managed: bool },
    /// One pane carrying an unmanaged non-Pi agent with this session identity.
    Session { value: &'static str },
    /// The inventory read fails.
    Unavailable,
}

#[derive(Clone)]
struct FakeRuntime {
    answer: Answer,
    /// Holds the close open until it is cancelled.
    block_close: bool,
    inventories: Arc<Mutex<usize>>,
    closed: Arc<Mutex<Vec<CloseTarget>>>,
}

impl FakeRuntime {
    fn new(answer: Answer) -> Self {
        Self {
            answer,
            block_close: false,
            inventories: Arc::new(Mutex::new(0)),
            closed: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn blocked() -> Self {
        Self {
            block_close: true,
            ..Self::new(Answer::Unmanaged)
        }
    }

    fn observation(&self) -> FleetObservation {
        let location = Location {
            workspace_id: "wA".into(),
            tab_id: "wA:t1".into(),
            pane_id: "wA:p1".into(),
        };
        let agents = match &self.answer {
            Answer::Agent { name, managed } => vec![AgentObservation {
                location: location.clone(),
                name: Some((*name).to_string()),
                label: None,
                status: None,
                session: None,
                lineage: None,
                facts: HerdsmanFacts {
                    managed_metadata: *managed,
                    ..HerdsmanFacts::default()
                },
            }],
            Answer::Session { value } => vec![AgentObservation {
                location: location.clone(),
                name: Some("claude".to_string()),
                label: None,
                status: None,
                session: Some(SessionIdentity::Reported {
                    source: None,
                    value: (*value).to_string(),
                }),
                lineage: None,
                facts: HerdsmanFacts::default(),
            }],
            _ => Vec::new(),
        };
        FleetObservation {
            workspaces: Vec::new(),
            tabs: vec![Tab {
                tab_id: "wA:t1".into(),
                workspace_id: "wA".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location,
                label: None,
                title: None,
            }],
            agents,
        }
    }
}

impl RuntimeProvider for FakeRuntime {
    fn inventory(&self, _cancel: &AtomicBool) -> Result<FleetObservation, String> {
        *self.inventories.lock().expect("inventories") += 1;
        match self.answer {
            Answer::Unavailable => Err("herdr is unavailable".to_string()),
            _ => Ok(self.observation()),
        }
    }

    fn foreground_evidence(&self, _pane_id: &str, _cancel: &AtomicBool) -> ForegroundEvidence {
        ForegroundEvidence::Inconclusive
    }

    fn focus(&self, _target: &agent_radar::Target, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the closer never focuses")
    }

    fn close(&self, target: &CloseTarget, cancel: &AtomicBool) -> Result<(), String> {
        self.closed.lock().expect("closed").push(target.clone());
        if self.block_close {
            while !cancel.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            return Err("close cancelled".to_string());
        }
        Ok(())
    }
}

/// Runs one request to its outcome, never waiting on the runtime itself.
fn request(runtime: FakeRuntime, target: CloseTarget) -> Result<String, String> {
    let identity = identity(&runtime.observation(), &target);
    request_with(runtime, target, identity)
}

/// Runs one request with an explicit frozen identity, for drift tests.
fn request_with(
    runtime: FakeRuntime,
    target: CloseTarget,
    identity: TargetIdentity,
) -> Result<String, String> {
    let mut closer = Closer::new(runtime);
    closer.start(CloseRequest { target, identity });
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(outcome) = closer.poll() {
            return outcome;
        }
        assert!(Instant::now() < deadline, "the close never finished");
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn an_unmanaged_pane_reaches_the_runtime() {
    let runtime = FakeRuntime::new(Answer::Unmanaged);
    let closed = Arc::clone(&runtime.closed);
    let inventories = Arc::clone(&runtime.inventories);
    let outcome = request(runtime, CloseTarget::Pane("wA:p1".into()));
    assert_eq!(outcome, Ok("closed pane wA:p1".to_string()));
    assert_eq!(
        *closed.lock().expect("closed"),
        vec![CloseTarget::Pane("wA:p1".into())]
    );
    assert!(
        *inventories.lock().expect("inventories") >= 1,
        "the worker takes its own inventory before acting"
    );
}

#[test]
fn a_managed_agent_is_refused_without_closing() {
    let runtime = FakeRuntime::new(Answer::Agent {
        name: "pi",
        managed: true,
    });
    let closed = Arc::clone(&runtime.closed);
    let outcome = request(runtime, CloseTarget::Pane("wA:p1".into()));
    let message = outcome.expect_err("a managed pane is refused");
    assert!(message.contains("managed"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn an_unverified_pi_pane_is_refused() {
    let runtime = FakeRuntime::new(Answer::Agent {
        name: "pi",
        managed: false,
    });
    let closed = Arc::clone(&runtime.closed);
    let message = request(runtime, CloseTarget::Pane("wA:p1".into()))
        .expect_err("an unverified pane is refused");
    assert!(message.contains("unmanaged"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn a_known_non_pi_agent_is_unmanaged() {
    let runtime = FakeRuntime::new(Answer::Agent {
        name: "claude",
        managed: false,
    });
    let outcome = request(runtime, CloseTarget::Pane("wA:p1".into()));
    assert_eq!(outcome, Ok("closed pane wA:p1".to_string()));
}

#[test]
fn a_failed_inventory_refuses_without_closing() {
    let runtime = FakeRuntime::new(Answer::Unavailable);
    let closed = Arc::clone(&runtime.closed);
    let message = request(runtime, CloseTarget::Pane("wA:p1".into()))
        .expect_err("an unavailable runtime is refused");
    assert!(message.contains("unavailable"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn a_target_that_is_gone_is_refused() {
    let runtime = FakeRuntime::new(Answer::Unmanaged);
    let closed = Arc::clone(&runtime.closed);
    let message = request(runtime, CloseTarget::Pane("wA:zz".into()))
        .expect_err("a pane no longer in the inventory is refused");
    assert!(message.contains("gone"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn a_tab_with_a_managed_member_is_refused_whole() {
    let runtime = FakeRuntime::new(Answer::Agent {
        name: "pi",
        managed: false,
    });
    let closed = Arc::clone(&runtime.closed);
    let message =
        request(runtime, CloseTarget::Tab("wA:t1".into())).expect_err("a mixed tab is refused");
    assert!(message.contains("unmanaged"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn a_replaced_unmanaged_occupant_is_refused() {
    // The frozen identity names session s1; the worker's fresh inventory reports
    // a different unmanaged session on the same pane. Both are individually
    // closeable, but the operator confirmed the first, so nothing is closed.
    let runtime = FakeRuntime::new(Answer::Session { value: "s1" });
    let closed = Arc::clone(&runtime.closed);
    let target = CloseTarget::Pane("wA:p1".into());
    let frozen = identity(&runtime.observation(), &target);
    let replaced = FakeRuntime::new(Answer::Session { value: "s2" });
    let message =
        request_with(replaced, target, frozen).expect_err("a replaced occupant is refused");
    assert!(message.contains("changed"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn a_changed_tab_membership_is_refused() {
    // The frozen identity describes a tab with two member panes; the fresh
    // inventory reports only one. Even though the remaining member is
    // unmanaged, the tab the operator confirmed no longer exists.
    let runtime = FakeRuntime::new(Answer::Unmanaged);
    let closed = Arc::clone(&runtime.closed);
    let target = CloseTarget::Tab("wA:t1".into());
    let mut frozen_inventory = runtime.observation();
    frozen_inventory.panes.push(Pane {
        location: Location {
            workspace_id: "wA".into(),
            tab_id: "wA:t1".into(),
            pane_id: "wA:p2".into(),
        },
        label: None,
        title: None,
    });
    let frozen = identity(&frozen_inventory, &target);
    let message =
        request_with(runtime, target, frozen).expect_err("a changed member set is refused");
    assert!(message.contains("changed"), "{message}");
    assert!(closed.lock().expect("closed").is_empty());
}

#[test]
fn an_unchanged_identity_closes() {
    let runtime = FakeRuntime::new(Answer::Session { value: "s1" });
    let target = CloseTarget::Pane("wA:p1".into());
    let frozen = identity(&runtime.observation(), &target);
    let outcome = request_with(runtime, target, frozen);
    assert_eq!(outcome, Ok("closed pane wA:p1".to_string()));
}

#[test]
fn an_unchanged_unmanaged_tab_closes() {
    let runtime = FakeRuntime::new(Answer::Unmanaged);
    let target = CloseTarget::Tab("wA:t1".into());
    let frozen = identity(&runtime.observation(), &target);
    let outcome = request_with(runtime, target, frozen);
    assert_eq!(outcome, Ok("closed tab wA:t1".to_string()));
}

#[test]
fn a_blocked_close_does_not_delay_shutdown() {
    let runtime = FakeRuntime::blocked();
    let target = CloseTarget::Pane("wA:p1".into());
    let frozen = identity(&runtime.observation(), &target);
    let mut closer = Closer::new(runtime);
    closer.start(CloseRequest {
        target,
        identity: frozen,
    });
    thread::sleep(Duration::from_millis(50));

    let started = Instant::now();
    closer.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown waited on the close instead of cancelling it"
    );
    assert!(closer.poll().is_none(), "a cancelled close reports nothing");
}
