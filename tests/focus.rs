//! Focus requests against a fake `herdr` and a stub API socket.
//!
//! The fake stands in for `herdr` on `PATH`: `status --json` reports the socket
//! the stub is listening on, and a focus command does whatever its mode says.
//! Nothing here touches a real Herdr, a real pane or the network.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::{FocusConfig, Focuser, Target};

/// How long one request may take before the test calls it stuck: long enough
/// for a process spawn, short enough to keep a failing run quick.
const PATIENCE: Duration = Duration::from_secs(10);

/// A directory of this test's own, removed when the run ends.
fn workspace(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("radar-focus-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("test directory");
    directory
}

/// A fake `herdr`: `workspace focus` follows `mode` (`ok`, `fail`, `hang`), and
/// `status --json` reports `socket`.
fn fake_herdr(directory: &Path, socket: &Path, mode: &str) -> PathBuf {
    let script = match mode {
        "ok" => "exit 0",
        "fail" => "printf 'controlled refusal\\n' >&2; exit 3",
        "hang" => "exec sleep 30",
        other => panic!("unknown mode {other}"),
    };
    let path = directory.join("herdr");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\ncase \"$1 $2\" in\n  \"status --json\") printf '{{\"server\":{{\"socket\":\"{}\"}}}}\\n' ;;\n  \"workspace focus\") {script} ;;\nesac\nexit 0\n",
            socket.display()
        ),
    )
    .expect("fake herdr");
    let mut permissions = std::fs::metadata(&path).expect("fake herdr").permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&path, permissions).expect("fake herdr is executable");
    path
}

/// A stub API socket that records the request it is sent and answers with
/// `answer` — or, with `None`, accepts and never answers.
fn stub_server(
    directory: &Path,
    answer: Option<&'static str>,
) -> (PathBuf, Arc<Mutex<Vec<String>>>) {
    let path = directory.join("herdr.sock");
    let listener = UnixListener::bind(&path).expect("stub socket binds");
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
    (path, seen)
}

/// Runs one request to its outcome, never waiting on Herdr itself.
fn request(config: FocusConfig, target: Target) -> Result<(), String> {
    let mut focuser = Focuser::new(config);
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

fn config(executable: PathBuf, timeout: Duration) -> FocusConfig {
    FocusConfig {
        executable,
        timeout,
    }
}

#[test]
fn a_workspace_focus_runs_the_cli() {
    let directory = workspace("workspace");
    let executable = fake_herdr(&directory, &directory.join("unused.sock"), "ok");
    let outcome = request(
        config(executable, Duration::from_secs(5)),
        Target::Workspace("wA".into()),
    );
    assert_eq!(outcome, Ok(()));
}

#[test]
fn a_failing_cli_reports_why() {
    let directory = workspace("failing-cli");
    let executable = fake_herdr(&directory, &directory.join("unused.sock"), "fail");
    let outcome = request(
        config(executable, Duration::from_secs(5)),
        Target::Workspace("wA".into()),
    );
    let message = outcome.expect_err("a nonzero exit is a failure");
    assert!(message.contains("status 3"), "{message}");
    assert!(message.contains("controlled refusal"), "{message}");
}

#[test]
fn a_pane_focus_asks_the_socket() {
    let directory = workspace("pane");
    let (socket, seen) = stub_server(
        &directory,
        Some(r#"{"id":"radar:focus","result":{"type":"ok"}}"#),
    );
    let executable = fake_herdr(&directory, &socket, "ok");
    let outcome = request(
        config(executable, Duration::from_secs(5)),
        Target::Pane("wA:p1".into()),
    );
    assert_eq!(outcome, Ok(()));
    let request = seen.lock().expect("recorder").join(" ");
    assert!(request.contains("\"method\":\"pane.focus\""), "{request}");
    assert!(request.contains("\"pane_id\":\"wA:p1\""), "{request}");
}

#[test]
fn a_refused_pane_focus_reports_the_refusal() {
    let directory = workspace("refused");
    let (socket, _) = stub_server(
        &directory,
        Some(
            r#"{"id":"radar:focus","error":{"code":"pane_not_found","message":"pane wA:p1 not found"}}"#,
        ),
    );
    let executable = fake_herdr(&directory, &socket, "ok");
    let outcome = request(
        config(executable, Duration::from_secs(5)),
        Target::Pane("wA:p1".into()),
    );
    let message = outcome.expect_err("a refusal is a failure");
    assert!(message.contains("pane wA:p1 not found"), "{message}");
}

#[test]
fn a_stalled_answer_times_out_and_leaves_nothing_running() {
    let directory = workspace("stalled");
    let (socket, _) = stub_server(&directory, None);
    let executable = fake_herdr(&directory, &socket, "ok");
    let started = Instant::now();
    let outcome = request(
        config(executable, Duration::from_millis(300)),
        Target::Pane("wA:p1".into()),
    );
    let message = outcome.expect_err("a stalled request is a failure");
    assert!(message.contains("timed out"), "{message}");
    assert!(started.elapsed() < PATIENCE, "gave up on the deadline");
}

#[test]
fn a_cli_that_hangs_is_killed_and_does_not_delay_shutdown() {
    let directory = workspace("hanging-cli");
    let executable = fake_herdr(&directory, &directory.join("unused.sock"), "hang");
    let mut focuser = Focuser::new(config(executable, Duration::from_secs(30)));
    focuser.start(Target::Workspace("wA".into()));
    thread::sleep(Duration::from_millis(50));

    let started = Instant::now();
    focuser.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown waited on the command instead of cancelling it"
    );
    assert!(
        focuser.poll().is_none(),
        "a cancelled request reports nothing"
    );
}

#[test]
fn a_stalled_socket_does_not_delay_shutdown() {
    let directory = workspace("stalled-shutdown");
    let (socket, seen) = stub_server(&directory, None);
    let executable = fake_herdr(&directory, &socket, "ok");
    let mut focuser = Focuser::new(config(executable, Duration::from_secs(30)));
    focuser.start(Target::Pane("wA:p1".into()));
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.lock().expect("recorder").is_empty() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !seen.lock().expect("recorder").is_empty(),
        "the request was sent"
    );

    let started = Instant::now();
    focuser.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "shutdown waited for the socket instead of cancelling it"
    );
}
