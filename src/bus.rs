//! Radar bus line protocol — the only module that sees raw publisher bytes.
//!
//! Publishers dial Radar over a Unix socket and write one JSON object per line
//! (`docs/radar-bus.md`). [`Connection::ingest`] decodes a line into a typed
//! [`Message`] and applies the connection rules, so a reader only has to write
//! the line it read and close on `Err`. Unknown message types and unknown
//! fields are not errors: a newer publisher must not break an older Radar.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

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
    /// Optional publisher hint; never used as task identity or a join key.
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

/// One session's live bus data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BusSession {
    /// The pane the publisher named, where it had one; never used for joining.
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

/// One item in the daemon task snapshot projected for the TUI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BusEvent {
    /// The daemon reports a connected publisher for this session.
    Connected {
        session: String,
        pane: Option<String>,
    },
    /// The session's full replacement list.
    Tasks { session: String, tasks: Vec<Task> },
    /// No connected publisher remains for this session.
    Disconnected { session: String },
}

/// Client-side projection of the daemon's volatile task snapshot.
#[derive(Clone, Debug, Default)]
pub struct BusState {
    sessions: BTreeMap<String, BusSession>,
}

impl BusState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one item from a fetched daemon task snapshot.
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
    rustix::process::getuid().as_raw()
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
