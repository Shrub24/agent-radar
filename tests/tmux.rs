#![cfg(unix)]
//! The tmux adapter: its transport against a stand-in `tmux`, and its effects
//! against a disposable tmux server.
//!
//! Every command the adapter runs names one server. The transport tests run a
//! checked-in `#!/bin/sh` script in place of `tmux` and read the log it keeps, so
//! the claim that no command can reach tmux's default server is checked on the
//! real command line and needs no tmux at all. The effect tests start a server
//! under a name of their own and end it however the test ends; they skip, loudly,
//! on a machine without tmux.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::model::ForegroundEvidence;
use agent_radar::runtime::{
    CloseOutcome, CreateOutcome, CreateRequest, FocusOutcome, InputOutcome, InputPayload,
    InputRequest, LaunchOutcome, LaunchRequest, OutputOutcome, OutputRequest, OutputSource,
    PaneExitObservation, RuntimeProvider, SplitDirection,
};
use agent_radar::{CloseTarget, Target, TmuxConfig, TmuxRuntime};

/// The longest any of these tests waits for something to happen.
const DEADLINE: Duration = Duration::from_secs(20);

/// Stand-ins for `tmux`, checked in so that no test writes an executable and
/// runs it in the same breath: a file that young can refuse to be run at all.
const FAILING: &str = "tests/fixtures/tmux-fake.sh";
const STALLING: &str = "tests/fixtures/tmux-stall.sh";
const LAUNCH_FAILING: &str = "tests/fixtures/tmux-launch-fail.sh";
const ESCAPING: &str = "tests/fixtures/tmux-escaped.sh";

fn cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// A number no other test in this binary uses.
fn unique() -> usize {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Where a stand-in keeps its state, under the same directory rules its own
/// shell applies.
fn state_path(script: &str, server: &str) -> PathBuf {
    let directory = std::env::var_os("TMPDIR")
        .filter(|directory| !directory.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    directory.join(format!("{script}-{}-{server}", std::process::id()))
}

/// One test's stand-in: a runtime pointed at a checked-in script, and the state
/// file that script keeps for the server name this test chose.
struct Fake {
    runtime: TmuxRuntime,
    server: String,
    state: PathBuf,
}

impl Fake {
    /// The stand-in that fails every command, recording what it was asked to do.
    fn failing() -> Self {
        Self::new("tmux-fake", FAILING, Duration::from_secs(5))
    }

    /// The stand-in that never answers.
    fn stalling(command_timeout: Duration) -> Self {
        Self::new("tmux-stall", STALLING, command_timeout)
    }

    fn new(script: &str, fixture: &str, command_timeout: Duration) -> Self {
        let executable = Path::new(env!("CARGO_MANIFEST_DIR")).join(fixture);
        // These stand-ins are run, not read: the mode is part of what they are,
        // and a checkout that lost it would look like an adapter that cannot run
        // tmux at all. The test tree is not written to here, so a source tree
        // mounted read-only still runs these tests.
        assert!(
            executable
                .metadata()
                .expect("the stand-in")
                .permissions()
                .mode()
                & 0o111
                != 0,
            "{} is not executable",
            executable.display()
        );
        let server = format!("probe-{}", unique());
        let state = state_path(script, &server);
        let _ = fs::remove_file(&state);
        Self {
            runtime: TmuxRuntime::new(TmuxConfig {
                executable,
                server: server.clone(),
                command_timeout,
            })
            .expect("a usable server name"),
            server,
            state,
        }
    }

    /// A handle this test's runtime would name, for a tmux id.
    fn handle(&self, id: &str) -> String {
        format!("tmux:{}:{id}", self.server)
    }

    /// One line per invocation: the client's `TMUX`, then the arguments.
    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.state)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The pid the stalling stand-in recorded, once it has recorded one.
    fn stalled_pid(&self) -> Option<i32> {
        fs::read_to_string(&self.state).ok()?.trim().parse().ok()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.state);
    }
}

/// Drives every operation the adapter implements against a failing command.
///
/// The point is not the outcomes — a failing command makes them all failures —
/// but that the adapter reaches one command per operation, each naming the
/// configured server and carrying no client environment.
fn drive(fake: &Fake) {
    let runtime = &fake.runtime;
    let cancel = cancel();
    let pane = fake.handle("%0");
    let workspace = fake.handle("$0");
    let _ = runtime.inventory(&cancel);
    let _ = runtime.foreground_evidence(&pane, &cancel);
    let _ = runtime.focus_outcome(&Target::Pane(pane.clone()), &cancel);
    let _ = runtime.focus_outcome(&Target::Workspace(workspace.clone()), &cancel);
    let _ = runtime.close_outcome(&CloseTarget::Pane(pane.clone()), &cancel);
    let _ = runtime.close_outcome(&CloseTarget::Tab(fake.handle("@0")), &cancel);
    let _ = runtime.create(
        &CreateRequest::Workspace {
            focus: Some(false),
            directory: None,
        },
        &cancel,
    );
    let _ = runtime.create(
        &CreateRequest::Tab {
            workspace_id: workspace.clone(),
            focus: Some(false),
        },
        &cancel,
    );
    let _ = runtime.create(
        &CreateRequest::PaneSplit {
            pane_id: pane.clone(),
            direction: SplitDirection::Right,
            focus: Some(false),
        },
        &cancel,
    );
    let _ = runtime.input(
        &InputRequest {
            pane_id: pane.clone(),
            payload: InputPayload::Text {
                text: "hello".to_string(),
            },
        },
        &cancel,
    );
    let _ = runtime.input(
        &InputRequest {
            pane_id: pane.clone(),
            payload: InputPayload::Keys {
                keys: vec!["enter".to_string()],
            },
        },
        &cancel,
    );
    let _ = runtime.output(
        &OutputRequest {
            pane_id: pane,
            source: OutputSource::Visible,
            lines: None,
            ansi: false,
        },
        &cancel,
    );
}

#[test]
fn every_command_names_the_server_and_carries_no_client_environment() {
    let fake = Fake::failing();
    drive(&fake);

    let calls = fake.calls();
    assert!(calls.len() >= 12, "one command per operation: {calls:?}");
    for call in &calls {
        let (client, arguments) = call.split_once('|').expect("a recorded call");
        assert_eq!(
            client, "<unset>",
            "a command inherited a client's TMUX: {call:?}"
        );
        assert!(
            arguments.starts_with(&format!("-L {} ", fake.server)),
            "a command did not name the configured server: {call:?}"
        );
    }
    // The observation reads the server before anything else can be said about
    // it, and a literal input is one command carrying the text as an argument.
    assert!(
        calls.iter().any(|call| call.contains("list-sessions -F")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.contains("send-keys -t %0 -l -- hello")),
        "{calls:?}"
    );
}

#[test]
fn launch_refuses_before_dispatch_when_remain_on_exit_cannot_be_armed() {
    let fake = Fake::failing();
    let request = LaunchRequest {
        pane_id: fake.handle("%0"),
        executable: "/bin/sh".to_string(),
        argv: vec!["-c".to_string(), "exit 0".to_string()],
        spawn_token: Some("private-token-sentinel".to_string()),
        cwd: None,
        env: BTreeMap::new(),
    };
    assert!(matches!(
        fake.runtime.launch(&request, &cancel()),
        LaunchOutcome::Refused(_)
    ));
    let calls = fake.calls();
    assert_eq!(calls.len(), 1, "launch command was dispatched: {calls:?}");
    assert!(
        calls[0].contains("set-option -p"),
        "preflight was not attempted: {calls:?}"
    );
    assert!(!calls[0].contains("private-token-sentinel"));
}

#[test]
fn launch_failure_diagnostics_never_echo_the_private_token_or_environment() {
    let fake = Fake::new("tmux-launch-fail", LAUNCH_FAILING, Duration::from_secs(5));
    let request = LaunchRequest {
        pane_id: fake.handle("%0"),
        executable: "/bin/sh".to_string(),
        argv: vec!["-c".to_string(), "exit 0".to_string()],
        spawn_token: Some("private-token-sentinel".to_string()),
        cwd: Some("/tmp".to_string()),
        env: BTreeMap::from([(
            "PRIVATE_VALUE".to_string(),
            "private-value-sentinel".to_string(),
        )]),
    };
    let outcome = fake.runtime.launch(&request, &cancel());
    let LaunchOutcome::Unknown(diagnostic) = outcome else {
        panic!("expected unknown launch result, got {outcome:?}")
    };
    assert!(!diagnostic.contains("private-token-sentinel"));
    assert!(!diagnostic.contains("private-value-sentinel"));
}

#[test]
fn a_handle_of_another_backend_or_server_runs_nothing() {
    let fake = Fake::failing();
    let runtime = &fake.runtime;
    let cancel = cancel();
    let foreign_pane = "wA:p1".to_string();
    let window_handle = fake.handle("@1");
    let foreign_workspace = "tmux:other:$0".to_string();

    assert!(
        matches!(
            runtime.focus_outcome(&Target::Pane(foreign_pane.clone()), &cancel),
            FocusOutcome::Refused(_)
        ),
        "another backend's pane handle was not refused"
    );
    assert!(
        matches!(
            runtime.focus_outcome(&Target::Workspace(foreign_workspace), &cancel),
            FocusOutcome::Refused(_)
        ),
        "another server's session handle was not refused"
    );
    assert!(
        matches!(
            runtime.close_outcome(&CloseTarget::Pane(foreign_pane), &cancel),
            CloseOutcome::Refused(_)
        ),
        "another backend's close target was not refused"
    );
    assert!(
        matches!(
            runtime.input(
                &InputRequest {
                    pane_id: window_handle,
                    payload: InputPayload::Keys {
                        keys: vec!["enter".to_string()],
                    },
                },
                &cancel,
            ),
            InputOutcome::Refused(_)
        ),
        "a window handle was accepted as a pane"
    );
    assert!(
        matches!(
            runtime.output(
                &OutputRequest {
                    pane_id: "other:pane".to_string(),
                    source: OutputSource::Visible,
                    lines: None,
                    ansi: false,
                },
                &cancel,
            ),
            OutputOutcome::Refused(_)
        ),
        "a pane of no server was accepted"
    );
    assert!(
        fake.calls().is_empty(),
        "a refused handle still ran a command: {:?}",
        fake.calls()
    );
}

#[test]
fn a_command_that_failed_after_dispatch_is_unknown_never_refused() {
    let fake = Fake::failing();
    let runtime = &fake.runtime;
    let cancel = cancel();
    let pane = fake.handle("%0");

    let focus = runtime.focus_outcome(&Target::Pane(pane.clone()), &cancel);
    assert!(
        matches!(focus, FocusOutcome::Unknown(_)),
        "a dispatched focus failure was not unknown: {focus:?}"
    );
    let close = runtime.close_outcome(&CloseTarget::Pane(pane.clone()), &cancel);
    assert!(
        matches!(close, CloseOutcome::Unknown(_)),
        "a dispatched close failure was not unknown: {close:?}"
    );
    let input = runtime.input(
        &InputRequest {
            pane_id: pane.clone(),
            payload: InputPayload::Text {
                text: "hello".to_string(),
            },
        },
        &cancel,
    );
    assert!(
        matches!(input, InputOutcome::Unknown(_)),
        "a dispatched input failure was not unknown: {input:?}"
    );
    let output = runtime.output(
        &OutputRequest {
            pane_id: pane,
            source: OutputSource::Visible,
            lines: None,
            ansi: false,
        },
        &cancel,
    );
    assert!(
        matches!(output, OutputOutcome::Unknown(_)),
        "a dispatched read failure was not unknown: {output:?}"
    );
    let created = runtime.create(
        &CreateRequest::Workspace {
            focus: Some(false),
            directory: None,
        },
        &cancel,
    );
    assert!(
        matches!(created, CreateOutcome::Unknown(_)),
        "a dispatched creation failure was not unknown: {created:?}"
    );
}

#[test]
fn a_workspace_starts_in_the_named_directory_and_refuses_one_that_does_not_exist() {
    let Some(probe) = Probe::start("workspace directory") else {
        return;
    };
    let directory = std::env::temp_dir()
        .canonicalize()
        .expect("a temp dir")
        .join(format!("radar-dir-{}-{}", std::process::id(), unique()));
    std::fs::create_dir_all(&directory).expect("a directory");
    let cancel = AtomicBool::new(false);
    let request = |directory: &str| CreateRequest::Workspace {
        focus: Some(false),
        directory: Some(directory.to_string()),
    };
    let created = probe
        .runtime
        .create(&request(directory.to_str().unwrap()), &cancel);
    let CreateOutcome::Completed(location) = created else {
        panic!("a workspace in an existing directory was not created: {created:?}");
    };
    assert!(location.workspace_id.is_some());
    let path = probe.tmux(&["display-message", "-p", "-t", "$1", "#{session_path}"]);
    assert_eq!(path.trim(), directory.to_str().unwrap());

    // A missing or relative directory is refused before tmux creates anything.
    let sessions = probe.ids(&["list-sessions", "-F", "#{session_id}"]).len();
    for bad in [directory.join("missing"), PathBuf::from("relative")] {
        let refused = probe
            .runtime
            .create(&request(bad.to_str().unwrap()), &cancel);
        assert!(matches!(refused, CreateOutcome::Refused(_)), "{refused:?}");
    }
    assert_eq!(
        probe.ids(&["list-sessions", "-F", "#{session_id}"]).len(),
        sessions
    );
    let _ = std::fs::remove_dir_all(&directory);
}

#[test]
fn a_stalled_command_is_killed_and_reaped_at_its_deadline() {
    let fake = Fake::stalling(Duration::from_millis(300));
    let started = Instant::now();
    let failure = fake
        .runtime
        .inventory(&cancel())
        .expect_err("a stalled command answers with a failure");
    assert!(
        failure.contains("timed out"),
        "the deadline was not what ended the command: {failure}"
    );
    assert!(
        started.elapsed() < DEADLINE,
        "the deadline took longer than the test's own bound"
    );

    assert!(
        eventually(DEADLINE, || fake.stalled_pid().is_some()),
        "the stand-in never recorded its pid"
    );
    let pid = fake.stalled_pid().expect("a recorded pid");
    assert!(
        eventually(DEADLINE, || !process_exists(pid)),
        "the stalled command was left behind"
    );
}

#[test]
fn a_cancelled_command_returns_without_waiting_out_its_deadline() {
    let fake = Fake::stalling(Duration::from_secs(30));
    let cancelled = AtomicBool::new(true);
    let started = Instant::now();
    let result = fake.runtime.inventory(&cancelled);
    assert!(result.is_err(), "{result:?}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "cancellation waited out the command deadline"
    );
}

/// tmux 3.5a writes the separator as its `\ooo` escape, so every record the
/// adapter asks for arrives with the separator as the literal text `\037`
/// where tmux 3.7c writes the byte — measured on Debian trixie's tmux 3.5a
/// versus the tmux 3.7c the rest of these tests run against. It is the same
/// record either way, and every read must decode it the same way.
#[test]
fn records_with_tmux_3_5as_escaped_separator_decode_the_same_way() {
    let fake = Fake::new("tmux-escaped", ESCAPING, Duration::from_secs(5));
    let runtime = &fake.runtime;
    let cancel = cancel();
    let pane = fake.handle("%1");

    let observation = runtime
        .inventory(&cancel)
        .expect("an inventory of escaped records");
    let workspace = fake.handle("$0");
    assert_eq!(observation.workspaces.len(), 1, "{observation:?}");
    assert_eq!(observation.workspaces[0].workspace_id, workspace);
    assert_eq!(observation.workspaces[0].label.as_deref(), Some("herdsman"));
    assert_eq!(
        observation.workspaces[0].session_path.as_deref(),
        Some("/tmp/herdr")
    );
    assert_eq!(observation.tabs.len(), 1, "{observation:?}");
    assert_eq!(observation.tabs[0].tab_id, fake.handle("@1"));
    assert_eq!(observation.tabs[0].workspace_id, workspace);
    assert_eq!(observation.tabs[0].number, Some(2));
    assert_eq!(observation.tabs[0].label.as_deref(), Some("editor"));
    assert_eq!(observation.panes.len(), 1, "{observation:?}");
    assert_eq!(observation.panes[0].location.pane_id, pane);
    assert_eq!(observation.panes[0].location.tab_id, fake.handle("@1"));
    assert_eq!(observation.panes[0].location.workspace_id, workspace);
    assert_eq!(observation.panes[0].title.as_deref(), Some("a title"));
    assert_eq!(
        observation.panes[0].current_path.as_deref(),
        Some("/tmp/herdr")
    );

    assert_eq!(
        runtime.foreground_evidence(&pane, &cancel),
        ForegroundEvidence::command(4242, Some("pi".to_string()), None)
    );

    let exits = runtime.exit_observations(std::slice::from_ref(&pane), &cancel);
    assert_eq!(
        exits.get(&pane),
        Some(&PaneExitObservation::Exited {
            code: Some(0),
            signal: None,
        })
    );

    let tab = fake.handle("@2");
    let created = runtime.create(
        &CreateRequest::Tab {
            workspace_id: workspace.clone(),
            focus: Some(false),
        },
        &cancel,
    );
    let CreateOutcome::Completed(created) = created else {
        panic!("a tab was not created: {created:?}")
    };
    assert_eq!(created.tab_id.as_deref(), Some(tab.as_str()));
    assert_eq!(created.workspace_id.as_deref(), Some(workspace.as_str()));
    // A tab is made with a pane, and tmux names it: a launch into a new tab runs
    // in the pane that tab opened.
    assert_eq!(created.pane_id.as_deref(), Some(fake.handle("%4").as_str()));

    let split_pane = fake.handle("%3");
    let created = runtime.create(
        &CreateRequest::PaneSplit {
            pane_id: pane,
            direction: SplitDirection::Right,
            focus: Some(false),
        },
        &cancel,
    );
    let CreateOutcome::Completed(created) = created else {
        panic!("a pane was not created: {created:?}")
    };
    assert_eq!(created.pane_id.as_deref(), Some(split_pane.as_str()));
    assert_eq!(created.tab_id.as_deref(), Some(tab.as_str()));
    assert_eq!(created.workspace_id.as_deref(), Some(workspace.as_str()));
}

/// The text of one visible read, or `None` when the read did not complete.
fn visible(runtime: &TmuxRuntime, pane: &str) -> Option<String> {
    match runtime.output(
        &OutputRequest {
            pane_id: pane.to_string(),
            source: OutputSource::Visible,
            lines: None,
            ansi: false,
        },
        &cancel(),
    ) {
        OutputOutcome::Completed(read) => Some(read.text),
        _ => None,
    }
}

/// A client attached to a probe's server, detached when dropped.
struct AttachedClient {
    child: std::process::Child,
    _stdin: Option<std::process::ChildStdin>,
}

impl Drop for AttachedClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A disposable tmux server, created and ended by the test that uses it.
struct Probe {
    name: String,
    socket: PathBuf,
    runtime: TmuxRuntime,
}

impl Probe {
    /// Starts one on a name no other server uses, or reports `None` on a
    /// machine without tmux.
    fn start(label: &str) -> Option<Self> {
        if !has_tmux() {
            eprintln!("skipping {label}: tmux is not installed");
            return None;
        }
        let name = format!("radar-test-{}-{}", std::process::id(), unique());
        let socket = socket_dir()?.join(&name);
        let probe = Self {
            name: name.clone(),
            socket,
            runtime: TmuxRuntime::new(TmuxConfig::for_server(name)).expect("a usable name"),
        };
        probe.tmux(&["new-session", "-d", "-s", "main", "-x", "100", "-y", "30"]);
        Some(probe)
    }

    /// Runs one tmux command against this probe's server, returning its stdout.
    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .args(["-f", "/dev/null", "-L"])
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

    /// The first field of each record one `-F` read answers with.
    fn ids(&self, args: &[&str]) -> Vec<String> {
        self.tmux(args)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| line.split(' ').next().unwrap_or_default().to_string())
            .collect()
    }

    /// The session's first pane, as the adapter names it.
    fn first_pane(&self) -> String {
        let id = self.ids(&["list-panes", "-a", "-F", "#{pane_id}"])[0].clone();
        self.runtime_handle(&id)
    }

    fn runtime_handle(&self, id: &str) -> String {
        format!("tmux:{}:{id}", self.name)
    }

    /// Attaches a control-mode client, which tmux counts like any other, and
    /// keeps it attached until the returned child is dropped.
    fn attach_client(&self) -> AttachedClient {
        let mut child = Command::new("tmux")
            .args(["-f", "/dev/null", "-L"])
            .arg(&self.name)
            .env_remove("TMUX")
            .args(["-C", "attach-session", "-t", "main"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("attach a control client");
        let stdin = child.stdin.take();
        // Owned by the guard from the start, so every path reaps it.
        let client = AttachedClient {
            child,
            _stdin: stdin,
        };
        for _ in 0..100 {
            if !self.tmux(&["list-clients"]).trim().is_empty() {
                return client;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the control client never attached");
    }

    /// Ends the server, whether the test passed or not.
    fn kill(&self) {
        let _ = Command::new("tmux")
            .args(["-f", "/dev/null", "-L"])
            .arg(&self.name)
            .env_remove("TMUX")
            .args(["kill-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.kill();
        // A server that exited leaves its socket behind; only this name's own
        // file is ever removed.
        let _ = fs::remove_file(&self.socket);
    }
}

fn has_tmux() -> bool {
    Command::new("tmux")
        .arg("-V")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Where tmux keeps its sockets: `$TMUX_TMPDIR/tmux-<uid>`, else `/tmp/tmux-<uid>`.
fn socket_dir() -> Option<PathBuf> {
    let parent = match std::env::var_os("TMUX_TMPDIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from("/tmp"),
    };
    let uid = rustix::process::getuid().as_raw();
    Some(parent.join(format!("tmux-{uid}")))
}

/// Whether the default server's socket is there, and what it looked like.
fn default_socket_state() -> (bool, u64, u64) {
    let Some(path) = socket_dir().map(|dir| dir.join("default")) else {
        return (false, 0, 0);
    };
    match fs::metadata(&path) {
        Ok(metadata) => {
            use std::os::unix::fs::MetadataExt;
            (true, metadata.mtime() as u64, metadata.ino())
        }
        Err(_) => (false, 0, 0),
    }
}

/// Waits, boundedly, for a file to hold something.
fn wait_for_text(path: &Path, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(text) = fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return Some(text);
        }
        thread::sleep(Duration::from_millis(5));
    }
    None
}

/// Waits, boundedly, for a condition the server applies asynchronously.
fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Whether a pid still exists. A killed but unreaped child is a zombie and still
/// exists, so this also proves the child was reaped.
fn process_exists(pid: i32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .expect("run kill -0")
        .success()
}

/// The daemon's server starts with no session, and `list-windows -a` and
/// `list-panes -a` fail with "no current target" there, so a naive read would
/// report the server unreachable when it is simply empty.
#[test]
fn an_inventory_of_a_server_with_no_session_is_an_empty_fleet_not_a_failure() {
    let Some(probe) =
        Probe::start("an_inventory_of_a_server_with_no_session_is_an_empty_fleet_not_a_failure")
    else {
        return;
    };
    probe.tmux(&["set-option", "-s", "exit-empty", "off"]);
    probe.tmux(&["kill-session", "-t", "main"]);
    assert!(
        probe
            .ids(&["list-sessions", "-F", "#{session_id}"])
            .is_empty(),
        "the server must still be up with no session"
    );

    let inventory = probe
        .runtime
        .inventory(&cancel())
        .expect("an empty server is an empty fleet");
    assert!(inventory.workspaces.is_empty());
    assert!(inventory.tabs.is_empty());
    assert!(inventory.panes.is_empty());
}

#[test]
fn an_inventory_lists_the_server_with_handles_that_survive_reordering() {
    let Some(probe) =
        Probe::start("an_inventory_lists_the_server_with_handles_that_survive_reordering")
    else {
        return;
    };
    let second = probe.tmux(&["new-window", "-d", "-t", "$0:", "-P", "-F", "#{window_id}"]);
    let second = second.trim().to_string();
    probe.tmux(&["split-window", "-d", "-t", &second]);
    let pane_ids = probe.ids(&["list-panes", "-a", "-F", "#{pane_id}"]);
    assert_eq!(pane_ids.len(), 3, "{pane_ids:?}");

    let before = probe.runtime.inventory(&cancel()).expect("an inventory");
    assert_eq!(before.workspaces.len(), 1);
    assert_eq!(before.tabs.len(), 2);
    assert_eq!(before.panes.len(), 3);
    assert!(before.agents.is_empty(), "tmux reports no agents");

    let workspace = &before.workspaces[0];
    assert_eq!(workspace.workspace_id, probe.runtime_handle("$0"));
    assert_eq!(workspace.label.as_deref(), Some("main"));
    let opened = before
        .tabs
        .iter()
        .find(|tab| tab.tab_id == probe.runtime_handle(&second))
        .expect("the window this test opened");
    assert_eq!(opened.workspace_id, workspace.workspace_id);
    assert!(opened.number.is_some(), "a window index is display text");
    for pane in &before.panes {
        assert!(
            pane.location
                .tab_id
                .starts_with(&format!("tmux:{}:@", probe.name)),
            "{pane:?}"
        );
        assert_eq!(pane.location.workspace_id, workspace.workspace_id);
    }

    // Reordering swaps the windows' positions. Indexes are not identity, so the
    // handles the inventory reported must describe the same objects afterwards.
    probe.tmux(&["swap-window", "-s", "@0", "-t", &second]);
    let after = probe.runtime.inventory(&cancel()).expect("an inventory");
    let handles = |observation: &agent_radar::FleetObservation| {
        let mut panes: Vec<String> = observation
            .panes
            .iter()
            .map(|pane| pane.location.pane_id.clone())
            .collect();
        panes.sort();
        panes
    };
    assert_eq!(handles(&before), handles(&after));
    let mut tabs: Vec<String> = before.tabs.iter().map(|tab| tab.tab_id.clone()).collect();
    tabs.sort();
    let mut tabs_after: Vec<String> = after.tabs.iter().map(|tab| tab.tab_id.clone()).collect();
    tabs_after.sort();
    assert_eq!(tabs, tabs_after, "a reordered window kept its handle");
}

#[test]
fn focus_moves_the_window_and_makes_the_pane_active() {
    let Some(probe) = Probe::start("focus_moves_the_window_and_makes_the_pane_active") else {
        return;
    };
    let pane = probe.tmux(&["new-window", "-d", "-t", "$0:", "-P", "-F", "#{pane_id}"]);
    let pane = pane.trim().to_string();
    let pane_handle = probe.runtime_handle(&pane);

    // With nothing attached there is nothing to show a focus to: refused first,
    // and nothing is changed.
    let before = probe.tmux(&["list-panes", "-a", "-F", "#{pane_id} #{pane_active}"]);
    let alone = probe
        .runtime
        .focus_outcome(&Target::Pane(pane_handle.clone()), &cancel());
    assert_eq!(alone, FocusOutcome::NoClient);
    assert_eq!(
        before,
        probe.tmux(&["list-panes", "-a", "-F", "#{pane_id} #{pane_active}"]),
        "a refused focus changed the server"
    );

    let _client = probe.attach_client();
    let outcome = probe
        .runtime
        .focus_outcome(&Target::Pane(pane_handle), &cancel());
    assert_eq!(outcome, FocusOutcome::Completed, "{outcome:?}");
    let active = probe.tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id} #{window_active} #{pane_active}",
    ]);
    let active: Vec<&str> = active
        .lines()
        .find(|line| line.starts_with(&pane))
        .expect("the pane this test focused")
        .split(' ')
        .collect();
    assert_eq!(active[1], "1", "the focused pane's window is current");
    assert_eq!(active[2], "1", "the focused pane is active in its window");

    // A pane in a session the client is not looking at must bring that session
    // into the client's view. Selecting a window alone only changes the target
    // session's own state, which the client never sees.
    let other = probe.tmux(&[
        "new-session",
        "-d",
        "-s",
        "other",
        "-x",
        "100",
        "-y",
        "30",
        "-P",
        "-F",
        "#{pane_id}",
    ]);
    let other = other.trim().to_string();
    assert_eq!(
        probe
            .tmux(&["list-clients", "-F", "#{client_session}"])
            .trim(),
        "main",
        "the client starts on the other session"
    );
    let outcome = probe
        .runtime
        .focus_outcome(&Target::Pane(probe.runtime_handle(&other)), &cancel());
    assert_eq!(outcome, FocusOutcome::Completed, "{outcome:?}");
    assert_eq!(
        probe
            .tmux(&["list-clients", "-F", "#{client_session}"])
            .trim(),
        "other",
        "the client shows the focused pane's session"
    );
    let active = probe.tmux(&[
        "list-panes",
        "-a",
        "-F",
        "#{pane_id} #{window_active} #{pane_active}",
    ]);
    let active = active
        .lines()
        .find(|line| line.split(' ').next() == Some(other.as_str()))
        .expect("the pane this test focused")
        .split(' ')
        .collect::<Vec<_>>();
    assert_eq!(active[1], "1", "its window is current in its session");
    assert_eq!(active[2], "1", "it is active in its window");
}

#[test]
fn create_names_the_locations_it_made() {
    let Some(probe) = Probe::start("create_names_the_locations_it_made") else {
        return;
    };
    let runtime = &probe.runtime;
    let cancel = cancel();

    let created = runtime.create(
        &CreateRequest::Workspace {
            focus: Some(false),
            directory: None,
        },
        &cancel,
    );
    let CreateOutcome::Completed(workspace) = created else {
        panic!("a workspace was not created: {created:?}");
    };
    assert_eq!(workspace.tab_id, None, "nothing else was named");
    let workspace_id = workspace.workspace_id.expect("the session tmux made");

    let created = runtime.create(
        &CreateRequest::Tab {
            workspace_id: workspace_id.clone(),
            focus: Some(false),
        },
        &cancel,
    );
    let CreateOutcome::Completed(tab) = created else {
        panic!("a tab was not created: {created:?}");
    };
    let tab_id = tab.tab_id.expect("the window tmux made");
    assert_eq!(tab.workspace_id.as_deref(), Some(workspace_id.as_str()));

    let pane = probe.tmux(&[
        "list-panes",
        "-t",
        tab_id.rsplit(':').next().expect("an id"),
        "-F",
        "#{pane_id}",
    ]);
    let pane = probe.runtime_handle(pane.trim());
    let created = runtime.create(
        &CreateRequest::PaneSplit {
            pane_id: pane,
            direction: SplitDirection::Right,
            focus: Some(false),
        },
        &cancel,
    );
    let CreateOutcome::Completed(split) = created else {
        panic!("a pane was not split: {created:?}");
    };
    assert!(split.pane_id.is_some(), "{split:?}");
    assert_eq!(split.tab_id.as_deref(), Some(tab_id.as_str()));
    assert_eq!(split.workspace_id.as_deref(), Some(workspace_id.as_str()));

    // Every identity tmux named is a location the inventory then lists.
    let after = runtime.inventory(&cancel).expect("an inventory");
    for id in [&workspace_id, &tab_id] {
        let listed = after.workspaces.iter().any(|w| &w.workspace_id == id)
            || after.tabs.iter().any(|t| &t.tab_id == id);
        assert!(listed, "{id} was created but not observed");
    }
    assert!(
        after
            .panes
            .iter()
            .any(|p| Some(&p.location.pane_id) == split.pane_id.as_ref()),
        "the created pane was not observed"
    );
}

#[test]
fn input_delivers_literal_text_and_presses_named_keys() {
    let Some(probe) = Probe::start("input_delivers_literal_text_and_presses_named_keys") else {
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "radar-tmux-input-{}-{}",
        std::process::id(),
        unique()
    ));
    fs::create_dir_all(&dir).expect("a directory for the pane's output");
    let recorded = dir.join("recorded");
    let pane = probe.tmux(&[
        "new-window",
        "-d",
        "-t",
        "$0:",
        "-P",
        "-F",
        "#{pane_id}",
        &format!("cat > {}", recorded.display()),
    ]);
    let pane_handle = probe.runtime_handle(pane.trim());
    let cancel = cancel();

    // Literal text is bytes, not a shell word: spaces, quotes, a tab, a leading
    // dash and a newline all arrive unchanged.
    let text = "hello \"world\"\t-x\n".to_string();
    let outcome = probe.runtime.input(
        &InputRequest {
            pane_id: pane_handle.clone(),
            payload: InputPayload::Text { text: text.clone() },
        },
        &cancel,
    );
    assert_eq!(outcome, InputOutcome::Completed, "{outcome:?}");
    let delivered = wait_for_text(&recorded, DEADLINE).expect("the pane's record of the text");
    assert_eq!(delivered, text, "the text did not arrive byte for byte");

    // A named key presses the key tmux calls it, and an unmapped name is
    // refused rather than pressed as nothing.
    let shell = probe.tmux(&[
        "new-window",
        "-d",
        "-t",
        "$0:",
        "-P",
        "-F",
        "#{pane_id}",
        "bash --norc -i",
    ]);
    let shell_handle = probe.runtime_handle(shell.trim());
    let refused = probe.runtime.input(
        &InputRequest {
            pane_id: shell_handle.clone(),
            payload: InputPayload::Keys {
                keys: vec!["nope".to_string()],
            },
        },
        &cancel,
    );
    assert!(
        matches!(refused, InputOutcome::Refused(_)),
        "an unmapped key name was not refused: {refused:?}"
    );
    let typed = probe.runtime.input(
        &InputRequest {
            pane_id: shell_handle.clone(),
            payload: InputPayload::Text {
                text: "echo radar-tmux-marker".to_string(),
            },
        },
        &cancel,
    );
    assert_eq!(typed, InputOutcome::Completed, "{typed:?}");
    let pressed = probe.runtime.input(
        &InputRequest {
            pane_id: shell_handle.clone(),
            payload: InputPayload::Keys {
                keys: vec!["enter".to_string()],
            },
        },
        &cancel,
    );
    assert_eq!(pressed, InputOutcome::Completed, "{pressed:?}");
    let echoed = eventually(DEADLINE, || {
        visible(&probe.runtime, &shell_handle)
            .is_some_and(|text| text.matches("radar-tmux-marker").count() >= 2)
    });
    assert!(
        echoed,
        "pressing enter did not run the typed command:\n{}",
        probe.tmux(&["capture-pane", "-p", "-t", shell.trim()])
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn launch_runs_argv_with_overlay_and_preserves_immediate_exit_status() {
    let Some(probe) =
        Probe::start("launch_runs_argv_with_overlay_and_preserves_immediate_exit_status")
    else {
        return;
    };
    let pane_id = probe.first_pane();
    probe.tmux(&[
        "set-environment",
        "-t",
        "$0",
        "RADAR_INHERITED_TEST",
        "inherited-sentinel",
    ]);
    let script = "printf '<%s>\\n' \"$@\"; printf 'ENV=%s\\nTOKEN=%s\\nINHERITED=%s\\n' \"$RADAR_LAUNCH_TEST\" \"$PI_RADAR_REGISTRATION_ID\" \"$RADAR_INHERITED_TEST\"; pwd";
    let request = LaunchRequest {
        pane_id: pane_id.clone(),
        executable: "/bin/sh".to_string(),
        argv: vec![
            "-c".to_string(),
            script.to_string(),
            "argv0".to_string(),
            "space argument".to_string(),
            "quote'argument".to_string(),
            "line\\nbreak".replace("\\n", "\n"),
        ],
        spawn_token: Some("private-launch-token-sentinel".to_string()),
        cwd: Some("/tmp".to_string()),
        env: BTreeMap::from([(
            "RADAR_LAUNCH_TEST".to_string(),
            "overlay-sentinel".to_string(),
        )]),
    };

    assert_eq!(probe.runtime.validate_launch(&request), Ok(()));
    assert_eq!(
        probe.runtime.launch(&request, &cancel()),
        LaunchOutcome::Completed
    );
    assert!(
        eventually(DEADLINE, || matches!(
            probe.runtime.output(
                &OutputRequest {
                    pane_id: pane_id.clone(),
                    source: OutputSource::Recent,
                    lines: None,
                    ansi: false,
                },
                &cancel(),
            ),
            OutputOutcome::Completed(read) if read.text.contains("Pane is dead (status 0")
        )),
        "the immediately exiting child never left a readable dead-pane status"
    );
    let read = probe.runtime.output(
        &OutputRequest {
            pane_id: pane_id.clone(),
            source: OutputSource::Recent,
            lines: None,
            ansi: false,
        },
        &cancel(),
    );
    let OutputOutcome::Completed(read) = read else {
        panic!("output: {read:?}")
    };
    assert!(
        read.text.contains("Pane is dead (status 0"),
        "pane did not remain with exit status: {:?}",
        read.text
    );
    assert!(
        read.text.contains("space argument"),
        "argv missing: {:?}",
        read.text
    );
    assert!(
        read.text.contains("quote'argument"),
        "quote changed: {:?}",
        read.text
    );
    assert!(
        read.text.contains("line\nbreak"),
        "newline argument changed: {:?}",
        read.text
    );
    assert!(
        read.text.contains("overlay-sentinel"),
        "environment overlay missing: {:?}",
        read.text
    );
    assert!(
        read.text.contains("inherited-sentinel"),
        "inherited session environment was lost: {:?}",
        read.text
    );
    assert!(
        read.text.contains("private-launch-token-sentinel"),
        "spawn token missing: {:?}",
        read.text
    );
    assert!(read.text.contains("/tmp"), "cwd missing: {:?}", read.text);
    assert_eq!(
        probe
            .runtime
            .exit_observations(std::slice::from_ref(&pane_id), &cancel())
            .get(&pane_id),
        Some(&PaneExitObservation::Exited {
            code: Some(0),
            signal: None,
        })
    );
    assert!(
        probe
            .runtime
            .primitives()
            .contains(agent_radar::runtime::Primitives::LAUNCH)
    );
    assert!(
        probe
            .runtime
            .primitives()
            .contains(agent_radar::runtime::Primitives::LAUNCH_CONTEXT)
    );
}

#[test]
fn tmux_records_signal_exit_and_missing_pane_stays_unknown() {
    let Some(probe) = Probe::start("tmux_records_signal_exit_and_missing_pane_stays_unknown")
    else {
        return;
    };
    let pane = probe.first_pane();
    let request = LaunchRequest {
        pane_id: pane.clone(),
        executable: "/bin/sh".to_string(),
        argv: vec!["-c".to_string(), "kill -TERM $$".to_string()],
        spawn_token: Some("signal-exit-token".to_string()),
        cwd: None,
        env: BTreeMap::new(),
    };
    assert_eq!(
        probe.runtime.launch(&request, &cancel()),
        LaunchOutcome::Completed
    );
    let exited = eventually(DEADLINE, || {
        matches!(
            probe
                .runtime
                .exit_observations(std::slice::from_ref(&pane), &cancel())
                .get(&pane),
            Some(PaneExitObservation::Exited {
                code: None,
                signal: Some(15),
            })
        )
    });
    assert!(exited, "tmux did not report the child's terminating signal");

    let raw_pane = pane.rsplit(':').next().unwrap();
    probe.tmux(&["kill-pane", "-t", raw_pane]);
    assert_eq!(
        probe
            .runtime
            .exit_observations(std::slice::from_ref(&pane), &cancel())
            .get(&pane),
        Some(&PaneExitObservation::Unknown),
        "pane disappearance is not evidence of process exit"
    );
}

#[test]
fn direct_launch_process_is_the_pane_process() {
    let Some(probe) = Probe::start("direct_launch_process_is_the_pane_process") else {
        return;
    };
    let pane = probe.first_pane();
    let executable = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|directory| directory.join("sleep"))
        .find(|path| path.is_file())
        .expect("sleep on PATH");
    let request = LaunchRequest {
        pane_id: pane.clone(),
        executable: executable.display().to_string(),
        argv: vec!["10".to_string()],
        spawn_token: Some("pid-check-token".to_string()),
        cwd: None,
        env: BTreeMap::new(),
    };
    assert_eq!(
        probe.runtime.launch(&request, &cancel()),
        LaunchOutcome::Completed
    );
    assert_eq!(
        probe
            .runtime
            .exit_observations(std::slice::from_ref(&pane), &cancel())
            .get(&pane),
        Some(&PaneExitObservation::Running)
    );
    let tmux_id = pane.rsplit(':').next().unwrap().to_string();
    assert!(
        eventually(DEADLINE, || {
            probe
                .tmux(&[
                    "display-message",
                    "-p",
                    "-t",
                    &tmux_id,
                    "#{pane_current_command}",
                ])
                .trim()
                == "sleep"
        }),
        "sleep did not become the pane's foreground command"
    );
    let pid = probe
        .tmux(&["display-message", "-p", "-t", &tmux_id, "#{pane_pid}"])
        .trim()
        .to_string();
    let process = Command::new("ps")
        .args(["-o", "args=", "-p", &pid])
        .output()
        .expect("read launched process identity");
    assert!(
        process.status.success(),
        "pane pid {pid} was not a live process: {}",
        String::from_utf8_lossy(&process.stderr)
    );
    assert!(
        String::from_utf8_lossy(&process.stdout)
            .trim()
            .ends_with("/sleep 10"),
        "pane pid is not the launched sleep process: {:?}",
        String::from_utf8_lossy(&process.stdout)
    );
}

#[test]
fn output_reads_a_bounded_snapshot_and_consumes_nothing() {
    let Some(probe) = Probe::start("output_reads_a_bounded_snapshot_and_consumes_nothing") else {
        return;
    };
    let pane = probe.tmux(&[
        "new-window",
        "-d",
        "-t",
        "$0:",
        "-P",
        "-F",
        "#{pane_id}",
        "bash --norc -i",
    ]);
    let pane_handle = probe.runtime_handle(pane.trim());
    let cancel = cancel();
    for payload in [
        InputPayload::Text {
            text: "for i in $(seq 1 40); do echo line-$i; done".to_string(),
        },
        InputPayload::Keys {
            keys: vec!["enter".to_string()],
        },
    ] {
        let outcome = probe.runtime.input(
            &InputRequest {
                pane_id: pane_handle.clone(),
                payload,
            },
            &cancel,
        );
        assert_eq!(outcome, InputOutcome::Completed, "{outcome:?}");
    }

    let read = |lines| {
        probe.runtime.output(
            &OutputRequest {
                pane_id: pane_handle.clone(),
                source: OutputSource::Visible,
                lines,
                ansi: false,
            },
            &cancel,
        )
    };
    let text = |lines| match read(lines) {
        OutputOutcome::Completed(read) => Some(read.text),
        _ => None,
    };
    assert!(
        eventually(DEADLINE, || text(None)
            .is_some_and(|snapshot| snapshot.contains("line-40"))),
        "the pane never printed its lines"
    );

    let whole = text(None).expect("a snapshot");
    let again = text(None).expect("a snapshot");
    assert_eq!(whole, again, "a read consumed the pane's output");

    let OutputOutcome::Completed(bounded) = read(Some(5)) else {
        panic!("a bounded read failed");
    };
    assert!(bounded.truncated, "a bounded read dropped rows unnoticed");
    assert_eq!(bounded.text.lines().count(), 5, "{}", bounded.text);
    assert!(
        bounded.text.contains("line-40"),
        "a bounded read keeps the newest rows, not the oldest:\n{}",
        bounded.text
    );
    assert_eq!(bounded.revision, None, "tmux reports no revision");

    // The other two declared sources read the same pane: recent output with
    // history behind it, and the same with soft wraps joined and styling kept.
    let recent = probe.runtime.output(
        &OutputRequest {
            pane_id: pane_handle.clone(),
            source: OutputSource::Recent,
            lines: Some(5),
            ansi: false,
        },
        &cancel,
    );
    let OutputOutcome::Completed(recent) = recent else {
        panic!("a recent read failed: {recent:?}");
    };
    assert!(
        recent.text.contains("line-40"),
        "a recent read keeps the newest rows:\n{}",
        recent.text
    );
    let unwrapped = probe.runtime.output(
        &OutputRequest {
            pane_id: pane_handle,
            source: OutputSource::RecentUnwrapped,
            lines: Some(20),
            ansi: true,
        },
        &cancel,
    );
    assert!(
        matches!(unwrapped, OutputOutcome::Completed(_)),
        "an unwrapped styled read failed: {unwrapped:?}"
    );
}

#[test]
fn close_ends_the_pane_and_then_the_window() {
    let Some(probe) = Probe::start("close_ends_the_pane_and_then_the_window") else {
        return;
    };
    let pane = probe.tmux(&["new-window", "-d", "-t", "$0:", "-P", "-F", "#{pane_id}"]);
    let pane = pane.trim().to_string();
    // A second pane keeps the window alive after the first one is closed: a
    // window ends with its last pane, which is not what this test is about.
    probe.tmux(&["split-window", "-d", "-t", &pane]);
    let pane_handle = probe.runtime_handle(&pane);
    let window = probe
        .runtime
        .inventory(&cancel())
        .expect("an inventory")
        .panes
        .iter()
        .find(|listed| listed.location.pane_id == pane_handle)
        .expect("the pane this test opened")
        .location
        .tab_id
        .clone();
    let cancel = cancel();

    let closed = probe
        .runtime
        .close_outcome(&CloseTarget::Pane(pane_handle.clone()), &cancel);
    assert_eq!(closed, CloseOutcome::Completed, "{closed:?}");
    let after = probe.runtime.inventory(&cancel).expect("an inventory");
    assert!(
        !after
            .panes
            .iter()
            .any(|listed| listed.location.pane_id == pane_handle),
        "a closed pane is still listed"
    );

    let closed = probe
        .runtime
        .close_outcome(&CloseTarget::Tab(window.clone()), &cancel);
    assert_eq!(closed, CloseOutcome::Completed, "{closed:?}");
    assert!(
        eventually(DEADLINE, || {
            probe
                .runtime
                .inventory(&cancel)
                .is_ok_and(|after| !after.tabs.iter().any(|tab| tab.tab_id == window))
        }),
        "a closed window is still listed"
    );
}

#[test]
fn a_killed_server_leaves_every_effect_unknown() {
    let Some(probe) = Probe::start("a_killed_server_leaves_every_effect_unknown") else {
        return;
    };
    let pane_handle = probe.first_pane();
    let window = probe
        .runtime
        .inventory(&cancel())
        .expect("an inventory")
        .panes[0]
        .location
        .tab_id
        .clone();
    probe.kill();
    let cancel = cancel();

    let unread = probe.runtime.inventory(&cancel);
    assert!(
        unread.is_err(),
        "an observation of a server that is gone answered: {unread:?}"
    );
    assert_eq!(
        probe.runtime.foreground_evidence(&pane_handle, &cancel),
        ForegroundEvidence::Inconclusive,
        "a pane on a server that is gone was read as evidence"
    );
    let focus = probe
        .runtime
        .focus_outcome(&Target::Pane(pane_handle.clone()), &cancel);
    assert!(
        matches!(focus, FocusOutcome::Unknown(_)),
        "a focus on a server that is gone was not unknown: {focus:?}"
    );
    let input = probe.runtime.input(
        &InputRequest {
            pane_id: pane_handle.clone(),
            payload: InputPayload::Text {
                text: "hello".to_string(),
            },
        },
        &cancel,
    );
    assert!(
        matches!(input, InputOutcome::Unknown(_)),
        "input to a server that is gone was not unknown: {input:?}"
    );
    let output = probe.runtime.output(
        &OutputRequest {
            pane_id: pane_handle.clone(),
            source: OutputSource::Visible,
            lines: None,
            ansi: false,
        },
        &cancel,
    );
    assert!(
        matches!(output, OutputOutcome::Unknown(_)),
        "a read from a server that is gone was not unknown: {output:?}"
    );
    let closed = probe
        .runtime
        .close_outcome(&CloseTarget::Tab(window), &cancel);
    assert!(
        matches!(closed, CloseOutcome::Unknown(_)),
        "a close on a server that is gone was not unknown: {closed:?}"
    );
}

/// The adapter names the process serving it, which is the one fact about a tmux
/// server that a restart under the same name does not reproduce.
#[test]
fn the_serving_process_is_read_from_the_server_and_is_none_once_it_is_gone() {
    let Some(probe) = Probe::start("the_serving_process_is_read_from_the_server") else {
        return;
    };
    let reported = probe
        .runtime
        .server_process(&cancel())
        .expect("a server that answers names its process");
    let asked = probe
        .tmux(&["display-message", "-p", "#{pid}"])
        .trim()
        .parse::<i32>()
        .expect("the server pid tmux names");
    assert_eq!(reported, asked, "the adapter reads the server's own pid");
    assert_eq!(
        probe.runtime.server_process(&cancel()),
        Some(reported),
        "a second client sees the same serving process"
    );
    probe.kill();
    assert_eq!(
        probe.runtime.server_process(&cancel()),
        None,
        "a server that is gone names no process"
    );
}

#[test]
fn adapter_work_leaves_tmuxs_default_server_alone() {
    let before = default_socket_state();
    let Some(probe) = Probe::start("adapter_work_leaves_tmuxs_default_server_alone") else {
        return;
    };
    let cancel = cancel();
    let pane_handle = probe.first_pane();
    let _ = probe.runtime.inventory(&cancel);
    let _ = probe.runtime.foreground_evidence(&pane_handle, &cancel);
    let _ = probe
        .runtime
        .focus_outcome(&Target::Pane(pane_handle.clone()), &cancel);
    let _ = probe.runtime.create(
        &CreateRequest::Tab {
            workspace_id: probe.runtime_handle("$0"),
            focus: Some(false),
        },
        &cancel,
    );
    let _ = probe.runtime.input(
        &InputRequest {
            pane_id: pane_handle.clone(),
            payload: InputPayload::Text {
                text: "hello".to_string(),
            },
        },
        &cancel,
    );
    let _ = probe.runtime.output(
        &OutputRequest {
            pane_id: pane_handle.clone(),
            source: OutputSource::Visible,
            lines: None,
            ansi: false,
        },
        &cancel,
    );
    assert_eq!(
        default_socket_state(),
        before,
        "the adapter brought tmux's default server up or took it down"
    );
}
