//! The durable managed-child close requests: what a daemon was asked to close,
//! what it admitted before dispatch, and what that dispatch did.
//!
//! A close is neither a spawn nor a registration: it references the spawn edge
//! and the exact child that edge bound, records the caller's intent, and carries
//! the one outcome its dispatch reached. That outcome is durable and final: a
//! replay of the same request id is answered from the record instead of being
//! dispatched again, a restart re-dispatches nothing, and a close that may have
//! been dispatched without a confirmation stays unknown. It settles no spawn edge
//! and rewrites no registration, so a closed child still reads as the same parent
//! and the same bound identity afterwards.
//!
//! What is recorded is the mux's pane-close result and nothing more: no process
//! or process-group exit is claimed from it, and no execution-status or
//! assignment-state fact is written.
//!
//! The pane a close targets is the one the spawn edge recorded: a caller names
//! the child, never a pane id, so there is nothing observable to guess from. The
//! edge's private token is not read here, is not needed to identify the target,
//! and appears in no field of this record — a close is not a credential for the
//! child.
//!
//! The record is a bounded regular file, written into a temporary sibling and
//! renamed into place, and read back strictly — the discipline the registration,
//! publication and spawn records already follow.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::control::timestamp_millis;
use crate::control_plane::store::{RequestOutcome, format_millis};
use crate::model::SessionUuid;

use super::publication::write_atomic;
use super::{MAX_TEXT_BYTES, Registry, RegistryLocation, bounded_text, read_record_file};

/// The record shape this build writes and reads. A record naming another version
/// fails explicitly rather than being read as this one.
pub const CHILD_CLOSE_VERSION: u32 = 1;

/// The largest close request this daemon reads or writes, so a planted file
/// cannot be read into memory whole.
pub const MAX_CHILD_CLOSE_BYTES: usize = 64 * 1024;

/// The directory inside the state root that holds one file per close request.
pub const CLOSES: &str = "closes";

/// The caller's reason for closing a managed child's pane.
///
/// It is an audit label only. The daemon records it and decides nothing from it:
/// whether an assignment is complete or cancelled stays the assignment owner's
/// call, and no execution report is written from a close.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseIntent {
    Complete,
    Cancel,
}

/// What a caller asks the daemon to close.
///
/// The request names the durable spawn edge and the exact child that edge bound,
/// plus the intent. It names no pane, no process and no label: the target pane is
/// whatever the edge recorded, and nothing else is eligible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildCloseRequest {
    /// The caller's own request id: the durable key of the close, and the
    /// identity a retry repeats.
    pub request_id: String,
    /// The spawn edge this close targets.
    pub spawn_request_id: String,
    /// The child the edge bound, named by its registry identity.
    pub source: String,
    pub incarnation: String,
    pub intent: CloseIntent,
}

impl ChildCloseRequest {
    pub fn validate(&self) -> Result<(), String> {
        canonical_uuid("request_id", &self.request_id)?;
        canonical_uuid("spawn_request_id", &self.spawn_request_id)?;
        bounded_text("source", &self.source, false)?;
        canonical_uuid("incarnation", &self.incarnation)?;
        Ok(())
    }
}

/// One durable close request: the target, the intent and the outcome.
///
/// The spawn edge and the bound child are recorded facts, copied here when the
/// close was admitted, so the request is readable without re-deriving a target
/// from state that may since have moved. The location is the edge's own recorded
/// location, and its pane is the only pane this close may ever name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildClose {
    pub version: u32,
    pub request_id: String,
    pub spawn_request_id: String,
    pub source: String,
    pub incarnation: String,
    /// The pane the edge recorded. A close targets this location or nothing.
    pub location: RegistryLocation,
    pub intent: CloseIntent,
    /// What the close did. `Unknown` until a dispatch answers: a recorded
    /// request claims no effect, and a lost answer leaves it here.
    pub outcome: RequestOutcome,
    /// The backend's own words for a refused or unconfirmed close, kept so a
    /// replay repeats the answer the original call gave. Absent for a confirmed
    /// close, and for a request whose dispatch never answered at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub recorded_at: String,
}

/// What asking the registry to admit one close returned.
///
/// The distinction is the whole idempotency of a close: only the call that wrote
/// the request may dispatch it, and every later call with the same request id is
/// answered by the record that call left behind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloseAdmission {
    /// This call wrote the request: it owns the one dispatch this close may make.
    Admitted(ChildClose),
    /// The request id already named this close: the stored record decides the
    /// answer, and nothing is dispatched.
    Recorded(ChildClose),
}

impl ChildClose {
    /// Whether this record is the close `request` asks for: the same edge, the
    /// same bound child and the same intent. Different words under one request id
    /// are another request, not a retry of this one.
    pub fn answers(&self, request: &ChildCloseRequest) -> bool {
        self.spawn_request_id == request.spawn_request_id
            && self.source == request.source
            && self.incarnation == request.incarnation
            && self.intent == request.intent
    }
}

/// Checks that the request names the exact bound child and has a recorded pane.
/// Does not persist anything; the socket must do its fresh process verification
/// before recording the request.
pub fn validate_target(
    registry: &Registry,
    request: &ChildCloseRequest,
) -> Result<RegistryLocation, String> {
    request.validate()?;
    let edge = super::spawn::edge(registry, &request.spawn_request_id)?.ok_or_else(|| {
        format!(
            "spawn request {} has no recorded edge",
            request.spawn_request_id
        )
    })?;
    let child = edge
        .bound
        .ok_or_else(|| "spawn edge has no bound child to close".to_string())?;
    if child.source != request.source || child.incarnation != request.incarnation {
        return Err("child close names a child the spawn edge did not bind".into());
    }
    let location = edge
        .location
        .ok_or_else(|| "spawn edge recorded no pane to close".to_string())?;
    if location.pane.is_none() {
        return Err("spawn edge recorded no pane to close".into());
    }
    Ok(location)
}

/// Admits one close request, or answers with the record this request id already
/// has.
///
/// Admission is exact: the referenced spawn edge must exist, must have bound a
/// child, that child must be the one this request names, and the edge must have
/// recorded a pane to target. A refused request writes nothing. The same request
/// id asking again with identical content is the same close and answers with its
/// stored record, whatever outcome it reached; different content under one
/// request id is refused rather than silently replacing the recorded target.
///
/// The existing record is read inside the transition, so one request id admits
/// exactly one close however many callers race for it: only the writer may
/// dispatch, and the rest read the outcome it recorded.
///
/// The close never mutates the spawn edge, the registration or a publication.
pub fn record(
    registry: &Registry,
    request: &ChildCloseRequest,
    now_ms: i64,
) -> Result<CloseAdmission, String> {
    request.validate()?;
    let _transition = registry.transition();
    if let Some(existing) = read_close(registry, &request.request_id)? {
        if existing.answers(request) {
            return Ok(CloseAdmission::Recorded(existing));
        }
        return Err(format!(
            "child close request {} already names another target",
            request.request_id
        ));
    }
    let location = validate_target(registry, request)?;
    let close = ChildClose {
        version: CHILD_CLOSE_VERSION,
        request_id: request.request_id.clone(),
        spawn_request_id: request.spawn_request_id.clone(),
        source: request.source.clone(),
        incarnation: request.incarnation.clone(),
        location,
        intent: request.intent,
        outcome: RequestOutcome::Unknown,
        message: None,
        recorded_at: format_millis(now_ms),
    };
    write_close(registry, &close)?;
    Ok(CloseAdmission::Admitted(close))
}

/// Records what the dispatch of one admitted close did, and answers with the
/// settled record.
///
/// Written before the caller is answered, so every later replay of this request
/// id — including one after a restart — reads this outcome instead of dispatching
/// a second close. `Unknown` is a real outcome: a close that may have reached the
/// pane keeps that word, and nothing here retries it.
pub fn record_outcome(
    registry: &Registry,
    request_id: &str,
    outcome: RequestOutcome,
    message: Option<&str>,
) -> Result<ChildClose, String> {
    canonical_uuid("request_id", request_id)?;
    let _transition = registry.transition();
    let stored = read_close(registry, request_id)?.ok_or_else(|| {
        format!("child close request {request_id} has no recorded request to settle")
    })?;
    let close = ChildClose {
        outcome,
        message: storable_message(message),
        ..stored
    };
    write_close(registry, &close)?;
    Ok(close)
}

/// The stored diagnostic, or none.
///
/// A backend's message is external text and the record is read back strictly, so
/// it is made storable here rather than allowed to write a record this daemon
/// would refuse to read: one line, bounded, and carrying nothing a terminal would
/// act on.
fn storable_message(message: Option<&str>) -> Option<String> {
    let mut storable = String::new();
    for character in message?.chars().filter(|character| !character.is_control()) {
        if storable.len() + character.len_utf8() > MAX_TEXT_BYTES {
            break;
        }
        storable.push(character);
    }
    (!storable.is_empty()).then_some(storable)
}

/// Reads one recorded close request, if it exists.
pub fn close(registry: &Registry, request_id: &str) -> Result<Option<ChildClose>, String> {
    canonical_uuid("request_id", request_id)?;
    read_close(registry, request_id)
}

fn read_close(registry: &Registry, request_id: &str) -> Result<Option<ChildClose>, String> {
    let path = close_path(registry, request_id);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("child close {}: {error}", path.display())),
        Ok(_) => {}
    }
    let bytes = read_record_file(&path, MAX_CHILD_CLOSE_BYTES)?;
    decode(&bytes, request_id).map(Some)
}

/// Decodes one close request, bounded and version-checked: a record this daemon
/// could not have written is not one it will act on.
fn decode(bytes: &[u8], request_id: &str) -> Result<ChildClose, String> {
    if bytes.len() > MAX_CHILD_CLOSE_BYTES {
        return Err(format!("child close exceeds {MAX_CHILD_CLOSE_BYTES} bytes"));
    }
    let close: ChildClose =
        serde_json::from_slice(bytes).map_err(|error| format!("not a child close: {error}"))?;
    if close.version != CHILD_CLOSE_VERSION {
        return Err(format!(
            "child close version {} is not served; this daemon writes {CHILD_CLOSE_VERSION}",
            close.version
        ));
    }
    if close.request_id != request_id {
        return Err("child close id does not match its filename".into());
    }
    if SessionUuid::parse(&close.spawn_request_id).is_none() {
        return Err("child close names a malformed spawn request".into());
    }
    bounded_text("source", &close.source, false)?;
    if SessionUuid::parse(&close.incarnation).is_none() {
        return Err("child close names a malformed incarnation".into());
    }
    close.location.validate()?;
    if close.location.pane.is_none() {
        return Err("child close names no pane".into());
    }
    if let Some(message) = &close.message {
        bounded_text("message", message, true)?;
    }
    if timestamp_millis(&close.recorded_at).is_none() {
        return Err("child close recorded_at is not a timestamp".into());
    }
    Ok(close)
}

fn write_close(registry: &Registry, close: &ChildClose) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(close).map_err(|error| error.to_string())?;
    write_atomic(
        &registry.closes(),
        &close_path(registry, &close.request_id),
        &bytes,
        MAX_CHILD_CLOSE_BYTES,
    )
}

fn close_path(registry: &Registry, request_id: &str) -> PathBuf {
    registry.closes().join(format!("{request_id}.json"))
}

/// A key that names a file has to be canonical before any path is formed from
/// it: a request id is a key, not a path.
fn canonical_uuid(field: &str, value: &str) -> Result<(), String> {
    if SessionUuid::parse(value).is_none() {
        return Err(format!("`{field}` must be a canonical UUID"));
    }
    Ok(())
}
