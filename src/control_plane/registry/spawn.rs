//! The durable spawn edges: one record per child this daemon was asked to
//! create, and the private token that binds it.
//!
//! An edge is neither a registration nor a publication. It answers which runtime
//! subject a child was created for and what the daemon confirmed, and it is
//! recorded before anything is created, so a daemon that dies mid-spawn leaves
//! the intent readable rather than an unattributable pane. Each effect is
//! recorded on its own — created, launched, bound — because a later step is
//! never claimed while an earlier one is uncertain.
//!
//! Only the private token binds an edge to a child: not a pane title, not an
//! alias, not a label and not a session UUID, because nothing observable about a
//! pane is the identity of the process in it. The token is minted here, is spent
//! by its first bind, and appears in no projection and in no refusal.
//!
//! The record is a bounded regular file, written into a temporary sibling and
//! renamed into place, and read back strictly — the discipline the registration
//! and publication records already follow.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::control::timestamp_millis;
use crate::control_plane::store::{RequestOutcome, format_millis, random_uuid};
use crate::model::SessionUuid;

use super::publication::write_atomic;
use super::{Freshness, Registry, RegistryLocation, bounded_text, read_record_file};

/// The record shape this build writes and reads. A record naming another version
/// fails explicitly rather than being read as this one.
pub const SPAWN_VERSION: u32 = 1;

/// The largest spawn edge this daemon reads or writes, so a planted file cannot
/// be read into memory whole.
pub const MAX_SPAWN_BYTES: usize = 64 * 1024;

/// The directory inside the state root that holds one file per spawn edge.
pub const SPAWNS: &str = "spawns";

/// What a caller asks the daemon to record before anything is created.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnRequest {
    /// The caller's own request id: the durable key of the edge, and the identity
    /// a retry repeats.
    pub request_id: String,
    /// The agent record this child is created for: a runtime subject, never a
    /// pane name.
    pub parent: String,
}

impl SpawnRequest {
    pub fn validate(&self) -> Result<(), String> {
        canonical_uuid("request_id", &self.request_id)?;
        canonical_uuid("parent", &self.parent)?;
        Ok(())
    }
}

/// A registration presenting the token minted for its launch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnBindRequest {
    /// Private: the token the launch carried into the child.
    pub token: String,
    pub source: String,
    pub incarnation: String,
}

impl SpawnBindRequest {
    pub fn validate(&self) -> Result<(), String> {
        canonical_uuid("token", &self.token)?;
        bounded_text("source", &self.source, false)?;
        canonical_uuid("incarnation", &self.incarnation)?;
        Ok(())
    }
}

/// The child a token bound: the registering incarnation's own registry identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundChild {
    pub source: String,
    pub incarnation: String,
    /// When the edge bound to this incarnation.
    pub bound_at: String,
}

/// One durable spawn edge.
///
/// The parent, the request id and the created location are recorded facts. The
/// three effects are separate because they happen separately, and the token is
/// the only thing that binds the edge to the child: it is what the daemon put in
/// the launch, so it never leaves this record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnEdge {
    pub version: u32,
    pub request_id: String,
    pub parent: String,
    /// Private. It appears in no projection and in no refusal.
    pub token: String,
    /// Where the child was created, once a create answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<RegistryLocation>,
    /// What the create did. `Unknown` until the backend answers: a recorded
    /// intent claims no effect.
    pub created: RequestOutcome,
    /// What the launch did. `Unknown` until the backend answers.
    pub launched: RequestOutcome,
    /// The child this edge's token bound, once one registered with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<BoundChild>,
    pub recorded_at: String,
}

impl SpawnEdge {
    /// What may be read about this edge.
    ///
    /// The token is absent by construction, not by redaction: there is no field
    /// here to forget to strip.
    pub fn public(&self) -> PublicSpawnEdge {
        PublicSpawnEdge {
            version: self.version,
            request_id: self.request_id.clone(),
            parent: self.parent.clone(),
            location: self.location.clone(),
            created: self.created,
            launched: self.launched,
            bound: self.bound.clone(),
            recorded_at: self.recorded_at.clone(),
        }
    }

    /// What a read may know about this edge, given what the backend reported
    /// about the location this edge recorded.
    ///
    /// Binding is never revised by observation: an edge whose child registered
    /// still names that child whatever the backend reports now. `unresolved`
    /// outranks `bound` because a client reading the state alone must not take a
    /// vanished location for a present one — the child is reported beside it
    /// either way.
    pub fn topology(&self, evidence: LocationEvidence) -> PublicSpawnTopology {
        let identity = match self.bound {
            Some(_) => SpawnState::Bound,
            None => SpawnState::Unbound,
        };
        let (state, freshness) = match (&self.location, evidence) {
            (None, _) => (identity, None),
            (Some(_), LocationEvidence::Present) => (identity, Some(Freshness::Fresh)),
            (Some(_), LocationEvidence::Absent) => (SpawnState::Unresolved, Some(Freshness::Stale)),
            (Some(_), LocationEvidence::Unavailable) => (identity, Some(Freshness::Stale)),
        };
        PublicSpawnTopology {
            edge: self.public(),
            state,
            freshness,
        }
    }
}

/// A spawn edge as a client reads it: the parent, the effects, the location and
/// the bound child, with the token that bound it left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSpawnEdge {
    pub version: u32,
    pub request_id: String,
    pub parent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<RegistryLocation>,
    pub created: RequestOutcome,
    pub launched: RequestOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<BoundChild>,
    pub recorded_at: String,
}

/// The state of one edge in the recorded topology: the strongest statement a
/// read can make about it, and nothing beyond the records and this read's own
/// evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpawnState {
    /// No child has presented this edge's token. The edge may still be pending,
    /// or its launch may never have reached a shell.
    Unbound,
    /// A child registered with this edge's token, so the edge names its exact
    /// identity. Observation does not revise this.
    Bound,
    /// The recorded location is not in the backend's own report: the record and
    /// the world disagree. This is never a statement that the child stopped — a
    /// read observes panes, and a pane's absence is not a process's death.
    Unresolved,
}

/// What one read could learn about a recorded location: the only external
/// evidence a topology read consults.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocationEvidence {
    /// The backend reports the recorded pane right now.
    Present,
    /// The backend answered and the recorded pane is not in its report.
    Absent,
    /// The daemon has no confirmation of the location: no backend could be asked
    /// — none is wired, it cannot be observed, it failed, the daemon is stopping
    /// — or the recorded location names no pane to check. Explicitly not evidence
    /// of absence.
    Unavailable,
}

/// One edge as a topology read reports it: the stored facts, plus the two things
/// a read derives instead of inferring.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PublicSpawnTopology {
    #[serde(flatten)]
    pub edge: PublicSpawnEdge,
    pub state: SpawnState,
    /// Freshness of the recorded location against this read's evidence, in the
    /// registry's one freshness vocabulary: `fresh` when the backend reports the
    /// location now, `stale` when it does not — which is never evidence that the
    /// child stopped. Absent when the edge recorded no location, because there is
    /// nothing to verify.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freshness: Option<Freshness>,
}

/// Records one spawn edge, or answers with the edge this request id already has.
///
/// The request and its parent are recorded before anything is created, so the
/// intent is durable even when the spawn never completes. The same request id
/// asking again with the same parent is the same spawn and returns its stored
/// edge, whatever state it reached; a different parent under one request id is
/// refused rather than silently replacing the recorded one.
pub fn record(
    registry: &Registry,
    request: &SpawnRequest,
    now_ms: i64,
) -> Result<SpawnEdge, String> {
    request.validate()?;
    let _transition = registry.transition();
    if let Some(existing) = read_edge(registry, &request.request_id)? {
        if existing.parent == request.parent {
            return Ok(existing);
        }
        return Err(format!(
            "spawn request {} already names another parent",
            request.request_id
        ));
    }
    registry
        .get(&request.parent)?
        .ok_or_else(|| format!("parent {} is not a registered agent", request.parent))?;
    let edge = SpawnEdge {
        version: SPAWN_VERSION,
        request_id: request.request_id.clone(),
        parent: request.parent.clone(),
        token: random_uuid(),
        location: None,
        created: RequestOutcome::Unknown,
        launched: RequestOutcome::Unknown,
        bound: None,
        recorded_at: format_millis(now_ms),
    };
    write_edge(registry, &edge)?;
    Ok(edge)
}

/// Records what the create of one spawn did, and the location it named.
///
/// Written before the launch is attempted, so a confirmed pane is readable even
/// when the launch never answers: an effect the daemon established is not lost
/// to a later step's failure. A location belongs to a completed create — a
/// refused or unconfirmed create named nothing, so the caller records none, and
/// the location it does record is a confirmed fact rather than a hopeful one.
pub fn record_created(
    registry: &Registry,
    request_id: &str,
    created: RequestOutcome,
    location: Option<RegistryLocation>,
) -> Result<SpawnEdge, String> {
    canonical_uuid("request_id", request_id)?;
    let _transition = registry.transition();
    let stored = stored_edge(registry, request_id)?;
    let edge = SpawnEdge {
        location,
        created,
        ..stored
    };
    write_edge(registry, &edge)?;
    Ok(edge)
}

/// Records what the launch of one spawn did.
///
/// A launch that may have reached the pane is [`RequestOutcome::Unknown`], which
/// is the same word the edge starts with: the record says the daemon cannot
/// claim a launch, never that one happened.
pub fn record_launched(
    registry: &Registry,
    request_id: &str,
    launched: RequestOutcome,
) -> Result<SpawnEdge, String> {
    canonical_uuid("request_id", request_id)?;
    let _transition = registry.transition();
    let stored = stored_edge(registry, request_id)?;
    let edge = SpawnEdge { launched, ..stored };
    write_edge(registry, &edge)?;
    Ok(edge)
}

/// The edge one request id already has.
///
/// An effect is recorded on an edge that exists: a spawn whose intent was never
/// recorded has nothing to attribute the effect to, and minting one here would
/// invent the token the child's binding depends on.
fn stored_edge(registry: &Registry, request_id: &str) -> Result<SpawnEdge, String> {
    read_edge(registry, request_id)?
        .ok_or_else(|| format!("spawn request {request_id} has no recorded edge"))
}

/// Reads one recorded edge, token included.
///
/// The token is a credential, so this is local use: what a client may read is
/// [`SpawnEdge::public`].
pub fn edge(registry: &Registry, request_id: &str) -> Result<Option<SpawnEdge>, String> {
    canonical_uuid("request_id", request_id)?;
    read_edge(registry, request_id)
}

/// The edges in stable request-id order, at most `limit` of them and starting
/// after `after`.
///
/// The order is the durable key's, not a timestamp's: a restore, a clock change
/// or two records written in the same millisecond would all move a timestamp
/// order, and a page resumed from one would then skip or repeat an edge. The
/// cursor names the last edge a page returned, so the boundary is the key
/// itself and no edge is served twice.
///
/// Every record is read as a bounded regular file, and a malformed one fails the
/// whole listing: a topology read must not forget a child because one file is
/// unreadable.
pub fn list_after(
    registry: &Registry,
    after: Option<&str>,
    limit: usize,
) -> Result<Vec<SpawnEdge>, String> {
    if let Some(after) = after {
        canonical_uuid("after", after)?;
    }
    let mut edges = edges(registry)?;
    edges.retain(|edge| after.is_none_or(|cursor| edge.request_id.as_str() > cursor));
    edges.truncate(limit);
    Ok(edges)
}

/// Binds the edge a spawn token minted to the child that presented it.
///
/// A token binds once: its first bind spends it, and every later presentation —
/// including the child it already bound — is refused as spent, so one spawn
/// cannot claim two children. Nothing observable about a pane is consulted: the
/// child is whichever incarnation presented the token.
pub fn bind(
    registry: &Registry,
    request: &SpawnBindRequest,
    now_ms: i64,
) -> Result<SpawnEdge, String> {
    request.validate()?;
    let _transition = registry.transition();
    bind_locked(registry, request, now_ms)
}

/// Binds the edge a registration's token names to that exact registration.
///
/// Called with the registration already written and before it is answered, so a
/// refused token leaves no registration behind. A token the daemon never minted,
/// one that names more than one edge, and one already spent are all refused with
/// a message that names none of them.
pub(super) fn bind_presented_token(
    registry: &Registry,
    request: &super::RegistrationRequest,
    now_ms: i64,
) -> Result<(), String> {
    let Some(token) = request.spawn_token.as_deref() else {
        return Ok(());
    };
    bind_locked(registry, &registration_bind(token, request), now_ms)?;
    Ok(())
}

/// Re-answers an identical registration retry that already spent its token.
///
/// Bind spends a token with its first write, so a retry that finds the edge
/// already bound to this exact `(source, incarnation)` is its own earlier effect
/// and is answered from the record. A token that bound another child, or that
/// never bound one, is refused — a spent token is spent for every presenter.
pub(super) fn heal_binding(
    registry: &Registry,
    request: &super::RegistrationRequest,
    now_ms: i64,
) -> Result<(), String> {
    let Some(token) = request.spawn_token.as_deref() else {
        return Ok(());
    };
    let bind = registration_bind(token, request);
    match bind_locked(registry, &bind, now_ms) {
        Ok(_) => Ok(()),
        Err(error) => {
            if already_bound_to(registry, &bind)? {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

fn registration_bind(token: &str, request: &super::RegistrationRequest) -> SpawnBindRequest {
    SpawnBindRequest {
        token: token.to_string(),
        source: request.source.clone(),
        incarnation: request.incarnation.clone(),
    }
}

/// Whether this token already bound exactly this child: the state a retry sees
/// when the bind write survived and its answer did not.
fn already_bound_to(registry: &Registry, request: &SpawnBindRequest) -> Result<bool, String> {
    Ok(edges(registry)?.into_iter().any(|edge| {
        edge.token == request.token
            && edge.bound.as_ref().is_some_and(|child| {
                child.source == request.source && child.incarnation == request.incarnation
            })
    }))
}

/// The bind itself, under the caller's admission lock.
fn bind_locked(
    registry: &Registry,
    request: &SpawnBindRequest,
    now_ms: i64,
) -> Result<SpawnEdge, String> {
    let mut matches = edges(registry)?
        .into_iter()
        .filter(|edge| edge.token == request.token);
    let Some(stored) = matches.next() else {
        return Err("no recorded spawn edge was minted with this token".into());
    };
    // A token names at most one edge. Anything else is a broken invariant, and
    // binding whichever was read first would attribute a child to a guess.
    if matches.next().is_some() {
        return Err("this spawn token names more than one edge".into());
    }
    if stored.bound.is_some() {
        return Err("this spawn token is already spent".into());
    }
    let edge = SpawnEdge {
        bound: Some(BoundChild {
            source: request.source.clone(),
            incarnation: request.incarnation.clone(),
            bound_at: format_millis(now_ms),
        }),
        ..stored
    };
    write_edge(registry, &edge)?;
    Ok(edge)
}

/// Every recorded edge, in stable request-id order.
fn edges(registry: &Registry) -> Result<Vec<SpawnEdge>, String> {
    let directory = registry.spawns();
    let entries = fs::read_dir(&directory).map_err(|error| format!("spawns: {error}"))?;
    let mut edges = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("spawns: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let request_id = entry
            .file_name()
            .to_string_lossy()
            .trim_end_matches(".json")
            .to_string();
        let bytes = read_record_file(&path, MAX_SPAWN_BYTES)?;
        edges.push(decode(&bytes, &request_id)?);
    }
    edges.sort_by(|left, right| left.request_id.cmp(&right.request_id));
    Ok(edges)
}

fn read_edge(registry: &Registry, request_id: &str) -> Result<Option<SpawnEdge>, String> {
    let path = edge_path(registry, request_id);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("spawn edge {}: {error}", path.display())),
        Ok(_) => {}
    }
    let bytes = read_record_file(&path, MAX_SPAWN_BYTES)?;
    decode(&bytes, request_id).map(Some)
}

/// Decodes one edge, bounded and version-checked: a record this daemon could not
/// have written is not one it will act on.
///
/// Every field a reader would act on is checked, so a partial write, a planted
/// file or a garbled record fails here rather than reading as an edge with no
/// parent, no token and no state.
fn decode(bytes: &[u8], request_id: &str) -> Result<SpawnEdge, String> {
    if bytes.len() > MAX_SPAWN_BYTES {
        return Err(format!("spawn edge exceeds {MAX_SPAWN_BYTES} bytes"));
    }
    let edge: SpawnEdge =
        serde_json::from_slice(bytes).map_err(|error| format!("not a spawn edge: {error}"))?;
    if edge.version != SPAWN_VERSION {
        return Err(format!(
            "spawn edge version {} is not served; this daemon writes {SPAWN_VERSION}",
            edge.version
        ));
    }
    if edge.request_id != request_id {
        return Err("spawn edge id does not match its filename".into());
    }
    if SessionUuid::parse(&edge.parent).is_none() {
        return Err("spawn edge names a malformed parent".into());
    }
    if SessionUuid::parse(&edge.token).is_none() {
        return Err("spawn edge names a malformed token".into());
    }
    if let Some(location) = &edge.location {
        location.validate()?;
    }
    if let Some(bound) = &edge.bound {
        bounded_text("source", &bound.source, false)?;
        if SessionUuid::parse(&bound.incarnation).is_none() {
            return Err("spawn edge names a malformed bound incarnation".into());
        }
        if timestamp_millis(&bound.bound_at).is_none() {
            return Err("spawn edge bound_at is not a timestamp".into());
        }
    }
    if timestamp_millis(&edge.recorded_at).is_none() {
        return Err("spawn edge recorded_at is not a timestamp".into());
    }
    Ok(edge)
}

fn write_edge(registry: &Registry, edge: &SpawnEdge) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(edge).map_err(|error| error.to_string())?;
    write_atomic(
        &registry.spawns(),
        &edge_path(registry, &edge.request_id),
        &bytes,
        MAX_SPAWN_BYTES,
    )
}

fn edge_path(registry: &Registry, request_id: &str) -> PathBuf {
    registry.spawns().join(format!("{request_id}.json"))
}

/// A key that names a file has to be canonical before any path is formed from
/// it: a request id is a key, not a path, and a token must never become one.
fn canonical_uuid(field: &str, value: &str) -> Result<(), String> {
    if SessionUuid::parse(value).is_none() {
        return Err(format!("`{field}` must be a canonical UUID"));
    }
    Ok(())
}
