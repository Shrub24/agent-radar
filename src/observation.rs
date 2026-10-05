//! Pure, in-memory continuity state for the fleet overview.
//!
//! [`ObservationState`] is the single mutable surface the collector drives:
//! successful inventories replace old ones, failures keep the last-good
//! inventory visibly stale, and an agent that stops being reported behind a
//! live pane is retained until positive evidence supersedes it. Nothing is
//! persisted — a fresh state (a Radar restart) starts empty.
//!
//! All transitions are pure: no I/O, no clock, no Herdr knowledge. The
//! collector supplies decoded [`ForegroundEvidence`] for the panes returned by
//! [`ObservationState::continuity_candidates`].

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{AgentObservation, FleetObservation, ForegroundEvidence};

/// Source-wide freshness of the latest collection outcome.
///
/// Presentation distinguishes all four states: current observations, a stale
/// last-good inventory, a source that never produced an inventory, and a
/// source whose first outcome is still pending.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum SourceFreshness {
    /// Started; no collection outcome has arrived yet.
    #[default]
    Pending,
    /// The latest collection succeeded.
    Current,
    /// The latest collection failed; the last successful inventory stands as
    /// stale and its retained associations are not treated as disappeared.
    Stale { diagnostic: String },
    /// Collection failed before any successful inventory, so there is no
    /// fleet to show — this is not a successful empty fleet.
    Unavailable { diagnostic: String },
}

/// Why a retained association is currently believed to be at rest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionBasis {
    /// process-info showed the pane shell owning the foreground.
    ShellForeground,
    /// Foreground evidence is pending or was inconclusive; the row is
    /// explicitly unverified rather than falsely current or superseded.
    Unverified,
}

/// A last-observed agent association kept alive through an idle-shell gap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedAgent {
    /// The last observed facts for this pane. Never merged into newer
    /// observations: a returning agent replaces these wholesale.
    pub observation: AgentObservation,
    pub basis: RetentionBasis,
}

/// The normalized observation surface: last-good inventory, source freshness
/// and pane-backed continuity, reconciled by pure transitions.
#[derive(Debug, Default)]
pub struct ObservationState {
    inventory: Option<FleetObservation>,
    source: SourceFreshness,
    /// Retained associations keyed by connector-scoped pane id. Invariant
    /// maintained by the transitions: every retained pane exists in the
    /// inventory and has no agent reported on it.
    retained: BTreeMap<String, RetainedAgent>,
    /// Retained panes that have not yet been evidence-queried this cycle;
    /// rebuilt on every successful inventory, drained by [`Self::apply_evidence`].
    awaiting_evidence: BTreeSet<String>,
    /// Latest foreground evidence per pane, for the process view. Rebuilt from
    /// the panes an inventory reports, so a vanished pane never keeps a process
    /// on screen.
    foreground: BTreeMap<String, ForegroundEvidence>,
}

impl ObservationState {
    /// A fresh state: no inventory, pending source, no retained associations.
    ///
    /// Continuity is memory-only by design — this is all a restart gets.
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one successfully decoded inventory.
    ///
    /// This replaces the previous inventory wholesale and reconciles
    /// continuity:
    /// - a previously current agent that vanishes while its pane stays is
    ///   retained (unverified, awaiting evidence);
    /// - a retained agent whose pane disappears, whose agent is reported
    ///   again, or whose session is reported on any current pane is cleared;
    /// - source freshness becomes [`SourceFreshness::Current`].
    pub fn apply_success(&mut self, observation: FleetObservation) {
        let previous = self.inventory.take();

        let current_sessions: Vec<_> = observation
            .agents
            .iter()
            .filter_map(|agent| agent.session.clone())
            .collect();
        let session_is_current = |session: &Option<crate::model::SessionIdentity>| {
            session
                .as_ref()
                .is_some_and(|session| current_sessions.contains(session))
        };

        let mut retained = BTreeMap::new();
        // Surviving retentions: still no reported agent, pane still present,
        // and the session not reported anywhere current (a moved session is
        // current elsewhere, never retained as "not observed").
        for (pane_id, entry) in std::mem::take(&mut self.retained) {
            let agent_reported = observation.agent_on_pane(&pane_id).is_some();
            let pane_present = observation.pane(&pane_id).is_some();
            if !agent_reported && pane_present && !session_is_current(&entry.observation.session) {
                retained.insert(pane_id, entry);
            }
        }
        // New retentions: agents that were current in the previous inventory,
        // are gone from this one, and whose pane lives on.
        if let Some(previous) = previous {
            for agent in &previous.agents {
                let pane_id = &agent.location.pane_id;
                if observation.agent_on_pane(pane_id).is_some()
                    || observation.pane(pane_id).is_none()
                    || session_is_current(&agent.session)
                {
                    continue;
                }
                retained
                    .entry(pane_id.clone())
                    .or_insert_with(|| RetainedAgent {
                        observation: agent.clone(),
                        basis: RetentionBasis::Unverified,
                    });
            }
        }

        self.awaiting_evidence = retained.keys().cloned().collect();
        self.retained = retained;
        // Evidence belongs to the inventory that was just replaced: a pane that
        // is gone takes its process with it, and the refresh re-queries what is
        // left.
        let panes: BTreeSet<&str> = observation
            .panes
            .iter()
            .map(|pane| pane.location.pane_id.as_str())
            .collect();
        self.foreground
            .retain(|pane_id, _| panes.contains(pane_id.as_str()));
        self.inventory = Some(observation);
        self.source = SourceFreshness::Current;
    }

    /// Applies one failed collection.
    ///
    /// Failure never proves disappearance: the last-good inventory and all
    /// retained associations stand untouched, and freshness records the
    /// diagnostic. Before any success the source is
    /// [`SourceFreshness::Unavailable`], not an empty fleet.
    pub fn apply_failure(&mut self, diagnostic: impl Into<String>) {
        let diagnostic = diagnostic.into();
        self.source = if self.inventory.is_some() {
            SourceFreshness::Stale { diagnostic }
        } else {
            SourceFreshness::Unavailable { diagnostic }
        };
    }

    /// Applies decoded foreground evidence for one pane.
    ///
    /// The evidence is recorded for every pane it is asked about, which is what
    /// the process view reads. Only a retained pane can consume it as
    /// continuity evidence; anything else (including late evidence for an agent
    /// that has since returned) is recorded and otherwise ignored. A positive
    /// non-shell replacement clears the association; shell or inconclusive
    /// evidence keeps it, updating its basis.
    pub fn apply_evidence(&mut self, pane_id: &str, evidence: ForegroundEvidence) {
        let pane_is_reported = self
            .inventory
            .as_ref()
            .is_some_and(|inventory| inventory.pane(pane_id).is_some());
        if pane_is_reported {
            self.foreground
                .insert(pane_id.to_string(), evidence.clone());
        }
        self.awaiting_evidence.remove(pane_id);
        match evidence {
            ForegroundEvidence::NonShell { .. } => {
                self.retained.remove(pane_id);
            }
            ForegroundEvidence::Shell => {
                if let Some(entry) = self.retained.get_mut(pane_id) {
                    entry.basis = RetentionBasis::ShellForeground;
                }
            }
            ForegroundEvidence::Inconclusive => {
                if let Some(entry) = self.retained.get_mut(pane_id) {
                    entry.basis = RetentionBasis::Unverified;
                }
            }
        }
    }

    /// The last successful inventory, if any has ever been applied.
    pub fn inventory(&self) -> Option<&FleetObservation> {
        self.inventory.as_ref()
    }

    /// Source-wide freshness, including the latest collection diagnostic.
    pub fn source_freshness(&self) -> &SourceFreshness {
        &self.source
    }

    /// Retained associations keyed by pane id (empty when nothing is retained).
    pub fn retained(&self) -> &BTreeMap<String, RetainedAgent> {
        &self.retained
    }

    /// The pane ids the current inventory reports, which is what a whole-fleet
    /// process sweep has to cover.
    pub fn pane_ids(&self) -> Vec<String> {
        self.inventory
            .as_ref()
            .map(|inventory| {
                inventory
                    .panes
                    .iter()
                    .map(|pane| pane.location.pane_id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The last decoded foreground evidence for a pane, if this refresh asked
    /// about it.
    pub fn foreground(&self, pane_id: &str) -> Option<&ForegroundEvidence> {
        self.foreground.get(pane_id)
    }

    /// Panes of retained associations that still need a process-info query in
    /// this cycle.
    ///
    /// The collector should query `herdr pane process-info` only for these and
    /// feed each result back through [`Self::apply_evidence`]. Owned, so the
    /// caller can mutate the state while iterating.
    pub fn continuity_candidates(&self) -> Vec<String> {
        self.awaiting_evidence.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Lineage, Location, RuntimeStatus, SessionIdentity, SessionUuid};

    fn uuid(value: &str) -> SessionUuid {
        SessionUuid::parse(value).expect("test UUID")
    }

    fn reported(value: &str) -> SessionIdentity {
        SessionIdentity::Reported {
            source: Some("herdr:pi".into()),
            value: value.into(),
        }
    }

    fn agent(
        pane_id: &str,
        session: Option<SessionIdentity>,
        status: RuntimeStatus,
    ) -> AgentObservation {
        AgentObservation {
            location: Location {
                workspace_id: "wA".into(),
                tab_id: "wA:t1".into(),
                pane_id: pane_id.into(),
            },
            name: Some("pi".into()),
            label: Some("fleet owner task".into()),
            status: Some(status),
            session,
            lineage: Some(Lineage {
                session: uuid("11111111-1111-4111-8111-111111111111"),
                parent: None,
            }),
            facts: crate::model::HerdsmanFacts::default(),
        }
    }

    fn observation(pane_ids: &[&str], agents: Vec<AgentObservation>) -> FleetObservation {
        FleetObservation {
            workspaces: vec![],
            tabs: vec![],
            panes: pane_ids
                .iter()
                .map(|pane_id| crate::model::Pane {
                    location: Location {
                        workspace_id: "wA".into(),
                        tab_id: "wA:t1".into(),
                        pane_id: (*pane_id).into(),
                    },
                    label: None,
                    title: None,
                })
                .collect(),
            agents,
        }
    }

    const SESSION: &str = "/home/dev/.pi/agent/sessions/--home-dev-projects-alpha--/2026-09-01T10-00-00-000Z_11111111-1111-4111-8111-111111111111.jsonl";

    fn state_with_agent() -> ObservationState {
        let mut state = ObservationState::new();
        state.apply_success(observation(
            &["wA:p1"],
            vec![agent("wA:p1", Some(reported(SESSION)), RuntimeStatus::Idle)],
        ));
        state
    }

    #[test]
    fn fresh_state_starts_empty() {
        let state = ObservationState::new();
        assert_eq!(state.source_freshness(), &SourceFreshness::Pending);
        assert!(state.inventory().is_none());
        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
    }

    #[test]
    fn missing_agent_behind_live_pane_is_retained_as_candidate() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));

        let retained = state.retained();
        assert_eq!(retained.len(), 1);
        let entry = retained.get("wA:p1").expect("retained entry");
        assert_eq!(entry.basis, RetentionBasis::Unverified);
        // Last-observed facts are kept verbatim for the retained row.
        assert_eq!(entry.observation.session, Some(reported(SESSION)));
        assert_eq!(entry.observation.status, Some(RuntimeStatus::Idle));
        // The current inventory genuinely has no agent on that pane.
        assert!(
            state
                .inventory()
                .expect("inventory")
                .agent_on_pane("wA:p1")
                .is_none()
        );
        assert_eq!(state.continuity_candidates(), vec!["wA:p1".to_string()]);
        assert_eq!(state.source_freshness(), &SourceFreshness::Current);
    }

    #[test]
    fn shell_evidence_keeps_retention_and_drains_the_candidate() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        state.apply_evidence("wA:p1", ForegroundEvidence::Shell);

        let entry = state.retained().get("wA:p1").expect("retained entry");
        assert_eq!(entry.basis, RetentionBasis::ShellForeground);
        assert!(state.continuity_candidates().is_empty());
    }

    #[test]
    fn inconclusive_evidence_keeps_retention_unverified() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        state.apply_evidence("wA:p1", ForegroundEvidence::Shell);
        state.apply_evidence("wA:p1", ForegroundEvidence::Inconclusive);

        let entry = state.retained().get("wA:p1").expect("retained entry");
        assert_eq!(entry.basis, RetentionBasis::Unverified);
        assert!(state.continuity_candidates().is_empty());
    }

    #[test]
    fn non_shell_evidence_clears_retention_and_leaves_an_ordinary_pane() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        state.apply_evidence(
            "wA:p1",
            ForegroundEvidence::command(4242, Some("nvim".into()), Some("nvim".into())),
        );

        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
        // The pane itself remains in the inventory as an ordinary runtime pane.
        assert!(
            state
                .inventory()
                .expect("inventory")
                .pane("wA:p1")
                .is_some()
        );
        assert!(
            state
                .inventory()
                .expect("inventory")
                .agent_on_pane("wA:p1")
                .is_none()
        );
    }

    #[test]
    fn pane_disappearance_clears_retention() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        state.apply_success(observation(&[], vec![]));

        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
    }

    #[test]
    fn returning_agent_replaces_retention_with_current_facts() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        state.apply_success(observation(
            &["wA:p1"],
            vec![agent(
                "wA:p1",
                Some(reported(SESSION)),
                RuntimeStatus::Working,
            )],
        ));

        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
        let current = state
            .inventory()
            .expect("inventory")
            .agent_on_pane("wA:p1")
            .expect("current agent");
        // The fresh observation is authoritative: new status, no merging.
        assert_eq!(current.status, Some(RuntimeStatus::Working));
    }

    #[test]
    fn new_session_on_the_same_pane_supersedes_without_retaining() {
        let mut state = state_with_agent();
        let other = "/home/dev/.pi/agent/sessions/--home-dev-projects-beta--/2026-09-02T11-00-00-000Z_22222222-2222-4222-8222-222222222222.jsonl";
        state.apply_success(observation(
            &["wA:p1"],
            vec![agent("wA:p1", Some(reported(other)), RuntimeStatus::Idle)],
        ));

        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
        let current = state
            .inventory()
            .expect("inventory")
            .agent_on_pane("wA:p1")
            .expect("current agent");
        assert_eq!(current.session, Some(reported(other)));
    }

    #[test]
    fn session_reported_on_another_pane_is_not_retained_on_the_old_one() {
        let mut state = state_with_agent();
        state.apply_success(observation(
            &["wA:p1", "wA:p2"],
            vec![agent("wA:p2", Some(reported(SESSION)), RuntimeStatus::Idle)],
        ));

        // The session is current at its new pane; the old pane must not also
        // claim it is "not currently observed".
        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
        assert!(
            state
                .inventory()
                .expect("inventory")
                .agent_on_pane("wA:p2")
                .is_some()
        );
    }

    #[test]
    fn source_failure_preserves_last_good_inventory_and_retention_as_stale() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        state.apply_evidence("wA:p1", ForegroundEvidence::Shell);
        state.apply_failure("herdr exited with status 1".to_string());

        // The inventory stands as last-good, retention survives, and the
        // diagnostic is recorded — failure is not disappearance.
        let inventory = state.inventory().expect("last-good inventory");
        assert_eq!(inventory.panes.len(), 1);
        assert_eq!(state.retained().len(), 1);
        assert_eq!(
            state.retained().get("wA:p1").expect("retained").basis,
            RetentionBasis::ShellForeground
        );
        assert_eq!(
            state.source_freshness(),
            &SourceFreshness::Stale {
                diagnostic: "herdr exited with status 1".to_string()
            }
        );
    }

    #[test]
    fn failure_before_first_inventory_is_unavailable_not_an_empty_fleet() {
        let mut state = ObservationState::new();
        state.apply_failure("initial collection timed out");

        assert!(state.inventory().is_none());
        assert_eq!(
            state.source_freshness(),
            &SourceFreshness::Unavailable {
                diagnostic: "initial collection timed out".to_string()
            }
        );
    }

    #[test]
    fn successful_empty_inventory_is_distinct_from_failure() {
        let mut state = ObservationState::new();
        state.apply_success(observation(&[], vec![]));

        assert_eq!(state.source_freshness(), &SourceFreshness::Current);
        let inventory = state.inventory().expect("successful inventory");
        assert!(inventory.workspaces.is_empty());
        assert!(inventory.panes.is_empty());
        assert!(inventory.agents.is_empty());
    }

    #[test]
    fn recovery_replaces_stale_inventory_and_clears_the_diagnostic() {
        let mut state = state_with_agent();
        state.apply_failure("connection refused".to_string());
        state.apply_success(observation(&["wB:p9"], vec![]));

        assert_eq!(state.source_freshness(), &SourceFreshness::Current);
        let inventory = state.inventory().expect("recovered inventory");
        assert!(inventory.pane("wA:p1").is_none());
        assert!(inventory.pane("wB:p9").is_some());
        // The pre-failure agent is gone with its pane: nothing retained.
        assert!(state.retained().is_empty());
    }

    #[test]
    fn restart_starts_with_no_retained_state() {
        let mut state = state_with_agent();
        state.apply_success(observation(&["wA:p1"], vec![]));
        assert_eq!(state.retained().len(), 1);

        let restarted = ObservationState::new();
        assert!(restarted.retained().is_empty());
        assert!(restarted.inventory().is_none());
        assert_eq!(restarted.source_freshness(), &SourceFreshness::Pending);
    }

    #[test]
    fn evidence_for_a_non_candidate_is_ignored() {
        let mut state = state_with_agent();
        state.apply_evidence("wA:p99", ForegroundEvidence::command(1, None, None));
        // Nothing was retained, so nothing can be cleared or corrupted.
        assert!(state.retained().is_empty());
        assert!(state.continuity_candidates().is_empty());
    }
}
