//! Complete, source-labelled publication channels with opaque writer fencing.
//!
//! Identity lives in [`super::Registration`]; mutable facts live here. Execution
//! and assignment are independent complete snapshots, each attributed to its
//! actual publisher. A writer is a separate daemon-issued credential scoped to a
//! target agent, channel, publisher source/incarnation and generation. Neither a
//! registration id nor an agent id is that credential.
//!
//! One channel is one record: the current writer binding and, when a report has
//! been accepted, the last snapshot carrying its *own* source/incarnation/handle
//! and generation. Replacing a writer rewrites the binding and leaves the
//! snapshot's authorship untouched; the old facts then read stale without ever
//! being relabelled as the successor's. Acquire, publish and retire each rewrite
//! that single record atomically, so a failed write leaves the previous binding,
//! sequence and snapshot byte-identical and an identical retry still succeeds.
//!
//! Equal-sequence identical replay is idempotent and does not refresh the lease.
//! Daemon receipt time controls expiry; producer observation time is recorded
//! only as provenance. A random serving epoch makes every restored snapshot
//! stale even when clocks repeat or go backwards. A fresh writer cannot be taken
//! over; replacement is explicit, bound to the expected incumbent generation and
//! handle, and fence-rejects every old handle thereafter. Stale means only stale:
//! it is never exit or restart evidence.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::control::timestamp_millis;
use crate::control_plane::store::{format_millis, random_uuid};
use crate::model::SessionUuid;

use super::{Registry, bounded_text, read_record_file, validate_agent_id};

/// Publication record directory under the private registry root.
pub const PUBLICATIONS: &str = "publications";
/// Record schema version.
pub const PUBLICATION_VERSION: u32 = 1;
/// Maximum size for one channel record.
pub const MAX_PUBLICATION_BYTES: usize = 64 * 1024;
/// Default daemon-clock lease.
pub const DEFAULT_LEASE_MS: i64 = 30_000;
/// Minimum permitted lease.
pub const MIN_LEASE_MS: i64 = 1_000;
/// Maximum permitted lease.
pub const MAX_LEASE_MS: i64 = 300_000;
/// Maximum advertised actions per snapshot.
pub const MAX_ACTIONS: usize = 16;

/// Independent publisher channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// Process execution evidence.
    Execution,
    /// Owner assignment projection.
    Assignment,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Execution => "execution",
            Self::Assignment => "assignment",
        }
    }
}

/// A turn outcome kept distinct from current activity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    /// Producer vocabulary is deliberately free-form.
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// One complete snapshot for one channel. Vocabulary is free-form; shape is
/// strict. Unknown state/reason/action words survive, unknown fields do not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub activity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

impl Snapshot {
    fn validate(&self) -> Result<(), String> {
        bounded_text("activity", &self.activity, false)?;
        if let Some(reason) = &self.waiting_reason {
            bounded_text("waiting_reason", reason, false)?;
        }
        if let Some(outcome) = &self.last_outcome {
            bounded_text("last_outcome.result", &outcome.result, false)?;
            if let Some(detail) = &outcome.detail {
                bounded_text("last_outcome.detail", detail, false)?;
            }
        }
        if self.actions.len() > MAX_ACTIONS {
            return Err(format!("`actions` names more than {MAX_ACTIONS} entries"));
        }
        for action in &self.actions {
            bounded_text("action", action, false)?;
        }
        Ok(())
    }
}

/// The publisher presenting itself to acquire a channel writer.
///
/// This is a strict local identity, not a registration lookup: a publisher that
/// restarts presents a new incarnation UUID and needs no second agent record. The
/// handle it is issued fences same-user writers; it is not an authorization
/// framework and proves nothing about assignment authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherIdentity {
    pub source: String,
    pub incarnation: String,
    /// The owner this publisher reports as, when it is an owner projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_owner: Option<String>,
}

impl PublisherIdentity {
    pub(crate) fn validate(&self) -> Result<(), String> {
        bounded_text("publisher.source", &self.source, false)?;
        if SessionUuid::parse(&self.incarnation).is_none() {
            return Err("`publisher.incarnation` must be a canonical UUID".into());
        }
        if let Some(owner) = &self.reporting_owner {
            bounded_text("publisher.reporting_owner", owner, false)?;
        }
        Ok(())
    }
}

/// The incumbent a replacement is allowed to supersede.
///
/// Binding the replacement to the exact generation and handle that were read
/// makes a replayed or delayed replacement harmless: once a newer writer exists,
/// the stale expectation no longer matches and is refused.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedWriter {
    pub generation: u64,
    pub handle: String,
}

impl ExpectedWriter {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.generation == 0 {
            return Err("`replace.generation` must be at least 1".into());
        }
        validate_agent_id(&self.handle)
    }
}

/// Acquire a first channel writer, or explicitly replace a retired/expired one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcquireRequest {
    /// The target agent incarnation the channel reports about.
    pub agent_id: String,
    pub channel: Channel,
    pub publisher: PublisherIdentity,
    /// Absent acquires only a channel with no writer. Present replaces exactly
    /// this incumbent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace: Option<ExpectedWriter>,
}

impl AcquireRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_agent_id(&self.agent_id)?;
        self.publisher.validate()?;
        if let Some(expected) = &self.replace {
            expected.validate()?;
        }
        Ok(())
    }
}

/// A durable authority to write one target agent's one channel. The opaque
/// handle differs from both publisher and target registration IDs but is not an
/// authorization token against hostile same-user code.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriterBinding {
    pub handle: String,
    pub source: String,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_owner: Option<String>,
    pub generation: u64,
    /// Last accepted sequence in this generation; zero until it reports.
    pub sequence: u64,
    /// Explicitly ended writer right; facts remain until they would expire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

impl WriterBinding {
    fn is(&self, publisher: &PublisherIdentity) -> bool {
        self.source == publisher.source
            && self.incarnation == publisher.incarnation
            && self.reporting_owner == publisher.reporting_owner
    }
}

/// One accepted snapshot, keeping the provenance of the writer that reported it.
///
/// Nothing here is rewritten by a later replacement: the source, incarnation,
/// handle and generation are those of the actual writer at the moment of
/// acceptance, so a reader can always tell whose report it is reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedSnapshot {
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
    pub snapshot: Snapshot,
}

impl AcceptedSnapshot {
    fn repeats(&self, request: &PublishRequest) -> bool {
        self.lease_ms == request.lease_ms()
            && self.observed_at == request.observed_at
            && self.snapshot == request.snapshot
    }

    /// Freshness of this accepted snapshot relative to the current writer.
    ///
    /// A snapshot whose writer has been replaced, or whose writer has retired,
    /// is stale even when its lease has not run out: the report was true when it
    /// was made, but it is no longer the current writer's report.
    fn freshness(&self, now_ms: i64, serving_epoch: &str, writer: &WriterBinding) -> Freshness {
        freshness_of(
            &AcceptedLease {
                serving_epoch: &self.serving_epoch,
                handle: &self.handle,
                generation: self.generation,
                expires_at: &self.expires_at,
            },
            writer,
            now_ms,
            serving_epoch,
        )
    }
}

/// One channel: the current writer and the last accepted snapshot, if any.
///
/// The two are stored together so every transition — acquire, publish, retire —
/// is one atomic record replacement. A reader never sees a new binding beside an
/// old snapshot that has not been reconciled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRecord {
    pub version: u32,
    pub agent_id: String,
    pub channel: Channel,
    pub writer: WriterBinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<AcceptedSnapshot>,
}

impl ChannelRecord {
    /// Freshness of the last accepted snapshot, or `None` when nothing has been
    /// reported. An acquired-but-unreported writer has no fabricated activity.
    pub fn freshness(&self, now_ms: i64, serving_epoch: &str) -> Option<Freshness> {
        self.snapshot
            .as_ref()
            .map(|snapshot| snapshot.freshness(now_ms, serving_epoch, &self.writer))
    }

    /// Whether this channel's writer is still live: not retired, with an
    /// unexpired accepted snapshot *of its own generation*. A writer that never
    /// reported — including a successor sitting beside the previous generation's
    /// stale facts — has no live lease and may be replaced explicitly.
    fn incumbent_is_fresh(&self, now_ms: i64) -> bool {
        let accepted = self
            .snapshot
            .as_ref()
            .map(|snapshot| (snapshot.generation, snapshot.expires_at.as_str()));
        incumbent_live(&self.writer, accepted, now_ms)
    }

    /// The channel as an unrelated reader sees it: [`PublicChannel`] with every
    /// fencing handle removed.
    pub fn facts(&self, now_ms: i64, serving_epoch: &str) -> PublicChannelFacts {
        let view = self.view(now_ms, serving_epoch);
        PublicChannelFacts {
            version: view.version,
            agent_id: view.agent_id,
            channel: view.channel,
            writer: PublicWriterFacts {
                source: view.writer.source,
                incarnation: view.writer.incarnation,
                reporting_owner: view.writer.reporting_owner,
                generation: view.writer.generation,
                sequence: view.writer.sequence,
                retired_at: view.writer.retired_at,
            },
            snapshot: view.snapshot.map(|snapshot| PublicSnapshotFacts {
                source: snapshot.source,
                incarnation: snapshot.incarnation,
                reporting_owner: snapshot.reporting_owner,
                generation: snapshot.generation,
                sequence: snapshot.sequence,
                lease_ms: snapshot.lease_ms,
                received_at: snapshot.received_at,
                expires_at: snapshot.expires_at,
                observed_at: snapshot.observed_at,
                restored: snapshot.restored,
                freshness: snapshot.freshness,
                snapshot: snapshot.snapshot,
            }),
        }
    }

    pub fn view(&self, now_ms: i64, serving_epoch: &str) -> PublicChannel {
        PublicChannel {
            version: self.version,
            agent_id: self.agent_id.clone(),
            channel: self.channel,
            writer: PublicWriter {
                handle: self.writer.handle.clone(),
                source: self.writer.source.clone(),
                incarnation: self.writer.incarnation.clone(),
                reporting_owner: self.writer.reporting_owner.clone(),
                generation: self.writer.generation,
                sequence: self.writer.sequence,
                retired_at: self.writer.retired_at.clone(),
            },
            snapshot: self.snapshot.as_ref().map(|snapshot| PublicSnapshot {
                source: snapshot.source.clone(),
                incarnation: snapshot.incarnation.clone(),
                reporting_owner: snapshot.reporting_owner.clone(),
                handle: snapshot.handle.clone(),
                generation: snapshot.generation,
                sequence: snapshot.sequence,
                lease_ms: snapshot.lease_ms,
                received_at: snapshot.received_at.clone(),
                expires_at: snapshot.expires_at.clone(),
                observed_at: snapshot.observed_at.clone(),
                restored: snapshot.serving_epoch != serving_epoch,
                freshness: snapshot.freshness(now_ms, serving_epoch, &self.writer),
                snapshot: snapshot.snapshot.clone(),
            }),
        }
    }
}

/// Freshness is independent of process identity or completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    /// Accepted in this serving epoch by the current writer and not expired.
    Fresh,
    /// Restored, reported by a replaced writer, retired, or expired.
    Stale,
}

/// The serving epoch, handle, generation and lease of one accepted report: the
/// four facts every freshness decision needs, from the channel records and the
/// context record alike.
pub(crate) struct AcceptedLease<'a> {
    pub serving_epoch: &'a str,
    pub handle: &'a str,
    pub generation: u64,
    pub expires_at: &'a str,
}

/// The one freshness rule every published record uses.
///
/// A report is fresh only when this serving epoch accepted it, the current
/// writer still holds the generation and handle it carries, that writer has not
/// retired, and its lease has not expired on the daemon's clock. Otherwise it is
/// stale — which is never evidence of exit, idleness or completion.
pub(crate) fn freshness_of(
    accepted: &AcceptedLease<'_>,
    writer: &WriterBinding,
    now_ms: i64,
    serving_epoch: &str,
) -> Freshness {
    if accepted.serving_epoch != serving_epoch
        || accepted.handle != writer.handle
        || accepted.generation != writer.generation
        || writer.retired_at.is_some()
    {
        return Freshness::Stale;
    }
    match timestamp_millis(accepted.expires_at) {
        Some(expires) if now_ms < expires => Freshness::Fresh,
        _ => Freshness::Stale,
    }
}

/// Whether a writer still holds a live lease *of its own generation*.
///
/// `accepted` is the generation and expiry of the record's last report, when it
/// has one. A writer that never reported — or whose report belongs to an earlier
/// generation — has no live lease and may be replaced explicitly.
pub(crate) fn incumbent_live(
    writer: &WriterBinding,
    accepted: Option<(u64, &str)>,
    now_ms: i64,
) -> bool {
    if writer.retired_at.is_some() {
        return false;
    }
    match accepted {
        Some((generation, expires_at)) if generation == writer.generation => {
            timestamp_millis(expires_at).is_some_and(|expires| now_ms < expires)
        }
        _ => false,
    }
}

/// The current writer as a client reads it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicWriter {
    pub handle: String,
    pub source: String,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_owner: Option<String>,
    pub generation: u64,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

/// The last accepted snapshot as a client reads it, with its own provenance.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSnapshot {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub restored: bool,
    pub freshness: Freshness,
    pub snapshot: Snapshot,
}

/// The current writer as an unrelated reader may see it: provenance and
/// generation, without the opaque handle an update must present.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicWriterFacts {
    pub source: String,
    pub incarnation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reporting_owner: Option<String>,
    pub generation: u64,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retired_at: Option<String>,
}

/// The last accepted report as an unrelated reader may see it, leaving out the
/// handle that fences further writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSnapshotFacts {
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
    pub snapshot: Snapshot,
}

/// A channel read for a reader that is not its writer: the same identity,
/// freshness and report facts as [`PublicChannel`], with the fencing handle
/// dropped because it is a credential rather than a fact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicChannelFacts {
    pub version: u32,
    pub agent_id: String,
    pub channel: Channel,
    pub writer: PublicWriterFacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PublicSnapshotFacts>,
}

/// A bounded channel read: current writer plus the actual last report, if any.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicChannel {
    pub version: u32,
    pub agent_id: String,
    pub channel: Channel,
    pub writer: PublicWriter,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<PublicSnapshot>,
}

/// A publish under an existing writer handle.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRequest {
    pub agent_id: String,
    pub channel: Channel,
    pub writer_handle: String,
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
    pub snapshot: Snapshot,
}

impl PublishRequest {
    pub fn validate(&self) -> Result<(), String> {
        validate_agent_id(&self.agent_id)?;
        validate_agent_id(&self.writer_handle)?;
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
        self.snapshot.validate()
    }

    pub fn lease_ms(&self) -> i64 {
        self.lease_ms.unwrap_or(DEFAULT_LEASE_MS)
    }
}

/// Acquire a first writer, or explicitly replace a retired/expired incumbent.
///
/// A retry with the identical publisher, target and channel returns the binding
/// that already exists. A different publisher cannot overwrite the incumbent
/// implicitly, and a replacement is refused unless it names the exact incumbent
/// generation and handle and that incumbent is retired or expired.
pub fn acquire_writer(
    registry: &Registry,
    request: &AcquireRequest,
    now_ms: i64,
) -> Result<WriterBinding, String> {
    request.validate()?;
    let _transition = registry.transition();
    // The subject must be a registered agent incarnation. The publisher is not
    // looked up: a publisher restart is a new incarnation UUID, not a second
    // registration, and its identity is recorded as provenance rather than
    // admitted against an agent record.
    registry
        .get(&request.agent_id)?
        .ok_or_else(|| format!("agent {} is not registered", request.agent_id))?;
    let existing = read_record(registry, &request.agent_id, request.channel)?;
    let record = match (existing, &request.replace) {
        (None, None) => ChannelRecord {
            version: PUBLICATION_VERSION,
            agent_id: request.agent_id.clone(),
            channel: request.channel,
            writer: new_binding(&request.publisher, 1),
            snapshot: None,
        },
        (None, Some(_)) => return Err("no writer exists to replace".into()),
        (Some(record), None) => {
            if record.writer.is(&request.publisher) {
                return Ok(record.writer);
            }
            return Err("writer already exists; replacement is explicit".into());
        }
        (Some(record), Some(expected)) => {
            if record.writer.generation != expected.generation
                || record.writer.handle != expected.handle
            {
                return Err("incumbent writer changed; replacement does not apply".into());
            }
            if record.incumbent_is_fresh(now_ms) {
                return Err("fresh writer takeover is refused".into());
            }
            let generation = record.writer.generation + 1;
            ChannelRecord {
                writer: new_binding(&request.publisher, generation),
                ..record
            }
        }
    };
    let binding = record.writer.clone();
    write_record(registry, &record)?;
    Ok(binding)
}

/// Read the current durable channel record.
pub fn channel(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
) -> Result<Option<ChannelRecord>, String> {
    read_record(registry, agent_id, channel)
}

/// Read the source-labelled public channel projection.
pub fn published(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
    now_ms: i64,
) -> Result<Option<PublicChannel>, String> {
    Ok(read_record(registry, agent_id, channel)?
        .map(|record| record.view(now_ms, registry.serving_epoch())))
}

/// Read the channel as an unrelated reader sees it: same facts, no handle.
pub fn published_facts(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
    now_ms: i64,
) -> Result<Option<PublicChannelFacts>, String> {
    Ok(read_record(registry, agent_id, channel)?
        .map(|record| record.facts(now_ms, registry.serving_epoch())))
}

/// Submit a complete snapshot under its current channel writer.
///
/// A strictly newer sequence replaces the whole snapshot and the writer's stored
/// sequence in one record write. An equal sequence is idempotent only when its
/// content, lease and observation time match exactly; otherwise it is a conflict.
/// A failed write leaves the stored sequence and snapshot unchanged, so an
/// identical retry still succeeds.
pub fn publish(
    registry: &Registry,
    request: &PublishRequest,
    now_ms: i64,
) -> Result<ChannelRecord, String> {
    request.validate()?;
    let _transition = registry.transition();
    let record = read_record(registry, &request.agent_id, request.channel)?
        .ok_or_else(|| "channel has no writer".to_string())?;
    if record.writer.handle != request.writer_handle {
        return Err("writer handle is stale or belongs to another channel generation".into());
    }
    if record.writer.retired_at.is_some() {
        return Err("writer is retired".into());
    }
    if request.sequence < record.writer.sequence {
        return Err("sequence is older than stored sequence".into());
    }
    if request.sequence == record.writer.sequence {
        let replay = record
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.repeats(request));
        if record.writer.sequence > 0 && replay {
            return Ok(record);
        }
        return Err("equal sequence has conflicting content".into());
    }
    let lease = request.lease_ms();
    let writer = &record.writer;
    let updated = ChannelRecord {
        version: record.version,
        agent_id: record.agent_id.clone(),
        channel: record.channel,
        writer: WriterBinding {
            sequence: request.sequence,
            ..writer.clone()
        },
        snapshot: Some(AcceptedSnapshot {
            source: writer.source.clone(),
            incarnation: writer.incarnation.clone(),
            reporting_owner: writer.reporting_owner.clone(),
            handle: writer.handle.clone(),
            generation: writer.generation,
            sequence: request.sequence,
            lease_ms: lease,
            received_at: format_millis(now_ms),
            expires_at: format_millis(now_ms + lease),
            serving_epoch: registry.serving_epoch().into(),
            observed_at: request.observed_at.clone(),
            snapshot: request.snapshot.clone(),
        }),
    };
    write_record(registry, &updated)?;
    Ok(updated)
}

/// Retire this channel's current writer without retracting its last facts.
pub fn retire(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
    writer_handle: &str,
    now_ms: i64,
) -> Result<ChannelRecord, String> {
    validate_agent_id(writer_handle)?;
    let _transition = registry.transition();
    let record = read_record(registry, agent_id, channel)?
        .ok_or_else(|| "channel has no writer".to_string())?;
    if record.writer.handle != writer_handle {
        return Err("writer handle is stale or belongs to another channel generation".into());
    }
    if record.writer.retired_at.is_some() {
        return Ok(record);
    }
    let updated = ChannelRecord {
        writer: WriterBinding {
            retired_at: Some(format_millis(now_ms)),
            ..record.writer.clone()
        },
        ..record
    };
    write_record(registry, &updated)?;
    Ok(updated)
}

pub(crate) fn new_binding(publisher: &PublisherIdentity, generation: u64) -> WriterBinding {
    WriterBinding {
        handle: random_uuid(),
        source: publisher.source.clone(),
        incarnation: publisher.incarnation.clone(),
        reporting_owner: publisher.reporting_owner.clone(),
        generation,
        sequence: 0,
        retired_at: None,
    }
}

fn read_record(
    registry: &Registry,
    agent_id: &str,
    channel: Channel,
) -> Result<Option<ChannelRecord>, String> {
    validate_agent_id(agent_id)?;
    let path = record_path(registry, agent_id, channel);
    let bytes = match read_record_file(&path, MAX_PUBLICATION_BYTES) {
        Ok(bytes) => bytes,
        Err(error) if error.contains("No such file or directory") => return Ok(None),
        Err(error) => return Err(error),
    };
    let record: ChannelRecord =
        serde_json::from_slice(&bytes).map_err(|error| format!("not a channel record: {error}"))?;
    if record.version != PUBLICATION_VERSION
        || record.agent_id != agent_id
        || record.channel != channel
    {
        return Err("channel record has invalid version, target or channel".into());
    }
    if record.writer.generation == 0 {
        return Err("channel writer generation is zero".into());
    }
    validate_agent_id(&record.writer.handle)?;
    validate_agent_id(&record.writer.incarnation)?;
    if let Some(snapshot) = &record.snapshot {
        if snapshot.generation == 0 || snapshot.generation > record.writer.generation {
            return Err("channel snapshot generation is out of range".into());
        }
        validate_agent_id(&snapshot.handle)?;
        validate_agent_id(&snapshot.incarnation)?;
    }
    Ok(Some(record))
}

fn record_path(registry: &Registry, agent_id: &str, channel: Channel) -> std::path::PathBuf {
    registry
        .publications()
        .join(format!("{agent_id}.{}.json", channel.name()))
}

/// Writes the whole channel record atomically: a mode-`0600` temporary sibling,
/// flushed, renamed into place, and the containing directory synced so the
/// acknowledgment claims durability and not just a visible rename.
fn write_record(registry: &Registry, record: &ChannelRecord) -> Result<(), String> {
    write_record_at(
        registry,
        record,
        &record_path(registry, &record.agent_id, record.channel),
    )
}

#[cfg(test)]
fn write_record_with_failure(
    registry: &Registry,
    record: &ChannelRecord,
    fail_before_rename: bool,
) -> Result<(), String> {
    let path = record_path(registry, &record.agent_id, record.channel);
    write_record_at_inner(registry, record, &path, fail_before_rename)
}

fn write_record_at(
    registry: &Registry,
    record: &ChannelRecord,
    path: &std::path::Path,
) -> Result<(), String> {
    #[cfg(test)]
    {
        write_record_at_inner(registry, record, path, false)
    }
    #[cfg(not(test))]
    {
        write_record_at_inner(registry, record, path)
    }
}

/// Writes one bounded registry record atomically: a mode-`0600` temporary
/// sibling of `path`, flushed, renamed into place, and the containing directory
/// synced so the acknowledgment claims durability and not just a visible rename.
///
/// Every registry record kind — channel and context alike — writes through this
/// one helper so the discipline cannot drift between them. The failure-injection
/// seam the channel tests use stays private to this module.
pub(crate) fn write_atomic(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
    bound: usize,
) -> Result<(), String> {
    #[cfg(test)]
    {
        write_atomic_inner(directory, path, bytes, bound, false)
    }
    #[cfg(not(test))]
    {
        write_atomic_inner(directory, path, bytes, bound)
    }
}

fn write_atomic_inner(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
    bound: usize,
    #[cfg(test)] fail_before_rename: bool,
) -> Result<(), String> {
    if bytes.len() > bound {
        return Err("record exceeds record bound".into());
    }
    let temporary = directory.join(format!(".tmp-{}", random_uuid()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        #[cfg(test)]
        if fail_before_rename {
            return Err(std::io::Error::other("injected write failure"));
        }
        fs::rename(&temporary, path)?;
        File::open(directory)?.sync_all()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(format!("record {}: {error}", path.display()));
    }
    Ok(())
}

fn write_record_at_inner(
    registry: &Registry,
    record: &ChannelRecord,
    path: &Path,
    #[cfg(test)] fail_before_rename: bool,
) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|error| error.to_string())?;
    let directory = registry.publications();
    #[cfg(test)]
    {
        write_atomic_inner(
            &directory,
            path,
            &bytes,
            MAX_PUBLICATION_BYTES,
            fail_before_rename,
        )
    }
    #[cfg(not(test))]
    {
        write_atomic_inner(&directory, path, &bytes, MAX_PUBLICATION_BYTES)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::control_plane::registry::RegistrationRequest;

    const TEST_INCARNATION: &str = "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22";

    fn temporary_registry() -> (Registry, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "radar-registry-write-failure-{}-{}",
            std::process::id(),
            random_uuid()
        ));
        fs::create_dir_all(&root).expect("temporary root");
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).expect("root mode");
        (Registry::open_at(&root, 1_000).expect("registry"), root)
    }

    fn test_registration() -> RegistrationRequest {
        RegistrationRequest {
            source: "test-publisher".into(),
            incarnation: TEST_INCARNATION.into(),
            session: None,
            owner: None,
            run: None,
            label: None,
            location: None,
            process: None,
            launch: None,
        }
    }

    fn snapshot(activity: &str) -> Snapshot {
        Snapshot {
            activity: activity.into(),
            waiting_reason: Some("waiting-for-owner".into()),
            last_outcome: Some(Outcome {
                result: "failed-tool-timeout".into(),
                detail: None,
            }),
            actions: vec!["retry-in-two-minutes".into()],
        }
    }

    #[test]
    fn a_failed_atomic_write_leaves_old_sequence_then_retry_survives_reopen() {
        let (registry, root) = temporary_registry();
        let agent = registry
            .register(&test_registration(), 1_000)
            .expect("register");
        let publisher = PublisherIdentity {
            source: "test-publisher".into(),
            incarnation: TEST_INCARNATION.into(),
            reporting_owner: None,
        };
        let writer = registry
            .acquire_writer(
                &AcquireRequest {
                    agent_id: agent.agent_id.clone(),
                    channel: Channel::Execution,
                    publisher,
                    replace: None,
                },
                1_000,
            )
            .expect("acquire");
        let make_request = |sequence| PublishRequest {
            agent_id: agent.agent_id.clone(),
            channel: Channel::Execution,
            writer_handle: writer.handle.clone(),
            sequence,
            lease_ms: Some(10_000),
            observed_at: None,
            snapshot: Snapshot {
                activity: "working".into(),
                waiting_reason: None,
                last_outcome: None,
                actions: Vec::new(),
            },
        };
        registry
            .publish(&make_request(1), 1_000)
            .expect("first publish");
        let path = record_path(&registry, &agent.agent_id, Channel::Execution);
        let before = fs::read(&path).expect("old record bytes");
        let old_record = registry
            .channel(&agent.agent_id, Channel::Execution)
            .expect("read")
            .expect("record");
        let failed = ChannelRecord {
            writer: WriterBinding {
                sequence: 2,
                ..old_record.writer.clone()
            },
            snapshot: Some(AcceptedSnapshot {
                source: old_record.writer.source.clone(),
                incarnation: old_record.writer.incarnation.clone(),
                reporting_owner: old_record.writer.reporting_owner.clone(),
                handle: old_record.writer.handle.clone(),
                generation: old_record.writer.generation,
                sequence: 2,
                lease_ms: 10_000,
                received_at: format_millis(2_000),
                expires_at: format_millis(12_000),
                serving_epoch: registry.serving_epoch().into(),
                observed_at: None,
                snapshot: make_request(2).snapshot,
            }),
            ..old_record.clone()
        };
        assert!(write_record_with_failure(&registry, &failed, true).is_err());
        assert_eq!(fs::read(&path).expect("old record still present"), before);
        assert_eq!(
            registry
                .channel(&agent.agent_id, Channel::Execution)
                .unwrap()
                .unwrap()
                .writer
                .sequence,
            1,
            "failed write consumes no sequence"
        );

        let accepted = registry.publish(&make_request(2), 2_000).expect("retry");
        assert_eq!(accepted.writer.sequence, 2);
        drop(registry);
        let reopened = Registry::open_at(&root, 2_000).expect("reopen");
        let restored = reopened
            .channel(&agent.agent_id, Channel::Execution)
            .unwrap()
            .unwrap();
        assert_eq!(restored.writer.sequence, 2);
        assert_eq!(restored.snapshot.unwrap().sequence, 2);
        fs::remove_dir_all(root).expect("remove temporary root");
    }

    #[test]
    fn vocabulary_is_open_but_shape_is_strict() {
        let snapshot = snapshot("future-activity-word");
        let bytes = serde_json::to_vec(&snapshot).expect("encode");
        assert_eq!(
            serde_json::from_slice::<Snapshot>(&bytes).expect("decode"),
            snapshot
        );
        assert!(serde_json::from_str::<Snapshot>(r#"{"activity":"idle","extra":true}"#).is_err());
        assert!(serde_json::from_str::<Outcome>(r#"{"result":"failed","code":1}"#).is_err());
    }
}
