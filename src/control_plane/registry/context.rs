//! The mutable current-session context: one agent record's live session UUID.
//!
//! A live Pi process can switch or fork its session without becoming a new
//! process subject. Registration is immutable per `(source, incarnation)`, and
//! the execution and assignment channels carry activity, waiting reason, outcome
//! and actions — none of which a session UUID is. So the current session is its
//! own record: one atomic file beside the channel records, under the same
//! fencing — one writer at a time, a strictly forward sequence, a bounded lease
//! and daemon-time freshness.
//!
//! The record carries a canonical session UUID or an explicit null meaning "no
//! current session", and nothing else private: no session path, no launch
//! argument, no environment and no raw provider text. No record at all means
//! "never published", which is a different answer from an accepted null.
//!
//! Publishing context never changes the agent id, the registration or the writer
//! generation: a session switch is a newer sequence under the same writer. A
//! retired or expired writer is succeeded only by an explicit replacement that
//! names the incumbent it observed, which advances the generation once and
//! fences every older handle thereafter. Equal-sequence identical replay is
//! answered with the stored record and a warning — never a refusal, and never a
//! refreshed lease, because a replay is not a heartbeat.
//!
//! Stale or absent context is unknown: it is never evidence that a process
//! exited, went idle or completed anything.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::control::timestamp_millis;
use crate::control_plane::store::format_millis;
use crate::model::SessionUuid;

use super::publication::{
    AcceptedLease, DEFAULT_LEASE_MS, ExpectedWriter, Freshness, MAX_LEASE_MS, MIN_LEASE_MS,
    PublicWriterFacts, PublisherIdentity, WriterBinding, freshness_of, incumbent_live, new_binding,
    write_atomic,
};
use super::{Registry, bounded_text, read_record_file, validate_agent_id};

/// Record schema version.
pub const CONTEXT_VERSION: u32 = 1;
/// Maximum size for one context record.
pub const MAX_CONTEXT_BYTES: usize = 64 * 1024;

/// The context a publish reports.
///
/// A canonical session UUID, or nothing for an explicit "no current session".
/// Paths, arguments and provider text cannot ride here: they are not this
/// field, and an unknown field is refused.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextValue {
    #[serde(default)]
    pub session: Option<String>,
}

/// A publish of one agent record's current session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextRequest {
    /// The agent record whose current session this is.
    pub agent_id: String,
    /// The publisher presenting itself: the writer a first publish binds, and
    /// the successor a replacement binds. Recorded as provenance, never looked
    /// up — a publisher restart is a new incarnation UUID, not a second agent.
    pub publisher: PublisherIdentity,
    /// The handle this publisher already holds. Absent on a first publish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub writer_handle: Option<String>,
    /// The exact incumbent a replacement observed. Present only to succeed a
    /// retired or expired writer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<ExpectedWriter>,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub context: ContextValue,
}

impl ContextRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_agent_id(&self.agent_id)?;
        self.publisher.validate()?;
        if let Some(handle) = &self.writer_handle {
            validate_agent_id(handle)?;
        }
        if let Some(expected) = &self.replace {
            expected.validate()?;
        }
        if self.sequence == 0 {
            return Err("`sequence` must be at least 1".into());
        }
        if let Some(lease) = self.lease_ms
            && !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&lease)
        {
            return Err(format!(
                "`lease_ms` must be between {MIN_LEASE_MS} and {MAX_LEASE_MS}"
            ));
        }
        if let Some(observed) = &self.observed_at {
            bounded_text("observed_at", observed, false)?;
            if timestamp_millis(observed).is_none() {
                return Err("`observed_at` is not a timestamp".into());
            }
        }
        if let Some(session) = &self.context.session
            && SessionUuid::parse(session).is_none()
        {
            return Err("`context.session` must be a canonical UUID".into());
        }
        Ok(())
    }

    pub fn lease_ms(&self) -> i64 {
        self.lease_ms.unwrap_or(DEFAULT_LEASE_MS)
    }
}

/// One accepted current-session report, with the provenance and lease of the
/// writer that made it.
///
/// Nothing here is rewritten by a later replacement: the source, incarnation,
/// handle and generation are those of the writer at the moment of acceptance, so
/// a reader can always tell whose report it is reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedContext {
    pub source: String,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_owner: Option<String>,
    pub handle: String,
    pub generation: u64,
    pub sequence: u64,
    pub lease_ms: i64,
    pub received_at: String,
    pub expires_at: String,
    pub serving_epoch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    /// The current session, or `None` for an explicit "no current session".
    #[serde(default)]
    pub session: Option<String>,
}

impl AcceptedContext {
    fn repeats(&self, request: &ContextRequest) -> bool {
        self.lease_ms == request.lease_ms()
            && self.observed_at == request.observed_at
            && self.session == request.context.session
    }
}

/// One agent record's current-session context: the current writer and the last
/// accepted report.
///
/// The two are stored together so every transition — first publish, report,
/// retire, replacement — is one atomic record replacement. A record with no
/// accepted report cannot exist: unlike a channel there is no acquire step, so a
/// stored file always answers "what is the current session", even when that
/// answer is an explicit null.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionContext {
    pub version: u32,
    pub agent_id: String,
    pub writer: WriterBinding,
    pub context: AcceptedContext,
}

impl SessionContext {
    /// Freshness of the accepted report relative to the current writer, under
    /// the one rule the channel records also use.
    pub fn freshness(&self, now_ms: i64, serving_epoch: &str) -> Freshness {
        freshness_of(
            &AcceptedLease {
                serving_epoch: &self.context.serving_epoch,
                handle: &self.context.handle,
                generation: self.context.generation,
                expires_at: &self.context.expires_at,
            },
            &self.writer,
            now_ms,
            serving_epoch,
        )
    }

    /// The context as an unrelated reader sees it: the session, its provenance
    /// and its freshness, with the fencing handle and the serving epoch dropped.
    pub fn public(&self, now_ms: i64, serving_epoch: &str) -> PublicContext {
        PublicContext {
            version: self.version,
            agent_id: self.agent_id.clone(),
            writer: PublicWriterFacts {
                source: self.writer.source.clone(),
                incarnation: self.writer.incarnation.clone(),
                reporting_owner: self.writer.reporting_owner.clone(),
                generation: self.writer.generation,
                sequence: self.writer.sequence,
                retired_at: self.writer.retired_at.clone(),
            },
            context: PublicSession {
                source: self.context.source.clone(),
                incarnation: self.context.incarnation.clone(),
                reporting_owner: self.context.reporting_owner.clone(),
                generation: self.context.generation,
                sequence: self.context.sequence,
                lease_ms: self.context.lease_ms,
                received_at: self.context.received_at.clone(),
                expires_at: self.context.expires_at.clone(),
                observed_at: self.context.observed_at.clone(),
                restored: self.context.serving_epoch != serving_epoch,
                freshness: self.freshness(now_ms, serving_epoch),
                session: self.context.session.clone(),
            },
        }
    }
}

/// The last accepted context as a client reads it.
///
/// There is no handle and no serving epoch here by construction, and no other
/// field a private path or argument could be forgotten into.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSession {
    pub source: String,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_owner: Option<String>,
    pub generation: u64,
    pub sequence: u64,
    pub lease_ms: i64,
    pub received_at: String,
    pub expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub restored: bool,
    pub freshness: Freshness,
    /// The current session, or an explicit null for "no current session".
    ///
    /// Written even when null: a present record is what distinguishes "no
    /// current session" from "never published", which is no context record at
    /// all.
    pub session: Option<String>,
}

/// One agent record's context as a client reads it. A `None` read of this whole
/// record means never published; a present record with a null session means no
/// current session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicContext {
    pub version: u32,
    pub agent_id: String,
    pub writer: PublicWriterFacts,
    pub context: PublicSession,
}

/// The answer to a context publish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextWrite {
    /// The stored record after the write.
    pub record: SessionContext,
    /// Set when an identical replay was answered from the stored record. A
    /// replay is never a refusal, and never a lease refresh.
    pub warning: Option<String>,
}

/// Publish one agent record's current session.
///
/// The first publish binds a generation-1 writer to the presenting publisher. A
/// report under the incumbent handle replaces the stored association when its
/// sequence is strictly newer, is answered with the stored record when it
/// exactly repeats the stored report, and is refused when its sequence is older
/// or its content conflicts. A retired or expired incumbent is succeeded only by
/// a replacement naming that incumbent's exact generation and handle, which
/// advances the generation and fences every older handle.
pub fn publish(
    registry: &Registry,
    request: &ContextRequest,
    now_ms: i64,
) -> Result<ContextWrite, String> {
    request.validate()?;
    let _transition = registry.transition();
    // The context is a fact about an agent record, so the subject must exist.
    registry
        .get(&request.agent_id)?
        .ok_or_else(|| format!("agent {} is not registered", request.agent_id))?;
    let existing = read_record(registry, &request.agent_id)?;

    let (writer, sequence) = match existing {
        None => {
            if request.writer_handle.is_some() || request.replace.is_some() {
                return Err("context has no writer yet: the first publish binds one".into());
            }
            (new_binding(&request.publisher, 1), request.sequence)
        }
        Some(record) => match &request.replace {
            Some(expected) => {
                let incumbent = &record.writer;
                if incumbent.generation != expected.generation
                    || incumbent.handle != expected.handle
                {
                    return Err(refusal(
                        "incumbent writer changed; replacement does not apply",
                        incumbent,
                    ));
                }
                if incumbent_live(
                    incumbent,
                    Some((record.context.generation, &record.context.expires_at)),
                    now_ms,
                ) {
                    return Err(refusal("fresh writer takeover is refused", incumbent));
                }
                (
                    new_binding(&request.publisher, incumbent.generation + 1),
                    request.sequence,
                )
            }
            None => {
                let incumbent = &record.writer;
                match request.writer_handle.as_deref() {
                    Some(handle) if handle == incumbent.handle => {}
                    Some(_) => {
                        return Err(refusal(
                            "context writer handle is stale or belongs to another generation",
                            incumbent,
                        ));
                    }
                    None => {
                        return Err(refusal(
                            "context has a writer this publisher did not present",
                            incumbent,
                        ));
                    }
                }
                if incumbent.retired_at.is_some() {
                    return Err(refusal("writer is retired", incumbent));
                }
                if request.sequence < incumbent.sequence {
                    return Err("sequence is older than stored sequence".into());
                }
                if request.sequence == incumbent.sequence {
                    if !record.context.repeats(request) {
                        return Err("equal sequence has conflicting content".into());
                    }
                    let warning = format!(
                        "identical context replay at sequence {} was answered from the stored record; its lease still ends at {} and was not refreshed",
                        incumbent.sequence, record.context.expires_at
                    );
                    return Ok(ContextWrite {
                        record,
                        warning: Some(warning),
                    });
                }
                (incumbent.clone(), request.sequence)
            }
        },
    };

    let lease = request.lease_ms();
    let accepted = AcceptedContext {
        source: writer.source.clone(),
        incarnation: writer.incarnation.clone(),
        reporting_owner: writer.reporting_owner.clone(),
        handle: writer.handle.clone(),
        generation: writer.generation,
        sequence,
        lease_ms: lease,
        received_at: format_millis(now_ms),
        expires_at: format_millis(now_ms + lease),
        serving_epoch: registry.serving_epoch().into(),
        observed_at: request.observed_at.clone(),
        session: request.context.session.clone(),
    };
    let record = SessionContext {
        version: CONTEXT_VERSION,
        agent_id: request.agent_id.clone(),
        writer: WriterBinding { sequence, ..writer },
        context: accepted,
    };
    write_record(registry, &record)?;
    Ok(ContextWrite {
        record,
        warning: None,
    })
}

/// Retire this agent's context writer without retracting its last report.
pub fn retire(
    registry: &Registry,
    agent_id: &str,
    writer_handle: &str,
    now_ms: i64,
) -> Result<SessionContext, String> {
    validate_agent_id(writer_handle)?;
    let _transition = registry.transition();
    let record =
        read_record(registry, agent_id)?.ok_or_else(|| "context has no writer".to_string())?;
    if record.writer.handle != writer_handle {
        return Err(refusal(
            "context writer handle is stale or belongs to another generation",
            &record.writer,
        ));
    }
    if record.writer.retired_at.is_some() {
        return Ok(record);
    }
    let updated = SessionContext {
        writer: WriterBinding {
            retired_at: Some(format_millis(now_ms)),
            ..record.writer.clone()
        },
        ..record
    };
    write_record(registry, &updated)?;
    Ok(updated)
}

/// Read one agent record's context as an unrelated reader sees it, or `None`
/// when this agent has never published one.
pub fn published(
    registry: &Registry,
    agent_id: &str,
    now_ms: i64,
) -> Result<Option<PublicContext>, String> {
    Ok(read_record(registry, agent_id)?
        .map(|record| record.public(now_ms, registry.serving_epoch())))
}

/// Refuse a handshake, naming both why and which writer actually holds the
/// record.
///
/// A publisher needs both to retry correctly. The handle is a fence between
/// cooperating producers of one record, not a secret from them: it appears in no
/// public read.
fn refusal(reason: &str, incumbent: &WriterBinding) -> String {
    format!(
        "{reason}: the incumbent is generation {} handle {}",
        incumbent.generation, incumbent.handle
    )
}

fn read_record(registry: &Registry, agent_id: &str) -> Result<Option<SessionContext>, String> {
    validate_agent_id(agent_id)?;
    let path = record_path(registry, agent_id);
    let bytes = match read_record_file(&path, MAX_CONTEXT_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.contains("No such file or directory") => return Ok(None),
        Err(error) => return Err(error),
    };
    let record: SessionContext =
        serde_json::from_slice(&bytes).map_err(|error| format!("not a context record: {error}"))?;
    if record.version != CONTEXT_VERSION {
        return Err(format!(
            "context record version {} is not served; this daemon writes {CONTEXT_VERSION}",
            record.version
        ));
    }
    if record.agent_id != agent_id {
        return Err(format!(
            "context record id does not match its filename: {}",
            path.display()
        ));
    }
    if record.writer.generation == 0 {
        return Err("context writer generation is zero".into());
    }
    validate_agent_id(&record.writer.handle)?;
    validate_agent_id(&record.writer.incarnation)?;
    if record.context.generation != record.writer.generation {
        return Err("context report generation is not the current writer generation".into());
    }
    validate_agent_id(&record.context.handle)?;
    validate_agent_id(&record.context.incarnation)?;
    if let Some(session) = &record.context.session
        && SessionUuid::parse(session).is_none()
    {
        return Err("context record names a malformed session".into());
    }
    Ok(Some(record))
}

fn record_path(registry: &Registry, agent_id: &str) -> PathBuf {
    registry
        .publications()
        .join(format!("{agent_id}.context.json"))
}

fn write_record(registry: &Registry, record: &SessionContext) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    write_atomic(
        &registry.publications(),
        &record_path(registry, &record.agent_id),
        &bytes,
        MAX_CONTEXT_BYTES,
    )
}
