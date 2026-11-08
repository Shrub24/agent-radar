//! Containment: whether a location is known to be outside Pi Herdsman's ownership.
//!
//! This is the policy the daemon and the views share, and it is only the policy:
//! closing is an operation over a place, and the daemon owns it. Everything
//! ambiguous refuses — an unverified Pi pane, an agent kind Radar cannot place, a
//! location an owner manages, or a tab holding any of those — and the
//! classification is derived from current evidence, never a cached confirmation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{AgentObservation, FleetObservation, SessionIdentity};
use crate::observation::RetainedAgent;
use crate::runtime::CloseTarget;

/// Whether a location is known to be outside Pi Herdsman's ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Containment {
    /// Positive evidence the location is unmanaged: no agent, or an agent kind
    /// Pi Herdsman never manages, with no retained association blocking it.
    Unmanaged,
    /// The owner's metadata says Pi Herdsman manages this location.
    Managed,
    /// Cannot be told: a Pi pane publishing no metadata, an agent kind Radar
    /// cannot place, or an agent whose kind is unknown.
    Uncertain,
}

impl Containment {
    /// Whether a direct runtime close may be sent for this location.
    pub fn is_unmanaged(self) -> bool {
        matches!(self, Self::Unmanaged)
    }
}

/// The pane's containment from current evidence.
pub fn pane(
    inventory: &FleetObservation,
    retained: &BTreeMap<String, RetainedAgent>,
    pane_id: &str,
) -> Containment {
    // A retained association is local continuity state: its pane reports no
    // agent now, but a managed one must still forbid a direct close. A non-Pi
    // association blocks nothing, so the live evidence below decides.
    if let Some(entry) = retained.get(pane_id) {
        let containment = agent_containment(&entry.observation);
        if !containment.is_unmanaged() {
            return containment;
        }
    }
    // Every record on the pane counts, presented or metadata, and the strictest
    // answer wins. No record at all is positive unmanaged evidence.
    inventory
        .agents
        .iter()
        .filter(|agent| agent.location.pane_id == pane_id)
        .map(agent_containment)
        .fold(Containment::Unmanaged, stricter)
}

/// The tab's containment: managed or uncertain if any member pane is, so a
/// mixed tab is refused whole rather than closed around its managed pane.
pub fn tab(
    inventory: &FleetObservation,
    retained: &BTreeMap<String, RetainedAgent>,
    tab_id: &str,
) -> Containment {
    inventory
        .panes
        .iter()
        .filter(|member| member.location.tab_id == tab_id)
        .map(|member| pane(inventory, retained, &member.location.pane_id))
        .fold(Containment::Unmanaged, stricter)
}

/// The workspace's containment: managed or uncertain if any pane in it is, so a
/// workspace holding one managed pane is refused whole.
pub fn workspace(
    inventory: &FleetObservation,
    retained: &BTreeMap<String, RetainedAgent>,
    workspace_id: &str,
) -> Containment {
    inventory
        .panes
        .iter()
        .filter(|member| member.location.workspace_id == workspace_id)
        .map(|member| pane(inventory, retained, &member.location.pane_id))
        .fold(Containment::Unmanaged, stricter)
}

/// The containment of a close target read from one inventory alone, as the
/// close worker has it: local retained associations are not visible off-thread,
/// and the confirmation already checked them against the full state.
pub fn target(inventory: &FleetObservation, target: &CloseTarget) -> Containment {
    let none = BTreeMap::new();
    match target {
        CloseTarget::Pane(pane_id) => pane(inventory, &none, pane_id),
        CloseTarget::Tab(tab_id) => tab(inventory, &none, tab_id),
        CloseTarget::Workspace(workspace_id) => workspace(inventory, &none, workspace_id),
    }
}

/// The containment of the registration on a pane: a registration protects the
/// place it occupies even when it has not joined a pane, so a pane naming one is
/// never an unmanaged close target.
fn agent_containment(_agent: &AgentObservation) -> Containment {
    Containment::Managed
}

/// The stricter of two answers: managed beats uncertain beats unmanaged.
fn stricter(a: Containment, b: Containment) -> Containment {
    match (a, b) {
        (Containment::Managed, _) | (_, Containment::Managed) => Containment::Managed,
        (Containment::Uncertain, _) | (_, Containment::Uncertain) => Containment::Uncertain,
        _ => Containment::Unmanaged,
    }
}

/// A confirmed direct close, frozen at confirmation time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CloseRequest {
    pub target: CloseTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<crate::control_plane::command::Subject>,
    /// The observed identity frozen when the operator confirmed. The worker
    /// refuses when its own fresh inventory no longer matches it, so a target
    /// replaced between confirmation and the close is never acted on.
    pub identity: TargetIdentity,
}

/// What was observed on a pane, normalized for identity comparison.
///
/// Only identity-bearing fields: the registration and its owner-published run.
/// Display titles are deliberately absent, so a cosmetic retitle is not a new
/// occupant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub registration_id: String,
    pub session: Option<SessionIdentity>,
    pub run: Option<String>,
}

/// One pane and what occupied it when the target was frozen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneIdentity {
    pub pane_id: String,
    pub occupant: Option<AgentIdentity>,
}

/// The observed identity of a close target, frozen at confirmation.
///
/// A pane target carries that pane's occupant; a tab target carries the whole
/// observed member set, so a member added, removed or replaced is drift even
/// when every member is individually unmanaged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetIdentity {
    Pane(PaneIdentity),
    Tab {
        tab_id: String,
        members: Vec<PaneIdentity>,
    },
    Workspace {
        workspace_id: String,
        members: Vec<PaneIdentity>,
    },
}

impl TargetIdentity {
    /// The target this identity describes.
    pub fn target(&self) -> CloseTarget {
        match self {
            Self::Pane(pane) => CloseTarget::Pane(pane.pane_id.clone()),
            Self::Tab { tab_id, .. } => CloseTarget::Tab(tab_id.clone()),
            Self::Workspace { workspace_id, .. } => CloseTarget::Workspace(workspace_id.clone()),
        }
    }
}

/// The observed identity of `target` in this inventory.
pub fn identity(inventory: &FleetObservation, target: &CloseTarget) -> TargetIdentity {
    match target {
        CloseTarget::Pane(pane_id) => TargetIdentity::Pane(pane_identity(inventory, pane_id)),
        CloseTarget::Tab(tab_id) => {
            let mut members: Vec<PaneIdentity> = inventory
                .panes
                .iter()
                .filter(|pane| pane.location.tab_id == *tab_id)
                .map(|pane| pane_identity(inventory, &pane.location.pane_id))
                .collect();
            // Membership is a set: order in the source is not identity.
            members.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
            TargetIdentity::Tab {
                tab_id: tab_id.clone(),
                members,
            }
        }
        CloseTarget::Workspace(workspace_id) => {
            let mut members: Vec<PaneIdentity> = inventory
                .panes
                .iter()
                .filter(|pane| pane.location.workspace_id == *workspace_id)
                .map(|pane| pane_identity(inventory, &pane.location.pane_id))
                .collect();
            // Membership is a set: order in the source is not identity.
            members.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
            TargetIdentity::Workspace {
                workspace_id: workspace_id.clone(),
                members,
            }
        }
    }
}

/// Whether a fresh inventory still holds the frozen identity unchanged.
pub fn matches(inventory: &FleetObservation, expected: &TargetIdentity) -> bool {
    identity(inventory, &expected.target()) == *expected
}

fn pane_identity(inventory: &FleetObservation, pane_id: &str) -> PaneIdentity {
    PaneIdentity {
        pane_id: pane_id.to_string(),
        occupant: inventory
            .agents
            .iter()
            .find(|agent| agent.location.pane_id == pane_id)
            .map(registration_identity),
    }
}

fn registration_identity(agent: &AgentObservation) -> AgentIdentity {
    AgentIdentity {
        registration_id: agent
            .registry
            .agent
            .registration
            .registration_id
            .to_string(),
        session: agent.session.clone(),
        run: agent.facts.run.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentObservation, HerdsmanFacts, Location, Pane};
    use crate::observation::RetentionBasis;
    use serde_json::json;

    fn location(pane_id: &str) -> Location {
        Location {
            workspace_id: "wA".into(),
            tab_id: "wA:t1".into(),
            pane_id: pane_id.into(),
        }
    }

    fn pane_row(pane_id: &str) -> Pane {
        Pane {
            location: location(pane_id),
            label: None,
            title: None,
            current_path: None,
        }
    }

    fn agent(pane_id: &str) -> AgentObservation {
        AgentObservation {
            location: location(pane_id),
            status: None,
            session: None,
            facts: HerdsmanFacts::default(),
            registry: crate::registry_observation::RegistryView::test_joined(
                &crate::test_support::registration_id(&format!("test-{pane_id}")),
                pane_id,
            ),
        }
    }

    fn inventory(panes: &[&str], agents: Vec<AgentObservation>) -> FleetObservation {
        FleetObservation {
            focus: Default::default(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
            workspaces: Vec::new(),
            tabs: vec![crate::model::Tab {
                tab_id: "wA:t1".into(),
                workspace_id: "wA".into(),
                label: None,
                number: None,
            }],
            panes: panes.iter().map(|id| pane_row(id)).collect(),
            agents,
        }
    }

    fn retained(agent: AgentObservation) -> BTreeMap<String, RetainedAgent> {
        let mut map = BTreeMap::new();
        map.insert(
            agent.location.pane_id.clone(),
            RetainedAgent {
                observation: agent,
                basis: RetentionBasis::Unverified,
            },
        );
        map
    }

    fn none() -> BTreeMap<String, RetainedAgent> {
        BTreeMap::new()
    }

    #[test]
    fn a_pane_with_no_agent_is_unmanaged() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Unmanaged);
    }

    #[test]
    fn a_registered_agent_on_a_pane_is_managed() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1")]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Managed);
    }

    #[test]
    fn a_retained_managed_association_forbids_a_direct_close() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        let retained = retained(agent("wA:p1"));
        assert_eq!(pane(&inventory, &retained, "wA:p1"), Containment::Managed);
    }

    #[test]
    fn a_mixed_tab_is_refused_whole() {
        let inventory = inventory(&["wA:p1", "wA:p2"], vec![agent("wA:p1")]);
        assert_eq!(tab(&inventory, &none(), "wA:t1"), Containment::Managed);
    }

    #[test]
    fn an_all_unmanaged_tab_is_unmanaged() {
        let inventory = inventory(&["wA:p1", "wA:p2"], Vec::new());
        assert_eq!(tab(&inventory, &none(), "wA:t1"), Containment::Unmanaged);
    }

    #[test]
    fn target_reads_pane_and_tab_from_one_inventory() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        assert!(target(&inventory, &CloseTarget::Pane("wA:p1".into())).is_unmanaged());
        assert!(target(&inventory, &CloseTarget::Tab("wA:t1".into())).is_unmanaged());
    }

    /// A pane whose only evidence is a registration that did not join: no
    /// backend agent record and nothing on the pane itself.
    fn registered_only_inventory() -> FleetObservation {
        let mut observation = inventory(&["wA:p1"], Vec::new());
        let agent: crate::registry_observation::RegistryAgent = serde_json::from_value(json!({
            "registration": {
                "version": 1,
                "registration_id": "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
                "registered_at": "2026-01-01T00:00:00.000Z",
                "source": "herdsman",
                "incarnation": "11111111-1111-4111-8111-111111111111",
                "location": {"backend": "tmux", "pane": "wA:p1"},
                "process_claimed": true,
                "launch": {"available": false}
            },
            "process": {"claimed": true, "verification": "unavailable"},
            "execution": null,
            "assignment": null,
            "facts": null
        }))
        .expect("registration decodes");
        observation.agents = crate::registry_observation::project(
            &observation,
            &crate::runtime::BackendIdentity::named("tmux"),
            crate::registry_observation::RegistryRead {
                agents: vec![agent],
                ..Default::default()
            },
            &[],
        );
        observation
    }

    #[test]
    fn a_registered_pane_is_protected_even_when_its_evidence_is_missing() {
        let observation = registered_only_inventory();
        assert_eq!(
            pane(&observation, &none(), "wA:p1"),
            Containment::Managed,
            "an unjoined registration is not positive unmanaged evidence"
        );
        assert_eq!(
            target(&observation, &CloseTarget::Tab("wA:t1".into())),
            Containment::Managed,
            "a tab holding that pane is refused whole"
        );
    }
}
