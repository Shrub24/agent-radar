#![cfg(unix)]
//! Adapter transport: the Herdr adapter against a throwaway `herdr`.
//!
//! Everything here belongs to the adapter, not to the collector: spawning the
//! executable, decoding its output, the command deadline, killing and reaping a
//! stalled command and draining both pipes. A `#!/bin/sh` script stands in for
//! `herdr`, so no runtime is needed and the failure boundaries are exact.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::control_plane::{PROTOCOL_VERSION, random_uuid};
use agent_radar::model::{ForegroundEvidence, LocalFacts};
use agent_radar::runtime::{
    CloseOutcome, CreateOutcome, CreateRequest, LAUNCH_CAPABILITY, LAUNCH_TOKEN_ENV, LaunchOutcome,
    LaunchRequest,
};
use agent_radar::{
    CloseTarget, Daemon, FocusOutcome, HerdrConfig, HerdrRuntime, RuntimeProvider, Target,
};

/// A successful snapshot of one pane in one workspace, without agents.
const SNAPSHOT_ONE_PANE: &str = r#"{"id":"cli:api:snapshot","result":{"type":"snapshot","snapshot":{"workspaces":[{"workspace_id":"wA","label":"main","number":1}],"tabs":[{"tab_id":"wA:t1","workspace_id":"wA","label":"agent tab","number":1}],"panes":[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"}],"agents":[]}}}"#;

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
            "radar-runtime-{}-{}",
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

    fn runtime(&self, command_timeout: Duration) -> HerdrRuntime {
        HerdrRuntime::new(HerdrConfig {
            executable: self.executable.clone(),
            command_timeout,
        })
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
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

fn cancel() -> AtomicBool {
    AtomicBool::new(false)
}

/// One adapter test at a time.
///
/// Each test writes its fake `herdr` and then execs it. On Linux a concurrent
/// `open` for writing in another thread can make `execve` fail with ETXTBSY for
/// an unrelated file, because a vforked child briefly shares the parent's file
/// table. Serializing keeps the transport checks deterministic; it retires when
/// the harness stops exec'ing a script it just wrote.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn an_inventory_and_foreground_answer_decode_through_the_adapter() {
    let _serial = serial();
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
    let runtime = fake.runtime(Duration::from_secs(2));

    let inventory = runtime
        .inventory(&cancel())
        .expect("a valid snapshot decodes");
    assert_eq!(inventory.panes.len(), 1);
    assert!(inventory.agents.is_empty());
    assert_eq!(inventory.workspaces[0].label.as_deref(), Some("main"));

    fake.write("mode", "shell");
    assert_eq!(
        runtime.foreground_evidence("wA:p1", &cancel()),
        ForegroundEvidence::Shell
    );

    fake.write("mode", "non-shell");
    assert_eq!(
        runtime.foreground_evidence("wA:p1", &cancel()),
        ForegroundEvidence::NonShell {
            pid: 200,
            name: Some("nvim".into()),
            command: Some("nvim".into()),
            local: LocalFacts::default(),
        }
    );
}

#[test]
fn a_nonzero_exit_is_a_diagnostic_that_quotes_stderr() {
    let _serial = serial();
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
    let runtime = fake.runtime(Duration::from_secs(2));
    fake.write("mode", "ok");
    assert!(runtime.inventory(&cancel()).is_ok());

    fake.write("mode", "fail");
    let diagnostic = runtime.inventory(&cancel()).expect_err("a failed command");
    assert!(diagnostic.contains("status 3"), "{diagnostic}");
    assert!(
        diagnostic.contains("cannot connect to the runtime"),
        "{diagnostic}"
    );
}

#[test]
fn malformed_output_is_an_error_not_an_empty_fleet() {
    let _serial = serial();
    let fake = Fake::new(|_| "printf '%s' 'not json at all'".to_string());
    let runtime = fake.runtime(Duration::from_secs(2));
    let diagnostic = runtime
        .inventory(&cancel())
        .expect_err("malformed output is not a fleet");
    assert!(diagnostic.contains("could not be read"), "{diagnostic}");
}

#[test]
fn a_missing_executable_is_a_diagnostic_not_a_panic() {
    let _serial = serial();
    let fake = Fake::new(|_| "exit 0".to_string());
    let runtime = HerdrRuntime::new(HerdrConfig {
        executable: fake.path("herdr-not-installed"),
        command_timeout: Duration::from_secs(2),
    });
    let diagnostic = runtime
        .inventory(&cancel())
        .expect_err("no such executable");
    assert!(diagnostic.contains("could not run"), "{diagnostic}");
}

#[test]
fn a_stalled_command_times_out_and_is_killed_and_reaped() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "echo $$ > \"{dir}/pid\"\nexec sleep 30",
            dir = dir.display()
        )
    });
    let runtime = fake.runtime(Duration::from_millis(150));

    let started = Instant::now();
    let diagnostic = runtime
        .inventory(&cancel())
        .expect_err("the command stalls past the deadline");
    assert!(diagnostic.contains("timed out"), "{diagnostic}");

    let pid = wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5))
        .then(|| fake.read("pid").trim().parse::<i32>().expect("fake pid"))
        .expect("the fake command never started");
    assert!(
        started.elapsed() >= Duration::from_millis(120),
        "the timeout was not honoured"
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the stalled command was waited out instead of killed"
    );
    assert!(
        !process_exists(pid),
        "the stalled command was not killed and reaped"
    );
}

#[test]
fn a_cancelled_command_is_abandoned_and_reaped() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "echo $$ > \"{dir}/pid\"\nexec sleep 60",
            dir = dir.display()
        )
    });
    // Far beyond the test: only cancellation ends this call.
    let runtime = Arc::new(fake.runtime(Duration::from_secs(30)));
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let worker = {
        let runtime = Arc::clone(&runtime);
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || runtime.inventory(&cancel))
    };

    assert!(wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5)));
    let pid: i32 = fake.read("pid").trim().parse().expect("fake pid");
    cancel.store(true, Ordering::SeqCst);
    let diagnostic = worker
        .join()
        .expect("the adapter worker returns")
        .expect_err("cancellation is a diagnostic");
    assert!(diagnostic.contains("cancelled"), "{diagnostic}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancellation waited for the command: {:?}",
        started.elapsed()
    );
    assert!(!process_exists(pid), "the cancelled command was not reaped");
}

#[test]
fn output_larger_than_the_pipe_buffer_is_drained_on_both_pipes() {
    let _serial = serial();
    let fake = Fake::new(|_| {
        r#"head -c 200000 /dev/zero | tr '\0' 'e' >&2
printf '{"id":"x","pad":"'
head -c 200000 /dev/zero | tr '\0' 'a'
printf '","result":{"type":"snapshot","snapshot":{"workspaces":[],"tabs":[],"panes":[],"agents":[]}}}'"#
            .to_string()
    });
    let runtime = fake.runtime(Duration::from_secs(2));
    let inventory = runtime
        .inventory(&cancel())
        .expect("a command filling both pipes must be drained, not left to time out");
    assert!(inventory.panes.is_empty());
}

#[test]
fn an_unreadable_foreground_answer_is_inconclusive() {
    let _serial = serial();
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
    let runtime = fake.runtime(Duration::from_secs(2));

    fake.write("mode", "shell");
    assert_eq!(
        runtime.foreground_evidence("wA:p1", &cancel()),
        ForegroundEvidence::Shell
    );

    // A failing query is inconclusive: it never proves the pane went away.
    fake.write("mode", "fail");
    assert_eq!(
        runtime.foreground_evidence("wA:p1", &cancel()),
        ForegroundEvidence::Inconclusive
    );

    // So is an answer that cannot be decoded.
    fake.write("mode", "malformed");
    assert_eq!(
        runtime.foreground_evidence("wA:p1", &cancel()),
        ForegroundEvidence::Inconclusive
    );
}

// --- the adapter's focus transports ---------------------------------------

/// A fake `herdr` whose `status --json` reports a socket beside itself, so a
/// pane focus reaches the stub listener bound at `Fake::path("herdr.sock")`.
fn status_fake() -> Fake {
    Fake::new(|dir| {
        format!(
            r#"if [ "$1 $2" = "status --json" ]; then
  printf '%s' '{{"server":{{"socket":"{}"}}}}'
  exit 0
fi
echo "unexpected subcommand: $1 $2" >&2
exit 9"#,
            dir.join("herdr.sock").display()
        )
    })
}

/// A stub API socket that serves a whole scripted session: one connection per
/// request, each answer chosen by the method the request names, `ok` where the
/// test staged nothing. The recorder holds every request it was sent.
fn answering_stub_at(
    path: &Path,
    answers: &'static [(&'static str, &'static str)],
) -> Arc<Mutex<Vec<String>>> {
    let listener = UnixListener::bind(path).expect("stub socket binds");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().expect("stub stream"));
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let trimmed = line.trim().to_string();
            let answer = answers
                .iter()
                .find(|(method, _)| trimmed.contains(&format!("\"method\":\"{method}\"")))
                .map(|(_, answer)| *answer)
                .unwrap_or(r#"{"id":"radar","result":{"type":"ok"}}"#);
            recorder.lock().expect("recorder").push(trimmed);
            let _ = stream.write_all(format!("{answer}\n").as_bytes());
        }
    });
    seen
}

/// A stub API socket at `path` that records the request it is sent and answers
/// with `answer` for a single connection — or, with `None`, accepts and never
/// answers. For a scripted session of several requests, use `answering_stub_at`.
fn stub_server_at(path: &Path, answer: Option<&'static str>) -> Arc<Mutex<Vec<String>>> {
    let listener = UnixListener::bind(path).expect("stub socket binds");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&seen);
    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut reader = BufReader::new(stream.try_clone().expect("stub stream"));
        let mut line = String::new();
        if reader.read_line(&mut line).is_ok() {
            recorder
                .lock()
                .expect("recorder")
                .push(line.trim().to_string());
        }
        match answer {
            Some(answer) => {
                let mut stream = stream;
                let _ = stream.write_all(format!("{answer}\n").as_bytes());
            }
            None => thread::sleep(Duration::from_secs(10)),
        }
    });
    seen
}

#[test]
fn a_workspace_focus_runs_the_cli() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            r#"case "$1 $2" in
"workspace focus")
  printf '%s' "$3" > "{dir}/focused"
  ;;
*)
  echo "unexpected subcommand: $1 $2" >&2
  exit 9
  ;;
esac"#,
            dir = dir.display()
        )
    });
    let runtime = fake.runtime(Duration::from_secs(2));
    assert_eq!(
        runtime.focus_outcome(&Target::Workspace("wA".into()), &cancel()),
        FocusOutcome::Completed
    );
    assert_eq!(fake.read("focused"), "wA");
}

#[test]
fn a_failed_workspace_focus_is_unknown() {
    let _serial = serial();
    let fake = Fake::new(|_| "printf 'controlled refusal\\n' >&2\nexit 3".to_string());
    let runtime = fake.runtime(Duration::from_secs(2));
    let FocusOutcome::Unknown(message) =
        runtime.focus_outcome(&Target::Workspace("wA".into()), &cancel())
    else {
        panic!("a CLI that may have dispatched cannot prove a refusal")
    };
    assert!(message.contains("status 3"), "{message}");
    assert!(message.contains("controlled refusal"), "{message}");
}

#[test]
fn a_pane_focus_asks_the_socket() {
    let _serial = serial();
    let fake = status_fake();
    let seen = stub_server_at(
        &fake.path("herdr.sock"),
        Some(r#"{"id":"radar:focus","result":{"type":"ok"}}"#),
    );
    let runtime = fake.runtime(Duration::from_secs(5));
    assert_eq!(
        runtime.focus_outcome(&Target::Pane("wA:p1".into()), &cancel()),
        FocusOutcome::Completed
    );
    let request = seen.lock().expect("recorder").join(" ");
    assert!(request.contains("\"method\":\"pane.focus\""), "{request}");
    assert!(request.contains("\"pane_id\":\"wA:p1\""), "{request}");
}

#[test]
fn a_refused_pane_focus_reports_the_refusal() {
    let _serial = serial();
    let fake = status_fake();
    let _ = stub_server_at(
        &fake.path("herdr.sock"),
        Some(
            r#"{"id":"radar:focus","error":{"code":"pane_not_found","message":"pane wA:p1 not found"}}"#,
        ),
    );
    let runtime = fake.runtime(Duration::from_secs(5));
    let FocusOutcome::Refused(message) =
        runtime.focus_outcome(&Target::Pane("wA:p1".into()), &cancel())
    else {
        panic!("socket error answer is a known rejection")
    };
    assert!(message.contains("pane wA:p1 not found"), "{message}");
}

#[test]
fn a_stalled_focus_answer_times_out() {
    let _serial = serial();
    let fake = status_fake();
    let _ = stub_server_at(&fake.path("herdr.sock"), None);
    let runtime = fake.runtime(Duration::from_millis(300));
    let started = Instant::now();
    let FocusOutcome::Unknown(message) =
        runtime.focus_outcome(&Target::Pane("wA:p1".into()), &cancel())
    else {
        panic!("lost socket reply must be unknown")
    };
    assert!(message.contains("timed out"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "gave up on the deadline"
    );
}

#[test]
fn a_cancelled_socket_wait_is_abandoned() {
    let _serial = serial();
    let fake = status_fake();
    let seen = stub_server_at(&fake.path("herdr.sock"), None);
    let runtime = Arc::new(fake.runtime(Duration::from_secs(30)));
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let worker = {
        let runtime = Arc::clone(&runtime);
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || runtime.focus(&Target::Pane("wA:p1".into()), &cancel))
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.lock().expect("recorder").is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !seen.lock().expect("recorder").is_empty(),
        "the request was sent"
    );
    cancel.store(true, Ordering::SeqCst);
    let message = worker
        .join()
        .expect("the adapter worker returns")
        .expect_err("cancellation is a failure");
    assert!(message.contains("cancelled"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancellation waited for the socket: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_hung_focus_cli_is_cancelled_and_reaped() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "echo $$ > \"{dir}/pid\"\nexec sleep 60",
            dir = dir.display()
        )
    });
    let runtime = Arc::new(fake.runtime(Duration::from_secs(30)));
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let worker = {
        let runtime = Arc::clone(&runtime);
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || runtime.focus(&Target::Workspace("wA".into()), &cancel))
    };

    assert!(wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5)));
    let pid: i32 = fake.read("pid").trim().parse().expect("fake pid");
    cancel.store(true, Ordering::SeqCst);
    let message = worker
        .join()
        .expect("the adapter worker returns")
        .expect_err("cancellation is a failure");
    assert!(message.contains("cancelled"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancellation waited for the command: {:?}",
        started.elapsed()
    );
    assert!(!process_exists(pid), "the cancelled command was not reaped");
}

#[test]
fn a_pane_close_runs_the_documented_cli_grammar() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "printf '%s\\n' \"$*\" > \"{dir}/args\"\nexit 0",
            dir = dir.display()
        )
    });
    let runtime = fake.runtime(Duration::from_secs(2));
    runtime
        .close(&CloseTarget::Pane("wA:p3".into()), &cancel())
        .expect("close succeeds");
    assert_eq!(fake.read("args").trim(), "pane close wA:p3");
}

#[test]
fn a_tab_close_uses_the_tab_grammar() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "printf '%s\\n' \"$*\" > \"{dir}/args\"\nexit 0",
            dir = dir.display()
        )
    });
    let runtime = fake.runtime(Duration::from_secs(2));
    runtime
        .close(&CloseTarget::Tab("wA:t1".into()), &cancel())
        .expect("close succeeds");
    assert_eq!(fake.read("args").trim(), "tab close wA:t1");
}

/// The Herdr CLI both connects and dispatches, so a nonzero exit cannot prove
/// the close was never applied: the certainty is unknown, and the diagnostic is
/// still what herdr said.
#[test]
fn a_failed_close_is_unknown_rather_than_refused() {
    let _serial = serial();
    let fake = Fake::new(|_| "echo 'herdr: pane wA:p3 not found' >&2\nexit 1".to_string());
    let runtime = fake.runtime(Duration::from_secs(2));
    match runtime.close_outcome(&CloseTarget::Pane("wA:p3".into()), &cancel()) {
        CloseOutcome::Unknown(message) => assert!(message.contains("not found"), "{message}"),
        other => panic!("a failed close is unknown, not {other:?}"),
    }
}

#[test]
fn a_refused_close_is_a_diagnostic_that_quotes_stderr() {
    let _serial = serial();
    let fake = Fake::new(|_| "echo 'herdr: pane wA:p3 not found' >&2\nexit 1".to_string());
    let runtime = fake.runtime(Duration::from_secs(2));
    let diagnostic = runtime
        .close(&CloseTarget::Pane("wA:p3".into()), &cancel())
        .expect_err("a refusal is a failure");
    assert!(diagnostic.contains("status 1"), "{diagnostic}");
    assert!(diagnostic.contains("not found"), "{diagnostic}");
}

#[test]
fn a_cancelled_close_is_abandoned_and_reaped() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "echo $$ > \"{dir}/pid\"\nexec sleep 60",
            dir = dir.display()
        )
    });
    let runtime = Arc::new(fake.runtime(Duration::from_secs(30)));
    let cancel = Arc::new(AtomicBool::new(false));
    let started = Instant::now();
    let worker = {
        let runtime = Arc::clone(&runtime);
        let cancel = Arc::clone(&cancel);
        thread::spawn(move || runtime.close(&CloseTarget::Pane("wA:p3".into()), &cancel))
    };

    assert!(wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5)));
    let pid: i32 = fake.read("pid").trim().parse().expect("fake pid");
    cancel.store(true, Ordering::SeqCst);
    let message = worker
        .join()
        .expect("the adapter worker returns")
        .expect_err("cancellation is a failure");
    assert!(message.contains("cancelled"), "{message}");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "cancellation waited for the command: {:?}",
        started.elapsed()
    );
    assert!(!process_exists(pid), "the cancelled command was not reaped");
}

#[test]
fn a_stalled_close_times_out_and_is_reaped() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            "echo $$ > \"{dir}/pid\"\nexec sleep 30",
            dir = dir.display()
        )
    });
    let runtime = fake.runtime(Duration::from_millis(150));
    let diagnostic = runtime
        .close(&CloseTarget::Pane("wA:p3".into()), &cancel())
        .expect_err("the close stalls past the deadline");
    assert!(diagnostic.contains("timed out"), "{diagnostic}");
    let pid = wait_for_pid_file(&fake.path("pid"), Duration::from_secs(5))
        .then(|| fake.read("pid").trim().parse::<i32>().expect("fake pid"))
        .expect("the fake wrote its pid");
    assert!(!process_exists(pid), "the stalled close was not reaped");
}

/// One control-plane request over a fresh connection, for the composed test.
fn control_call(
    socket: &Path,
    id: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let stream = UnixStream::connect(socket).expect("dial the control socket");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("a read deadline");
    let mut writer = stream.try_clone().expect("a writer");
    let mut reader = BufReader::new(stream);
    let line = serde_json::json!({
        "version": PROTOCOL_VERSION,
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    writer
        .write_all(line.as_bytes())
        .expect("write the request");
    writer.write_all(b"\n").expect("terminate the request");
    writer.flush().expect("flush the request");
    let mut response = String::new();
    reader.read_line(&mut response).expect("read the answer");
    serde_json::from_str(&response).unwrap_or_else(|error| panic!("{response:?}: {error}"))
}

/// The control daemon and the real Herdr adapter together: `observe`,
/// `process_info`, `focus`, guarded `close`, creation, input and a bounded read
/// reach the scripted `herdr` through `HerdrRuntime` and come back normalized
/// over the real control socket.
#[test]
fn the_daemon_serves_a_scripted_herdr_runtime() {
    let _serial = serial();
    let fake = Fake::new(|dir| {
        format!(
            r#"case "$1 $2" in
"api snapshot")
  printf '%s\n' '{snapshot}'
  ;;
"status --json")
  printf '{{"server":{{"socket":"{socket}"}}}}\n'
  ;;
"pane process-info")
  printf '%s\n' '{evidence}'
  ;;
"pane close")
  printf '%s\n' "$*" > "{close_args}"
  ;;
*)
  echo "unexpected subcommand: $1 $2" >&2
  exit 9
  ;;
esac"#,
            snapshot = SNAPSHOT_ONE_PANE,
            socket = dir.join("herdr.sock").display(),
            evidence = NON_SHELL_EVIDENCE,
            close_args = dir.join("close-args").display(),
        )
    });
    let seen = answering_stub_at(
        &fake.path("herdr.sock"),
        &[
            (
                "pane.focus",
                r#"{"id":"radar:pane.focus","result":{"type":"ok"}}"#,
            ),
            (
                "pane.split",
                r#"{"id":"radar:pane.split","result":{"type":"pane_info","pane":{"pane_id":"wA:p2"}}}"#,
            ),
            (
                "pane.send_text",
                r#"{"id":"radar:pane.send_text","result":{"type":"ok"}}"#,
            ),
            (
                "pane.read",
                r#"{"id":"radar:pane.read","result":{"type":"pane_read","read":{"pane_id":"wA:p1","workspace_id":"wA","tab_id":"wA:t1","source":"visible","format":"text","text":"$ ls\n","revision":3,"truncated":false}}}"#,
            ),
            (
                "pane.report_agent",
                r#"{"id":"radar:pane.report_agent","result":{"type":"ok"}}"#,
            ),
            (
                "pane.report_metadata",
                r#"{"id":"radar:pane.report_metadata","result":{"type":"ok"}}"#,
            ),
        ],
    );
    let control = fake.path("control");
    fs::create_dir_all(&control).expect("control directory");
    fs::set_permissions(&control, fs::Permissions::from_mode(0o700)).expect("mode 0700");
    let socket = control.join("control.sock");
    let mut daemon = Daemon::bind_herdr_at(
        &socket,
        &control.join("state"),
        HerdrConfig {
            executable: fake.executable.clone(),
            command_timeout: Duration::from_secs(5),
        },
    )
    .expect("bind the daemon over the scripted runtime");

    let observed = control_call(&socket, &random_uuid(), "observe", serde_json::json!({}));
    assert_eq!(
        observed["result"]["inventory"]["panes"][0]["location"]["pane_id"],
        "wA:p1"
    );
    let evidence = control_call(
        &socket,
        &random_uuid(),
        "process_info",
        serde_json::json!({"pane_id": "wA:p1"}),
    );
    assert_eq!(evidence["result"]["evidence"]["kind"], "non_shell");
    assert_eq!(evidence["result"]["evidence"]["pid"], 200);
    let focused = control_call(
        &socket,
        &random_uuid(),
        "focus",
        serde_json::json!({"target": "wA:p1", "target_kind": "pane"}),
    );
    assert_eq!(focused["result"]["request"]["outcome"], "completed");
    let request = seen.lock().expect("recorder").join(" ");
    assert!(request.contains("\"method\":\"pane.focus\""), "{request}");

    // The whole production close path, not a fake backend: the wire form crosses
    // the socket, the lifecycle guard re-observes through this same runtime, and
    // the adapter runs its documented grammar against the scripted herdr.
    let closed = control_call(
        &socket,
        &random_uuid(),
        "close",
        serde_json::json!({
            "request": {
                "target": {"pane": "wA:p1"},
                "identity": {"pane": {"pane_id": "wA:p1", "occupant": null}},
            }
        }),
    );
    assert_eq!(closed["result"]["request"]["outcome"], "completed");
    assert_eq!(
        closed["result"]["request"]["effects"],
        serde_json::json!(["close"])
    );
    let grammar = fs::read_to_string(fake.path("close-args")).expect("the close invocation");
    assert_eq!(grammar.trim(), "pane close wA:p1");

    // The mux primitives through the same real adapter: the daemon asks for the
    // schema's method, the adapter owns the wire params, and the identities the
    // answer named are what the record reports.
    let created = control_call(
        &socket,
        &random_uuid(),
        "create",
        serde_json::json!({
            "request": {
                "kind": "pane_split",
                "pane_id": "wA:p1",
                "direction": "down",
                "focus": false,
            }
        }),
    );
    assert_eq!(created["result"]["request"]["outcome"], "completed");
    assert_eq!(created["result"]["request"]["target"], "wA:p1");
    // The split's own identity reaches the client as data, from the adapter's
    // decode of the answer rather than from the effect prose.
    assert_eq!(
        created["result"]["request"]["created"],
        serde_json::json!({"kind": "pane", "id": "wA:p2"})
    );
    assert_eq!(
        created["result"]["request"]["effects"],
        serde_json::json!(["created pane wA:p2"])
    );

    let sent = control_call(
        &socket,
        &random_uuid(),
        "input",
        serde_json::json!({
            "request": {
                "pane_id": "wA:p1",
                "payload": {"kind": "text", "text": "ls\n"},
            }
        }),
    );
    assert_eq!(sent["result"]["request"]["outcome"], "completed");
    assert_eq!(
        sent["result"]["request"]["effects"],
        serde_json::json!(["sent text"])
    );

    let read = control_call(
        &socket,
        &random_uuid(),
        "output",
        serde_json::json!({"pane_id": "wA:p1", "source": "visible", "lines": 5}),
    );
    assert_eq!(read["result"]["output"]["text"], "$ ls\n");
    assert_eq!(read["result"]["output"]["revision"], 3);
    assert_eq!(read["result"]["output"]["truncated"], false);

    // Reporting through the same adapter: the schema's own methods, the caller's
    // values under the schema's own names, and nothing derived from them.
    let reported = control_call(
        &socket,
        &random_uuid(),
        "report",
        serde_json::json!({"request": {
            "kind": "state",
            "pane_id": "wA:p1",
            "source": "pi-herdsman",
            "agent": "worker",
            "state": "working",
            "message": "2 tasks",
            "sequence": 4,
        }}),
    );
    assert_eq!(reported["result"]["request"]["outcome"], "completed");
    assert_eq!(
        reported["result"]["request"]["effects"],
        serde_json::json!(["reported state working"])
    );

    let display = control_call(
        &socket,
        &random_uuid(),
        "report",
        serde_json::json!({"request": {
            "kind": "metadata",
            "target": {"kind": "pane", "pane_id": "wA:p1"},
            "source": "herdsman",
            "tokens": {"summary": "3 tasks", "title-suffix": null},
            "applies_to_source": "herdr:pi",
            "state_labels": {"working": "thinking"},
            "clear_display_agent": true,
            "ttl_ms": 30000,
            "sequence": 6,
        }}),
    );
    assert_eq!(display["result"]["request"]["outcome"], "completed");
    assert_eq!(display["result"]["request"]["target"], "wA:p1");

    let requests = seen.lock().expect("recorder").join("\n");
    for method in [
        "pane.focus",
        "pane.split",
        "pane.send_text",
        "pane.read",
        "pane.report_agent",
        "pane.report_metadata",
    ] {
        assert!(
            requests.contains(&format!("\"method\":\"{method}\"")),
            "{method} never reached the adapter: {requests}"
        );
    }
    assert!(
        requests.contains(r#""target_pane_id":"wA:p1""#)
            && requests.contains(r#""direction":"down""#),
        "the split was not named on the wire: {requests}"
    );
    assert!(
        requests.contains(r#""text":"ls\n""#),
        "the literal text was not sent as itself: {requests}"
    );
    // The report grammar: the schema's `seq`, the token map with a withdrawn
    // token as null, and the TTL, unchanged from what the caller sent.
    assert!(
        requests.contains(r#""state":"working""#)
            && requests.contains(r#""message":"2 tasks""#)
            && requests.contains(r#""seq":4"#),
        "the state report was not forwarded as given: {requests}"
    );
    assert!(
        requests.contains(r#""tokens":{"summary":"3 tasks","title-suffix":null}"#)
            && requests.contains(r#""state_labels":{"working":"thinking"}"#)
            && requests.contains(r#""applies_to_source":"herdr:pi""#)
            && requests.contains(r#""clear_display_agent":true"#)
            && requests.contains(r#""ttl_ms":30000"#),
        "the display report was not forwarded as given: {requests}"
    );
    daemon.stop();
}

// --- the adapter's launch transport ---------------------------------------

/// The argv shapes pi-extensions ADR 0029 measured as fragile in a pane shell:
/// a space, a quote, a substitution, a `;`, a glob and a `=`-carrying argument.
const LAUNCH_ARGV: [&str; 6] = [
    "a b",
    "sq'uote",
    "$(echo INJECTED)",
    "semi;echo SPLIT",
    "star*glob",
    "model=omniroute/coder-high",
];

/// A child that records the argv it was run with and the token it was given.
fn launch_child(dir: &Path, out: &Path) -> PathBuf {
    let script = dir.join("dump.sh");
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
{{
  printf 'TOKEN=%s\n' "${{{}:-}}"
  n=0
  for a in "$@"; do
    n=$((n + 1))
    printf 'ARG[%s]\n' "$a"
  done
  printf 'COUNT=%s\n' "$n"
}} > '{}'
"#,
            LAUNCH_TOKEN_ENV,
            out.display()
        ),
    )
    .expect("write the child");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).expect("make it runnable");
    script
}

fn launch_request(pane: &str, executable: &Path, argv: &[&str], token: &str) -> LaunchRequest {
    LaunchRequest {
        pane_id: pane.to_string(),
        executable: executable.display().to_string(),
        argv: argv.iter().map(|argument| argument.to_string()).collect(),
        spawn_token: token.to_string(),
    }
}

/// Waits for a file a launched child wrote.
fn wait_for_file(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if fs::read_to_string(path).is_ok_and(|text| !text.is_empty()) {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn a_launch_types_one_quoted_line_the_pane_shell_yields_the_argv_from() {
    let _serial = serial();
    let fake = status_fake();
    let seen = answering_stub_at(
        &fake.path("herdr.sock"),
        &[(
            "pane.send_input",
            r#"{"id":"radar:pane.send_input","result":{"type":"ok"}}"#,
        )],
    );
    let runtime = fake.runtime(Duration::from_secs(2));
    assert!(
        runtime.capabilities().contains(&LAUNCH_CAPABILITY),
        "an adapter that implements launch must declare it: {:?}",
        runtime.capabilities()
    );

    let dir = fake.path("child");
    fs::create_dir_all(&dir).expect("child directory");
    let out = dir.join("argv.txt");
    let child = launch_child(&dir, &out);
    let request = launch_request(
        "wA:p1",
        &child,
        &LAUNCH_ARGV,
        "0f8fad5b-d9cb-469f-a165-70867728950e",
    );

    assert_eq!(
        runtime.launch(&request, &cancel()),
        LaunchOutcome::Completed
    );

    let requests = seen.lock().expect("recorder").clone();
    assert_eq!(
        requests.len(),
        1,
        "a launch is one request, so an accepted line cannot lose its Enter: {requests:?}"
    );
    let sent: serde_json::Value = serde_json::from_str(&requests[0]).expect("decode the request");
    assert_eq!(sent["method"], "pane.send_input");
    assert_eq!(sent["params"]["pane_id"], "wA:p1");
    assert_eq!(sent["params"]["keys"], serde_json::json!(["enter"]));
    let line = sent["params"]["text"]
        .as_str()
        .expect("the typed line is text")
        .to_string();
    // The measured rule: every element single-quoted, on one line, with the
    // token exported to the child by the same line.
    assert_eq!(
        line,
        format!(
            "PI_RADAR_SPAWN_TOKEN='0f8fad5b-d9cb-469f-a165-70867728950e' '{}' \
             'a b' 'sq'\\''uote' '$(echo INJECTED)' 'semi;echo SPLIT' 'star*glob' \
             'model=omniroute/coder-high'",
            child.display()
        )
    );
    assert!(!line.contains('\n'), "a launch types one line: {line:?}");

    // The oracle for the paraphrase: a shell reading that line must produce the
    // resolved argv byte-exact, which is what the pane's own shell has to do.
    let shell = Command::new("sh")
        .arg("-c")
        .arg(&line)
        .status()
        .expect("run the typed line in a shell");
    assert!(shell.success(), "the typed line did not run: {line}");
    let expected = format!(
        "TOKEN={}\n{}COUNT={}\n",
        request.spawn_token,
        request
            .argv
            .iter()
            .map(|argument| format!("ARG[{argument}]\n"))
            .collect::<String>(),
        request.argv.len()
    );
    assert_eq!(
        fs::read_to_string(&out).expect("the child recorded its argv"),
        expected,
        "the pane shell must yield the resolved argv unchanged"
    );
}

#[test]
fn a_launch_whose_argv_holds_a_newline_is_refused_before_any_pane_input() {
    let _serial = serial();
    let fake = status_fake();
    let seen = answering_stub_at(&fake.path("herdr.sock"), &[]);
    let runtime = fake.runtime(Duration::from_secs(2));
    let child = fake.path("child");
    let request = launch_request("wA:p1", &child, &["first\nsecond"], "token");

    let LaunchOutcome::Refused(message) = runtime.launch(&request, &cancel()) else {
        panic!("a multi-line command has no fallback and must be refused")
    };
    assert!(message.contains("newline"), "{message}");
    assert!(
        seen.lock().expect("recorder").is_empty(),
        "a refused launch may not touch the pane"
    );

    // The single-line command is the whole supported surface, and it still runs.
    let request = launch_request("wA:p1", &child, &["line"], "token");
    assert_eq!(
        runtime.launch(&request, &cancel()),
        LaunchOutcome::Completed
    );
}

#[test]
fn a_launch_that_may_have_been_dispatched_without_an_answer_is_unknown() {
    let _serial = serial();
    let fake = status_fake();
    let _ = stub_server_at(&fake.path("herdr.sock"), None);
    let runtime = fake.runtime(Duration::from_millis(300));
    let request = launch_request("wA:p1", &fake.path("child"), &[], "token");
    let LaunchOutcome::Unknown(message) = runtime.launch(&request, &cancel()) else {
        panic!("a lost socket reply cannot prove the command was not typed")
    };
    assert!(message.contains("timed out"), "{message}");
}

#[test]
fn a_launch_the_backend_refuses_reports_its_refusal() {
    let _serial = serial();
    let fake = status_fake();
    let _ = stub_server_at(
        &fake.path("herdr.sock"),
        Some(
            r#"{"id":"radar:pane.send_input","error":{"code":"pane_not_found","message":"pane wA:p1 not found"}}"#,
        ),
    );
    let runtime = fake.runtime(Duration::from_secs(5));
    let request = launch_request("wA:p1", &fake.path("child"), &[], "token");
    let LaunchOutcome::Refused(message) = runtime.launch(&request, &cancel()) else {
        panic!("a socket error answer is a known rejection")
    };
    assert!(message.contains("pane wA:p1 not found"), "{message}");
}

/// The adapter against the operator's live Herdr, gated on `RADAR_HERDR_SMOKE=1`
/// because it creates a real tab in the first workspace and closes it again.
/// The stub tests above pin the wire form; this is the check that a real pane
/// shell — this machine's login shell, not `sh` — runs the typed line and
/// yields the resolved argv byte-exact.
#[test]
fn a_real_herdr_launch_runs_the_resolved_command_in_its_pane() {
    if std::env::var("RADAR_HERDR_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipped: set RADAR_HERDR_SMOKE=1 to launch in a real disposable Herdr tab");
        return;
    }
    let _serial = serial();
    let runtime = HerdrRuntime::new(HerdrConfig {
        executable: PathBuf::from("herdr"),
        command_timeout: Duration::from_secs(10),
    });
    let workspace = runtime
        .inventory(&cancel())
        .expect("a live Herdr answers its snapshot")
        .workspaces
        .first()
        .expect("a live Herdr has a workspace")
        .workspace_id
        .clone();
    let CreateOutcome::Completed(created) = runtime.create(
        &CreateRequest::Tab {
            workspace_id: workspace,
            focus: false,
        },
        &cancel(),
    ) else {
        panic!("the disposable tab was not created")
    };
    let tab = created.tab_id.clone().expect("the tab was named");
    let pane = created.pane_id.clone().expect("its pane was named");

    let dir = std::env::temp_dir().join(format!("radar-herdr-smoke-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("smoke directory");
    let out = dir.join("argv.txt");
    let child = launch_child(&dir, &out);
    let token = random_uuid();
    let request = launch_request(&pane, &child, &LAUNCH_ARGV, &token);

    let launched = runtime.launch(&request, &cancel());
    let arrived = wait_for_file(&out, Duration::from_secs(15));
    let closed = runtime.close(&CloseTarget::Tab(tab), &cancel());

    assert_eq!(launched, LaunchOutcome::Completed);
    assert!(
        closed.is_ok(),
        "the disposable tab was left behind: {closed:?}"
    );
    assert!(arrived, "the launched child wrote nothing");
    let expected = format!(
        "TOKEN={token}\n{}COUNT={}\n",
        request
            .argv
            .iter()
            .map(|argument| format!("ARG[{argument}]\n"))
            .collect::<String>(),
        request.argv.len()
    );
    assert_eq!(
        fs::read_to_string(&out).expect("the child recorded its argv"),
        expected,
        "the pane shell must yield the resolved argv unchanged"
    );
    let _ = fs::remove_dir_all(&dir);
}
