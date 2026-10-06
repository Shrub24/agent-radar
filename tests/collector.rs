//! Collector behaviour against an in-memory runtime.
//!
//! The collector's own job is the refresh schedule, the reconciliation
//! hand-off, the cancellation rules and the two facts it completes — assignment
//! ages and local `/proc` facts. The runtime's transport lives behind the
//! provider seam and is tested with a fake executable in `tests/runtime.rs`, so
//! everything here drives a small fake provider and a normalized observation:
//! no CLI, no wire fixture, no subprocess.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::model::{
    AgentObservation, CpuPercent, FleetObservation, ForegroundEvidence, HerdsmanFacts, LocalFacts,
    Location, Pane, ProcessResources, ProcessState, RuntimeStatus, SessionIdentity, Total,
};
use agent_radar::procfs::{Procs, Sampler};
use agent_radar::{
    CloseTarget, Collector, CollectorConfig, ObservationState, RetentionBasis, RuntimeProvider,
    SourceFreshness, Target,
};

/// An in-memory runtime: the test keeps the `Arc`, so it can change what the
/// next call answers and read back what was asked.
struct FakeRuntime {
    /// What `inventory` answers next.
    inventory: Mutex<Result<FleetObservation, String>>,
    /// What every foreground query answers.
    evidence: Mutex<ForegroundEvidence>,
    /// Every pane id a foreground query named, in order.
    queried: Mutex<Vec<String>>,
    /// A pause inside every call, so a slow or stalled runtime can be staged.
    delay: Mutex<Duration>,
    /// Calls in flight, the high-water mark, and inventories handed out.
    concurrent: AtomicUsize,
    peak: AtomicUsize,
    refreshes: AtomicUsize,
    /// Whether a call returned after seeing the cancellation flag.
    cancelled: AtomicBool,
}

impl Default for FakeRuntime {
    fn default() -> Self {
        Self {
            inventory: Mutex::new(Ok(inventory(&[], vec![]))),
            evidence: Mutex::new(ForegroundEvidence::Shell),
            queried: Mutex::new(Vec::new()),
            delay: Mutex::new(Duration::ZERO),
            concurrent: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            refreshes: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
        }
    }
}

impl FakeRuntime {
    /// Stands in for one runtime call: records overlap, waits out the staged
    /// delay while watching for cancellation, and notes a call that saw it.
    fn enter(&self, cancel: &AtomicBool) {
        let in_flight = self.concurrent.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(in_flight, Ordering::SeqCst);
        let deadline = Instant::now() + *self.delay.lock().expect("delay");
        while Instant::now() < deadline && !cancel.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(2));
        }
        if cancel.load(Ordering::SeqCst) {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        self.concurrent.fetch_sub(1, Ordering::SeqCst);
    }

    fn set_inventory(&self, inventory: FleetObservation) {
        *self.inventory.lock().expect("inventory") = Ok(inventory);
    }

    fn fail_inventory(&self, diagnostic: &str) {
        *self.inventory.lock().expect("inventory") = Err(diagnostic.to_string());
    }

    fn set_evidence(&self, evidence: ForegroundEvidence) {
        *self.evidence.lock().expect("evidence") = evidence;
    }
}

impl RuntimeProvider for FakeRuntime {
    fn inventory(&self, cancel: &AtomicBool) -> Result<FleetObservation, String> {
        self.enter(cancel);
        self.refreshes.fetch_add(1, Ordering::SeqCst);
        self.inventory.lock().expect("inventory").clone()
    }

    fn foreground_evidence(&self, pane_id: &str, cancel: &AtomicBool) -> ForegroundEvidence {
        self.enter(cancel);
        self.queried
            .lock()
            .expect("queried")
            .push(pane_id.to_string());
        self.evidence.lock().expect("evidence").clone()
    }

    fn focus(&self, _target: &Target, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the collector never focuses")
    }

    fn close(&self, _target: &CloseTarget, _cancel: &AtomicBool) -> Result<(), String> {
        unreachable!("the collector never closes anything")
    }
}

/// A collector reading the fake runtime on a fast poll.
fn collector_for(runtime: &Arc<FakeRuntime>) -> Collector {
    Collector::new(
        CollectorConfig {
            poll_interval: Duration::from_millis(10),
        },
        Arc::clone(runtime),
    )
}

/// A `/proc/<pid>/stat` line with the fields Radar samples, for a process
/// started by `parent`.
fn stat_line_under(parent: i32, cpu_ticks: u64, start_ticks: u64, rss_pages: i64) -> String {
    format!(
        "42 (pi) S {parent} 42 42 0 -1 4194560 100 0 0 0 0 {cpu_ticks} 3 4 20 0 3 0 \
         {start_ticks} 0 {rss_pages}"
    )
}

/// A `/proc/<pid>/stat` line for a process no root in a test claims.
fn stat_line(cpu_ticks: u64, start_ticks: u64, rss_pages: i64) -> String {
    stat_line_under(1, cpu_ticks, start_ticks, rss_pages)
}

/// A process table the test scripts, so metrics come from readings it set
/// rather than from this machine. The test keeps a handle to change what the
/// next sample finds.
#[derive(Clone)]
struct ScriptedProcs {
    readings: Arc<Mutex<HashMap<i32, String>>>,
}

impl ScriptedProcs {
    fn holding(pid: i32, stat: &str) -> Self {
        Self {
            readings: Arc::new(Mutex::new(HashMap::from([(pid, stat.to_string())]))),
        }
    }

    /// What the next sample of `pid` finds.
    fn set(&self, pid: i32, stat: &str) {
        self.readings
            .lock()
            .expect("readings")
            .insert(pid, stat.to_string());
    }
}

impl Procs for ScriptedProcs {
    fn boot_id(&mut self) -> Option<String> {
        Some("boot-1".to_string())
    }

    fn pids(&mut self, limit: usize, _cancel: &AtomicBool) -> Option<Vec<i32>> {
        let mut pids: Vec<i32> = self
            .readings
            .lock()
            .expect("readings")
            .keys()
            .copied()
            .collect();
        pids.sort_unstable();
        pids.truncate(limit);
        Some(pids)
    }

    fn stat(&mut self, pid: i32) -> Option<String> {
        self.readings.lock().expect("readings").get(&pid).cloned()
    }

    fn page_size(&mut self) -> Option<u64> {
        Some(4096)
    }
}

/// The root sample the state holds for `wA:p1`, if the collector took one.
fn sample(state: &ObservationState) -> Option<ProcessResources> {
    state.foreground("wA:p1")?.local().resources
}

/// Drives the collector the way the main loop does until `done` holds.
fn refresh_until(
    collector: &mut Collector,
    state: &mut ObservationState,
    mut done: impl FnMut(&ObservationState) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if done(state) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        if !collector.tick(state, false) {
            thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Waits until the runtime is inside a call, so cancellation has something to
/// abandon.
fn wait_for_call(runtime: &FakeRuntime) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if runtime.concurrent.load(Ordering::SeqCst) > 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

fn location(pane_id: &str) -> Location {
    Location {
        workspace_id: "wA".into(),
        tab_id: "wA:t1".into(),
        pane_id: pane_id.into(),
    }
}

/// An agent with a published assignment start, for the age the collector
/// measures.
fn agent(pane_id: &str, assigned_started_unix_ms: Option<u64>) -> AgentObservation {
    AgentObservation {
        location: location(pane_id),
        name: Some("pi".into()),
        label: Some("owner task".into()),
        status: Some(RuntimeStatus::Idle),
        session: Some(SessionIdentity::Reported {
            source: Some("herdr:pi".into()),
            value: "/home/dev/sessions/2026-09-01T10-00-00.jsonl".into(),
        }),
        lineage: None,
        facts: HerdsmanFacts {
            assigned_started_unix_ms,
            ..HerdsmanFacts::default()
        },
    }
}

fn inventory(pane_ids: &[&str], agents: Vec<AgentObservation>) -> FleetObservation {
    FleetObservation {
        workspaces: vec![],
        tabs: vec![],
        panes: pane_ids
            .iter()
            .map(|pane_id| Pane {
                location: location(pane_id),
                label: None,
                title: None,
            })
            .collect(),
        agents,
    }
}

/// A state holding one unverified retained association on `wA:p1`.
fn state_with_retention() -> ObservationState {
    let mut state = ObservationState::new();
    state.apply_success(inventory(&["wA:p1"], vec![agent("wA:p1", None)]));
    state.apply_success(inventory(&["wA:p1"], vec![]));
    assert_eq!(state.continuity_candidates(), vec!["wA:p1".to_string()]);
    state
}

#[test]
fn a_normalized_inventory_and_its_candidate_evidence_reach_the_reconciler() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    let mut state = state_with_retention();
    let mut collector = collector_for(&runtime);

    // Shell evidence confirms the retention through the collector path.
    assert!(
        refresh_until(&mut collector, &mut state, |state| state
            .retained()
            .get("wA:p1")
            .is_some_and(|entry| entry.basis == RetentionBasis::ShellForeground)),
        "shell foreground evidence should confirm the retention: {:?}",
        state.retained().get("wA:p1")
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);

    // The inventory reached the reconciler as normalized facts.
    let inventory = state.inventory().expect("inventory");
    assert_eq!(inventory.panes.len(), 1);

    // A non-shell replacement supersedes, leaving an ordinary pane.
    runtime.set_evidence(ForegroundEvidence::NonShell {
        pid: 200,
        name: Some("nvim".into()),
        command: Some("nvim".into()),
        local: LocalFacts::default(),
    });
    assert!(
        refresh_until(&mut collector, &mut state, |state| state
            .retained()
            .is_empty()),
        "non-shell foreground evidence should supersede the retention"
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
    assert!(
        state
            .inventory()
            .expect("inventory")
            .pane("wA:p1")
            .is_some()
    );

    // Only the continuity candidate was asked for foreground evidence.
    let queried = runtime.queried.lock().expect("queried");
    assert!(!queried.is_empty(), "the candidate was never queried");
    assert!(
        queried.iter().all(|pane_id| pane_id == "wA:p1"),
        "only continuity candidates may be queried, saw: {queried:?}"
    );
}

#[test]
fn assignment_age_is_measured_from_the_published_start() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(
        &["wA:p1", "wA:p2", "wA:p3"],
        vec![
            agent("wA:p1", Some(946_684_800_000)),
            agent("wA:p2", Some(4_102_444_800_000)),
            agent("wA:p3", None),
        ],
    ));
    let mut state = ObservationState::new();
    let mut collector = collector_for(&runtime);

    assert!(refresh_until(&mut collector, &mut state, |state| state
        .source_freshness()
        == &SourceFreshness::Current));
    let inventory = state.inventory().expect("inventory");

    // The age is measured from the published start, not presented as a
    // timestamp a reader would have to interpret against its own clock.
    let aged = inventory.agent_on_pane("wA:p1").expect("aged agent");
    let age = aged.facts.assigned_for.expect("an age");
    assert!(
        age > Duration::from_secs(60 * 60 * 24 * 365 * 20),
        "{age:?}"
    );

    // A start in the future is no age rather than a panic.
    let future = inventory.agent_on_pane("wA:p2").expect("future agent");
    assert_eq!(future.facts.assigned_for, Some(Duration::ZERO));

    // No published start: no age.
    let unassigned = inventory.agent_on_pane("wA:p3").expect("unassigned agent");
    assert_eq!(unassigned.facts.assigned_for, None);
}

#[test]
fn a_runtime_diagnostic_is_a_stale_source_that_keeps_the_last_good_inventory() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    let mut state = ObservationState::new();
    let mut collector = collector_for(&runtime);

    assert!(refresh_until(&mut collector, &mut state, |state| state
        .source_freshness()
        == &SourceFreshness::Current));
    assert_eq!(state.inventory().expect("inventory").panes.len(), 1);

    runtime.fail_inventory("herdr exited with status 3: cannot connect to the runtime");
    assert!(refresh_until(&mut collector, &mut state, |state| matches!(
        state.source_freshness(),
        SourceFreshness::Stale { .. }
    )));
    match state.source_freshness() {
        SourceFreshness::Stale { diagnostic } => {
            assert!(diagnostic.contains("status 3"), "{diagnostic}");
            assert!(
                diagnostic.contains("cannot connect to the runtime"),
                "{diagnostic}"
            );
        }
        other => panic!("expected a stale source, got {other:?}"),
    }
    // Failure is not disappearance: the last good inventory stands.
    assert_eq!(state.inventory().expect("last-good").panes.len(), 1);
}

#[test]
fn an_unreadable_inventory_is_unavailable_rather_than_an_empty_fleet() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.fail_inventory("herdr snapshot could not be read: not valid Herdr JSON");
    let mut state = ObservationState::new();
    let mut collector = collector_for(&runtime);

    assert!(refresh_until(&mut collector, &mut state, |state| matches!(
        state.source_freshness(),
        SourceFreshness::Unavailable { .. }
    )));
    match state.source_freshness() {
        SourceFreshness::Unavailable { diagnostic } => {
            assert!(diagnostic.contains("could not be read"), "{diagnostic}");
        }
        other => panic!("expected an unavailable source, got {other:?}"),
    }
    assert!(state.inventory().is_none());
}

#[test]
fn a_slow_runtime_never_blocks_the_loop() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    *runtime.delay.lock().expect("delay") = Duration::from_millis(400);
    let mut state = ObservationState::new();
    let mut collector = collector_for(&runtime);

    // Starting a refresh does not wait for the runtime.
    let started = Instant::now();
    assert!(!collector.tick(&mut state, false));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "tick waited for the runtime"
    );

    // The loop keeps running while the call is outstanding.
    let mut passes = 0;
    let finished = refresh_until(&mut collector, &mut state, |state| {
        passes += 1;
        state.source_freshness() == &SourceFreshness::Current
    });
    assert!(finished, "the refresh should have landed");
    assert!(passes > 3, "the main loop did not keep running: {passes}");
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "the refresh landed before it could have: {:?}",
        started.elapsed()
    );
}

#[test]
fn inconclusive_evidence_leaves_the_association_unverified_and_the_inventory_current() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    let mut state = state_with_retention();
    let mut collector = collector_for(&runtime);

    assert!(refresh_until(&mut collector, &mut state, |state| state
        .retained()
        .get("wA:p1")
        .is_some_and(
            |entry| entry.basis == RetentionBasis::ShellForeground
        )));

    // An answer with no usable foreground is not evidence of a replacement.
    runtime.set_evidence(ForegroundEvidence::Inconclusive);
    assert!(
        refresh_until(&mut collector, &mut state, |state| state
            .retained()
            .get("wA:p1")
            .is_some_and(|entry| entry.basis == RetentionBasis::Unverified)),
        "inconclusive evidence should leave the association unverified: {:?}",
        state.retained().get("wA:p1")
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
    assert_eq!(state.retained().len(), 1);
    assert_eq!(state.inventory().expect("inventory").panes.len(), 1);

    // Collection recovers on the next usable answer.
    runtime.set_evidence(ForegroundEvidence::Shell);
    assert!(refresh_until(&mut collector, &mut state, |state| state
        .retained()
        .get("wA:p1")
        .is_some_and(
            |entry| entry.basis == RetentionBasis::ShellForeground
        )));
}

#[test]
fn live_agent_panes_receive_foreground_evidence_in_the_agents_view() {
    let runtime = Arc::new(FakeRuntime::default());
    // Two live agents and one agent-less pane.
    let inventory = inventory(
        &["wA:p1", "wA:p2", "wA:p3"],
        vec![agent("wA:p1", None), agent("wA:p3", None)],
    );
    runtime.set_inventory(inventory.clone());
    // The state already holds the inventory, so the first refresh's candidates
    // are the live agent panes rather than an empty first pass.
    let mut state = ObservationState::new();
    state.apply_success(inventory);
    // A long poll interval so exactly one refresh runs: the query list is then
    // the candidate set of a single refresh, not an accumulation of them.
    let mut collector = Collector::new(
        CollectorConfig {
            poll_interval: Duration::from_secs(30),
        },
        Arc::clone(&runtime),
    );
    assert!(refresh_until(&mut collector, &mut state, |state| state
        .foreground("wA:p1")
        .is_some()
        && state.foreground("wA:p3").is_some()));

    let mut queried = runtime.queried.lock().expect("queried").clone();
    queried.sort();
    // Both agent panes were asked about, so each agent row has a live process
    // to compare; the agent-less pane was not, and no pane was asked twice.
    assert_eq!(queried, vec!["wA:p1".to_string(), "wA:p3".to_string()]);
    collector.shutdown();
}

#[test]
fn foreground_evidence_is_queried_only_for_continuity_candidates() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1", "wA:p2", "wA:p3"], vec![]));

    // Only wA:p2 ever hosted an agent, so only it is a continuity candidate.
    let mut state = ObservationState::new();
    state.apply_success(inventory(
        &["wA:p1", "wA:p2", "wA:p3"],
        vec![agent("wA:p2", None)],
    ));
    state.apply_success(inventory(&["wA:p1", "wA:p2", "wA:p3"], vec![]));
    assert_eq!(state.continuity_candidates(), vec!["wA:p2".to_string()]);

    let mut collector = collector_for(&runtime);
    assert!(refresh_until(&mut collector, &mut state, |state| state
        .retained()
        .get("wA:p2")
        .is_some_and(
            |entry| entry.basis == RetentionBasis::ShellForeground
        )));

    let queried = runtime.queried.lock().expect("queried");
    assert!(!queried.is_empty(), "the candidate was never queried");
    assert!(
        queried.iter().all(|pane_id| pane_id == "wA:p2"),
        "only continuity candidates may be queried, saw: {queried:?}"
    );
    let inventory = state.inventory().expect("inventory");
    assert_eq!(inventory.panes.len(), 3);
    assert_eq!(state.retained().len(), 1);
}

#[test]
fn refreshes_never_overlap() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    *runtime.delay.lock().expect("delay") = Duration::from_millis(100);
    let mut state = ObservationState::new();
    let mut collector = collector_for(&runtime);

    let deadline = Instant::now() + Duration::from_millis(700);
    while Instant::now() < deadline {
        collector.tick(&mut state, false);
        thread::sleep(Duration::from_millis(5));
    }
    collector.shutdown();

    assert_eq!(
        runtime.peak.load(Ordering::SeqCst),
        1,
        "two refreshes ran at once"
    );
    let refreshes = runtime.refreshes.load(Ordering::SeqCst);
    assert!(
        refreshes >= 2,
        "expected repeated refreshes, saw {refreshes}"
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
}

#[test]
fn shutdown_and_drop_abandon_a_stalled_runtime_without_waiting() {
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    // Far beyond the test: only cancellation ends this call.
    *runtime.delay.lock().expect("delay") = Duration::from_secs(30);

    let mut state = ObservationState::new();
    let mut collector = collector_for(&runtime);
    assert!(!collector.tick(&mut state, false));
    assert!(wait_for_call(&runtime), "the runtime call never started");

    let started = Instant::now();
    collector.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown waited {:?} for a stalled runtime",
        started.elapsed()
    );
    assert!(
        runtime.cancelled.load(Ordering::SeqCst),
        "shutdown did not cancel the call"
    );
    // Cancelled work is never applied, and a stopped collector does no more.
    assert_eq!(state.source_freshness(), &SourceFreshness::Pending);
    assert!(!collector.tick(&mut state, false));
    assert_eq!(state.source_freshness(), &SourceFreshness::Pending);

    // Dropping the collector shuts down too, so a forgotten shutdown leaks no
    // thread waiting on the runtime.
    let runtime = Arc::new(FakeRuntime::default());
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    *runtime.delay.lock().expect("delay") = Duration::from_secs(30);
    let mut collector = collector_for(&runtime);
    assert!(!collector.tick(&mut state, false));
    assert!(wait_for_call(&runtime), "the runtime call never started");
    drop(collector);
    assert!(
        runtime.cancelled.load(Ordering::SeqCst),
        "dropping the collector left the runtime call outstanding"
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Pending);
}

#[test]
fn a_live_pane_carries_a_root_sample_and_a_stale_or_retained_row_does_not() {
    let runtime = Arc::new(FakeRuntime::default());
    let procs = ScriptedProcs::holding(4242, &stat_line(0, 1_000, 8));
    let mut collector = Collector::sampling(
        CollectorConfig {
            poll_interval: Duration::from_millis(10),
        },
        Arc::clone(&runtime),
        Sampler::reading(procs.clone()),
    );
    let mut state = ObservationState::new();
    runtime.set_inventory(inventory(&["wA:p1"], vec![agent("wA:p1", None)]));
    runtime.set_evidence(ForegroundEvidence::command(
        4242,
        Some("pi".into()),
        Some("pi".into()),
    ));

    // The pane's own process is sampled: who it is and what it is doing, with
    // no CPU until a second reading of the same identity exists.
    assert!(refresh_until(&mut collector, &mut state, |state| sample(
        state
    )
    .is_some()));
    let first = sample(&state).expect("a sample");
    assert_eq!(first.identity.pid, 4242);
    assert_eq!(first.identity.boot_id, "boot-1");
    assert_eq!(first.identity.start_ticks, 1_000);
    assert_eq!(first.state, ProcessState::Sleeping);
    assert_eq!(first.rss_bytes, Some(8 * 4096));
    assert_eq!(first.cpu, None);

    // A later reading of the same incarnation measures the interval.
    procs.set(4242, &stat_line(1_000_000, 1_000, 8));
    assert!(
        refresh_until(&mut collector, &mut state, |state| sample(state)
            .and_then(|sample| sample.cpu)
            .is_some()),
        "the second sample should measure an interval"
    );

    // A refresh that fails samples nothing. The machine moves on while
    // collection is down, and neither its newer reading nor the last-good one
    // reaches a row as current: the source says it is stale.
    runtime.fail_inventory("herdr is down");
    assert!(refresh_until(&mut collector, &mut state, |state| matches!(
        state.source_freshness(),
        SourceFreshness::Stale { .. }
    )));
    let failed = runtime.refreshes.load(Ordering::SeqCst);
    procs.set(4242, &stat_line(2_000_000, 1_000, 16));
    assert!(refresh_until(&mut collector, &mut state, |_| {
        runtime.refreshes.load(Ordering::SeqCst) > failed + 2
    }));
    assert_eq!(
        sample(&state).expect("the last-good sample").rss_bytes,
        Some(8 * 4096),
        "a failed refresh must not sample the machine"
    );
    assert!(matches!(
        state.source_freshness(),
        SourceFreshness::Stale { .. }
    ));

    // A retained row is a pane whose agent stopped being reported. Its pane
    // reports a shell, and a shell is not a process to sample.
    runtime.set_inventory(inventory(&["wA:p1"], vec![]));
    runtime.set_evidence(ForegroundEvidence::Shell);
    assert!(refresh_until(&mut collector, &mut state, |state| {
        state.retained().contains_key("wA:p1")
            && matches!(state.foreground("wA:p1"), Some(ForegroundEvidence::Shell))
    }));
    assert_eq!(sample(&state), None);
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
}

#[test]
fn a_build_beneath_a_pane_reaches_the_row_as_a_measured_sum() {
    let runtime = Arc::new(FakeRuntime::default());
    let procs = ScriptedProcs::holding(4242, &stat_line(0, 1_000, 8));
    // A build the pane launched: a process of its own, beneath it.
    procs.set(4243, &stat_line_under(4242, 0, 2_000, 64));
    let mut collector = Collector::sampling(
        CollectorConfig {
            poll_interval: Duration::from_millis(10),
        },
        Arc::clone(&runtime),
        Sampler::reading(procs.clone()),
    );
    let mut state = ObservationState::new();
    runtime.set_inventory(inventory(&["wA:p1"], vec![agent("wA:p1", None)]));
    runtime.set_evidence(ForegroundEvidence::command(
        4242,
        Some("pi".into()),
        Some("pi".into()),
    ));

    // The pane is sampled and the build beneath it is summed from the same
    // refresh: it is not the pane's own resident set, and it has no interval
    // until a second reading of it exists.
    assert!(
        refresh_until(&mut collector, &mut state, |state| sample(state)
            .is_some_and(|sample| sample.descendants.observed == Some(1))),
        "the pane's sample should carry the build beneath it"
    );
    let first = sample(&state).expect("a sample");
    assert_eq!(first.rss_bytes, Some(8 * 4096));
    assert_eq!(first.descendants.rss_bytes, Total::Complete(64 * 4096));
    assert_eq!(
        first.descendants.cpu,
        Total::Partial(
            CpuPercent::from_hundredths(0),
            "no CPU sample could be compared with for 1 of them".to_string()
        )
    );

    // The build runs on: the next refresh measures its interval, and the total
    // stops being a lower bound.
    procs.set(4243, &stat_line_under(4242, 1_000_000, 2_000, 64));
    assert!(
        refresh_until(&mut collector, &mut state, |state| matches!(
            sample(state).map(|sample| sample.descendants.cpu),
            Some(Total::Complete(_))
        )),
        "the build's interval should be measured"
    );
}
