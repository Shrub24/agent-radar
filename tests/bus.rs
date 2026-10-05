#![cfg(unix)]
//! Listener tests: a stub publisher over a real Unix socket in a private
//! temporary directory, and the events the listener produces from it.
//!
//! Every wait is on a condition with a bounded deadline, so a listener that
//! never delivers fails its test instead of hanging it.

use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_radar::bus::{
    BusState, Listener, MAX_CONNECTIONS, MAX_LINE_BYTES, TaskState, socket_path_in,
};

/// How long a test waits for the listener to deliver.
const DEADLINE: Duration = Duration::from_secs(10);

/// A private directory for one test's socket, removed with the test.
struct PrivateDir {
    path: PathBuf,
}

impl PrivateDir {
    fn new(name: &str) -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("radar-bus-{name}-{}-{unique}", process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp dir");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("mode 0700");
        Self { path }
    }

    fn socket(&self) -> PathBuf {
        self.path.join("radar.sock")
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// A bound listener with the state its events build.
struct Bus {
    listener: Listener,
    state: BusState,
}

impl Bus {
    fn bind(dir: &PrivateDir) -> Self {
        let listener = Listener::bind_at(&dir.socket()).expect("bind the bus socket");
        Self {
            listener,
            state: BusState::new(),
        }
    }

    /// Waits, with a bounded deadline, until the state satisfies `done`.
    fn wait_for(&mut self, what: &str, mut done: impl FnMut(&BusState) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            for event in self.listener.drain() {
                self.state.apply(event);
            }
            if done(&self.state) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}; state is {:?}",
                self.state
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
}

/// The task ids held for `session`.
fn ids(state: &BusState, session: &str) -> Vec<String> {
    state
        .get(session)
        .map(|held| held.tasks.iter().map(|task| task.id.clone()).collect())
        .unwrap_or_default()
}

/// Binds a path that must not bind, returning the diagnostic.
fn bind_error(path: &Path) -> String {
    match Listener::bind_at(path) {
        Err(error) => error,
        Ok(_) => panic!("expected {} not to bind", path.display()),
    }
}

/// A stub publisher over a real socket.
struct Client {
    stream: UnixStream,
}

impl Client {
    fn connect(path: &Path) -> Self {
        let stream = UnixStream::connect(path).expect("connect to the listener");
        stream
            .set_read_timeout(Some(Duration::from_millis(20)))
            .expect("read timeout");
        Self { stream }
    }

    fn send(&mut self, line: &str) {
        self.try_send(line).expect("write a line");
    }

    /// Writes a line, reporting a closed connection instead of panicking.
    ///
    /// A listener that refuses a connection at its cap closes the socket at
    /// accept, so the write that follows races that close and may lose.
    fn try_send(&mut self, line: &str) -> std::io::Result<()> {
        self.stream.write_all(line.as_bytes())?;
        self.stream.write_all(b"\n")
    }

    /// Whether the listener has closed this connection.
    fn ended(&mut self) -> bool {
        let mut byte = [0u8; 1];
        match self.stream.read(&mut byte) {
            Ok(0) => true,
            Ok(_) => panic!("the listener sent data; publishers only write"),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                false
            }
            Err(_) => true,
        }
    }

    /// Waits, with a bounded deadline, for the listener to close this
    /// connection.
    fn wait_ended(&mut self) {
        let deadline = Instant::now() + DEADLINE;
        while !self.ended() {
            assert!(
                Instant::now() < deadline,
                "the listener kept the connection open"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }
}

fn hello(session: &str, pane: Option<&str>) -> String {
    let pane = match pane {
        Some(pane) => format!(r#","pane":"{pane}""#),
        None => String::new(),
    };
    format!(r#"{{"type":"hello","v":1,"session":"{session}"{pane},"ops":[]}}"#)
}

fn list(ids: &[&str]) -> String {
    let tasks: Vec<String> = ids
        .iter()
        .map(|id| format!(r#"{{"id":"{id}","state":"running"}}"#))
        .collect();
    format!(r#"{{"type":"tasks","tasks":[{}]}}"#, tasks.join(","))
}

#[test]
fn socket_path_follows_the_documented_order() {
    let xdg = OsStr::new("/run/user/1000");
    assert_eq!(
        socket_path_in(Some(OsStr::new("/run/some.sock")), Some(xdg), 1000),
        PathBuf::from("/run/some.sock")
    );
    assert_eq!(
        socket_path_in(None, Some(xdg), 1000),
        PathBuf::from("/run/user/1000/agent-radar/radar.sock")
    );
    assert_eq!(
        socket_path_in(None, None, 1000),
        PathBuf::from("/tmp/agent-radar-1000/radar.sock")
    );
}

#[test]
fn the_socket_directory_is_created_private() {
    let dir = PrivateDir::new("mkdir");
    let nested = dir.path.join("bus");
    let socket = nested.join("radar.sock");
    let listener = Listener::bind_at(&socket).expect("bind creates the directory");
    assert_eq!(listener.path(), socket);

    let ours = fs::metadata(&dir.path).expect("our own directory").uid();
    let metadata = fs::symlink_metadata(&nested).expect("the directory exists");
    assert!(metadata.is_dir());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    assert_eq!(metadata.uid(), ours, "the directory is ours");
    assert!(
        fs::symlink_metadata(&socket)
            .expect("the socket exists")
            .file_type()
            .is_socket()
    );

    drop(listener);
    assert!(!socket.exists(), "the socket file goes with the listener");
    assert!(nested.exists(), "the directory outlives it");
}

#[test]
fn a_publisher_greets_and_publishes_its_tasks() {
    let dir = PrivateDir::new("greet");
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&dir.socket());
    client.send(&hello("session-a", Some("wA:p1")));
    client.send(&list(&["bg-1", "bg-2"]));

    bus.wait_for("the session's tasks", |state| {
        state.get("session-a").map(|held| held.tasks.len()) == Some(2)
    });
    let held = bus.state.get("session-a").expect("the session is held");
    assert_eq!(held.pane.as_deref(), Some("wA:p1"));
    assert_eq!(ids(&bus.state, "session-a"), ["bg-1", "bg-2"]);
    assert_eq!(held.tasks[0].state, TaskState::Running);
    // A greeting without a pane is held without one.
    let mut other = Client::connect(&dir.socket());
    other.send(&hello("session-b", None));
    bus.wait_for("the second session", |state| {
        state.get("session-b").is_some()
    });
    assert_eq!(bus.state.get("session-b").expect("held").pane, None);
}

#[test]
fn a_tasks_message_replaces_the_previous_list() {
    let dir = PrivateDir::new("replace");
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&dir.socket());
    client.send(&hello("session-a", None));
    client.send(&list(&["bg-1", "bg-2"]));
    bus.wait_for("the first list", |state| ids(state, "session-a").len() == 2);

    client.send(&list(&["bg-3"]));
    bus.wait_for("the replacement list", |state| {
        ids(state, "session-a") == ["bg-3"]
    });
}

#[test]
fn an_empty_list_is_kept_as_an_empty_list() {
    let dir = PrivateDir::new("empty");
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&dir.socket());
    client.send(&hello("session-a", None));
    client.send(&list(&["bg-1"]));
    bus.wait_for("the first list", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    client.send(&list(&[]));
    bus.wait_for("the published empty list", |state| {
        state
            .get("session-a")
            .is_some_and(|held| held.tasks.is_empty())
    });
    // The empty list is a message, not silence: the connection is still live.
    client.send(&list(&["bg-2"]));
    bus.wait_for("the list after the empty one", |state| {
        ids(state, "session-a") == ["bg-2"]
    });
}

#[test]
fn a_disconnect_removes_the_session() {
    let dir = PrivateDir::new("disconnect");
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&dir.socket());
    client.send(&hello("session-a", None));
    client.send(&list(&["bg-1"]));
    bus.wait_for("the session's tasks", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    drop(client);
    bus.wait_for("the session to go", |state| {
        state.get("session-a").is_none()
    });
    assert!(bus.state.sessions().count() == 0);
}

#[test]
fn a_second_radar_owns_the_socket() {
    let dir = PrivateDir::new("second");
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&dir.socket());
    client.send(&hello("session-a", None));
    client.send(&list(&["bg-1"]));
    bus.wait_for("the first list", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    let error = bind_error(&dir.socket());
    assert!(error.contains("another Radar"), "{error}");

    // The first listener still owns the file and still serves it.
    client.send(&list(&["bg-2"]));
    bus.wait_for("the first listener's second list", |state| {
        ids(state, "session-a") == ["bg-2"]
    });
}

#[test]
fn a_stale_socket_is_replaced() {
    let dir = PrivateDir::new("stale");
    let socket = dir.socket();
    // A Radar that exited without cleaning up leaves the file behind.
    drop(UnixListener::bind(&socket).expect("bind the socket to leave behind"));
    assert!(socket.exists());

    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&socket);
    client.send(&hello("session-a", None));
    client.send(&list(&["bg-1"]));
    bus.wait_for("the session on the rebound socket", |state| {
        ids(state, "session-a") == ["bg-1"]
    });
}

#[test]
fn a_loose_directory_is_no_bus() {
    let dir = PrivateDir::new("loose");
    fs::set_permissions(&dir.path, fs::Permissions::from_mode(0o755)).expect("loosen");
    let error = bind_error(&dir.socket());
    assert!(error.contains("0700"), "{error}");
    assert!(
        !dir.socket().exists(),
        "a directory we do not trust gets nothing bound in it"
    );

    fs::set_permissions(&dir.path, fs::Permissions::from_mode(0o700)).expect("tighten");
    let listener = Listener::bind_at(&dir.socket()).expect("bind once the directory is ours");
    assert_eq!(listener.path(), dir.socket());
}

#[test]
fn a_symlinked_or_foreign_directory_is_no_bus() {
    let dir = PrivateDir::new("symlink");
    let real = dir.path.join("real");
    fs::create_dir(&real).expect("the real directory");
    fs::set_permissions(&real, fs::Permissions::from_mode(0o700)).expect("mode 0700");
    let link = dir.path.join("link");
    std::os::unix::fs::symlink(&real, &link).expect("the symlink");

    let error = bind_error(&link.join("radar.sock"));
    assert!(error.contains("symlink"), "{error}");
    assert!(!real.join("radar.sock").exists());
}

#[test]
fn a_file_that_is_not_a_socket_is_left_alone() {
    let dir = PrivateDir::new("not-a-socket");
    let path = dir.socket();
    fs::write(&path, b"not a socket").expect("a stray file");
    let error = bind_error(&path);
    assert!(error.contains("not a socket"), "{error}");
    assert_eq!(
        fs::read(&path).expect("the file is still there"),
        b"not a socket"
    );
}

#[test]
fn a_new_session_in_the_same_pane_replaces_the_old_one() {
    let dir = PrivateDir::new("same-pane");
    let mut bus = Bus::bind(&dir);
    let mut first = Client::connect(&dir.socket());
    first.send(&hello("session-a", Some("wA:p1")));
    first.send(&list(&["bg-1"]));
    bus.wait_for("the first session", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    // `/new` in the same pane: the old connection closes and a new session
    // arrives on its own connection.
    drop(first);
    bus.wait_for("the first session to go", |state| {
        state.get("session-a").is_none()
    });
    let mut second = Client::connect(&dir.socket());
    second.send(&hello("session-b", Some("wA:p1")));
    second.send(&list(&["bg-9"]));
    bus.wait_for("the new session in the same pane", |state| {
        ids(state, "session-b") == ["bg-9"] && state.get("session-a").is_none()
    });
    assert_eq!(
        bus.state
            .get("session-b")
            .expect("the new session is held")
            .pane
            .as_deref(),
        Some("wA:p1")
    );
}

#[test]
fn a_duplicate_session_is_won_by_the_newest_connection() {
    let dir = PrivateDir::new("duplicate");
    let mut bus = Bus::bind(&dir);
    let mut first = Client::connect(&dir.socket());
    first.send(&hello("session-a", None));
    first.send(&list(&["bg-1"]));
    bus.wait_for("the first connection", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    // A second publisher claims the same session: the newest connection wins
    // and the older one is closed, so the session has one writer.
    let mut second = Client::connect(&dir.socket());
    second.send(&hello("session-a", None));
    first.wait_ended();
    second.send(&list(&["bg-2"]));
    bus.wait_for("the newest connection's list", |state| {
        ids(state, "session-a") == ["bg-2"]
    });
    // The displaced connection's end does not take the session with it.
    bus.wait_for("the session to stay", |state| {
        state.get("session-a").is_some()
    });
}

#[test]
fn the_connection_cap_closes_extra_connections() {
    let dir = PrivateDir::new("cap");
    let mut bus = Bus::bind(&dir);
    let mut sessions = Vec::new();
    let mut held = Vec::new();
    for index in 0..MAX_CONNECTIONS {
        let session = format!("session-{index}");
        let mut client = Client::connect(&dir.socket());
        client.send(&hello(&session, None));
        sessions.push(session);
        held.push(client);
    }
    // A hello per connection, so every one of them is proven held before the
    // cap can be tested.
    bus.wait_for("every connection held", |state| {
        state.sessions().count() == MAX_CONNECTIONS
    });

    let mut extra = Client::connect(&dir.socket());
    // A listener at its cap closes the connection at accept, so this write may
    // already meet a closed socket; the close is the point, not the write.
    let _ = extra.try_send(&hello("session-extra", None));
    extra.wait_ended();
    bus.wait_for("nothing from the extra connection", |state| {
        state.get("session-extra").is_none()
    });

    // One connection ends and frees a slot for the next one.
    let dropped = sessions.pop().expect("the last session of a full listener");
    drop(held.pop().expect("the last connection of a full listener"));
    bus.wait_for("the dropped session to go", |state| {
        state.get(&dropped).is_none()
    });
    let mut later = Client::connect(&dir.socket());
    later.send(&hello("session-later", None));
    bus.wait_for("the freed slot", |state| {
        state.get("session-later").is_some()
    });
}

/// Runs `script` on a second connection and checks that it ends while the
/// first one keeps publishing: a protocol error closes that connection only.
fn a_bad_connection_closes_alone(name: &str, script: impl FnOnce(&mut Client)) {
    let dir = PrivateDir::new(name);
    let mut bus = Bus::bind(&dir);
    let mut good = Client::connect(&dir.socket());
    good.send(&hello("session-a", None));
    good.send(&list(&["bg-1"]));
    bus.wait_for("the good connection", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    let mut bad = Client::connect(&dir.socket());
    script(&mut bad);
    bad.wait_ended();

    good.send(&list(&["bg-2"]));
    bus.wait_for("the good connection's second list", |state| {
        ids(state, "session-a") == ["bg-2"] && state.sessions().count() == 1
    });
}

#[test]
fn bad_json_closes_only_that_connection() {
    a_bad_connection_closes_alone("bad-json", |bad| bad.send("not json at all"));
}

#[test]
fn an_unsupported_version_closes_only_that_connection() {
    a_bad_connection_closes_alone("version", |bad| {
        bad.send(r#"{"type":"hello","v":2,"session":"session-b"}"#)
    });
}

#[test]
fn an_oversize_line_closes_only_that_connection() {
    a_bad_connection_closes_alone("oversize", |bad| {
        bad.send(&hello("session-b", None));
        // Past the cap the connection is closed mid-line instead of buffered,
        // so this write may fail: that is the point.
        let _ = bad.stream.write_all(&vec![b'a'; MAX_LINE_BYTES + 1]);
    });
}

#[test]
fn tasks_before_hello_closes_only_that_connection() {
    a_bad_connection_closes_alone("before-hello", |bad| bad.send(&list(&["bg-9"])));
}

#[test]
fn an_unknown_message_type_keeps_the_connection() {
    let dir = PrivateDir::new("unknown");
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&dir.socket());
    client.send(&hello("session-a", None));
    client.send(r#"{"type":"future","detail":"ignored"}"#);
    // Still on the same connection, and the unknown line changed nothing.
    client.send(&list(&["bg-1"]));
    bus.wait_for("the list after the unknown line", |state| {
        ids(state, "session-a") == ["bg-1"]
    });
}

#[test]
fn stopping_the_listener_ends_its_connections_and_removes_its_socket() {
    let dir = PrivateDir::new("stop");
    let socket = dir.socket();
    let mut bus = Bus::bind(&dir);
    let mut client = Client::connect(&socket);
    client.send(&hello("session-a", None));
    client.send(&list(&["bg-1"]));
    bus.wait_for("the connection", |state| {
        ids(state, "session-a") == ["bg-1"]
    });

    // A stop waits for no client: the accept thread polls and the blocked read
    // is ended by shutting the connection's socket down.
    let started = Instant::now();
    bus.listener.stop();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "stopping waited on a client"
    );
    assert!(!socket.exists(), "the socket file goes with the listener");
    client.wait_ended();
    bus.listener.stop();

    // The same through the drop path, with a connection open.
    let second_socket = dir.path.join("second.sock");
    let second = Listener::bind_at(&second_socket).expect("bind the second socket");
    let mut second_client = Client::connect(&second_socket);
    drop(second);
    second_client.wait_ended();
    assert!(!second_socket.exists());
}
