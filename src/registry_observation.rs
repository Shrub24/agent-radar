//! Registered agents as the dashboard observes them.
//!
//! The daemon's registry is the only source of agent facts on the daemon path.
//! This module joins each registration to a pane by exact evidence and projects
//! its fresh channels onto the normalized agent model. Nothing here reads Herdr
//! agent records, pane metadata or native status.
//!
//! A registration joins a pane only when its backend-qualified location names the
//! pane and that pane's foreground process has the registration's full birth
//! identity (boot, pid and start time). Anything less is unjoined and keeps its
//! last reported location. A registration that names a pane still protects that
//! pane: the close gate reads it as managed, so a pane is never an unmanaged close
//! target because its evidence is missing or ambiguous.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::control_plane::registry::{
    Channel, Freshness, PublicChannelFacts, PublicRegistration, PublicSpawnTopology, Report,
    SpawnState,
};
use crate::control_plane::store::format_millis;
use crate::model::{
    AgentObservation, FleetObservation, ForegroundEvidence, HerdsmanFacts, Location,
    ProcessIdentity, RuntimeStatus, SemanticState,
};
use crate::runtime::BackendIdentity;

/// One registered agent as the daemon's `agent.list` serves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryAgent {
    pub registration: PublicRegistration,
    pub process: ProcessEvidence,
    pub execution: Option<PublicChannelFacts>,
    pub assignment: Option<PublicChannelFacts>,
    pub facts: Option<PublicChannelFacts>,
}

/// What one registry read returns: every registered agent and every spawn edge,
/// each read to the end of its pages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryRead {
    pub agents: Vec<RegistryAgent>,
    pub topology: Vec<PublicSpawnTopology>,
}

/// The process evidence `agent.list` serves beside a registration. Only the
/// claimed birth identity is read here; the registry's own verdict is not used
/// to join, because a join needs the claim to match the live foreground.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessEvidence {
    #[serde(default)]
    pub claim: Option<ProcessIdentity>,
}

/// Why a registration did not join a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unjoined {
    /// No pane is named, or the registration names no location at all.
    NoLocation,
    /// The registration claimed no process identity to compare.
    NoClaim,
    /// The named pane is not in this inventory.
    PaneAbsent,
    /// The backend, instance, tab or workspace differ from the pane's.
    LocationMismatch,
    /// The pane's foreground identity was not sampled or could not be read.
    ForegroundUnreadable,
    /// The pane's foreground is a different process incarnation.
    IdentityMismatch,
    /// Two registrations claim the same pane's foreground identity.
    Ambiguous,
}

/// How a registration relates to the live panes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    Joined,
    Unjoined(Unjoined),
}

/// The spawn edge that binds this registration as a child, when one exists.
///
/// Parentage and the edge's own state come from the topology read. The edge's
/// state is never a process-exit claim: an unresolved child is shown as such.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnBinding {
    pub parent: String,
    pub state: SpawnState,
}

/// A registered agent with its binding, carried on the agent it projects to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryView {
    pub agent: RegistryAgent,
    pub binding: Binding,
    pub spawn: Option<SpawnBinding>,
}

/// Projects a registry read onto `inventory`'s panes as agent observations.
///
/// `evidence` is the foreground answer for each pane this refresh sampled. The
/// returned agents are the only agent facts a registry-serving refresh presents.
pub fn project(
    inventory: &FleetObservation,
    backend: &BackendIdentity,
    read: RegistryRead,
    evidence: &[(String, ForegroundEvidence)],
) -> Vec<AgentObservation> {
    let RegistryRead { agents, topology } = read;
    let foreground: HashMap<&str, &ProcessIdentity> = evidence
        .iter()
        .filter_map(|(pane_id, evidence)| match evidence {
            ForegroundEvidence::NonShell { local, .. } => local
                .resources
                .as_ref()
                .map(|resources| (pane_id.as_str(), &resources.identity)),
            _ => None,
        })
        .collect();
    let bound: Vec<Result<String, Unjoined>> = agents
        .iter()
        .map(|agent| bind(inventory, backend, agent, &foreground))
        .collect();
    // Two registrations on one foreground identity cannot both be its agent, so
    // neither joins: an ambiguous pane is shown and protected, never guessed.
    let mut claimed: HashMap<&str, usize> = HashMap::new();
    for pane_id in bound.iter().flatten() {
        *claimed.entry(pane_id.as_str()).or_default() += 1;
    }
    let ambiguous: HashSet<String> = claimed
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .map(|(pane_id, _)| pane_id.to_string())
        .collect();

    agents
        .into_iter()
        .zip(bound)
        .map(|(agent, bound)| {
            let binding = match bound {
                Ok(pane_id) if ambiguous.contains(&pane_id) => {
                    Binding::Unjoined(Unjoined::Ambiguous)
                }
                Ok(_) => Binding::Joined,
                Err(reason) => Binding::Unjoined(reason),
            };
            // Only a joined registration takes the pane's own location. Anything
            // else keeps what it reported, so a failed join never borrows a pane
            // it did not prove.
            let location = match (&binding, agent.registration.location.as_ref()) {
                (Binding::Joined, Some(reported)) => reported
                    .pane
                    .as_deref()
                    .and_then(|id| inventory.pane(id))
                    .map_or_else(|| last_location(&agent), |pane| pane.location.clone()),
                _ => last_location(&agent),
            };
            let spawn = spawn_binding(&agent, &topology);
            AgentObservation {
                location,
                name: None,
                label: None,
                status: execution_status(&agent),
                session: None,
                lineage: None,
                facts: facts_of(&agent),
                registry: Some(RegistryView {
                    agent,
                    binding,
                    spawn,
                }),
            }
        })
        .collect()
}

/// The pane a registration exactly joins, or why it does not.
fn bind(
    inventory: &FleetObservation,
    backend: &BackendIdentity,
    agent: &RegistryAgent,
    foreground: &HashMap<&str, &ProcessIdentity>,
) -> Result<String, Unjoined> {
    let reported = agent
        .registration
        .location
        .as_ref()
        .ok_or(Unjoined::NoLocation)?;
    let pane_id = reported.pane.as_deref().ok_or(Unjoined::NoLocation)?;
    let claim = agent.process.claim.as_ref().ok_or(Unjoined::NoClaim)?;
    let pane = inventory.pane(pane_id).ok_or(Unjoined::PaneAbsent)?;
    // Exact, both ways: a registration that names no server never matches a
    // backend that has one, and the reverse is also a mismatch.
    let instance_differs = reported.instance != backend.instance;
    let tab_differs = reported
        .tab
        .as_ref()
        .is_some_and(|tab| *tab != pane.location.tab_id);
    let workspace_differs = reported
        .workspace
        .as_ref()
        .is_some_and(|workspace| *workspace != pane.location.workspace_id);
    if reported.backend != backend.name || instance_differs || tab_differs || workspace_differs {
        return Err(Unjoined::LocationMismatch);
    }
    match foreground.get(pane_id) {
        None => Err(Unjoined::ForegroundUnreadable),
        Some(identity) if *identity != claim => Err(Unjoined::IdentityMismatch),
        Some(_) => Ok(pane_id.to_string()),
    }
}

/// The spawn edge a registration's exact identity is bound by, when one exists.
fn spawn_binding(agent: &RegistryAgent, topology: &[PublicSpawnTopology]) -> Option<SpawnBinding> {
    let id = agent.registration.agent_id.as_str();
    topology
        .iter()
        .find(|edge| {
            edge.edge
                .bound
                .as_ref()
                .is_some_and(|bound| bound.agent_id.as_deref() == Some(id))
        })
        .map(|edge| SpawnBinding {
            parent: edge.edge.parent.clone(),
            state: edge.state,
        })
}

/// The last location a registration reported, for an agent with no pane to show.
///
/// A publisher may omit workspace or tab. Those are empty here, which means
/// reported as unknown; a row must not treat them as identifiers.
fn last_location(agent: &RegistryAgent) -> Location {
    let reported = agent.registration.location.as_ref();
    Location {
        workspace_id: reported
            .and_then(|l| l.workspace.clone())
            .unwrap_or_default(),
        tab_id: reported.and_then(|l| l.tab.clone()).unwrap_or_default(),
        pane_id: reported.and_then(|l| l.pane.clone()).unwrap_or_default(),
    }
}

/// A channel's report read as that channel's own schema, only while the registry
/// says it is fresh.
///
/// A stale or absent channel yields nothing, so an old value is never shown as
/// current. Its freshness still travels on the registry view. A payload that
/// does not fit the channel is absent, not guessed into another schema.
fn fresh(channel: &Option<PublicChannelFacts>, kind: Channel) -> Option<Report> {
    if channel_freshness(channel)? != Freshness::Fresh {
        return None;
    }
    channel
        .as_ref()?
        .snapshot
        .as_ref()?
        .snapshot
        .for_channel(kind)
        .ok()
}

/// How fresh a channel's last report is, or `None` when it has never reported.
///
/// The presentation labels each authority with this, so a row and the projection
/// read the same freshness.
pub fn channel_freshness(channel: &Option<PublicChannelFacts>) -> Option<Freshness> {
    channel
        .as_ref()?
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.freshness)
}

/// The execution channel's reported activity as a runtime status.
fn execution_status(agent: &RegistryAgent) -> Option<RuntimeStatus> {
    match fresh(&agent.execution, Channel::Execution)? {
        Report::Execution(snapshot) => Some(RuntimeStatus::from_reported(&snapshot.activity)),
        _ => None,
    }
}

/// The Herdsman-shaped facts a fresh registry read establishes.
///
/// The assignment's start is left unmeasured here: its age is stamped against
/// one clock reading per refresh by the collector.
fn facts_of(agent: &RegistryAgent) -> HerdsmanFacts {
    let mut facts = HerdsmanFacts {
        label: agent.registration.label.clone(),
        ..HerdsmanFacts::default()
    };
    if let Some(Report::Assignment(assignment)) = fresh(&agent.assignment, Channel::Assignment) {
        facts.state = Some(SemanticState::from_published(&assignment.activity));
        facts.role = assignment.role.clone();
        facts.definition = assignment.definition.clone();
        facts.run = assignment.run.clone();
        facts.request = assignment.request.clone();
        facts.assignment = assignment.assignment.clone();
        facts.assigned_started_unix_ms = assignment.assigned_started_unix_ms;
        facts.awaited = assignment.awaited.clone();
        facts.ask = assignment.ask.clone();
        facts.waiting_reason = assignment.waiting_reason.clone();
        facts.last_outcome =
            assignment
                .last_outcome
                .as_ref()
                .map(|outcome| match &outcome.detail {
                    Some(detail) => format!("{} — {detail}", outcome.result),
                    None => outcome.result.clone(),
                });
    }
    if let Some(Report::Facts(own)) = fresh(&agent.facts, Channel::Facts) {
        facts.model = own.model.clone();
        facts.provider = own.provider.clone();
        facts.thinking = own.thinking.clone();
        facts.context_usage = own.context_usage.clone();
        facts.session_name = own.session_name.clone();
        facts.background_tasks = own
            .background_tasks
            .iter()
            .map(|task| format!("{}:{}", task.id, task.phase))
            .collect();
        facts.background_started = own
            .background_started_unix_ms
            .map(|millis| format_millis(millis as i64));
    }
    facts
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::control_plane::registry::PublicSpawnTopology;
    use crate::model::{
        DescendantResources, LocalFacts, Pane, ProcessResources, ProcessState, Total,
    };

    const BOOT: &str = "3f1c2a9e-0000-4000-8000-000000000001";

    /// A registry read with agents and no spawn topology.
    fn read(agents: Vec<RegistryAgent>) -> RegistryRead {
        RegistryRead {
            agents,
            ..RegistryRead::default()
        }
    }

    fn identity(pid: i32, start_ticks: u64) -> ProcessIdentity {
        ProcessIdentity {
            boot_id: BOOT.into(),
            pid,
            start_ticks,
        }
    }

    fn pane(pane_id: &str, tab_id: &str) -> Pane {
        Pane {
            location: Location {
                workspace_id: "w1".into(),
                tab_id: tab_id.into(),
                pane_id: pane_id.into(),
            },
            label: None,
            title: None,
        }
    }

    fn inventory(panes: Vec<Pane>) -> FleetObservation {
        FleetObservation {
            metadata_agents: Vec::new(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
            panes,
            agents: Vec::new(),
        }
    }

    /// The pane's foreground as the collector composes it: a sampled process.
    fn foreground(pane_id: &str, process: ProcessIdentity) -> (String, ForegroundEvidence) {
        let pid = process.pid;
        let resources = ProcessResources {
            identity: process,
            state: ProcessState::from_letter('S'),
            rss_bytes: None,
            cpu: None,
            descendants: DescendantResources {
                observed: None,
                members: Vec::new(),
                rss_bytes: Total::Unknown("test".into()),
                cpu: Total::Unknown("test".into()),
            },
        };
        let evidence = ForegroundEvidence::NonShell {
            pid,
            name: None,
            command: None,
            local: LocalFacts {
                resources: Some(resources),
                ..LocalFacts::default()
            },
        };
        (pane_id.into(), evidence)
    }

    /// A registration as `agent.list` serves it, claiming `claim` at `location`.
    fn registered(
        agent_id: &str,
        location: Value,
        claim: Option<ProcessIdentity>,
    ) -> RegistryAgent {
        serde_json::from_value(json!({
            "registration": {
                "version": 1,
                "agent_id": agent_id,
                "registered_at": "2026-01-01T00:00:00.000Z",
                "source": "herdsman",
                "incarnation": "11111111-1111-4111-8111-111111111111",
                "location": location,
                "process_claimed": claim.is_some(),
                "launch": {"available": false}
            },
            "process": {"claimed": claim.is_some(), "claim": claim, "verification": "unavailable"},
            "execution": null,
            "assignment": null,
            "facts": null
        }))
        .expect("registration fixture decodes")
    }

    fn herdr() -> BackendIdentity {
        BackendIdentity::named("herdr")
    }

    fn at(pane_id: &str) -> Value {
        json!({"backend": "herdr", "workspace": "w1", "tab": "t1", "pane": pane_id})
    }

    #[test]
    fn a_registration_joins_only_on_its_exact_location_and_birth_identity() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let live = identity(42, 900);
        let cases = [
            (
                "exact",
                registered("a", at("p1"), Some(live.clone())),
                vec![foreground("p1", live.clone())],
                Binding::Joined,
            ),
            (
                "pid reused by a later incarnation",
                registered("a", at("p1"), Some(live.clone())),
                vec![foreground("p1", identity(42, 901))],
                Binding::Unjoined(Unjoined::IdentityMismatch),
            ),
            (
                "foreground not sampled",
                registered("a", at("p1"), Some(live.clone())),
                Vec::new(),
                Binding::Unjoined(Unjoined::ForegroundUnreadable),
            ),
            (
                "no claimed identity",
                registered("a", at("p1"), None),
                vec![foreground("p1", live.clone())],
                Binding::Unjoined(Unjoined::NoClaim),
            ),
            (
                "tab disagrees with the pane",
                registered(
                    "a",
                    json!({"backend": "herdr", "tab": "t9", "pane": "p1"}),
                    Some(live.clone()),
                ),
                vec![foreground("p1", live.clone())],
                Binding::Unjoined(Unjoined::LocationMismatch),
            ),
        ];
        for (name, agent, evidence, expected) in cases {
            let projected = project(&panes, &herdr(), read(vec![agent]), &evidence);
            let view = projected[0].registry.as_ref().expect("registry view");
            assert_eq!(view.binding, expected, "case: {name}");
            assert_eq!(
                projected[0].location.pane_id, "p1",
                "case: {name} keeps its pane"
            );
        }
    }

    #[test]
    fn a_registration_on_an_absent_pane_keeps_its_last_location_unjoined() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let agent = registered("a", at("p9"), Some(identity(7, 5)));
        let projected = project(&panes, &herdr(), read(vec![agent]), &[]);
        let view = projected[0].registry.as_ref().expect("registry view");
        assert_eq!(view.binding, Binding::Unjoined(Unjoined::PaneAbsent));
        assert_eq!(projected[0].location.pane_id, "p9");
        assert_eq!(projected[0].location.tab_id, "t1");
    }

    #[test]
    fn an_unjoined_registration_keeps_its_reported_location_not_the_panes() {
        // The pane exists in t1, but the registration reports t9: it must not
        // borrow the pane's own tab.
        let panes = inventory(vec![pane("p1", "t1")]);
        let reported = json!({"backend": "herdr", "workspace": "w1", "tab": "t9", "pane": "p1"});
        let agent = registered("a", reported, Some(identity(7, 5)));
        let projected = project(&panes, &herdr(), read(vec![agent]), &[]);
        assert_eq!(
            projected[0].registry.as_ref().map(|view| &view.binding),
            Some(&Binding::Unjoined(Unjoined::LocationMismatch))
        );
        assert_eq!(projected[0].location.tab_id, "t9");
    }

    #[test]
    fn a_named_backend_instance_must_match_exactly_in_both_directions() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let live = identity(42, 900);
        let named = |instance: Option<&str>| {
            let mut location = at("p1");
            if let Some(instance) = instance {
                location["instance"] = json!(instance);
            }
            registered("a", location, Some(live.clone()))
        };
        let on = |instance: Option<&str>| BackendIdentity {
            name: "herdr".into(),
            instance: instance.map(str::to_string),
        };
        let fg = || vec![foreground("p1", live.clone())];

        let joined = project(
            &panes,
            &on(Some("srv-a")),
            read(vec![named(Some("srv-a"))]),
            &fg(),
        );
        assert_eq!(
            joined[0].registry.as_ref().map(|v| &v.binding),
            Some(&Binding::Joined)
        );

        // A registration that names no server never joins a named backend, and a
        // named registration never joins a backend that reports none.
        for (backend, agent) in [
            (on(Some("srv-a")), named(None)),
            (on(None), named(Some("srv-a"))),
        ] {
            let projected = project(&panes, &backend, read(vec![agent]), &fg());
            assert_eq!(
                projected[0].registry.as_ref().map(|v| &v.binding),
                Some(&Binding::Unjoined(Unjoined::LocationMismatch))
            );
        }
    }

    #[test]
    fn a_registration_carries_the_spawn_edge_that_binds_its_identity() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let agent = registered("child", at("p1"), None);
        let edge: PublicSpawnTopology = serde_json::from_value(json!({
            "version": 1,
            "request_id": "r1",
            "parent": "parent-1",
            "created": "completed",
            "launched": "completed",
            "bound": {"source": "test", "incarnation": "11111111-1111-4111-8111-111111111111", "agent_id": "child", "bound_at": "2026-01-01T00:00:00.000Z"},
            "recorded_at": "2026-01-01T00:00:00.000Z",
            "state": "bound"
        }))
        .expect("a topology fixture");
        let read = RegistryRead {
            agents: vec![agent],
            topology: vec![edge],
        };
        let projected = project(&panes, &herdr(), read, &[]);
        let view = projected[0].registry.as_ref().expect("registry view");
        assert_eq!(
            view.spawn,
            Some(SpawnBinding {
                parent: "parent-1".into(),
                state: SpawnState::Bound,
            })
        );
    }

    #[test]
    fn duplicate_sessions_on_different_panes_each_join_their_own_pane() {
        let panes = inventory(vec![pane("p1", "t1"), pane("p2", "t1")]);
        let (first, second) = (identity(10, 1), identity(11, 2));
        let agents = vec![
            registered("a", at("p1"), Some(first.clone())),
            registered("b", at("p2"), Some(second.clone())),
        ];
        let evidence = [foreground("p1", first), foreground("p2", second)];
        let projected = project(&panes, &herdr(), read(agents), &evidence);
        assert!(
            projected
                .iter()
                .all(|agent| agent.registry.as_ref().unwrap().binding == Binding::Joined)
        );
        assert_eq!(projected[0].location.pane_id, "p1");
        assert_eq!(projected[1].location.pane_id, "p2");
    }

    #[test]
    fn two_registrations_claiming_one_foreground_identity_are_both_unjoined() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let shared = identity(10, 1);
        let agents = vec![
            registered("a", at("p1"), Some(shared.clone())),
            registered("b", at("p1"), Some(shared.clone())),
        ];
        let projected = project(&panes, &herdr(), read(agents), &[foreground("p1", shared)]);
        for agent in &projected {
            assert_eq!(
                agent.registry.as_ref().unwrap().binding,
                Binding::Unjoined(Unjoined::Ambiguous)
            );
            assert_eq!(agent.location.pane_id, "p1", "the pane stays protected");
        }
    }

    #[test]
    fn a_stale_execution_snapshot_is_unknown_not_idle() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let mut agent = registered("a", at("p1"), None);
        agent.execution = serde_json::from_value(json!({
            "version": 1,
            "agent_id": "a",
            "channel": "execution",
            "writer": {"source": "herdsman", "incarnation": "11111111-1111-4111-8111-111111111111",
                       "generation": 1, "sequence": 3},
            "snapshot": {"source": "herdsman", "incarnation": "11111111-1111-4111-8111-111111111111",
                         "generation": 1, "sequence": 3, "lease_ms": 30000,
                         "received_at": "2026-01-01T00:00:00.000Z",
                         "expires_at": "2026-01-01T00:00:30.000Z",
                         "restored": false, "freshness": "stale",
                         "snapshot": {"activity": "idle"}}
        }))
        .expect("execution fixture decodes");
        let projected = project(&panes, &herdr(), read(vec![agent]), &[]);
        assert_eq!(projected[0].status, None);
        assert_eq!(projected[0].state(), crate::model::AgentState::Unknown);
    }
}
