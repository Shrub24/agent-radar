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
use crate::control_plane::lifecycle;
use crate::control_plane::store::{RequestOutcome, format_millis, random_uuid};
use crate::model::{ProcessIdentity, SessionUuid};

use super::publication::write_atomic;
use super::{Freshness, Registry, RegistryLocation, bounded_text, read_record_file};

/// The record shape this build writes and reads. A record naming another version
/// fails explicitly rather than being read as this one.
pub const SPAWN_VERSION: u32 = 2;

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
    /// Exact agent registration used as the live process parent.
    pub parent: String,
    /// Stable logical parent identity, verified against this registration's fresh context.
    pub parent_session_id: String,
    /// The logical parent subject, when the request was made on behalf of one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_subject_id: Option<String>,
}

impl SpawnRequest {
    pub fn validate(&self) -> Result<(), String> {
        canonical_uuid("request_id", &self.request_id)?;
        canonical_uuid("parent", &self.parent)?;
        canonical_uuid("parent_session_id", &self.parent_session_id)?;
        if let Some(id) = self.parent_subject_id.as_deref() {
            canonical_uuid("parent_subject_id", id)?;
        }
        Ok(())
    }
}

/// A registration presenting the token minted for its launch.
struct SpawnBindRequest {
    /// Private: the token the launch carried into the child.
    token: String,
    source: String,
    incarnation: String,
}

/// The child a token bound: the registering incarnation's own registry identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoundChild {
    pub source: String,
    pub incarnation: String,
    /// Exact registry identity returned to the registering child.
    pub registration_id: String,
    /// The durable agent the bound child attaches as. Absent for an edge without
    /// a tracked launch and in records written before it was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_id: Option<String>,
    /// When the edge bound to this incarnation.
    pub bound_at: String,
}

/// One durable, typed exit observation for a direct-argv child.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnExitObservation {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub observed_at: String,
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
    /// Logical session parent.
    pub parent_session_id: String,
    /// The logical subject that requested this launch, when it had one. Durable
    /// parentage: a session moves on `/resume`, so it is not the parent key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_subject_id: Option<String>,
    /// Exact tracked attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    /// Binding credential: the claim id for tracked launches, private otherwise.
    pub token: String,
    /// Where the child was created, once a create answered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<RegistryLocation>,
    /// The verified birth identity of the process that served the create which
    /// named this location: the instance this pane belongs to.
    ///
    /// Absent on an edge whose create recorded none — a backend that names no
    /// server process, or a capture that could not be verified.
    /// A generation is never inferred from a pane id, a server name or history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<ProcessIdentity>,
    /// What the create did. `Unknown` until the backend answers: a recorded
    /// intent claims no effect.
    pub created: RequestOutcome,
    /// What the launch did. `Unknown` until the backend answers.
    pub launched: RequestOutcome,
    /// The child this edge's token bound, once one registered with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<BoundChild>,
    /// First typed process exit observed while the daemon still had the pane.
    /// Pane disappearance alone never creates this fact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit: Option<SpawnExitObservation>,
    /// Set when the daemon withdrew this launch before its process presented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<String>,
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
            parent_session_id: self.parent_session_id.clone(),
            parent_subject_id: self.parent_subject_id.clone(),
            claim_id: self.claim_id.clone(),
            location: self.location.clone(),
            created: self.created,
            launched: self.launched,
            bound: self.bound.clone(),
            withdrawn: self.withdrawn.clone(),
            granted_at: self.granted_at.clone(),
            exit_observation: match &self.exit {
                Some(exit) => PublicSpawnExitObservation::Exited {
                    code: exit.code,
                    signal: exit.signal,
                    observed_at: exit.observed_at.clone(),
                },
                None => PublicSpawnExitObservation::Unknown,
            },
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

/// What a spawn topology read can say about a child's process exit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PublicSpawnExitObservation {
    Unknown,
    Exited {
        code: Option<i32>,
        signal: Option<i32>,
        observed_at: String,
    },
}

/// A spawn edge as a client reads it: the parent, the effects, the location and
/// the bound child, with the token that bound it left behind.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicSpawnEdge {
    pub version: u32,
    pub request_id: String,
    pub parent: String,
    pub parent_session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<RegistryLocation>,
    pub created: RequestOutcome,
    pub launched: RequestOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound: Option<BoundChild>,
    pub exit_observation: PublicSpawnExitObservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<String>,
    pub recorded_at: String,
}

/// Why and when a launch ended without ever presenting, so a reader never infers
/// failure from absence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Withdrawn {
    pub at: String,
    pub reason: String,
}

/// The state of one edge in the recorded topology: the strongest statement a
/// read can make about it, and nothing beyond the records and this read's own
/// evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
        if existing.parent == request.parent
            && existing.parent_session_id == request.parent_session_id
        {
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
    let context = registry
        .context(&request.parent, now_ms)?
        .ok_or_else(|| format!("parent {} has no published session context", request.parent))?;
    if context.context.freshness != Freshness::Fresh
        || context.context.session.as_deref() != Some(&request.parent_session_id)
    {
        return Err(format!(
            "parent {} has no fresh matching session context",
            request.parent
        ));
    }
    let edge = SpawnEdge {
        version: SPAWN_VERSION,
        request_id: request.request_id.clone(),
        parent: request.parent.clone(),
        parent_session_id: request.parent_session_id.clone(),
        parent_subject_id: request.parent_subject_id.clone(),
        claim_id: None,
        token: random_uuid(),
        location: None,
        instance: None,
        created: RequestOutcome::Unknown,
        launched: RequestOutcome::Unknown,
        bound: None,
        exit: None,
        withdrawn: None,
        granted_at: None,
        recorded_at: format_millis(now_ms),
    };
    write_edge(registry, &edge)?;
    registry.append_event(
        "spawn.requested",
        Some(&edge.parent),
        None,
        Some(&edge.request_id),
    );
    Ok(edge)
}

/// Attach a launch reservation. The child presents its pre-minted registration,
/// not the claim; binding still requires the captured process birth identity.
pub fn attach_subject(
    registry: &Registry,
    request_id: &str,
    claim_id: &str,
    registration_id: &str,
) -> Result<SpawnEdge, String> {
    canonical_uuid("request_id", request_id)?;
    canonical_uuid("claim_id", claim_id)?;
    canonical_uuid("registration_id", registration_id)?;
    let _transition = registry.transition();
    let stored = stored_edge(registry, request_id)?;
    if stored.claim_id.is_some() {
        return Err(format!(
            "spawn request {request_id} already names a launch subject"
        ));
    }
    let edge = SpawnEdge {
        claim_id: Some(claim_id.to_string()),
        token: registration_id.to_string(),
        ..stored
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
    instance: Option<ProcessIdentity>,
) -> Result<SpawnEdge, String> {
    canonical_uuid("request_id", request_id)?;
    let _transition = registry.transition();
    let stored = stored_edge(registry, request_id)?;
    let edge = SpawnEdge {
        location,
        instance,
        created,
        ..stored
    };
    write_edge(registry, &edge)?;
    registry.append_event("spawn.create_effect", None, None, Some(request_id));
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
    if stored.launched == launched {
        return Ok(stored);
    }
    let edge = SpawnEdge { launched, ..stored };
    write_edge(registry, &edge)?;
    registry.append_event("spawn.launched", None, None, Some(request_id));
    Ok(edge)
}

/// Records the first typed exit observed for a spawned pane. Repeated and
/// concurrent observations return the original fact without rewriting it.
pub fn record_exit_observation(
    registry: &Registry,
    request_id: &str,
    code: Option<i32>,
    signal: Option<i32>,
    now_ms: i64,
) -> Result<SpawnEdge, String> {
    canonical_uuid("request_id", request_id)?;
    match (code, signal) {
        (Some(code), None) if (0..=255).contains(&code) => {}
        (None, Some(signal)) if signal > 0 => {}
        _ => return Err("exit observation must name one valid code or signal".into()),
    }
    let _transition = registry.transition();
    let stored = stored_edge(registry, request_id)?;
    if stored.exit.is_some() {
        return Ok(stored);
    }
    let edge = SpawnEdge {
        exit: Some(SpawnExitObservation {
            code,
            signal,
            observed_at: format_millis(now_ms),
        }),
        ..stored
    };
    write_edge(registry, &edge)?;
    registry.append_event("spawn.exited", None, None, Some(request_id));
    Ok(edge)
}

pub fn record_granted(registry: &Registry, request_id: &str, now_ms: i64) -> Result<(), String> {
    let _transition = registry.transition();
    let mut edge = stored_edge(registry, request_id)?;
    if edge.withdrawn.is_some() {
        return Err("the launch was withdrawn".into());
    }
    if edge.granted_at.is_none() {
        edge.granted_at = Some(format_millis(now_ms));
        write_edge(registry, &edge)?;
        registry.append_event("spawn.granted", None, None, Some(request_id));
    }
    Ok(())
}

/// Records that the daemon withdrew a launch whose process never presented. The
/// first withdrawal stands.
pub fn record_withdrawn(
    registry: &Registry,
    request_id: &str,
    reason: &str,
    now_ms: i64,
) -> Result<SpawnEdge, String> {
    canonical_uuid("request_id", request_id)?;
    let _transition = registry.transition();
    let stored = stored_edge(registry, request_id)?;
    if stored.withdrawn.is_some() {
        return Ok(stored);
    }
    let edge = SpawnEdge {
        withdrawn: Some(Withdrawn {
            at: format_millis(now_ms),
            reason: reason.into(),
        }),
        ..stored
    };
    write_edge(registry, &edge)?;
    registry.append_event("spawn.withdrawn", None, None, Some(request_id));
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
    parent_subject: Option<&str>,
) -> Result<Vec<SpawnEdge>, String> {
    if let Some(after) = after {
        canonical_uuid("after", after)?;
    }
    if let Some(subject) = parent_subject {
        canonical_uuid("parent_subject_id", subject)?;
    }
    let mut edges = edges(registry)?;
    edges.retain(|edge| after.is_none_or(|cursor| edge.request_id.as_str() > cursor));
    // The filter applies before the page is cut, so a caller that pages its own
    // launches never walks the whole topology to find them.
    if let Some(subject) = parent_subject {
        edges.retain(|edge| edge.parent_subject_id.as_deref() == Some(subject));
    }
    edges.truncate(limit);
    Ok(edges)
}

pub(super) fn launched_registration(
    registry: &Registry,
    token: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(token) = token else { return Ok(None) };
    let Some(edge) = edges(registry)?
        .into_iter()
        .find(|edge| edge.token == token)
    else {
        if lifecycle::admitted_agent(registry, token)? {
            return Ok(Some(token.to_owned()));
        }
        return Err("no recorded spawn edge was minted with this token".into());
    };
    match edge.claim_id {
        Some(claim_id) => lifecycle::read(registry, &claim_id)?
            .map(|launch| Some(launch.process.registration_id))
            .ok_or_else(|| "launch reservation is missing".into()),
        None => Ok(None),
    }
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
    registration_id: &str,
    now_ms: i64,
) -> Result<(), String> {
    let Some(token) = request.spawn_token.as_deref() else {
        return Ok(());
    };
    if lifecycle::admitted_agent(registry, token)? {
        return lifecycle::bind_admitted(
            registry,
            registration_id,
            request.process.as_ref(),
            now_ms,
        );
    }
    bind_locked(
        registry,
        &registration_bind(token, request),
        now_ms,
        registration_id,
        request.process.as_ref(),
    )?;
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
    let registration = registry
        .registration_for_incarnation(&request.source, &request.incarnation)?
        .ok_or_else(|| "registered child identity is unavailable for spawn binding".to_string())?;
    if lifecycle::admitted_agent(registry, token)? {
        return lifecycle::bind_admitted(
            registry,
            &registration.registration_id,
            request.process.as_ref(),
            now_ms,
        );
    }
    let bind = registration_bind(token, request);
    bind_locked(
        registry,
        &bind,
        now_ms,
        &registration.registration_id,
        request.process.as_ref(),
    )
    .map(|_| ())
}

fn registration_bind(token: &str, request: &super::RegistrationRequest) -> SpawnBindRequest {
    SpawnBindRequest {
        token: token.to_string(),
        source: request.source.clone(),
        incarnation: request.incarnation.clone(),
    }
}

/// The bind itself, under the caller's admission lock.
fn bind_locked(
    registry: &Registry,
    request: &SpawnBindRequest,
    now_ms: i64,
    registration_id: &str,
    process: Option<&ProcessIdentity>,
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
    // A tracked launch binds only to the process its daemon captured, and only
    // while its subject is still live. The subject is written first: an edge that
    // could not be written leaves the subject bound, which a same-content retry heals.
    let mut subject = None;
    if let Some(claim_id) = &stored.claim_id {
        lifecycle::fence_registration(registry, claim_id, process, Some(registration_id), now_ms)?;
        subject = lifecycle::read(registry, claim_id)?
            .map(|launch| launch.subject_id.unwrap_or(launch.claim_id));
    }
    if let Some(bound) = &stored.bound {
        if bound.source == request.source && bound.incarnation == request.incarnation {
            if bound.registration_id != registration_id {
                return Err("spawn binding names a different registry identity".into());
            }
            return Ok(stored);
        }
        return Err("this spawn token is already spent".into());
    }
    let edge = SpawnEdge {
        bound: Some(BoundChild {
            source: request.source.clone(),
            incarnation: request.incarnation.clone(),
            registration_id: registration_id.to_owned(),
            subject_id: subject,
            bound_at: format_millis(now_ms),
        }),
        ..stored
    };
    write_edge(registry, &edge)?;
    registry.append_event(
        "spawn.bound",
        Some(registration_id),
        None,
        Some(&edge.request_id),
    );
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
            "spawn edge version {} is not served; this daemon reads v{SPAWN_VERSION}",
            edge.version
        ));
    }
    canonical_uuid("parent_session_id", &edge.parent_session_id)?;
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
        canonical_uuid("bound.registration_id", &bound.registration_id)?;
    }
    if let Some(exit) = &edge.exit {
        match (exit.code, exit.signal) {
            (Some(code), None) if (0..=255).contains(&code) => {}
            (None, Some(signal)) if signal > 0 => {}
            _ => return Err("spawn edge exit must name one valid code or signal".into()),
        }
        if timestamp_millis(&exit.observed_at).is_none() {
            return Err("spawn edge exit observed_at is not a timestamp".into());
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

#[cfg(test)]
mod public_schema_tests {
    use super::PublicSpawnExitObservation;

    #[test]
    fn public_exit_observation_uses_the_tagged_wire_shape() {
        let unknown = PublicSpawnExitObservation::Unknown;
        assert_eq!(
            serde_json::to_value(unknown).expect("serialize unknown"),
            serde_json::json!({ "state": "unknown" })
        );

        let exited = PublicSpawnExitObservation::Exited {
            code: Some(17),
            signal: None,
            observed_at: "2026-01-01T00:00:00.000Z".into(),
        };
        assert_eq!(
            serde_json::to_value(exited).expect("serialize exited"),
            serde_json::json!({
                "state": "exited",
                "code": 17,
                "signal": null,
                "observed_at": "2026-01-01T00:00:00.000Z"
            })
        );
        let partial: PublicSpawnExitObservation = serde_json::from_value(serde_json::json!({
            "state": "exited",
            "observed_at": "2026-01-01T00:00:00.000Z"
        }))
        .expect("optional exit values may be absent on input");
        assert_eq!(
            partial,
            PublicSpawnExitObservation::Exited {
                code: None,
                signal: None,
                observed_at: "2026-01-01T00:00:00.000Z".into(),
            }
        );
    }
}
