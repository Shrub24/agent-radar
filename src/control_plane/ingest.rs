//! Volatile extension publications owned by the daemon.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::bus::{Connection, Message};
use crate::tasks::{TaskBoard, TaskEvent};

use super::transport::IngestResult;

struct Client {
    decoder: Connection,
    session: Option<String>,
}

#[derive(Default)]
struct State {
    clients: HashMap<u64, Client>,
    owners: HashMap<String, u64>,
    sessions: TaskBoard,
    updated_at: BTreeMap<String, u64>,
    revision: u64,
}

/// The daemon's in-memory projection of publisher connections. A session is
/// present only while its current connection is open.
#[derive(Default)]
pub(crate) struct TaskIngest {
    state: Mutex<State>,
}

impl TaskIngest {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingest one protocol line. Invalid messages close only this connection.
    pub(crate) fn ingest(&self, connection_id: u64, line: &[u8]) -> IngestResult {
        let mut state = lock(&self.state);
        let message = {
            let client = state
                .clients
                .entry(connection_id)
                .or_insert_with(|| Client {
                    decoder: Connection::new(),
                    session: None,
                });
            match client.decoder.ingest(line) {
                Ok(message) => message,
                Err(_) => {
                    return IngestResult {
                        keep_connection: false,
                        close_connections: Vec::new(),
                    };
                }
            }
        };
        match message {
            Some(Message::Hello(hello)) => {
                let session = hello.session;
                let displaced = state.owners.insert(session.clone(), connection_id);
                if let Some(previous_id) = displaced.filter(|previous| *previous != connection_id)
                    && let Some(previous) = state.clients.get_mut(&previous_id)
                {
                    previous.session = None;
                }
                if let Some(client) = state.clients.get_mut(&connection_id) {
                    client.session = Some(session.clone());
                }
                state.sessions.apply(TaskEvent::Connected {
                    session: session.clone(),
                    pane: hello.pane,
                });
                state.updated_at.insert(session, now_millis());
                state.revision = state.revision.wrapping_add(1);
                IngestResult {
                    keep_connection: true,
                    close_connections: displaced
                        .filter(|previous| *previous != connection_id)
                        .into_iter()
                        .collect(),
                }
            }
            Some(Message::Tasks(tasks)) => {
                let Some(session) = state
                    .clients
                    .get(&connection_id)
                    .and_then(|client| client.session.clone())
                else {
                    return IngestResult {
                        keep_connection: false,
                        close_connections: Vec::new(),
                    };
                };
                if state.owners.get(&session) != Some(&connection_id) {
                    return IngestResult {
                        keep_connection: false,
                        close_connections: Vec::new(),
                    };
                }
                state.sessions.apply(TaskEvent::Tasks {
                    session: session.clone(),
                    tasks,
                });
                state.updated_at.insert(session, now_millis());
                state.revision = state.revision.wrapping_add(1);
                IngestResult {
                    keep_connection: true,
                    close_connections: Vec::new(),
                }
            }
            None => IngestResult {
                keep_connection: true,
                close_connections: Vec::new(),
            },
        }
    }

    /// Forget a closed publisher, but never remove a newer connection's data.
    pub(crate) fn disconnect(&self, connection_id: u64) {
        let mut state = lock(&self.state);
        let Some(client) = state.clients.remove(&connection_id) else {
            return;
        };
        if let Some(session) = client.session
            && state.owners.get(&session) == Some(&connection_id)
        {
            state.owners.remove(&session);
            state.sessions.apply(TaskEvent::Disconnected {
                session: session.clone(),
            });
            state.updated_at.remove(&session);
            state.revision = state.revision.wrapping_add(1);
        }
    }

    /// Bounded, session-id ordered task list. The handle field from `hello` is
    /// intentionally not returned as an identity or join key.
    pub(crate) fn list(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> (u64, Vec<Value>, Option<String>) {
        let state = lock(&self.state);
        let revision = state.revision;
        let mut sessions = Vec::new();
        let mut next_after = None;
        for (session, held) in state.sessions.sessions() {
            if after.is_some_and(|after| session <= after) {
                continue;
            }
            if sessions.len() == limit {
                next_after = sessions.last().and_then(|value: &Value| {
                    value
                        .get("session")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                });
                break;
            }
            sessions.push(json!({
                "session": session,
                "connected": true,
                "last_updated_at": state.updated_at.get(session).copied().unwrap_or_default(),
                "tasks": held.tasks.iter().map(|task| json!({
                    "id": task.id,
                    "state": task.state.to_string(),
                    "command": task.command,
                    "cwd": task.cwd,
                    "pid": task.pid,
                    "started_at": task.started_at,
                    "last_output_at": task.last_output_at,
                    "output_bytes": task.output_bytes,
                    "exit_code": task.exit_code,
                })).collect::<Vec<_>>(),
            }));
        }
        (revision, sessions, next_after)
    }
}

fn lock(mutex: &Mutex<State>) -> MutexGuard<'_, State> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::TaskIngest;
    fn hello(session: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "type": "hello", "v": 1, "session": session, "pane": "%8"
        }))
        .unwrap()
    }

    #[test]
    fn session_data_is_connection_scoped_and_page_continuation_is_stable() {
        let ingest = TaskIngest::new();
        assert!(ingest.ingest(1, &hello("a")).keep_connection);
        assert!(ingest.ingest(2, &hello("b")).keep_connection);
        let (_, first, after) = ingest.list(None, 1);
        assert_eq!(first[0]["session"], "a");
        assert_eq!(after.as_deref(), Some("a"));
        let (_, second, next) = ingest.list(after.as_deref(), 1);
        assert_eq!(second[0]["session"], "b");
        assert!(next.is_none());
        ingest.disconnect(1);
        let (_, remaining, _) = ingest.list(None, 10);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0]["session"], "b");
    }

    #[test]
    fn a_reconnected_session_wins_and_the_displaced_connection_cannot_publish() {
        let ingest = TaskIngest::new();
        assert!(ingest.ingest(1, &hello("same")).keep_connection);
        let replacement = ingest.ingest(2, &hello("same"));
        assert!(replacement.keep_connection);
        assert_eq!(replacement.close_connections, vec![1]);
        let tasks = serde_json::to_vec(&serde_json::json!({"type":"tasks", "tasks":[]})).unwrap();
        assert!(!ingest.ingest(1, &tasks).keep_connection);
        ingest.disconnect(1);
        assert_eq!(ingest.list(None, 10).1.len(), 1);
        ingest.disconnect(2);
        assert!(ingest.list(None, 10).1.is_empty());
    }
}
