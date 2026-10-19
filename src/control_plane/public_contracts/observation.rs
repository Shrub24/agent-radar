use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::control_plane::observation::{
    Snapshot as InternalSnapshot, SourceState as InternalSource,
};
use crate::model::{AgentObservation, FleetObservation, ForegroundEvidence};
use crate::observation::{ObservationState, RetainedAgent, RetentionBasis, SourceFreshness};

/// Input accepted by `observation.get`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObservationGetParams {
    #[serde(default)]
    pub epoch: Option<String>,
    #[serde(default)]
    #[schemars(range(max = 9007199254740991u64))]
    pub since_revision: Option<u64>,
    #[serde(default)]
    #[schemars(range(max = 30000u64))]
    pub wait_ms: Option<u64>,
    #[serde(default)]
    pub classes: Option<Vec<String>>,
}

/// Source freshness included in an observation response.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationFreshness {
    #[default]
    Pending,
    Current,
    Stale {
        diagnostic: String,
    },
    Unavailable {
        diagnostic: String,
    },
}

/// One source's freshness in a snapshot revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationSource {
    pub name: String,
    pub state: ObservationFreshness,
    pub last_success_at: Option<String>,
}

/// Why a retained association is reported as not current.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetainedBasis {
    ShellForeground,
    Unverified,
}

/// Retained agent facts carried across an idle-shell gap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RetainedObservation {
    pub observation: AgentObservation,
    pub basis: RetainedBasis,
}

/// Public observation projection. The sampler's mutable state remains private;
/// this DTO names the fields the daemon publishes without serializing that state
/// type directly.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationProjection {
    pub inventory: Option<FleetObservation>,
    pub source: ObservationFreshness,
    #[serde(default)]
    pub registry: ObservationFreshness,
    pub retained: BTreeMap<String, RetainedObservation>,
    pub containment: BTreeMap<String, RetainedObservation>,
    pub awaiting_evidence: BTreeSet<String>,
    pub foreground: BTreeMap<String, ForegroundEvidence>,
}

/// One immutable observation revision on the public wire.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationSnapshot {
    pub epoch: String,
    #[schemars(range(max = 9007199254740991u64))]
    pub revision: u64,
    pub collected_at: Option<String>,
    pub sources: Vec<ObservationSource>,
    pub tasks: Option<Value>,
    pub observation: ObservationProjection,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationRevision {
    pub epoch: String,
    #[schemars(range(max = 9007199254740991u64))]
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationTooLarge {
    pub epoch: String,
    #[schemars(range(max = 9007199254740991u64))]
    pub revision: u64,
    #[schemars(range(max = 9007199254740991u64))]
    pub limit_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationSnapshotResult {
    pub epoch: String,
    #[schemars(range(max = 9007199254740991u64))]
    pub revision: u64,
    pub snapshot: ObservationSnapshot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationNotModifiedResult {
    pub not_modified: ObservationRevision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationExpiredResult {
    pub snapshot_expired: ObservationRevision,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObservationTooLargeResult {
    pub snapshot_too_large: ObservationTooLarge,
}

/// The result forms returned by `observation.get`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ObservationGetResponse {
    Snapshot(Box<ObservationSnapshotResult>),
    NotModified(ObservationNotModifiedResult),
    Expired(ObservationExpiredResult),
    TooLarge(ObservationTooLargeResult),
}

impl ObservationGetResponse {
    pub fn snapshot(snapshot: &InternalSnapshot) -> Self {
        let snapshot = ObservationSnapshot::from(snapshot);
        Self::Snapshot(Box::new(ObservationSnapshotResult {
            epoch: snapshot.epoch.clone(),
            revision: snapshot.revision,
            snapshot,
        }))
    }

    pub fn not_modified(epoch: String, revision: u64) -> Self {
        Self::NotModified(ObservationNotModifiedResult {
            not_modified: ObservationRevision { epoch, revision },
        })
    }

    pub fn expired(epoch: String, revision: u64) -> Self {
        Self::Expired(ObservationExpiredResult {
            snapshot_expired: ObservationRevision { epoch, revision },
        })
    }

    pub fn too_large(epoch: String, revision: u64, limit_bytes: usize) -> Self {
        Self::TooLarge(ObservationTooLargeResult {
            snapshot_too_large: ObservationTooLarge {
                epoch,
                revision,
                limit_bytes: limit_bytes as u64,
            },
        })
    }
}

impl From<&InternalSnapshot> for ObservationSnapshot {
    fn from(snapshot: &InternalSnapshot) -> Self {
        Self {
            epoch: snapshot.epoch.clone(),
            revision: snapshot.revision,
            collected_at: snapshot.collected_at.clone(),
            sources: snapshot
                .sources
                .iter()
                .map(ObservationSource::from)
                .collect(),
            tasks: snapshot.tasks.clone(),
            observation: ObservationProjection::from(&snapshot.observation),
        }
    }
}

impl From<ObservationSnapshot> for InternalSnapshot {
    fn from(snapshot: ObservationSnapshot) -> Self {
        Self {
            epoch: snapshot.epoch,
            revision: snapshot.revision,
            collected_at: snapshot.collected_at,
            sources: snapshot.sources.into_iter().map(Into::into).collect(),
            tasks: snapshot.tasks,
            observation: snapshot.observation.into(),
        }
    }
}

impl From<&InternalSource> for ObservationSource {
    fn from(source: &InternalSource) -> Self {
        Self {
            name: source.name.clone(),
            state: (&source.state).into(),
            last_success_at: source.last_success_at.clone(),
        }
    }
}

impl From<&ObservationState> for ObservationProjection {
    fn from(state: &ObservationState) -> Self {
        let foreground = state
            .pane_ids()
            .into_iter()
            .filter_map(|pane_id| {
                state
                    .foreground(&pane_id)
                    .cloned()
                    .map(|evidence| (pane_id, evidence))
            })
            .collect();
        Self {
            inventory: state.inventory().cloned(),
            source: state.source_freshness().into(),
            registry: state.registry_freshness().into(),
            retained: state
                .retained()
                .iter()
                .map(|(pane, retained)| (pane.clone(), retained.into()))
                .collect(),
            containment: state
                .retained_containment()
                .iter()
                .map(|(pane, retained)| (pane.clone(), retained.into()))
                .collect(),
            awaiting_evidence: state.continuity_candidates().into_iter().collect(),
            foreground,
        }
    }
}

impl From<&RetainedAgent> for RetainedObservation {
    fn from(retained: &RetainedAgent) -> Self {
        Self {
            observation: retained.observation.clone(),
            basis: (&retained.basis).into(),
        }
    }
}

impl From<&SourceFreshness> for ObservationFreshness {
    fn from(freshness: &SourceFreshness) -> Self {
        match freshness {
            SourceFreshness::Pending => Self::Pending,
            SourceFreshness::Current => Self::Current,
            SourceFreshness::Stale { diagnostic } => Self::Stale {
                diagnostic: diagnostic.clone(),
            },
            SourceFreshness::Unavailable { diagnostic } => Self::Unavailable {
                diagnostic: diagnostic.clone(),
            },
        }
    }
}

impl From<&RetentionBasis> for RetainedBasis {
    fn from(basis: &RetentionBasis) -> Self {
        match basis {
            RetentionBasis::ShellForeground => Self::ShellForeground,
            RetentionBasis::Unverified => Self::Unverified,
        }
    }
}

impl From<ObservationFreshness> for SourceFreshness {
    fn from(freshness: ObservationFreshness) -> Self {
        match freshness {
            ObservationFreshness::Pending => Self::Pending,
            ObservationFreshness::Current => Self::Current,
            ObservationFreshness::Stale { diagnostic } => Self::Stale { diagnostic },
            ObservationFreshness::Unavailable { diagnostic } => Self::Unavailable { diagnostic },
        }
    }
}

impl From<RetainedObservation> for RetainedAgent {
    fn from(retained: RetainedObservation) -> Self {
        Self {
            observation: retained.observation,
            basis: match retained.basis {
                RetainedBasis::ShellForeground => RetentionBasis::ShellForeground,
                RetainedBasis::Unverified => RetentionBasis::Unverified,
            },
        }
    }
}

impl From<ObservationSource> for InternalSource {
    fn from(source: ObservationSource) -> Self {
        Self {
            name: source.name,
            state: source.state.into(),
            last_success_at: source.last_success_at,
        }
    }
}

impl From<ObservationProjection> for ObservationState {
    fn from(projection: ObservationProjection) -> Self {
        Self::from_public(projection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn response_shapes_keep_the_existing_observation_wire_envelopes() {
        let response = ObservationGetResponse::NotModified(ObservationNotModifiedResult {
            not_modified: ObservationRevision {
                epoch: "epoch-1".into(),
                revision: 12,
            },
        });
        assert_eq!(
            serde_json::to_value(response).expect("serialize response"),
            json!({"not_modified": {"epoch": "epoch-1", "revision": 12}})
        );
    }

    #[test]
    fn public_snapshot_conversion_preserves_the_internal_json_shape_and_round_trips() {
        let mut observation = ObservationState::new();
        observation.apply_success(FleetObservation::default());
        let internal = InternalSnapshot {
            epoch: "epoch-1".into(),
            revision: 12,
            collected_at: Some("2026-10-20T00:00:00.000Z".into()),
            sources: vec![InternalSource {
                name: "inventory".into(),
                state: SourceFreshness::Stale {
                    diagnostic: "read failed".into(),
                },
                last_success_at: Some("2026-10-19T23:59:59.000Z".into()),
            }],
            tasks: Some(json!({"revision": 3, "sessions": []})),
            observation,
        };
        let internal_json = serde_json::to_value(&internal).expect("serialize internal snapshot");
        let public = ObservationSnapshot::from(&internal);
        let public_json = serde_json::to_value(&public).expect("serialize public snapshot");
        assert_eq!(public_json, internal_json);
        let round_trip: InternalSnapshot = public.into();
        assert_eq!(
            serde_json::to_value(round_trip).expect("serialize round trip"),
            internal_json
        );
    }
}
