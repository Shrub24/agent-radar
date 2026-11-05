//! Lifecycle actions: closing a location Radar has positive evidence is
//! unmanaged.
//!
//! Containment is the whole policy. Pi Herdsman owns the panes it manages, so a
//! direct runtime close is only ever sent for a location with positive evidence
//! that the owner does not have it. Everything ambiguous refuses: an
//! unverified Pi pane, an agent kind Radar cannot place, a location an owner
//! manages, or a tab holding any of those. The classification is derived from
//! current evidence, never from a cached confirmation.
//!
//! [`Closer`] runs a confirmed close off the calling thread and takes its own
//! inventory immediately before acting. A direct mux close has no conditional
//! form, so a fresh containment check as close as the runtime allows is the
//! best guard against closing a location that became managed in the meantime;
//! nothing here promises atomicity across the inventory read and the close.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};

use serde::{Deserialize, Serialize};

use crate::control_plane::registry::{PublicRegistration, RegistryLocation};
use crate::model::{AgentObservation, FleetObservation, Lineage, SessionIdentity};
use crate::observation::RetainedAgent;
use crate::runtime::{CloseOutcome, CloseTarget, RuntimeProvider};
use crate::theme;

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
        .all_agents()
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

/// The containment of the agent recorded on a pane.
fn agent_containment(agent: &AgentObservation) -> Containment {
    // Any owner metadata makes the pane managed, whether or not this build can
    // read the key: presence is the signal, absence is unknown.
    if is_protected(agent) {
        return Containment::Managed;
    }
    match agent.name.as_deref() {
        // Pi without owner metadata is unverified, never unmanaged.
        Some(name) if name.eq_ignore_ascii_case("pi") => Containment::Uncertain,
        // A vendor Radar ships a mark for is a kind Pi Herdsman never manages.
        Some(name) if theme::shipped_agent(name) => Containment::Unmanaged,
        // A kind Radar cannot place is never guessed unmanaged.
        _ => Containment::Uncertain,
    }
}

/// Whether the pane's agent is protected from unmanaged control: owner metadata
/// was published, or the daemon's registry knows an agent on this location.
///
/// A registration counts even unjoined, so a pane never becomes an unmanaged
/// close target because its evidence is missing or ambiguous.
fn is_protected(agent: &AgentObservation) -> bool {
    agent.facts.managed_metadata || agent.registry.is_some()
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
/// Only identity-bearing fields: the agent kind, its session, its ownership
/// lineage and its owner-published label and run. Display titles are
/// deliberately absent, so a cosmetic retitle is not a new occupant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub name: Option<String>,
    pub session: Option<SessionIdentity>,
    pub lineage: Option<Lineage>,
    pub managed: bool,
    pub label: Option<String>,
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
            .all_agents()
            .find(|agent| agent.location.pane_id == pane_id)
            .map(registration_identity),
    }
}

fn registration_identity(agent: &AgentObservation) -> AgentIdentity {
    AgentIdentity {
        name: agent.name.clone(),
        session: agent.session.clone(),
        lineage: agent.lineage.clone(),
        managed: is_protected(agent),
        label: agent.facts.label.clone(),
        run: agent.facts.run.clone(),
    }
}

/// A close running on its own thread.
struct Request {
    cancel: Arc<AtomicBool>,
    outcome: Receiver<Result<String, String>>,
    worker: JoinHandle<()>,
}

/// Runs direct closes off the calling thread, one at a time.
pub struct Closer {
    provider: Arc<dyn RuntimeProvider>,
    in_flight: Option<Request>,
}

impl Closer {
    /// A closer that runs nothing until [`Self::start`].
    pub fn new(provider: impl RuntimeProvider + 'static) -> Self {
        Self {
            provider: Arc::new(provider),
            in_flight: None,
        }
    }

    /// Whether a confirmed close is still running.
    pub fn is_busy(&self) -> bool {
        self.in_flight.is_some()
    }

    /// Starts a confirmed close off the calling thread, replacing a request
    /// already in flight. Replacing waits only for that request's worker to
    /// notice the cancellation, never for its timeout.
    pub fn start(&mut self, request: CloseRequest) {
        self.cancel_in_flight();
        let cancel = Arc::new(AtomicBool::new(false));
        let (sender, outcome) = mpsc::channel();
        let worker = {
            let provider = Arc::clone(&self.provider);
            let cancel = Arc::clone(&cancel);
            thread::Builder::new()
                .name("radar-close".to_string())
                .spawn(move || {
                    let _ = sender.send(run(provider.as_ref(), &request, &cancel));
                })
                .expect("close thread")
        };
        self.in_flight = Some(Request {
            cancel,
            outcome,
            worker,
        });
    }

    /// The finished close's outcome, if one is waiting. Never blocks.
    pub fn poll(&mut self) -> Option<Result<String, String>> {
        let request = self.in_flight.as_mut()?;
        let outcome = match request.outcome.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            // The worker always hands over exactly one outcome before
            // returning, so this is only reachable through a panic in it.
            Err(TryRecvError::Disconnected) => {
                Err("the close request stopped unexpectedly".to_string())
            }
        };
        let request = self.in_flight.take().expect("checked above");
        let _ = request.worker.join();
        Some(outcome)
    }

    /// Cancels an outstanding request and waits for its worker. Idempotent.
    ///
    /// Cancellation abandons a close still waiting on the runtime; a close
    /// already handed over is not retracted, and nothing here claims it was.
    pub fn shutdown(&mut self) {
        self.cancel_in_flight();
    }

    fn cancel_in_flight(&mut self) {
        if let Some(request) = self.in_flight.take() {
            request.cancel.store(true, Ordering::SeqCst);
            let _ = request.worker.join();
        }
    }
}

impl Drop for Closer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// What one guarded direct close established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectClose {
    /// The runtime reported the close performed.
    Completed,
    /// Refused before dispatch, or positively rejected by the runtime.
    Refused(String),
    /// Dispatch or completion may have happened without a trustworthy answer.
    Unknown(String),
}

/// The policy a guarded direct close applies to one fresh inventory: the target
/// still exists, still holds the frozen identity, and is still positively
/// unmanaged. `Err` is the message a user reads; nothing here dispatches.
///
/// The check and the close are not atomic: a location can become managed
/// between this verdict and the runtime acting on it.
pub fn guard(inventory: &FleetObservation, request: &CloseRequest) -> Result<(), String> {
    let described = request.target.description();
    // A wire caller can name a target and an identity that disagree; every
    // later check reads the identity, so they must describe one location.
    if request.identity.target() != request.target {
        return Err(format!(
            "{described} does not match the frozen identity's target: not closing"
        ));
    }
    match &request.target {
        CloseTarget::Pane(pane_id) if inventory.pane(pane_id).is_none() => {
            return Err(format!("pane {pane_id} is gone: nothing to close"));
        }
        CloseTarget::Tab(tab_id) if !inventory.tabs.iter().any(|tab| tab.tab_id == *tab_id) => {
            return Err(format!("tab {tab_id} is gone: nothing to close"));
        }
        CloseTarget::Workspace(workspace_id)
            if !inventory
                .workspaces
                .iter()
                .any(|workspace| workspace.workspace_id == *workspace_id) =>
        {
            return Err(format!(
                "workspace {workspace_id} is gone: nothing to close"
            ));
        }
        _ => {}
    }
    if !matches(inventory, &request.identity) {
        return Err(format!("{described} changed: nothing was closed"));
    }
    match target(inventory, &request.target) {
        Containment::Unmanaged => Ok(()),
        Containment::Managed => Err(format!(
            "{described} is managed by its owner: direct close refused"
        )),
        Containment::Uncertain => Err(format!(
            "{described} cannot be verified as unmanaged: not closing"
        )),
    }
}

/// Whether a registration names a place inside `target`.
///
/// A registration counts whether or not it joins a pane: one made after the
/// operator confirmed must still stop a generic close of its pane or tab.
fn registered_within(
    inventory: &FleetObservation,
    registrations: &[PublicRegistration],
    target: &CloseTarget,
) -> bool {
    registrations
        .iter()
        .filter_map(|registration| registration.location.as_ref())
        .any(|location| match target {
            CloseTarget::Pane(pane_id) => location.pane.as_deref() == Some(pane_id),
            CloseTarget::Tab(tab_id) => {
                location.tab.as_deref() == Some(tab_id)
                    || location.pane.as_deref().is_some_and(|pane_id| {
                        inventory
                            .pane(pane_id)
                            .is_some_and(|pane| pane.location.tab_id == *tab_id)
                    })
            }
            CloseTarget::Workspace(workspace_id) => {
                location.workspace.as_deref() == Some(workspace_id)
                    || location.pane.as_deref().is_some_and(|pane_id| {
                        inventory
                            .pane(pane_id)
                            .is_some_and(|pane| pane.location.workspace_id == *workspace_id)
                    })
                    || location.tab.as_deref().is_some_and(|tab_id| {
                        inventory
                            .tabs
                            .iter()
                            .any(|tab| tab.tab_id == *tab_id && tab.workspace_id == *workspace_id)
                    })
            }
        })
}

/// The registrations whose own published location names a place the caller is
/// about to change.
///
/// Each registration carries the location it published, so this needs no second
/// backend read, and a registration counts whether or not it joins a pane: one
/// made after the operator confirmed must still stop an effect over its place.
pub fn registrations_in(
    registrations: &[PublicRegistration],
    inside: impl Fn(&RegistryLocation) -> bool,
) -> Vec<&PublicRegistration> {
    registrations
        .iter()
        .filter(|registration| registration.location.as_ref().is_some_and(&inside))
        .collect()
}

/// One confirmed close: take a fresh inventory, prove the target is still
/// positively unmanaged and unregistered, then ask the runtime to close it.
///
/// `registrations` is read after the inventory and before dispatch. A registry
/// that cannot be read refuses the close.
pub fn close_unmanaged(
    provider: &dyn RuntimeProvider,
    request: &CloseRequest,
    cancel: &AtomicBool,
    registrations: impl FnOnce() -> Result<Vec<PublicRegistration>, String>,
) -> DirectClose {
    let inventory = match provider.inventory(cancel) {
        Ok(inventory) => inventory,
        Err(message) => return DirectClose::Refused(message),
    };
    if let Err(message) = guard(&inventory, request) {
        return DirectClose::Refused(message);
    }
    let described = request.target.description();
    let registrations = match registrations() {
        Ok(registrations) => registrations,
        Err(message) => {
            return DirectClose::Refused(format!(
                "{described} cannot be checked against the registry: not closing: {message}"
            ));
        }
    };
    if registered_within(&inventory, &registrations, &request.target) {
        return DirectClose::Refused(format!(
            "{described} is registered by its owner: direct close refused"
        ));
    }
    match provider.close_outcome(&request.target, cancel) {
        CloseOutcome::Completed => DirectClose::Completed,
        CloseOutcome::Refused(message) => DirectClose::Refused(message),
        CloseOutcome::Unknown(message) => DirectClose::Unknown(message),
    }
}

/// The operator-facing form of [`close_unmanaged`]: the same policy and the
/// same messages, collapsed to what Radar's own close worker reports.
fn run(
    provider: &dyn RuntimeProvider,
    request: &CloseRequest,
    cancel: &AtomicBool,
) -> Result<String, String> {
    // The operator path has no registry of its own: a daemon applies its registry
    // guard when it dispatches, and a direct adapter has no registrations.
    match close_unmanaged(provider, request, cancel, || Ok(Vec::new())) {
        DirectClose::Completed => Ok(format!("closed {}", request.target.description())),
        DirectClose::Refused(message) | DirectClose::Unknown(message) => Err(message),
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

    fn agent(pane_id: &str, name: Option<&str>, managed: bool) -> AgentObservation {
        AgentObservation {
            location: location(pane_id),
            name: name.map(str::to_string),
            label: None,
            status: None,
            session: None,
            lineage: None,
            facts: HerdsmanFacts {
                managed_metadata: managed,
                ..HerdsmanFacts::default()
            },
            registry: None,
        }
    }

    fn inventory(panes: &[&str], agents: Vec<AgentObservation>) -> FleetObservation {
        FleetObservation {
            focus: Default::default(),
            metadata_agents: Vec::new(),
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
    fn a_non_pi_known_agent_is_unmanaged() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("claude"), false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Unmanaged);
    }

    #[test]
    fn a_pi_pane_without_metadata_is_uncertain() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("pi"), false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn pi_metadata_marks_a_pane_managed() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("pi"), true)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Managed);
    }

    #[test]
    fn an_unrecognized_agent_kind_is_never_unmanaged() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", Some("mystery"), false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn an_agent_without_a_kind_is_uncertain() {
        let inventory = inventory(&["wA:p1"], vec![agent("wA:p1", None, false)]);
        assert_eq!(pane(&inventory, &none(), "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn a_retained_managed_association_forbids_a_direct_close() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        let retained = retained(agent("wA:p1", Some("pi"), true));
        assert_eq!(pane(&inventory, &retained, "wA:p1"), Containment::Managed);
    }

    #[test]
    fn a_retained_uncertain_association_forbids_a_direct_close() {
        let inventory = inventory(&["wA:p1"], Vec::new());
        let retained = retained(agent("wA:p1", Some("pi"), false));
        assert_eq!(pane(&inventory, &retained, "wA:p1"), Containment::Uncertain);
    }

    #[test]
    fn a_mixed_tab_is_refused_whole() {
        let inventory = inventory(&["wA:p1", "wA:p2"], vec![agent("wA:p1", Some("pi"), true)]);
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
    /// pane metadata, no lineage, no label.
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
