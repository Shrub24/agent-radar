//! Radar bus line protocol — the only module that sees raw publisher bytes.
//!
//! Publishers dial Radar over a Unix socket and write one JSON object per line
//! (`docs/radar-bus.md`). [`Connection::ingest`] decodes a line into a typed
//! [`Message`] and applies the connection rules, so a reader only has to write
//! the line it read and close on `Err`. Unknown message types and unknown
//! fields are not errors: a newer publisher must not break an older Radar.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::io::{ErrorKind, Read};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use serde::Deserialize;

/// The only protocol version this Radar accepts.
pub const PROTOCOL_VERSION: u32 = 1;

/// Line cap, in bytes, excluding the terminator. A longer line closes the
/// connection.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// A connection's opening message: the session whose tasks follow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// Protocol version; [`PROTOCOL_VERSION`] by construction.
    pub version: u32,
    /// The exact Pi session UUID, as published. Changes on `/new` and `/resume`.
    pub session: String,
    /// Herdr pane id where the publisher has one; a join fallback only.
    pub pane: Option<String>,
}

/// A publisher's task state word.
///
/// The three known words are the publisher's vocabulary; any other word is a
/// newer publisher's and is kept verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Flushing,
    Review,
    Unknown(String),
}

impl std::fmt::Display for TaskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Running => "running",
            Self::Flushing => "flushing",
            Self::Review => "review",
            Self::Unknown(word) => word,
        })
    }
}

/// One unresolved task as published. Every field except `id` and `state` is
/// optional, and absent is not the same as a zero value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub id: String,
    pub state: TaskState,
    /// Sensitive: written only to the terminal, never to a file or a log.
    pub command: Option<String>,
    /// Sensitive, as `command`.
    pub cwd: Option<String>,
    pub pid: Option<u32>,
    /// Unix milliseconds.
    pub started_at: Option<u64>,
    /// Unix milliseconds of the last output.
    pub last_output_at: Option<u64>,
    pub output_bytes: Option<u64>,
    pub exit_code: Option<i32>,
}

/// A decoded message. `Tasks` replaces the session's list; an empty list is a
/// real message, not silence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Hello(Hello),
    Tasks(Vec<Task>),
}

/// Why a line must close its connection.
#[derive(Debug)]
pub enum ProtocolError {
    /// The line exceeded [`MAX_LINE_BYTES`].
    TooLong,
    /// Not valid JSON, or missing a required field.
    Malformed(serde_json::Error),
    /// `hello` named a version other than [`PROTOCOL_VERSION`], or none at all.
    UnsupportedVersion(Option<u32>),
    /// A known message other than `hello` arrived before the connection's hello.
    MissingHello,
    /// A second `hello` on one connection.
    DuplicateHello,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLong => write!(f, "line exceeds {MAX_LINE_BYTES} bytes"),
            Self::Malformed(error) => write!(f, "not a valid bus line: {error}"),
            Self::UnsupportedVersion(Some(v)) => write!(f, "unsupported protocol version: {v}"),
            Self::UnsupportedVersion(None) => write!(f, "hello without a protocol version"),
            Self::MissingHello => write!(f, "message before hello"),
            Self::DuplicateHello => write!(f, "second hello on one connection"),
        }
    }
}

impl std::error::Error for ProtocolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Malformed(error) => Some(error),
            _ => None,
        }
    }
}

/// One connection's protocol state; feed it the lines it reads, in order.
#[derive(Debug, Default)]
pub struct Connection {
    greeted: bool,
}

impl Connection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes `line` (without its terminator) and applies the connection
    /// rules.
    ///
    /// `Ok(None)` is a valid line Radar does not act on — an unknown message
    /// type — and the connection stays open. `Err` names the reason this
    /// connection must be closed; other connections are unaffected.
    pub fn ingest(&mut self, line: &[u8]) -> Result<Option<Message>, ProtocolError> {
        match decode_line(line)? {
            None => Ok(None),
            Some(Message::Hello(hello)) => {
                if self.greeted {
                    return Err(ProtocolError::DuplicateHello);
                }
                self.greeted = true;
                Ok(Some(Message::Hello(hello)))
            }
            Some(Message::Tasks(tasks)) => {
                if !self.greeted {
                    return Err(ProtocolError::MissingHello);
                }
                Ok(Some(Message::Tasks(tasks)))
            }
        }
    }
}

/// Decodes one line, without the connection rules: the cap, the version check
/// and the message shape. `Ok(None)` is an unknown message type.
pub fn decode_line(line: &[u8]) -> Result<Option<Message>, ProtocolError> {
    if line.len() > MAX_LINE_BYTES {
        return Err(ProtocolError::TooLong);
    }
    match serde_json::from_slice::<Line>(line).map_err(ProtocolError::Malformed)? {
        Line::Hello(hello) => {
            if hello.v != Some(PROTOCOL_VERSION) {
                return Err(ProtocolError::UnsupportedVersion(hello.v));
            }
            Ok(Some(Message::Hello(Hello {
                version: PROTOCOL_VERSION,
                session: hello.session,
                pane: hello.pane,
            })))
        }
        Line::Tasks(tasks) => Ok(Some(Message::Tasks(
            tasks.tasks.into_iter().map(Task::from).collect(),
        ))),
        Line::Unknown => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Private wire shapes: unknown fields and unknown message types are tolerated.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Line {
    Hello(HelloWire),
    Tasks(TasksWire),
    /// A type this version does not know; ignored on a valid connection.
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
struct HelloWire {
    v: Option<u32>,
    session: String,
    pane: Option<String>,
}

#[derive(Deserialize)]
struct TasksWire {
    tasks: Vec<TaskWire>,
}

#[derive(Deserialize)]
struct TaskWire {
    id: String,
    state: String,
    command: Option<String>,
    cwd: Option<String>,
    pid: Option<u32>,
    started_at: Option<u64>,
    last_output_at: Option<u64>,
    output_bytes: Option<u64>,
    exit_code: Option<i32>,
}

impl From<TaskWire> for Task {
    fn from(wire: TaskWire) -> Self {
        Self {
            id: wire.id,
            state: match wire.state.as_str() {
                "running" => TaskState::Running,
                "flushing" => TaskState::Flushing,
                "review" => TaskState::Review,
                _ => TaskState::Unknown(wire.state),
            },
            command: wire.command,
            cwd: wire.cwd,
            pid: wire.pid,
            started_at: wire.started_at,
            last_output_at: wire.last_output_at,
            output_bytes: wire.output_bytes,
            exit_code: wire.exit_code,
        }
    }
}

// ---------------------------------------------------------------------------
// Listener: one accept thread, one thread per connection, and the state the
// app loop applies their events to. Nothing here is wired into the app yet.
// ---------------------------------------------------------------------------

/// At most this many connections at once; further ones are closed.
pub const MAX_CONNECTIONS: usize = 16;

/// How much of a connection is read at a time. The line cap is enforced on the
/// bytes read so far, so a publisher cannot make Radar buffer an unbounded line.
const READ_CHUNK: usize = 8 * 1024;

/// How long the accept thread waits for a connection before it rechecks
/// whether the listener was stopped. `accept` on a nonblocking socket would
/// otherwise spin, and a blocking one could not be stopped at all.
const ACCEPT_POLL_MS: libc::c_int = 100;

/// One session's live bus data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusSession {
    /// The pane the publisher named, where it had one. A join fallback only.
    pub pane: Option<String>,
    /// The session's complete unresolved list as last published. An empty list
    /// is a published "nothing unresolved", not silence.
    pub tasks: Vec<Task>,
}

impl BusSession {
    /// How many of the session's tasks are `running`. The `pi_bg_running` pane
    /// token counts nothing else, so a comparison against the tokens is made
    /// on this and not on the whole list.
    pub fn running(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.state == TaskState::Running)
            .count()
    }
}

/// What a connection reports to the app loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BusEvent {
    /// A connection greeted: this session now has a live publisher.
    Connected {
        session: String,
        pane: Option<String>,
    },
    /// The session's full replacement list.
    Tasks { session: String, tasks: Vec<Task> },
    /// The connection closed: hold nothing for this session.
    Disconnected { session: String },
}

/// The bus data held beside the observation, one entry per live connection.
#[derive(Clone, Debug, Default)]
pub struct BusState {
    sessions: BTreeMap<String, BusSession>,
}

impl BusState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one event from the listener.
    ///
    /// A session's data exists only while its connection does: a disconnect
    /// drops it entirely rather than leaving a last known list behind, so a
    /// vanished publisher is never read as its tasks having ended.
    pub fn apply(&mut self, event: BusEvent) {
        match event {
            BusEvent::Connected { session, pane } => {
                self.sessions.insert(
                    session,
                    BusSession {
                        pane,
                        tasks: Vec::new(),
                    },
                );
            }
            BusEvent::Tasks { session, tasks } => {
                if let Some(held) = self.sessions.get_mut(&session) {
                    held.tasks = tasks;
                }
            }
            BusEvent::Disconnected { session } => {
                self.sessions.remove(&session);
            }
        }
    }

    /// The session's bus data, where a publisher is connected for it.
    pub fn get(&self, session: &str) -> Option<&BusSession> {
        self.sessions.get(session)
    }

    /// Every held session, for rendering beside the observation.
    pub fn sessions(&self) -> impl Iterator<Item = (&str, &BusSession)> {
        self.sessions
            .iter()
            .map(|(session, held)| (session.as_str(), held))
    }
}

/// The socket file's name inside the directory Radar owns.
const SOCKET_NAME: &str = "radar.sock";

/// The socket path, in the documented order: `RADAR_SOCKET`, else
/// `$XDG_RUNTIME_DIR/agent-radar/radar.sock`, else
/// `/tmp/agent-radar-<uid>/radar.sock`.
pub fn socket_path() -> PathBuf {
    socket_path_in(
        std::env::var_os("RADAR_SOCKET")
            .filter(|value| !value.is_empty())
            .as_deref(),
        std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .as_deref(),
        current_uid(),
    )
}

/// [`socket_path`] over an environment the caller supplies, so the rule can be
/// read and tested without touching the process environment.
pub fn socket_path_in(
    radar_socket: Option<&OsStr>,
    xdg_runtime_dir: Option<&OsStr>,
    uid: u32,
) -> PathBuf {
    if let Some(path) = radar_socket {
        return PathBuf::from(path);
    }
    match xdg_runtime_dir {
        Some(runtime_dir) => Path::new(runtime_dir).join("agent-radar").join(SOCKET_NAME),
        None => Path::new("/tmp")
            .join(format!("agent-radar-{uid}"))
            .join(SOCKET_NAME),
    }
}

/// The current uid, the value the socket directory must be owned by.
fn current_uid() -> u32 {
    // SAFETY: `getuid` takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

/// The listener's bookkeeping, shared with its accept and connection threads.
#[derive(Default)]
struct Live {
    /// Accepted connections, kept so stopping can end their blocked reads.
    connections: HashMap<u64, UnixStream>,
    /// Which connection last greeted for a session.
    owners: HashMap<String, u64>,
    workers: Vec<JoinHandle<()>>,
    next_id: u64,
    stopped: bool,
}

fn locked(live: &Arc<Mutex<Live>>) -> MutexGuard<'_, Live> {
    // Every critical section is map work and event sends, so a poison here
    // means a panic in the accept thread's own code; the listener keeps going.
    live.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The local bus listener.
///
/// [`Listener::bind`] verifies the socket's directory before binding, so a
/// directory that is not ours is an error and never a looser fallback. Each
/// accepted connection is served on its own thread, at most
/// [`MAX_CONNECTIONS`] at once, and its decoded messages arrive from
/// [`Listener::drain`] as [`BusEvent`]s for the app loop to apply to a
/// [`BusState`].
pub struct Listener {
    path: PathBuf,
    live: Arc<Mutex<Live>>,
    events: Receiver<BusEvent>,
    accept: Option<JoinHandle<()>>,
}

impl Listener {
    /// Binds the socket at [`socket_path`].
    pub fn bind() -> Result<Self, String> {
        Self::bind_at(&socket_path())
    }

    /// Binds the socket at `path`, creating and verifying its directory.
    ///
    /// `Err` is the diagnostic for running without the bus: a directory that
    /// is not a real `0700` directory of this user, a socket another Radar
    /// answers on, or a bind that failed.
    pub fn bind_at(path: &Path) -> Result<Self, String> {
        let directory = match path.parent() {
            Some(directory) if !directory.as_os_str().is_empty() => directory,
            _ => Path::new("."),
        };
        prepare_directory(directory)?;
        let listener = bind_socket(path)?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("bus socket {}: {error}", path.display()))?;
        let (sender, events) = mpsc::channel();
        let live = Arc::new(Mutex::new(Live::default()));
        let accept = thread::Builder::new()
            .name("radar-bus-accept".to_string())
            .spawn({
                let live = Arc::clone(&live);
                move || accept_loop(listener, live, sender)
            })
            .map_err(|error| format!("bus accept thread: {error}"))?;
        Ok(Self {
            path: path.to_path_buf(),
            live,
            events,
            accept: Some(accept),
        })
    }

    /// The path this listener bound.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Takes the events that have arrived since the last call.
    pub fn drain(&self) -> Vec<BusEvent> {
        self.events.try_iter().collect()
    }

    /// Stops listening, ends every connection, and removes the socket file
    /// this listener created. Idempotent; dropping the listener stops it.
    pub fn stop(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        locked(&self.live).stopped = true;
        // The accept thread is at most one poll timeout away from noticing, and
        // it is joined before the connections are taken, so it cannot add one
        // behind this drain.
        let _ = accept.join();
        let (connections, workers): (Vec<UnixStream>, Vec<JoinHandle<()>>) = {
            let mut live = locked(&self.live);
            (
                live.connections.drain().map(|(_, seen)| seen).collect(),
                std::mem::take(&mut live.workers),
            )
        };
        // Shutting a connection's socket down ends the read its thread is
        // blocked in, so every worker exits on its own and joining cannot hang.
        for connection in &connections {
            let _ = connection.shutdown(Shutdown::Both);
        }
        for worker in workers {
            let _ = worker.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Creates `directory` `0700` where it is missing, then requires it to be a
/// real directory owned by this user with mode `0700`.
///
/// The publisher dials this exact path, so a directory that fails the check is
/// no bus at all: binding in a looser one would hand it whatever the publisher
/// sends, `command` and `cwd` included.
fn prepare_directory(directory: &Path) -> Result<(), String> {
    let diagnostic =
        |error: std::io::Error| format!("bus socket directory {}: {error}", directory.display());
    if !directory.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(diagnostic)?;
        // The umask can take bits off the creation mode, and the mode is what
        // the publisher checks, so set what is required explicitly.
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(diagnostic)?;
    }
    let metadata = std::fs::symlink_metadata(directory).map_err(diagnostic)?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "bus socket directory {} is a symlink",
            directory.display()
        ));
    }
    if !metadata.is_dir() {
        return Err(format!(
            "bus socket directory {} is not a directory",
            directory.display()
        ));
    }
    if metadata.uid() != current_uid() {
        return Err(format!(
            "bus socket directory {} is owned by uid {}",
            directory.display(),
            metadata.uid()
        ));
    }
    if metadata.permissions().mode() & 0o777 != 0o700 {
        return Err(format!(
            "bus socket directory {} is not mode 0700",
            directory.display()
        ));
    }
    Ok(())
}

/// Binds `path`, first clearing a socket file a dead Radar left behind.
///
/// A file that answers a connection belongs to a live Radar and is left
/// untouched; a refused connection means the file is stale and it is removed.
fn bind_socket(path: &Path) -> Result<UnixListener, String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_socket() => {
            return Err(format!(
                "bus socket {} exists and is not a socket",
                path.display()
            ));
        }
        Ok(_) => match UnixStream::connect(path) {
            Ok(_) => {
                return Err(format!(
                    "bus socket {}: another Radar owns it",
                    path.display()
                ));
            }
            Err(error) if error.kind() == ErrorKind::ConnectionRefused => {
                std::fs::remove_file(path)
                    .map_err(|error| format!("bus socket {}: {error}", path.display()))?;
            }
            Err(error) => return Err(format!("bus socket {}: {error}", path.display())),
        },
        Err(_) => {}
    }
    UnixListener::bind(path).map_err(|error| format!("bus socket {}: {error}", path.display()))
}

/// Accepts connections until the listener stops, one thread each. A connection
/// beyond [`MAX_CONNECTIONS`] is closed instead of served.
fn accept_loop(listener: UnixListener, live: Arc<Mutex<Live>>, events: Sender<BusEvent>) {
    loop {
        if locked(&live).stopped {
            return;
        }
        if !connection_waiting(&listener) {
            continue;
        }
        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                // The backlog this poll woke for is drained; poll again.
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                // Not a signal or an empty backlog: the listening socket itself
                // failed, so stop accepting rather than spin on it.
                Err(_) => return,
            };
            let Ok(seen) = stream.try_clone() else {
                continue;
            };
            let id = {
                let mut live = locked(&live);
                if live.connections.len() >= MAX_CONNECTIONS {
                    // Dropping the stream closes it: the publisher sees an ended
                    // connection rather than a served one.
                    continue;
                }
                // Finished workers are dropped rather than joined here, so
                // reconnects cannot grow the list for a long-lived listener.
                live.workers.retain(|worker| !worker.is_finished());
                let id = live.next_id;
                live.next_id += 1;
                live.connections.insert(id, seen);
                id
            };
            let spawned = thread::Builder::new()
                .name("radar-bus-connection".to_string())
                .spawn({
                    let live = Arc::clone(&live);
                    let events = events.clone();
                    move || serve(id, stream, &live, &events)
                });
            match spawned {
                Ok(worker) => locked(&live).workers.push(worker),
                // No thread came up: close the connection instead of holding a
                // slot for it until the listener stops.
                Err(_) => {
                    locked(&live).connections.remove(&id);
                }
            }
        }
    }
}

/// Whether a connection is ready, waiting at most [`ACCEPT_POLL_MS`].
///
/// The socket is nonblocking and the wait is a poll, not an `accept`, so the
/// accept thread can notice a stop with no client connecting.
fn connection_waiting(listener: &UnixListener) -> bool {
    let mut ready = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one `pollfd` describing our own listening socket, live for the
    // call, and a count of one.
    let polled = unsafe { libc::poll(&mut ready, 1, ACCEPT_POLL_MS) };
    polled > 0
}

/// Serves one connection to its end: lines in, events out. A protocol error
/// closes this connection only.
fn serve(id: u64, mut stream: UnixStream, live: &Arc<Mutex<Live>>, events: &Sender<BusEvent>) {
    let mut connection = Connection::new();
    // The session this connection greeted, once it has.
    let mut session: Option<String> = None;
    let mut pending: Vec<u8> = Vec::new();
    'connection: loop {
        let mut chunk = [0u8; READ_CHUNK];
        let read = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            // The listener stopped or the publisher went away: either way this
            // connection is over.
            Err(_) => break,
        };
        pending.extend_from_slice(&chunk[..read]);
        while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            match connection.ingest(&line[..line.len() - 1]) {
                Ok(Some(Message::Hello(hello))) => {
                    greet(live, id, &hello.session, hello.pane, events);
                    session = Some(hello.session);
                }
                Ok(Some(Message::Tasks(tasks))) => {
                    // `None` is unreachable: the decoder rejects `tasks` before
                    // a `hello`.
                    let Some(name) = session.as_deref() else {
                        break 'connection;
                    };
                    publish(live, id, name, tasks, events);
                }
                Ok(None) => {}
                Err(_) => break 'connection,
            }
        }
        // A line is capped, so an unterminated run past the cap ends the
        // connection instead of growing this buffer.
        if pending.len() > MAX_LINE_BYTES {
            break;
        }
    }
    end(id, session, live, events);
}

/// Records `id` as `session`'s owner, displacing an earlier live connection.
///
/// The newest connection wins: a publisher that reconnects before Radar has
/// noticed the old connection ending keeps its session, and the displaced
/// connection is closed so the session still has one writer.
fn greet(
    live: &Arc<Mutex<Live>>,
    id: u64,
    session: &str,
    pane: Option<String>,
    events: &Sender<BusEvent>,
) {
    let displaced = {
        let mut live = locked(live);
        let displaced = live.owners.insert(session.to_string(), id);
        // Sent while holding the lock: ownership and the event must not invert
        // between two connections, or the app could apply a session's
        // `Connected` after its own `Disconnected` and hold nothing.
        let _ = events.send(BusEvent::Connected {
            session: session.to_string(),
            pane,
        });
        displaced
    };
    if let Some(old) = displaced.filter(|old| *old != id) {
        // Shutting the displaced connection's socket down here, under the same
        // lock, ends the read its thread is blocked in without blocking this
        // one: it makes the reader fail now rather than wait for its peer.
        let live = locked(live);
        if let Some(seen) = live.connections.get(&old) {
            let _ = seen.shutdown(Shutdown::Both);
        }
    }
}

/// Publishes a full list for `session`, where `id` still owns it. A displaced
/// connection's late lines must not overwrite the connection that replaced it.
fn publish(
    live: &Arc<Mutex<Live>>,
    id: u64,
    session: &str,
    tasks: Vec<Task>,
    events: &Sender<BusEvent>,
) {
    let live = locked(live);
    if live.owners.get(session) == Some(&id) {
        // Sent while holding the lock, as the `Connected` above is.
        let _ = events.send(BusEvent::Tasks {
            session: session.to_string(),
            tasks,
        });
    }
}

/// Unregisters the connection and, where it still owns its session, ends that
/// session's data. A connection displaced by a newer one for the same session
/// must not take the newer one's data with it.
fn end(id: u64, session: Option<String>, live: &Arc<Mutex<Live>>, events: &Sender<BusEvent>) {
    let mut live = locked(live);
    live.connections.remove(&id);
    let owned = session.filter(|session| live.owners.get(session) == Some(&id));
    if let Some(session) = owned.as_ref() {
        live.owners.remove(session);
        let _ = events.send(BusEvent::Disconnected {
            session: session.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/radar-bus.fixture.json");

    /// The published example lines, as a reader hands them over: one line,
    /// without its terminator.
    fn fixture_lines() -> Vec<String> {
        std::fs::read_to_string(FIXTURE)
            .expect("the bus fixture exists")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn decode(line: &str) -> Option<Message> {
        decode_line(line.as_bytes())
            .unwrap_or_else(|error| panic!("fixture line rejected ({error}): {line}"))
    }

    fn fixture_line<F: Fn(&Option<Message>) -> bool>(matches: F, what: &str) -> String {
        fixture_lines()
            .into_iter()
            .find(|line| matches(&decode(line)))
            .unwrap_or_else(|| panic!("the fixture carries a {what}"))
    }

    fn fixture_hello() -> String {
        fixture_line(
            |message| matches!(message, Some(Message::Hello(_))),
            "hello",
        )
    }

    #[test]
    fn fixture_is_one_json_object_per_line() {
        for line in fixture_lines() {
            assert!(
                matches!(
                    serde_json::from_str::<serde_json::Value>(&line),
                    Ok(serde_json::Value::Object(_))
                ),
                "not one JSON object: {line}"
            );
            decode(&line);
        }
    }

    #[test]
    fn fixture_covers_the_contract_cases() {
        let text = std::fs::read_to_string(FIXTURE).expect("the bus fixture exists");
        // The default examples never carry the `pi_bg_tasks` pane-token
        // grammar.
        assert!(!text.contains("pi_bg"), "the fixture publishes pi_bg");

        let messages: Vec<Option<Message>> =
            fixture_lines().iter().map(|line| decode(line)).collect();
        let hellos: Vec<&Hello> = messages
            .iter()
            .filter_map(|message| match message {
                Some(Message::Hello(hello)) => Some(hello),
                _ => None,
            })
            .collect();
        assert_eq!(
            hellos.len(),
            3,
            "hello with a pane, without one, and with an unknown field"
        );
        assert!(hellos.iter().any(|hello| hello.pane.is_none()));
        assert!(hellos.iter().any(|hello| hello.pane.is_some()));

        let lists: Vec<&Vec<Task>> = messages
            .iter()
            .filter_map(|message| match message {
                Some(Message::Tasks(tasks)) => Some(tasks),
                _ => None,
            })
            .collect();
        let tasks: Vec<&Task> = lists.iter().flat_map(|list| list.iter()).collect();
        assert!(tasks.iter().any(|task| task.state == TaskState::Running
            && task.pid.is_some()
            && task.started_at.is_some()
            && task.last_output_at.is_some()
            && task.output_bytes.is_some()));
        assert!(tasks.iter().any(|task| task.state == TaskState::Flushing));
        assert!(
            tasks
                .iter()
                .any(|task| task.state == TaskState::Review && task.exit_code == Some(0))
        );
        // The two display fields the publisher sends on every task, and the
        // fields' absence on another task below.
        assert!(
            tasks
                .iter()
                .any(|task| task.command.is_some() && task.cwd.is_some())
        );
        // Every optional field absent is not the same as a zero value.
        assert!(tasks.iter().any(|task| task.command.is_none()
            && task.cwd.is_none()
            && task.pid.is_none()
            && task.started_at.is_none()
            && task.last_output_at.is_none()
            && task.output_bytes.is_none()
            && task.exit_code.is_none()));
        // An unknown state word is kept as published.
        assert!(
            tasks
                .iter()
                .any(|task| task.state == TaskState::Unknown("paused".into()))
        );
        // An explicit empty list is a real message, not silence.
        assert!(lists.iter().any(|list| list.is_empty()));
        // One unknown message type, ignored instead of rejected.
        assert_eq!(
            messages.iter().filter(|message| message.is_none()).count(),
            1
        );
    }

    #[test]
    fn bad_json_closes_the_connection() {
        let hello = fixture_hello();
        let truncated = &hello[..hello.len() - 1];
        let mut connection = Connection::new();
        assert!(matches!(
            connection.ingest(truncated.as_bytes()),
            Err(ProtocolError::Malformed(_))
        ));
        assert!(matches!(
            connection.ingest(b"not json at all"),
            Err(ProtocolError::Malformed(_))
        ));
        // A required field missing.
        assert!(matches!(
            connection.ingest(br#"{"type":"tasks"}"#),
            Err(ProtocolError::Malformed(_))
        ));
    }

    #[test]
    fn unsupported_version_closes_the_connection() {
        let hello = fixture_hello();
        assert!(hello.contains(r#""v":1"#), "the fixture hello carries `v`");
        let newer = hello.replace(r#""v":1"#, r#""v":2"#);
        assert!(matches!(
            decode_line(newer.as_bytes()),
            Err(ProtocolError::UnsupportedVersion(Some(2)))
        ));
        let mut connection = Connection::new();
        assert!(matches!(
            connection.ingest(newer.as_bytes()),
            Err(ProtocolError::UnsupportedVersion(_))
        ));
        assert!(matches!(
            connection.ingest(br#"{"type":"hello","session":"x"}"#),
            Err(ProtocolError::UnsupportedVersion(None))
        ));
    }

    #[test]
    fn oversize_line_closes_the_connection() {
        // Whitespace past the cap: the cap is checked before the line is parsed.
        let mut line = fixture_hello();
        line.push_str(&" ".repeat(MAX_LINE_BYTES));
        assert!(line.len() > MAX_LINE_BYTES);
        assert!(matches!(
            Connection::new().ingest(line.as_bytes()),
            Err(ProtocolError::TooLong)
        ));
    }

    #[test]
    fn tasks_before_hello_closes_the_connection() {
        let tasks = fixture_line(
            |message| matches!(message, Some(Message::Tasks(_))),
            "tasks list",
        );
        let mut connection = Connection::new();
        assert!(matches!(
            connection.ingest(tasks.as_bytes()),
            Err(ProtocolError::MissingHello)
        ));
        // On a greeted connection the same line is accepted.
        assert!(matches!(
            connection.ingest(fixture_hello().as_bytes()),
            Ok(Some(Message::Hello(_)))
        ));
        assert!(matches!(
            connection.ingest(tasks.as_bytes()),
            Ok(Some(Message::Tasks(_)))
        ));
    }

    #[test]
    fn second_hello_closes_the_connection() {
        let hello = fixture_hello();
        let mut connection = Connection::new();
        assert!(matches!(
            connection.ingest(hello.as_bytes()),
            Ok(Some(Message::Hello(_)))
        ));
        assert!(matches!(
            connection.ingest(hello.as_bytes()),
            Err(ProtocolError::DuplicateHello)
        ));
    }

    #[test]
    fn unknown_types_and_fields_are_ignored_on_a_live_connection() {
        let unknown_type = fixture_line(|message| message.is_none(), "unknown message type");
        let mut connection = Connection::new();
        assert!(matches!(
            connection.ingest(fixture_hello().as_bytes()),
            Ok(Some(Message::Hello(_)))
        ));
        assert!(matches!(
            connection.ingest(unknown_type.as_bytes()),
            Ok(None)
        ));

        let line = br#"{"type":"tasks","extra":"future","tasks":[{"id":"bg-9","state":"running","title":"ignored"}]}"#;
        match connection.ingest(line) {
            Ok(Some(Message::Tasks(tasks))) => {
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].state, TaskState::Running);
            }
            other => panic!("unknown field rejected: {other:?}"),
        }
    }

    #[test]
    fn every_optional_field_decodes_when_published() {
        let line = br#"{"type":"tasks","tasks":[{"id":"bg-9","state":"review","command":"nix build .#radar","cwd":"/home/x/proj","pid":9001,"started_at":10,"last_output_at":20,"output_bytes":30,"exit_code":130}]}"#;
        let Some(Message::Tasks(tasks)) = decode_line(line).expect("a complete task decodes")
        else {
            panic!("expected a tasks message");
        };
        assert_eq!(tasks.len(), 1);
        let task = &tasks[0];
        assert_eq!(task.id, "bg-9");
        assert_eq!(task.state, TaskState::Review);
        assert_eq!(task.command.as_deref(), Some("nix build .#radar"));
        assert_eq!(task.cwd.as_deref(), Some("/home/x/proj"));
        assert_eq!(task.pid, Some(9001));
        assert_eq!(task.started_at, Some(10));
        assert_eq!(task.last_output_at, Some(20));
        assert_eq!(task.output_bytes, Some(30));
        assert_eq!(task.exit_code, Some(130));
        assert_eq!(task.state.to_string(), "review");
    }
}
