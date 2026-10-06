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
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::model::{ForegroundEvidence, LocalFacts};
use agent_radar::{CloseTarget, HerdrConfig, HerdrRuntime, RuntimeProvider, Target};

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

/// A stub API socket at `path` that records the request it is sent and answers
/// with `answer` — or, with `None`, accepts and never answers.
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
    runtime
        .focus(&Target::Workspace("wA".into()), &cancel())
        .expect("the workspace focus CLI runs");
    assert_eq!(fake.read("focused"), "wA");
}

#[test]
fn a_refused_workspace_focus_reports_why() {
    let _serial = serial();
    let fake = Fake::new(|_| "printf 'controlled refusal\\n' >&2\nexit 3".to_string());
    let runtime = fake.runtime(Duration::from_secs(2));
    let message = runtime
        .focus(&Target::Workspace("wA".into()), &cancel())
        .expect_err("a nonzero exit is a failure");
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
    runtime
        .focus(&Target::Pane("wA:p1".into()), &cancel())
        .expect("the socket answers");
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
    let message = runtime
        .focus(&Target::Pane("wA:p1".into()), &cancel())
        .expect_err("a refusal is a failure");
    assert!(message.contains("pane wA:p1 not found"), "{message}");
}

#[test]
fn a_stalled_focus_answer_times_out() {
    let _serial = serial();
    let fake = status_fake();
    let _ = stub_server_at(&fake.path("herdr.sock"), None);
    let runtime = fake.runtime(Duration::from_millis(300));
    let started = Instant::now();
    let message = runtime
        .focus(&Target::Pane("wA:p1".into()), &cancel())
        .expect_err("a stalled request is a failure");
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
