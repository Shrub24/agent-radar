//! What an effectful operation answers with: its request record, and the public
//! facts the operation itself names beside it.
//!
//! One type serves both ends of the socket. The daemon builds it and renders it
//! once, in [`respond`]; a Rust client reads it back instead of probing keys.

use serde::{Deserialize, Deserializer, Serialize};

use super::containers::PublicContainer;
use super::protocol::{Refusal, Response};
use super::registry::spawn::PublicSpawnEdge;
use super::store::RequestRecord;

/// The `result` of an operation response.
///
/// A fact is present only when the operation names it, and a creation or reload
/// names its container even when there is none, so `container` distinguishes an
/// absent fact from an answered `null`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    pub request: RequestRecord,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub container: Option<Option<PublicContainer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<Vec<PublicContainer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn: Option<PublicSpawnEdge>,
}

fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl Answer {
    /// The record's public view alone.
    pub fn of(record: RequestRecord) -> Self {
        Self {
            request: record.public(),
            container: None,
            removed: None,
            spawn: None,
        }
    }

    /// Names the container the operation made or acted on, if there was one.
    pub fn with_container(mut self, container: Option<PublicContainer>) -> Self {
        self.container = Some(container);
        self
    }

    /// Names the containers a deletion marked removed.
    pub fn with_removed(mut self, removed: Vec<PublicContainer>) -> Self {
        self.removed = Some(removed);
        self
    }

    /// Names the spawn edge the request authored.
    pub fn with_spawn(mut self, spawn: PublicSpawnEdge) -> Self {
        self.spawn = Some(spawn);
        self
    }
}

/// What serving an operation reaches: its answer, or a refusal before one exists.
pub type Reply = Result<Answer, Refusal>;

/// The one place an operation's reply becomes a response line.
pub fn respond(id: &str, reply: Reply) -> Response {
    match reply {
        Ok(answer) => Response::ok(
            id,
            serde_json::to_value(answer).expect("an operation answer is JSON-serializable"),
        ),
        Err(refusal) => Response::refused(id, &refusal),
    }
}
