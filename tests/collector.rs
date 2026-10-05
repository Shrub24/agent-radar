#![cfg(unix)]
//! Fake-executable tests for the `herdr` collector.
//!
//! Every test stands a throwaway `#!/bin/sh` script in for `herdr` and drives
//! the collector the way the main loop does: `tick` until the reconciler sees
//! an outcome. The scripts cover the transport boundaries that matter — valid,
//! failing, malformed, oversized and stalled output, candidate-only
//! process-info queries, one refresh in flight, and cancellation.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::collector::{Collector, CollectorConfig};
use agent_radar::model::{
    AgentObservation, FleetObservation, Location, Pane, RuntimeStatus, SessionIdentity,
};
use agent_radar::{ObservationState, RetentionBasis, SourceFreshness};

/// A successful snapshot of one pane in one workspace, without agents.
const SNAPSHOT_ONE_PANE: &str = r#"{"id":"cli:api:snapshot","result":{"type":"snapshot","snapshot":{"workspaces":[{"workspace_id":"wA","label":"main","number":1}],"tabs":[{"tab_id":"wA:t1","workspace_id":"wA","label":"agent tab","number":1}],"panes":[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"}],"agents":[]}}}"#;

/// The same fleet with three panes and no agents.
const SNAPSHOT_THREE_PANES: &str = r#"{"id":"cli:api:snapshot","result":{"type":"snapshot","snapshot":{"workspaces":[{"workspace_id":"wA","label":"main","number":1}],"tabs":[{"tab_id":"wA:t1","workspace_id":"wA","number":1}],"panes":[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"},{"pane_id":"wA:p2","tab_id":"wA:t1","workspace_id":"wA"},{"pane_id":"wA:p3","tab_id":"wA:t1","workspace_id":"wA"}],"agents":[]}}}"#;

/// Three workers: an assignment started in 2000, one whose published start is
/// in 2100, and one with no assignment at all.
const SNAPSHOT_ASSIGNMENTS: &str = r#"{"id":"cli:api:snapshot","result":{"type":"snapshot","snapshot":{
  "workspaces":[{"workspace_id":"wA","label":"main","number":1}],
  "tabs":[{"tab_id":"wA:t1","workspace_id":"wA","number":1}],
  "panes":[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"},
           {"pane_id":"wA:p2","tab_id":"wA:t1","workspace_id":"wA"},
           {"pane_id":"wA:p3","tab_id":"wA:t1","workspace_id":"wA"}],
  "agents":[
    {"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA","agent_status":"working",
     "tokens":{"pi_herdsman_role":"worker","pi_herdsman_started":"946684800000"}},
    {"pane_id":"wA:p2","tab_id":"wA:t1","workspace_id":"wA","agent_status":"working",
     "tokens":{"pi_herdsman_role":"worker","pi_herdsman_started":"4102444800000"}},
    {"pane_id":"wA:p3","tab_id":"wA:t1","workspace_id":"wA","agent_status":"idle",
     "tokens":{"pi_herdsman_role":"worker"}}
  ]}}}"#;

/// The pane shell owns the foreground process group.
const SHELL_EVIDENCE: &str = r#"{"id":"cli:pane:process_info","result":{"process_info":{"pane_id":"wA:p1","shell_pid":100,"foreground_process_group_id":100,"foreground_processes":[{"pid":100,"name":"zsh","cmdline":"zsh"}]}}}"#;

/// A non-shell command owns the foreground process group.
const NON_SHELL_EVIDENCE: &str = r#"{"id":"cli:pane:process_info","result":{"process_info":{"pane_id":"wA:p1","shell_pid":100,"foreground_process_group_id":200,"foreground_processes":[{"pid":200,"name":"nvim","cmdline":"nvim"}]}}}"#;

/// A throwaway executable that stands in for `herdr`.
struct Fake {
    dir: PathBuf,
    executable: PathBuf,
}

impl Fake {
    /// Writes a `#!/bin/sh` executable whose body is built from its own
    /// directory, so scripts can keep state between invocations.
    fn new(body: impl FnOnce(&Path) -> String) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "radar-collector-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("create fake directory");
        let executable = dir.join("herdr");
        fs::write(&executable, format!("#!/bin/sh\n{}\n", body(&dir))).expect("write fake herdr");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("make it runnable");
        Self { dir, executable }
    }

    /// A path inside the fake's directory, for scripts that record state.
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    fn write(&self, name: &str, contents: &str) {
        fs::write(self.path(name), contents).expect("write fake state");
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).expect("read fake state")
    }

    fn config(&self, poll_interval: Duration, command_timeout: Duration) -> CollectorConfig {
        CollectorConfig {
            executable: self.executable.clone(),
            poll_interval,
            command_timeout,
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
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

/// Waits for the fake command to write its pid, and to finish writing it.
///
/// The shell creates the file before the redirect lands in it, so seeing the
/// file is not seeing the pid: a test that read as soon as it existed would
/// parse an empty string and fail on a machine that happened to be fast.
fn wait_for_pid_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if std::fs::read_to_string(path).is_ok_and(|text| !text.trim().is_empty()) {
            return true;
        }
        thread::sleep(Duration::from_millis(2));
    }
    false
}

/// Whether a pid still exists. A killed but unreaped child is a zombie and
/// still exists, so this also proves the child was reaped.
fn process_exists(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("run kill -0")
        .success()
}

fn location(pane_id: &str) -> Location {
    Location {
        workspace_id: "wA".into(),
        tab_id: "wA:t1".into(),
        pane_id: pane_id.into(),
    }
}

fn agent(pane_id: &str) -> AgentObservation {
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
        facts: agent_radar::model::HerdsmanFacts::default(),
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
    state.apply_success(inventory(&["wA:p1"], vec![agent("wA:p1")]));
    state.apply_success(inventory(&["wA:p1"], vec![]));
    assert_eq!(state.continuity_candidates(), vec!["wA:p1".to_string()]);
    state
}

#[test]
fn valid_snapshot_and_candidate_evidence_reach_the_reconciler() {
    let fake = Fake::new(|dir| {
        format!(
            r#"case "$1 $2" in
"api snapshot")
  printf '%s' '{snapshot}'
  ;;
"pane process-info")
  case "$(cat "{dir}/mode")" in
  non-shell) printf '%s' '{non_shell}' ;;
  *) printf '%s' '{shell}' ;;
  esac
  ;;
*)
  echo "unexpected subcommand: $1 $2" >&2
  exit 9
  ;;
esac"#,
            dir = dir.display(),
            snapshot = SNAPSHOT_ONE_PANE,
            shell = SHELL_EVIDENCE,
            non_shell = NON_SHELL_EVIDENCE,
        )
    });
    let mut state = state_with_retention();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

    // Shell PID evidence confirms the retention through the collector path.
    fake.write("mode", "shell");
    assert!(
        refresh_until(&mut collector, &mut state, |state| state
            .retained()
            .get("wA:p1")
            .is_some_and(|entry| entry.basis == RetentionBasis::ShellForeground)),
        "shell foreground evidence should confirm the retention: {:?}",
        state.retained().get("wA:p1")
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);

    // The snapshot reached the reconciler as normalized facts.
    let inventory = state.inventory().expect("inventory");
    assert_eq!(inventory.panes.len(), 1);
    assert!(inventory.agents.is_empty());
    assert_eq!(inventory.workspaces[0].label.as_deref(), Some("main"));

    // A non-shell replacement supersedes, leaving an ordinary pane.
    fake.write("mode", "non-shell");
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
}

#[test]
fn assignment_age_is_measured_from_the_published_start() {
    let fake = Fake::new(|_| format!("printf '%s' '{SNAPSHOT_ASSIGNMENTS}'"));
    let mut state = ObservationState::new();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

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
fn a_nonzero_exit_is_a_diagnostic_that_keeps_the_last_good_inventory() {
    let fake = Fake::new(|dir| {
        format!(
            r#"if [ "$(cat "{dir}/mode")" = "ok" ]; then
  printf '%s' '{snapshot}'
  exit 0
fi
echo "herdr: cannot connect to the runtime" >&2
exit 3"#,
            dir = dir.display(),
            snapshot = SNAPSHOT_ONE_PANE,
        )
    });
    fake.write("mode", "ok");
    let mut state = ObservationState::new();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

    assert!(refresh_until(&mut collector, &mut state, |state| state
        .source_freshness()
        == &SourceFreshness::Current));
    assert_eq!(state.inventory().expect("inventory").panes.len(), 1);

    fake.write("mode", "fail");
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
fn malformed_output_is_a_failure_not_an_empty_fleet() {
    let fake = Fake::new(|_| "printf '%s' 'not json at all'".to_string());
    let mut state = ObservationState::new();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

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
fn a_missing_executable_is_a_diagnostic_not_a_panic() {
    let fake = Fake::new(|_| "exit 0".to_string());
    let mut config = fake.config(Duration::from_millis(10), Duration::from_secs(2));
    config.executable = fake.path("herdr-not-installed");
    let mut state = ObservationState::new();
    let mut collector = Collector::new(config);

    assert!(refresh_until(&mut collector, &mut state, |state| matches!(
        state.source_freshness(),
        SourceFreshness::Unavailable { .. }
    )));
    match state.source_freshness() {
        SourceFreshness::Unavailable { diagnostic } => {
            assert!(diagnostic.contains("could not run"), "{diagnostic}");
        }
        other => panic!("expected an unavailable source, got {other:?}"),
    }
    assert!(state.inventory().is_none());
}

#[test]
fn a_stalled_command_times_out_without_blocking_the_loop() {
    let fake = Fake::new(|dir| {
        format!(
            "echo $$ > \"{dir}/pid\"\nexec sleep 30",
            dir = dir.display()
        )
    });
    let mut state = ObservationState::new();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_millis(150)));

    // Starting a refresh does not wait for `herdr`.
    let started = Instant::now();
    assert!(!collector.tick(&mut state, false));
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "tick waited for the command"
    );
    assert!(
        wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5)),
        "the fake command never started"
    );
    let pid: i32 = fake.read("pid").trim().parse().expect("fake pid");
    assert!(process_exists(pid), "the stalled command should be running");

    // The loop keeps running while the command stalls.
    let mut passes = 0;
    let killed = refresh_until(&mut collector, &mut state, |state| {
        passes += 1;
        matches!(
            state.source_freshness(),
            SourceFreshness::Unavailable { .. }
        )
    });
    assert!(
        killed,
        "the stalled command should have been killed at the timeout"
    );
    assert!(
        passes > 3,
        "the main loop did not keep running while the command stalled"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(120),
        "the timeout was not honoured"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the stalled command was waited out instead of killed"
    );
    match state.source_freshness() {
        SourceFreshness::Unavailable { diagnostic } => {
            assert!(diagnostic.contains("timed out"), "{diagnostic}");
        }
        other => panic!("expected an unavailable source, got {other:?}"),
    }
    assert!(state.inventory().is_none());
    assert!(
        !process_exists(pid),
        "the stalled command was not killed and reaped"
    );
}

#[test]
fn output_larger_than_the_pipe_buffer_is_drained_on_both_pipes() {
    let fake = Fake::new(|_| {
        r#"head -c 200000 /dev/zero | tr '\0' 'e' >&2
printf '{"id":"x","pad":"'
head -c 200000 /dev/zero | tr '\0' 'a'
printf '","result":{"type":"snapshot","snapshot":{"workspaces":[],"tabs":[],"panes":[],"agents":[]}}}'"#
            .to_string()
    });
    let mut state = ObservationState::new();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

    assert!(
        refresh_until(&mut collector, &mut state, |state| state.source_freshness()
            == &SourceFreshness::Current),
        "a command filling both pipes must be drained, not left to time out: {:?}",
        state.source_freshness()
    );
    assert!(state.inventory().expect("inventory").panes.is_empty());
}

#[test]
fn process_info_failures_are_inconclusive_and_never_discard_the_inventory() {
    let fake = Fake::new(|dir| {
        format!(
            r#"case "$1 $2" in
"api snapshot")
  printf '%s' '{snapshot}'
  ;;
"pane process-info")
  case "$(cat "{dir}/mode")" in
  fail)
    echo "herdr: no such pane" >&2
    exit 4
    ;;
  malformed)
    printf '%s' 'not json'
    ;;
  *)
    printf '%s' '{shell}'
    ;;
  esac
  ;;
*)
  exit 9
  ;;
esac"#,
            dir = dir.display(),
            snapshot = SNAPSHOT_ONE_PANE,
            shell = SHELL_EVIDENCE,
        )
    });
    let mut state = state_with_retention();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

    let basis_is = |wanted: RetentionBasis| {
        let wanted = wanted.clone();
        move |state: &ObservationState| {
            state
                .retained()
                .get("wA:p1")
                .is_some_and(|entry| entry.basis == wanted)
        }
    };

    fake.write("mode", "shell");
    assert!(refresh_until(
        &mut collector,
        &mut state,
        basis_is(RetentionBasis::ShellForeground)
    ));

    // A failing query is inconclusive evidence: the association survives, the
    // inventory stays current, and the row is explicitly unverified.
    fake.write("mode", "fail");
    assert!(
        refresh_until(
            &mut collector,
            &mut state,
            basis_is(RetentionBasis::Unverified)
        ),
        "a failed process-info should leave the association unverified: {:?}",
        state.retained().get("wA:p1")
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
    assert_eq!(state.retained().len(), 1);
    assert_eq!(state.inventory().expect("inventory").panes.len(), 1);

    // Collection recovers on the next successful query.
    fake.write("mode", "shell");
    assert!(refresh_until(
        &mut collector,
        &mut state,
        basis_is(RetentionBasis::ShellForeground)
    ));

    // An unreadable answer is inconclusive too.
    fake.write("mode", "malformed");
    assert!(refresh_until(
        &mut collector,
        &mut state,
        basis_is(RetentionBasis::Unverified)
    ));
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
    assert_eq!(state.retained().len(), 1);
    assert!(state.inventory().is_some());
}

#[test]
fn process_info_is_queried_only_for_continuity_candidates() {
    let fake = Fake::new(|dir| {
        format!(
            r#"case "$1 $2" in
"api snapshot")
  printf '%s' '{snapshot}'
  ;;
"pane process-info")
  echo "$4" >> "{dir}/queried"
  printf '%s' '{shell}'
  ;;
*)
  exit 9
  ;;
esac"#,
            dir = dir.display(),
            snapshot = SNAPSHOT_THREE_PANES,
            shell = SHELL_EVIDENCE,
        )
    });

    // Only wA:p2 ever hosted an agent, so only it is a continuity candidate.
    let mut state = ObservationState::new();
    state.apply_success(inventory(
        &["wA:p1", "wA:p2", "wA:p3"],
        vec![agent("wA:p2")],
    ));
    state.apply_success(inventory(&["wA:p1", "wA:p2", "wA:p3"], vec![]));
    assert_eq!(state.continuity_candidates(), vec!["wA:p2".to_string()]);

    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));
    assert!(refresh_until(&mut collector, &mut state, |state| state
        .retained()
        .get("wA:p2")
        .is_some_and(
            |entry| entry.basis == RetentionBasis::ShellForeground
        )));

    let queried = fake.read("queried");
    assert!(!queried.is_empty(), "the candidate was never queried");
    assert!(
        queried.lines().all(|line| line == "wA:p2"),
        "only continuity candidates may be queried, saw: {queried}"
    );
    let inventory = state.inventory().expect("inventory");
    assert_eq!(inventory.panes.len(), 3);
    assert_eq!(state.retained().len(), 1);
}

#[test]
fn refreshes_never_overlap() {
    let fake = Fake::new(|dir| {
        format!(
            r#"if [ -e "{dir}/lock" ]; then
  echo overlap >> "{dir}/overlap"
fi
: > "{dir}/lock"
echo start >> "{dir}/starts"
sleep 0.2
rm -f "{dir}/lock"
printf '%s' '{snapshot}'"#,
            dir = dir.display(),
            snapshot = SNAPSHOT_ONE_PANE,
        )
    });
    let mut state = ObservationState::new();
    let mut collector =
        Collector::new(fake.config(Duration::from_millis(10), Duration::from_secs(2)));

    let deadline = Instant::now() + Duration::from_millis(900);
    while Instant::now() < deadline {
        collector.tick(&mut state, false);
        thread::sleep(Duration::from_millis(5));
    }
    collector.shutdown();

    assert!(!fake.path("overlap").exists(), "two refreshes ran at once");
    let starts = fake.read("starts").lines().count();
    assert!(starts >= 2, "expected repeated refreshes, saw {starts}");
    assert_eq!(state.source_freshness(), &SourceFreshness::Current);
}

#[test]
fn shutdown_and_drop_kill_and_reap_a_stalled_command() {
    let stalled = || {
        Fake::new(|dir| {
            format!(
                "echo $$ > \"{dir}/pid\"\nexec sleep 60",
                dir = dir.display()
            )
        })
    };
    // The command timeout is far beyond the test: only cancellation ends this.
    let timeout = Duration::from_secs(30);

    let fake = stalled();
    let mut state = ObservationState::new();
    let mut collector = Collector::new(fake.config(Duration::from_millis(10), timeout));
    assert!(!collector.tick(&mut state, false));
    assert!(wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5)));
    let pid: i32 = fake.read("pid").trim().parse().expect("fake pid");
    assert!(process_exists(pid), "the stalled command should be running");

    let started = Instant::now();
    collector.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown waited {:?} for a stalled command",
        started.elapsed()
    );
    assert!(
        !process_exists(pid),
        "shutdown did not kill and reap the command"
    );
    // Cancelled work is never applied, and a stopped collector does no more.
    assert_eq!(state.source_freshness(), &SourceFreshness::Pending);
    assert!(!collector.tick(&mut state, false));
    assert_eq!(state.source_freshness(), &SourceFreshness::Pending);

    // Dropping the collector shuts down too, so a forgotten shutdown leaks
    // neither a child nor a zombie.
    let fake = stalled();
    let mut state = ObservationState::new();
    let mut collector = Collector::new(fake.config(Duration::from_millis(10), timeout));
    assert!(!collector.tick(&mut state, false));
    assert!(wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5)));
    let pid: i32 = fake.read("pid").trim().parse().expect("fake pid");
    drop(collector);
    assert!(
        !process_exists(pid),
        "dropping the collector left the command running"
    );
    assert_eq!(state.source_freshness(), &SourceFreshness::Pending);
}
