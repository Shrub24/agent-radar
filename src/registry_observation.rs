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

use crate::control_plane::containers::PublicContainer;
use crate::control_plane::registry::{
    Channel, Freshness, PublicChannelFacts, PublicContext, PublicRegistration, PublicSpawnTopology,
    Report, SpawnState,
};
use crate::control_plane::store::format_millis;
use crate::model::{
    AgentObservation, FleetObservation, ForegroundEvidence, HerdsmanFacts, Location,
    ProcessIdentity, RuntimeStatus, SemanticState, SessionIdentity, SessionUuid,
};
use crate::runtime::BackendIdentity;

/// One registered agent as the daemon's `agent.list` serves it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RegistryAgent {
    pub registration: PublicRegistration,
    /// Durable claim decision, if this registration has claimed a session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claim_standing: Option<crate::control_plane::registry::presence::Standing>,
    /// Whether this process runs inside a Radar-established execution scope. A
    /// process started outside the managed launch path has none and is observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scoped: Option<bool>,
    pub process: ProcessEvidence,
    /// Mutable current-session context, kept separate from registration identity.
    pub context: Option<PublicContext>,
    pub execution: Option<PublicChannelFacts>,
    pub assignment: Option<PublicChannelFacts>,
    pub facts: Option<PublicChannelFacts>,
}

impl RegistryAgent {
    /// The exact current session a publisher task snapshot may join to.
    pub fn current_session_uuid(&self) -> Option<SessionUuid> {
        let context = self.context.as_ref()?;
        if context.context.freshness != Freshness::Fresh {
            return None;
        }
        SessionUuid::parse(context.context.session.as_deref()?)
    }
}

/// What one registry read returns: every registered agent, every spawn edge and
/// every container this daemon records, each read to the end of its pages.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RegistryRead {
    pub agents: Vec<RegistryAgent>,
    pub topology: Vec<PublicSpawnTopology>,
    /// Daemon-owned lifecycle subjects. Only a unique, exact registration and
    /// process-incarnation match can become a managed TUI target.
    pub lifecycle: Vec<LifecycleSubject>,
    /// Durable containers, which the backend's own inventory does not carry: a
    /// fleet Radar created has a topology of its own.
    pub containers: Vec<PublicContainer>,
}

/// The safe identity fields from `lifecycle.list`; private launch data is never
/// part of the dashboard's registry read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LifecycleSubject {
    /// Stable logical subject used by public control requests.
    pub subject_id: String,
    /// Exact execution record ID used to match its spawn edge and process.
    #[serde(default)]
    pub claim_id: Option<String>,
    #[serde(default)]
    pub spawn_request_id: Option<String>,
    #[serde(default)]
    pub bound_registration_id: Option<String>,
    #[serde(default)]
    pub process: Option<ProcessIdentity>,
    pub state: crate::control_plane::lifecycle::State,
    #[serde(default)]
    pub managed: crate::control_plane::lifecycle::ManagedMetadata,
    #[serde(default)]
    pub session: Option<String>,
}

/// A daemon-owned process lifecycle target, joined to one registration by exact
/// agent id and process birth identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ManagedSubject {
    pub subject_id: String,
    pub claim_id: String,
    pub process: ProcessIdentity,
    pub metadata: crate::control_plane::lifecycle::ManagedMetadata,
    pub child_close: Option<ChildCloseTarget>,
    pub state: crate::control_plane::lifecycle::State,
    pub session: Option<String>,
}

/// The process evidence `agent.list` serves beside a registration. Only the
/// claimed birth identity is read here; the registry's own verdict is not used
/// to join, because a join needs the claim to match the live foreground.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProcessEvidence {
    #[serde(default)]
    pub claim: Option<ProcessIdentity>,
}

/// Why a registration did not join a pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    Joined,
    Unjoined(Unjoined),
}

/// The spawn edge that binds this registration as a child, when one exists.
///
/// Parentage and the edge's own state come from the topology read. The edge's
/// state is never a process-exit claim: an unresolved child is shown as such.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpawnBinding {
    pub parent: String,
    pub request_id: String,
    pub claim_id: Option<String>,
    pub child_source: Option<String>,
    pub child_incarnation: Option<String>,
    pub state: SpawnState,
}

/// Exact spawn-edge child identity required by `spawn.close`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChildCloseTarget {
    pub spawn_request_id: String,
    pub source: String,
    pub incarnation: String,
}

/// A registered agent with its binding, carried on the agent it projects to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RegistryView {
    pub agent: RegistryAgent,
    pub binding: Binding,
    pub spawn: Option<SpawnBinding>,
    pub managed: Option<ManagedSubject>,
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
    let RegistryRead {
        agents,
        topology,
        lifecycle,
        containers: _,
    } = read;
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
        .filter_map(|(agent, bound)| {
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
            if matches!(binding, Binding::Unjoined(_))
                && !belongs_in_view(&agent, backend, inventory, spawn.is_some())
            {
                return None;
            }
            let managed = (binding == Binding::Joined)
                .then(|| managed_subject(&agent, &lifecycle, spawn.as_ref()))
                .flatten();
            let session = agent.current_session_uuid().map(SessionIdentity::Uuid);
            Some(AgentObservation {
                location,
                name: None,
                label: None,
                status: execution_status(&agent),
                session,
                lineage: None,
                facts: facts_of(&agent),
                registry: Some(RegistryView {
                    agent,
                    binding,
                    spawn,
                    managed,
                }),
            })
        })
        .collect()
}

/// Whether a registration that did not join a pane still belongs in this
/// backend's fleet view.
///
/// Radar reaches only what is inside its own server. A registration for another
/// backend or server is never shown, and neither is one that names no pane, or a
/// pane this inventory lacks, unless the daemon recorded a spawn edge for it: a
/// registration is durable but a place is not, so a dead agent's record ages out
/// of the view instead of piling up. A registration whose pane is here stays,
/// joined or not, and that pane stays protected.
fn belongs_in_view(
    agent: &RegistryAgent,
    backend: &BackendIdentity,
    inventory: &FleetObservation,
    has_spawn: bool,
) -> bool {
    let foreign = agent
        .registration
        .location
        .as_ref()
        .is_some_and(|location| {
            location.backend != backend.name
                || (location.instance.is_some() && location.instance != backend.instance)
        });
    if foreign {
        return false;
    }
    let pane_here = agent
        .registration
        .location
        .as_ref()
        .and_then(|location| location.pane.as_deref())
        .is_some_and(|pane| inventory.pane(pane).is_some());
    pane_here || has_spawn
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

/// The managed subject for a registration, if exactly one public subject names
/// both its agent id and claimed process incarnation.
fn managed_subject(
    agent: &RegistryAgent,
    subjects: &[LifecycleSubject],
    spawn: Option<&SpawnBinding>,
) -> Option<ManagedSubject> {
    let claim = agent.process.claim.as_ref()?;
    let mut matches = subjects.iter().filter(|subject| {
        subject.bound_registration_id.as_deref()
            == Some(agent.registration.registration_id.as_str())
            && subject.process.as_ref() == Some(claim)
    });
    let subject = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(ManagedSubject {
        subject_id: subject.subject_id.clone(),
        claim_id: subject.claim_id.clone()?,
        process: claim.clone(),
        metadata: subject.managed.clone(),
        child_close: spawn
            .filter(|spawn| {
                spawn.claim_id.as_deref()
                    == Some(subject.claim_id.as_deref().unwrap_or(&subject.subject_id))
            })
            .and_then(|spawn| {
                Some(ChildCloseTarget {
                    spawn_request_id: spawn.request_id.clone(),
                    source: spawn.child_source.clone()?,
                    incarnation: spawn.child_incarnation.clone()?,
                })
            }),
        state: subject.state,
        session: subject.session.clone(),
    })
}

/// The spawn edge a registration's exact identity is bound by, when one exists.
fn spawn_binding(agent: &RegistryAgent, topology: &[PublicSpawnTopology]) -> Option<SpawnBinding> {
    let id = agent.registration.registration_id.as_str();
    topology
        .iter()
        .find(|edge| {
            edge.edge
                .bound
                .as_ref()
                .is_some_and(|bound| bound.registration_id == id)
        })
        .map(|edge| SpawnBinding {
            parent: edge.edge.parent.clone(),
            request_id: edge.edge.request_id.clone(),
            claim_id: edge.edge.claim_id.clone(),
            child_source: edge.edge.bound.as_ref().map(|bound| bound.source.clone()),
            child_incarnation: edge
                .edge
                .bound
                .as_ref()
                .map(|bound| bound.incarnation.clone()),
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
        facts.context_window = own.context_window;
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
            current_path: None,
        }
    }

    fn inventory(panes: Vec<Pane>) -> FleetObservation {
        FleetObservation {
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
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
        registration_id: &str,
        location: Value,
        claim: Option<ProcessIdentity>,
    ) -> RegistryAgent {
        serde_json::from_value(json!({
            "registration": {
                "version": 1,
                "registration_id": registration_id,
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

    /// The daemon's spawn edge for a child already bound to `registration_id`.
    fn spawn_edge(registration_id: &str) -> PublicSpawnTopology {
        serde_json::from_value(json!({
            "version": 1,
            "request_id": "r1",
            "parent": "parent-1",
            "parent_session_id": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
            "exit_observation": {"state": "unknown"},
            "created": "completed",
            "launched": "completed",
            "bound": {"source": "test", "incarnation": "11111111-1111-4111-8111-111111111111", "registration_id": registration_id, "bound_at": "2026-01-01T00:00:00.000Z"},
            "recorded_at": "2026-01-01T00:00:00.000Z",
            "state": "bound"
        }))
        .expect("a topology fixture")
    }

    /// A registration whose publisher still holds a live lease.
    fn leased(mut agent: RegistryAgent) -> RegistryAgent {
        agent.context = Some(
            serde_json::from_value(json!({
                "version": 1,
                "registration_id": agent.registration.registration_id,
                "writer": {
                    "source": "herdsman",
                    "incarnation": "22222222-2222-4222-8222-222222222222",
                    "generation": 1,
                    "sequence": 1
                },
                "context": {
                    "source": "herdsman",
                    "incarnation": "22222222-2222-4222-8222-222222222222",
                    "generation": 1,
                    "sequence": 1,
                    "lease_ms": 30_000,
                    "received_at": "2026-01-01T00:00:00.000Z",
                    "expires_at": "2026-01-01T00:00:30.000Z",
                    "restored": false,
                    "freshness": "fresh"
                }
            }))
            .expect("leased context fixture"),
        );
        agent
    }

    #[test]
    fn another_servers_registration_is_never_this_fleets() {
        let tmux = BackendIdentity::tmux("radar");
        let elsewhere = json!({
            "backend": "tmux", "instance": "herdsman", "workspace": "w1",
            "tab": "t1", "pane": "p1"
        });
        // Live or not, a registration for another server is not shown.
        let projected = project(
            &inventory(vec![pane("p1", "t1")]),
            &tmux,
            read(vec![
                leased(registered("live", elsewhere.clone(), None)),
                registered("dead", elsewhere, None),
            ]),
            &[],
        );
        assert!(projected.is_empty(), "{projected:?}");
    }

    #[test]
    fn a_registration_shows_only_for_a_pane_here_or_a_recorded_spawn() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let gone = at("p9");
        let ids = |agents: Vec<RegistryAgent>, topology: Vec<PublicSpawnTopology>| -> Vec<String> {
            let read = RegistryRead {
                agents,
                topology,
                ..RegistryRead::default()
            };
            project(&panes, &herdr(), read, &[])
                .into_iter()
                .filter_map(|agent| Some(agent.registry?.agent.registration.registration_id))
                .collect()
        };
        // Radar reaches only what is inside its own server: a pane it does not
        // hold is not shown, however live its publisher is.
        assert!(ids(vec![registered("dead", gone.clone(), None)], Vec::new()).is_empty());
        assert!(
            ids(
                vec![leased(registered("live", gone.clone(), None))],
                Vec::new()
            )
            .is_empty()
        );
        // The daemon's own spawn edge keeps a launch visible while it resolves.
        assert_eq!(
            ids(
                vec![registered("child", gone, None)],
                vec![spawn_edge("child")]
            ),
            ["child"]
        );
        // A pane that is here keeps its registration however stale: the pane stays
        // protected.
        assert_eq!(
            ids(vec![registered("held", at("p1"), None)], Vec::new()),
            ["held"]
        );
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
        let read = RegistryRead {
            agents: vec![agent],
            topology: vec![spawn_edge("a")],
            ..RegistryRead::default()
        };
        let projected = project(&panes, &herdr(), read, &[]);
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

        // A registration that names no server never joins a named backend, and its
        // pane stays protected, so the row stays.
        let projected = project(&panes, &on(Some("srv-a")), read(vec![named(None)]), &fg());
        assert_eq!(
            projected[0].registry.as_ref().map(|v| &v.binding),
            Some(&Binding::Unjoined(Unjoined::LocationMismatch))
        );

        // A registration naming a server the backend does not report is another
        // server's and is not shown at all.
        let projected = project(&panes, &on(None), read(vec![named(Some("srv-a"))]), &fg());
        assert!(projected.is_empty(), "{projected:?}");
    }

    #[test]
    fn a_registration_carries_the_spawn_edge_that_binds_its_identity() {
        let panes = inventory(vec![pane("p1", "t1")]);
        let agent = registered("child", at("p1"), None);
        let edge: PublicSpawnTopology = serde_json::from_value(json!({
            "version": 1,
            "request_id": "r1",
            "parent": "parent-1",
            "parent_session_id": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
            "exit_observation": {"state": "unknown"},
            "created": "completed",
            "launched": "completed",
            "bound": {"source": "test", "incarnation": "11111111-1111-4111-8111-111111111111", "registration_id": "child", "bound_at": "2026-01-01T00:00:00.000Z"},
            "recorded_at": "2026-01-01T00:00:00.000Z",
            "state": "bound"
        }))
        .expect("a topology fixture");
        let read = RegistryRead {
            agents: vec![agent],
            topology: vec![edge],
            ..RegistryRead::default()
        };
        let projected = project(&panes, &herdr(), read, &[]);
        let view = projected[0].registry.as_ref().expect("registry view");
        assert_eq!(
            view.spawn,
            Some(SpawnBinding {
                parent: "parent-1".into(),
                request_id: "r1".into(),
                claim_id: None,
                child_source: Some("test".into()),
                child_incarnation: Some("11111111-1111-4111-8111-111111111111".into()),
                state: SpawnState::Bound,
            })
        );
    }

    #[test]
    fn lifecycle_subject_joins_only_on_agent_and_process_birth_identity() {
        let claim = identity(10, 1);
        let panes = inventory(vec![pane("p1", "t1")]);
        let agent = registered("lead", at("p1"), Some(claim.clone()));
        let subject = LifecycleSubject {
            subject_id: "logical-subject-1".into(),
            claim_id: Some("subject-1".into()),

            spawn_request_id: Some("subject-1".into()),
            bound_registration_id: Some("lead".into()),
            process: Some(claim.clone()),
            state: crate::control_plane::lifecycle::State::Bound,
            managed: crate::control_plane::lifecycle::ManagedMetadata::default(),
            session: Some("session".into()),
        };
        let edge: PublicSpawnTopology = serde_json::from_value(json!({
            "version": 1,
            "request_id": "spawn-1",
            "parent": "parent-1",
            "parent_session_id": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
            "exit_observation": {"state": "unknown"},
            "claim_id": "subject-1",
            "created": "completed",
            "launched": "completed",
            "bound": {"source":"lead", "incarnation":"11111111-1111-4111-8111-111111111111", "registration_id":"lead", "bound_at":"2026-01-01T00:00:00.000Z"},
            "recorded_at": "2026-01-01T00:00:00.000Z",
            "state": "bound"
        })).expect("spawn edge");
        let read = RegistryRead {
            agents: vec![agent.clone()],
            topology: vec![edge.clone()],
            lifecycle: vec![subject.clone()],
            ..RegistryRead::default()
        };
        let projected = project(&panes, &herdr(), read, &[foreground("p1", claim.clone())]);
        assert_eq!(
            projected[0]
                .registry
                .as_ref()
                .and_then(|view| view.managed.as_ref())
                .map(|managed| managed.subject_id.as_str()),
            Some("logical-subject-1")
        );
        assert_eq!(
            projected[0]
                .registry
                .as_ref()
                .and_then(|view| view.managed.as_ref())
                .and_then(|managed| managed.child_close.as_ref()),
            Some(&ChildCloseTarget {
                spawn_request_id: "spawn-1".into(),
                source: "lead".into(),
                incarnation: "11111111-1111-4111-8111-111111111111".into(),
            })
        );

        let replaced = LifecycleSubject {
            process: Some(identity(10, 2)),
            ..subject.clone()
        };
        let duplicate = LifecycleSubject {
            subject_id: "subject-2".into(),
            ..subject.clone()
        };
        for lifecycle in [vec![replaced], vec![duplicate.clone(), duplicate.clone()]] {
            let read = RegistryRead {
                agents: vec![agent.clone()],
                lifecycle,
                ..RegistryRead::default()
            };
            let projected = project(&panes, &herdr(), read, &[foreground("p1", claim.clone())]);
            assert!(
                projected[0]
                    .registry
                    .as_ref()
                    .is_some_and(|view| view.managed.is_none())
            );
        }

        let unjoined = registered("lead", at("p9"), Some(claim.clone()));
        let read = RegistryRead {
            agents: vec![unjoined],
            topology: vec![edge],
            lifecycle: vec![subject],
            ..RegistryRead::default()
        };
        let projected = project(&panes, &herdr(), read, &[foreground("p1", claim)]);
        assert!(
            projected[0]
                .registry
                .as_ref()
                .is_some_and(|view| view.managed.is_none())
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
            "registration_id": "a",
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
