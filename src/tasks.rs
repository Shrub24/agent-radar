//! The daemon's volatile background-task model and the dashboard's projection of it.

use std::collections::BTreeMap;

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

/// One session's live task data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTasks {
    /// The pane the publisher named, where it had one; never used for joining.
    pub pane: Option<String>,
    /// The session's complete unresolved list as last published. An empty list
    /// is a published "nothing unresolved", not silence.
    pub tasks: Vec<Task>,
}

impl SessionTasks {
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
pub enum TaskEvent {
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
pub struct TaskBoard {
    sessions: BTreeMap<String, SessionTasks>,
}

impl TaskBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one item from a fetched daemon task snapshot.
    ///
    /// A session's data exists only while its connection does: a disconnect
    /// drops it entirely rather than leaving a last known list behind, so a
    /// vanished publisher is never read as its tasks having ended.
    pub fn apply(&mut self, event: TaskEvent) {
        match event {
            TaskEvent::Connected { session, pane } => {
                self.sessions.insert(
                    session,
                    SessionTasks {
                        pane,
                        tasks: Vec::new(),
                    },
                );
            }
            TaskEvent::Tasks { session, tasks } => {
                if let Some(held) = self.sessions.get_mut(&session) {
                    held.tasks = tasks;
                }
            }
            TaskEvent::Disconnected { session } => {
                self.sessions.remove(&session);
            }
        }
    }

    /// The session's task data, where a publisher is connected for it.
    pub fn get(&self, session: &str) -> Option<&SessionTasks> {
        self.sessions.get(session)
    }

    /// Every held session, for rendering beside the observation.
    pub fn sessions(&self) -> impl Iterator<Item = (&str, &SessionTasks)> {
        self.sessions
            .iter()
            .map(|(session, held)| (session.as_str(), held))
    }
}
