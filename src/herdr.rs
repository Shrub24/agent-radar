//! Herdr CLI wire decoding — the only module that sees raw `herdr` output.
//!
//! Consumers are the collector boundary: pass the captured stdout of
//! `herdr api snapshot` to [`decode_snapshot`] and of
//! `herdr pane process-info --pane <id>` to [`decode_process_info`]. DTOs stay
//! private here; only normalized [`crate::model`] facts leave this module, so
//! presentation code never touches source shapes or metadata tokens.

use serde::Deserialize;
use std::collections::HashMap;

use crate::model::{
    AgentObservation, FleetObservation, ForegroundEvidence, HerdsmanFacts, Lineage, Location, Pane,
    RuntimeStatus, SemanticState, SessionIdentity, SessionUuid, Tab, Workspace,
};

/// Metadata tokens carrying explicit pi-herdsman ownership UUIDs.
const LINEAGE_SESSION_TOKEN: &str = "pi_herdsman_session";
const LINEAGE_PARENT_TOKEN: &str = "pi_herdsman_parent_session";

/// Metadata tokens carrying Herdsman's published facts about a pane's agent.
///
/// Every other key in a pane's token map is ignored: another publisher's
/// tokens (`pi_bg_*` belongs to `pi-bash-processes`), Herdsman tokens Radar
/// does not model, and names a later version introduces.
const ROLE_TOKEN: &str = "pi_herdsman_role";
const LABEL_TOKEN: &str = "pi_herdsman_label";
const NAME_TOKEN: &str = "pi_herdsman_name";
const RUN_TOKEN: &str = "pi_herdsman_run";
const REQUEST_TOKEN: &str = "pi_herdsman_request";
const ASK_TOKEN: &str = "pi_herdsman_ask";
const TASK_TOKEN: &str = "pi_herdsman_task";
const STARTED_TOKEN: &str = "pi_herdsman_started";
const STATE_TOKEN: &str = "pi_herdsman_state";
const AWAITED_TOKEN: &str = "pi_herdsman_awaited";
const BACKGROUND_RUNNING_TOKEN: &str = "pi_bg_running";
const BACKGROUND_TASKS_TOKEN: &str = "pi_bg_tasks";
const BACKGROUND_STARTED_TOKEN: &str = "pi_bg_started";
const MODEL_TOKEN: &str = "model";
const PROVIDER_TOKEN: &str = "provider";
const THINKING_TOKEN: &str = "thinking";
const CONTEXT_USAGE_TOKEN: &str = "context_usage";
const SESSION_NAME_TOKEN: &str = "session";

/// A required inventory shape was missing, malformed or carried an empty id.
///
/// Malformed required inventory is rejected outright: it must never be
/// presented as a successful empty fleet.
#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    Malformed(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(error) => write!(f, "not valid Herdr JSON: {error}"),
            Self::Malformed(what) => write!(f, "malformed Herdr inventory: {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl From<serde_json::Error> for DecodeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

// ---------------------------------------------------------------------------
// Private DTOs: deserialize consumed fields, tolerate every unknown field.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Output<R> {
    result: R,
}

#[derive(Deserialize)]
struct SnapshotResult {
    snapshot: SnapshotInventory,
}

#[derive(Deserialize)]
struct SnapshotInventory {
    workspaces: Vec<WorkspaceRecord>,
    tabs: Vec<TabRecord>,
    panes: Vec<PaneRecord>,
    agents: Vec<AgentRecord>,
}

#[derive(Deserialize)]
struct WorkspaceRecord {
    workspace_id: String,
    label: Option<String>,
    number: Option<u32>,
}

#[derive(Deserialize)]
struct TabRecord {
    tab_id: String,
    workspace_id: String,
    label: Option<String>,
    number: Option<u32>,
}

#[derive(Deserialize)]
struct PaneRecord {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    label: Option<String>,
    terminal_title: Option<String>,
    terminal_title_stripped: Option<String>,
}

#[derive(Deserialize)]
struct AgentRecord {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    agent: Option<String>,
    agent_status: Option<String>,
    agent_session: Option<SessionRecord>,
    display_agent: Option<String>,
    terminal_title: Option<String>,
    terminal_title_stripped: Option<String>,
    tokens: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct SessionRecord {
    kind: Option<String>,
    source: Option<String>,
    value: Option<String>,
}

#[derive(Deserialize)]
struct ProcessInfoResult {
    process_info: ProcessInfoRecord,
}

#[derive(Deserialize)]
struct ProcessInfoRecord {
    shell_pid: Option<i32>,
    foreground_process_group_id: Option<i32>,
    #[serde(default)]
    foreground_processes: Vec<ForegroundProcessRecord>,
}

#[derive(Deserialize)]
struct ForegroundProcessRecord {
    pid: i32,
    name: Option<String>,
    cmdline: Option<String>,
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Decodes `herdr api snapshot` output into a normalized inventory.
///
/// Rejects output whose required inventory is missing or malformed; a
/// successfully decoded but empty inventory is a valid observation.
pub fn decode_snapshot(output: &str) -> Result<FleetObservation, DecodeError> {
    let parsed: Output<SnapshotResult> = serde_json::from_str(output)?;
    let inventory = parsed.result.snapshot;

    for workspace in &inventory.workspaces {
        require_id(&workspace.workspace_id, "workspace without workspace_id")?;
    }
    for tab in &inventory.tabs {
        require_id(&tab.tab_id, "tab without tab_id")?;
        require_id(&tab.workspace_id, "tab without workspace_id")?;
    }
    for pane in &inventory.panes {
        require_id(&pane.pane_id, "pane without pane_id")?;
        require_id(&pane.tab_id, "pane without tab_id")?;
        require_id(&pane.workspace_id, "pane without workspace_id")?;
    }
    for agent in &inventory.agents {
        require_id(&agent.pane_id, "agent without pane_id")?;
        require_id(&agent.tab_id, "agent without tab_id")?;
        require_id(&agent.workspace_id, "agent without workspace_id")?;
    }

    Ok(FleetObservation {
        workspaces: inventory
            .workspaces
            .into_iter()
            .map(|w| Workspace {
                workspace_id: w.workspace_id,
                label: w.label,
                number: w.number,
            })
            .collect(),
        tabs: inventory
            .tabs
            .into_iter()
            .map(|t| Tab {
                tab_id: t.tab_id,
                workspace_id: t.workspace_id,
                label: t.label,
                number: t.number,
            })
            .collect(),
        panes: inventory
            .panes
            .into_iter()
            .map(|p| Pane {
                location: Location {
                    workspace_id: p.workspace_id,
                    tab_id: p.tab_id,
                    pane_id: p.pane_id,
                },
                label: non_empty(p.label),
                title: non_empty(p.terminal_title_stripped).or_else(|| non_empty(p.terminal_title)),
            })
            .collect(),
        agents: inventory
            .agents
            .into_iter()
            .map(|a| {
                let facts = herdsman_facts(&a);
                let session = session_identity(a.agent_session);
                let lineage = lineage(a.tokens.as_ref());
                AgentObservation {
                    location: Location {
                        workspace_id: a.workspace_id,
                        tab_id: a.tab_id,
                        pane_id: a.pane_id,
                    },
                    name: non_empty(a.agent),
                    label: non_empty(a.terminal_title_stripped)
                        .or_else(|| non_empty(a.terminal_title)),
                    status: a.agent_status.as_deref().map(RuntimeStatus::from_reported),
                    session,
                    lineage,
                    facts,
                }
            })
            .collect(),
    })
}

/// Decodes `herdr pane process-info --pane <id>` output into foreground
/// evidence.
///
/// Only PID/process-group facts are interpreted, never shell executable names.
/// Malformed output is an error (the collector reports it as inconclusive);
/// absent or inconsistent PID fields decode to
/// [`ForegroundEvidence::Inconclusive`].
pub fn decode_process_info(output: &str) -> Result<ForegroundEvidence, DecodeError> {
    let parsed: Output<ProcessInfoResult> = serde_json::from_str(output)?;
    let record = parsed.result.process_info;

    let (Some(shell_pid), Some(foreground_group)) =
        (record.shell_pid, record.foreground_process_group_id)
    else {
        return Ok(ForegroundEvidence::Inconclusive);
    };
    if foreground_group == shell_pid {
        return Ok(ForegroundEvidence::Shell);
    }
    // The foreground group leader identifies the command occupying the pane.
    // If it is already gone, nothing can be positively named.
    match record
        .foreground_processes
        .iter()
        .find(|p| p.pid == foreground_group)
    {
        Some(leader) => Ok(ForegroundEvidence::command(
            leader.pid,
            non_empty(leader.name.clone()),
            non_empty(leader.cmdline.clone()),
        )),
        None => Ok(ForegroundEvidence::Inconclusive),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_id(value: &str, message: &'static str) -> Result<(), DecodeError> {
    if value.is_empty() {
        return Err(DecodeError::Malformed(message));
    }
    Ok(())
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.is_empty())
}

/// Classifies a reported session record.
///
/// Only `kind == "id"` with a UUID-shaped value becomes an explicit UUID
/// identity; every other reported value stays a source-qualified reference.
fn session_identity(record: Option<SessionRecord>) -> Option<SessionIdentity> {
    let record = record?;
    let value = non_empty(record.value)?;
    if record.kind.as_deref() == Some("id")
        && let Some(uuid) = SessionUuid::parse(&value)
    {
        return Some(SessionIdentity::Uuid(uuid));
    }
    Some(SessionIdentity::Reported {
        source: non_empty(record.source),
        value,
    })
}

/// Extracts ownership lineage from the two UUID tokens that carry it.
///
/// A malformed or path-like session token yields no lineage; a malformed
/// parent token only drops the ownership link. Ownership is never taken from
/// the human session name or from a pane's position.
fn lineage(tokens: Option<&HashMap<String, serde_json::Value>>) -> Option<Lineage> {
    let tokens = tokens?;
    let session = SessionUuid::parse(tokens.get(LINEAGE_SESSION_TOKEN)?.as_str()?)?;
    let parent = tokens
        .get(LINEAGE_PARENT_TOKEN)
        .and_then(|value| value.as_str())
        .and_then(SessionUuid::parse);
    Some(Lineage { session, parent })
}

/// Decodes the Herdsman facts an agent record carries.
///
/// An absent or cleared key stays unavailable: Herdsman clears a fact by
/// clearing its key, and Herdr's own `agent` and title fields are never read as
/// Herdsman facts. The owner's published state is taken as published — its
/// presence is its freshness — and the active assignment's age is not measured
/// here, because the caller's clock does that once per collection.
fn herdsman_facts(record: &AgentRecord) -> HerdsmanFacts {
    // Herdr's display agent is what a pane declares itself to be, which
    // Herdsman's publisher sets from the worker's agent definition. It is not a
    // token, so it is read from the record rather than from the token map.
    let mut facts = HerdsmanFacts {
        definition: non_empty(record.display_agent.clone()),
        ..HerdsmanFacts::default()
    };
    let Some(tokens) = record.tokens.as_ref() else {
        return facts;
    };
    facts.role = token(tokens, ROLE_TOKEN);
    facts.label = token(tokens, LABEL_TOKEN);
    facts.name = token(tokens, NAME_TOKEN);
    facts.run = token(tokens, RUN_TOKEN);
    facts.request = token(tokens, REQUEST_TOKEN);
    facts.ask = token(tokens, ASK_TOKEN);
    facts.assignment = token(tokens, TASK_TOKEN);
    facts.assigned_started_unix_ms =
        token(tokens, STARTED_TOKEN).and_then(|started| started.parse().ok());
    facts.state = token(tokens, STATE_TOKEN).map(|state| SemanticState::from_published(&state));
    facts.awaited = token_items(tokens, AWAITED_TOKEN);
    // The background-task keys have another publisher; a value this version
    // cannot read leaves the fact absent rather than failing the inventory.
    facts.background_running =
        token(tokens, BACKGROUND_RUNNING_TOKEN).and_then(|running| running.parse().ok());
    facts.background_tasks = token_items(tokens, BACKGROUND_TASKS_TOKEN);
    // A timestamp is the publisher's own text; Radar neither parses nor
    // normalises it.
    facts.background_started = token(tokens, BACKGROUND_STARTED_TOKEN);
    facts.model = token(tokens, MODEL_TOKEN);
    facts.provider = token(tokens, PROVIDER_TOKEN);
    facts.thinking = token(tokens, THINKING_TOKEN);
    facts.context_usage = token(tokens, CONTEXT_USAGE_TOKEN);
    facts.session_name = token(tokens, SESSION_NAME_TOKEN);
    facts
}

/// A token's text, or `None` when it is absent, empty or not text.
///
/// Herdsman clears a key to clear a fact, so an absent key is unavailable
/// rather than a default.
fn token(tokens: &HashMap<String, serde_json::Value>, key: &str) -> Option<String> {
    let text = tokens.get(key)?.as_str()?;
    (!text.is_empty()).then(|| text.to_string())
}

/// A token's comma-separated items, in published order, with blanks dropped.
fn token_items(tokens: &HashMap<String, serde_json::Value>, key: &str) -> Vec<String> {
    let Some(value) = token(tokens, key) else {
        return Vec::new();
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_SHAPED: &str = include_str!("../tests/fixtures/snapshot_real_shaped.json");
    const EMPTY: &str = include_str!("../tests/fixtures/snapshot_empty.json");
    /// Herdsman's own consumer fixture, copied from
    /// `pi-herdsman/docs/reference/pane-metadata.fixture.json`.
    const PANE_METADATA: &str = include_str!("../tests/fixtures/herdsman_pane_metadata.json");

    fn uuid(value: &str) -> SessionUuid {
        SessionUuid::parse(value).expect("test UUID")
    }

    /// Herdsman's fixture document.
    fn herdsman_fixture() -> serde_json::Value {
        serde_json::from_str(PANE_METADATA).expect("Herdsman's fixture is JSON")
    }

    /// The fixture's flattened agent records, as a `herdr api snapshot` body.
    fn snapshot_of(agents: &serde_json::Value) -> String {
        snapshot_body("[]", &agents.to_string())
    }

    /// Minimal valid envelope around a snapshot body, for targeted cases.
    fn snapshot_body(panes: &str, agents: &str) -> String {
        format!(
            r#"{{"id":"cli:api:snapshot","result":{{"type":"snapshot","snapshot":{{"workspaces":[],"tabs":[],"panes":{panes},"agents":{agents}}}}}}}"#
        )
    }

    #[test]
    fn decodes_real_shaped_inventory() {
        let observation = decode_snapshot(REAL_SHAPED).expect("real-shaped fixture decodes");

        assert_eq!(observation.workspaces.len(), 2);
        assert_eq!(observation.tabs.len(), 4);
        assert_eq!(observation.panes.len(), 7);
        assert_eq!(observation.agents.len(), 4);

        let workspace = &observation.workspaces[0];
        assert_eq!(workspace.workspace_id, "wA");
        assert_eq!(workspace.label.as_deref(), Some("main"));
        assert_eq!(workspace.number, Some(1));

        let tab = &observation.tabs[1];
        assert_eq!(tab.tab_id, "wA:t2");
        assert_eq!(tab.workspace_id, "wA");
        assert_eq!(tab.label.as_deref(), Some("agent tab"));
        assert_eq!(tab.number, Some(2));

        // Pane display text: declared label, stripped title, or neither.
        let pane = observation.pane("wA:p1").expect("pane p1");
        assert_eq!(pane.label.as_deref(), Some("fleet owner task"));
        assert_eq!(pane.display_name(), Some("fleet owner task"));
        let pane = observation.pane("wA:p3").expect("pane p3");
        assert_eq!(pane.label, None);
        assert_eq!(pane.display_name(), Some("build log tail"));
        let pane = observation.pane("wA:p4").expect("pane p4");
        assert_eq!(pane.display_name(), None);
        assert!(observation.pane("wZ:p9").is_none());

        // Owner: path identity stays a reported reference even though the path
        // embeds a UUID; the explicit UUID appears only as lineage.
        let owner = observation.agent_on_pane("wA:p1").expect("owner agent");
        assert_eq!(
            owner.location,
            Location {
                workspace_id: "wA".into(),
                tab_id: "wA:t1".into(),
                pane_id: "wA:p1".into(),
            }
        );
        assert_eq!(owner.name.as_deref(), Some("pi"));
        assert_eq!(owner.label.as_deref(), Some("fleet owner task"));
        assert_eq!(owner.status, Some(RuntimeStatus::Idle));
        assert_eq!(
            owner.session,
            Some(SessionIdentity::Reported {
                source: Some("herdr:pi".into()),
                value: "/home/dev/.pi/agent/sessions/--home-dev-projects-alpha--/2026-09-01T10-00-00-000Z_11111111-1111-4111-8111-111111111111.jsonl".into(),
            })
        );
        assert_eq!(
            owner.lineage,
            Some(Lineage {
                session: uuid("11111111-1111-4111-8111-111111111111"),
                parent: None,
            })
        );
        assert_eq!(owner.role(), None);
        assert_eq!(owner.facts.assignment, None);

        // Worker: explicit parent link for ownership joins.
        let worker = observation.agent_on_pane("wA:p2").expect("worker agent");
        assert_eq!(worker.status, Some(RuntimeStatus::Working));
        let lineage = worker.lineage.clone().expect("worker lineage");
        assert_eq!(
            lineage.session,
            uuid("22222222-2222-4222-8222-222222222222")
        );
        assert_eq!(
            lineage.parent,
            Some(uuid("11111111-1111-4111-8111-111111111111"))
        );

        // No session record and no tokens: identity and lineage unavailable.
        let monitor = observation.agent_on_pane("wA:p5").expect("monitor agent");
        assert_eq!(monitor.name.as_deref(), Some("herdr-radar"));
        assert_eq!(monitor.status, Some(RuntimeStatus::Done));
        assert_eq!(monitor.session, None);
        assert_eq!(monitor.lineage, None);

        // Explicit id-kind session becomes a UUID identity; a path-like
        // lineage token in the same record is rejected as lineage.
        let reviewer = observation.agent_on_pane("wB:p1").expect("reviewer agent");
        assert_eq!(
            reviewer.session,
            Some(SessionIdentity::Uuid(uuid(
                "55555555-5555-4555-8555-555555555555"
            )))
        );
        assert_eq!(reviewer.lineage, None);
    }

    #[test]
    fn empty_inventory_is_a_successful_observation() {
        let observation = decode_snapshot(EMPTY).expect("empty fixture decodes");
        assert!(observation.workspaces.is_empty());
        assert!(observation.tabs.is_empty());
        assert!(observation.panes.is_empty());
        assert!(observation.agents.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let output = r#"{
            "id": "cli:api:snapshot",
            "future_top_level": {"nested": [1, 2, 3]},
            "result": {
                "type": "snapshot",
                "snapshot": {
                    "protocol": 99,
                    "version": "9.9.9",
                    "layouts": [{"totally": "unknown"}],
                    "workspaces": [],
                    "tabs": [],
                    "panes": [{"pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                               "label": null, "future_pane_field": "x"}],
                    "agents": []
                }
            }
        }"#;
        let observation = decode_snapshot(output).expect("unknown fields do not break decoding");
        assert_eq!(observation.panes.len(), 1);
    }

    #[test]
    fn malformed_required_inventory_is_rejected() {
        let cases: Vec<(&str, String)> = vec![
            ("not JSON at all", "definitely not json".to_string()),
            ("missing result", "{}".to_string()),
            (
                "missing snapshot",
                r#"{"id":"x","result":{"type":"snapshot"}}"#.to_string(),
            ),
            ("panes not an array", snapshot_body(r#""oops""#, "[]")),
            (
                "pane record without pane_id",
                snapshot_body(r#"[{"tab_id":"wA:t1","workspace_id":"wA"}]"#, "[]"),
            ),
            (
                "agent record without tab_id",
                snapshot_body("[]", r#"[{"pane_id":"wA:p1","workspace_id":"wA"}]"#),
            ),
            (
                "empty pane id",
                snapshot_body(
                    r#"[{"pane_id":"","tab_id":"wA:t1","workspace_id":"wA"}]"#,
                    "[]",
                ),
            ),
        ];
        for (name, output) in cases {
            assert!(
                decode_snapshot(&output).is_err(),
                "expected rejection: {name}"
            );
        }
    }

    #[test]
    fn unknown_runtime_status_is_preserved() {
        let output = snapshot_body(
            "[]",
            r#"[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA","agent_status":"replaying"}]"#,
        );
        let observation = decode_snapshot(&output).expect("unknown status still decodes");
        let status = observation.agents[0].status.clone().expect("status");
        assert_eq!(status, RuntimeStatus::Other("replaying".into()));
        assert_eq!(status.to_string(), "replaying");
    }

    #[test]
    fn missing_runtime_status_is_unavailable() {
        let output = snapshot_body(
            "[]",
            r#"[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA"}]"#,
        );
        let observation = decode_snapshot(&output).expect("status-less agent still decodes");
        assert_eq!(observation.agents[0].status, None);
    }

    #[test]
    fn herdsman_tokens_decode_into_the_published_facts() {
        let fixture = herdsman_fixture();
        let expected = &fixture["expected"];
        let observation = decode_snapshot(&snapshot_of(&expected["agent_list"]["agents"]))
            .expect("the fixture's flattened records decode");

        // A lead: a role, the session's model facts and an awaited worker.
        // Every key the fixture cleared stays unavailable.
        let lead = observation.agent_on_pane("fixture:p1").expect("lead agent");
        assert_eq!(lead.facts.role.as_deref(), Some("lead"));
        assert_eq!(lead.facts.label, None);
        assert_eq!(lead.facts.name, None);
        assert_eq!(lead.facts.ask, None);
        assert_eq!(lead.facts.run, None);
        assert_eq!(lead.facts.request, None);
        assert_eq!(lead.facts.assignment, None);
        assert_eq!(lead.facts.assigned_started_unix_ms, None);
        assert_eq!(lead.facts.assigned_for, None);
        assert_eq!(lead.facts.state, None);
        assert_eq!(lead.facts.awaited, vec!["agent:researcher-1".to_string()]);
        assert_eq!(lead.facts.background_running, None);
        assert!(lead.facts.background_tasks.is_empty());
        assert_eq!(lead.facts.background_started, None);
        assert_eq!(lead.facts.model.as_deref(), Some("example/model"));
        assert_eq!(lead.facts.provider.as_deref(), Some("example"));
        assert_eq!(lead.facts.thinking.as_deref(), Some("high"));
        assert_eq!(lead.facts.context_usage.as_deref(), Some("43%"));
        assert_eq!(lead.facts.session_name, None);
        assert_eq!(lead.facts.definition, None);
        assert_eq!(
            lead.lineage,
            Some(Lineage {
                session: uuid("11111111-1111-4111-8111-111111111111"),
                parent: None,
            })
        );

        // A managed worker: its runtime label and run, the owner's projection,
        // and the background facts its pane's other publisher reports.
        let worker = observation
            .agent_on_pane("fixture:p2")
            .expect("worker agent");
        assert_eq!(worker.facts.role.as_deref(), Some("researcher"));
        assert_eq!(worker.facts.label.as_deref(), Some("researcher-1"));
        assert_eq!(
            worker.facts.run.as_deref(),
            Some("22222222-aaaa-4aaa-8aaa-aaaaaaaaaaaa")
        );
        assert_eq!(worker.facts.state, Some(SemanticState::Blocked));
        assert_eq!(
            worker.facts.awaited,
            vec!["agent:worker-1".to_string(), "owner".to_string()]
        );
        assert_eq!(worker.facts.background_running, Some(2));
        assert_eq!(
            worker.facts.background_tasks,
            vec!["bg-2".to_string(), "bg-1".to_string()]
        );
        assert_eq!(
            worker.facts.background_started.as_deref(),
            Some("2026-01-01T00:00:00.000Z")
        );

        // A worker its owner reports lost, with nothing outstanding.
        let lost = observation.agent_on_pane("fixture:p3").expect("lost agent");
        assert_eq!(lost.facts.label.as_deref(), Some("worker-1"));
        assert_eq!(lost.facts.state, Some(SemanticState::Lost));
        assert!(lost.facts.awaited.is_empty());
        assert_eq!(lost.facts.background_running, None);
        assert_eq!(lost.status, Some(RuntimeStatus::Unknown));

        // The ownership the fixture expects: each worker's parent session is
        // the session its owner's pane reports, matched by exact identity.
        let session_of = |pane_id: &str| {
            observation
                .agent_on_pane(pane_id)
                .and_then(|agent| agent.lineage.as_ref())
                .map(|lineage| lineage.session.clone())
        };
        let parents = expected["parents"].as_object().expect("expected parents");
        for (pane_id, owner_pane_id) in parents {
            let owner_pane_id = owner_pane_id.as_str().expect("a pane id");
            let parent = observation
                .agent_on_pane(pane_id)
                .and_then(|agent| agent.lineage.as_ref())
                .and_then(|lineage| lineage.parent.clone());
            assert_eq!(parent, session_of(owner_pane_id), "{pane_id} ownership");
        }

        // The states the fixture expects the owners to have projected. The
        // projection is a fact Radar decodes and shows; the row's state word is
        // derived from the pane instead.
        for (pane_id, state) in expected["owner_states"].as_object().expect("states") {
            let agent = observation.agent_on_pane(pane_id).expect("projected agent");
            let projected = agent.facts.state.as_ref().expect("a projected state");
            assert_eq!(
                projected.word(),
                state.as_str().expect("a state word"),
                "{pane_id} projection"
            );
        }
    }

    #[test]
    fn an_expired_owner_state_falls_back_to_the_derived_activity() {
        let fixture = herdsman_fixture();
        let expired = &fixture["expected"]["owner_source_expired"];
        let observation = decode_snapshot(&snapshot_of(&expired["agents"]))
            .expect("expired-owner records decode");

        // The owner's key is gone, so no projection is claimed; the state a row
        // presents is the one the contract derives without it.
        for (pane_id, state) in expired["consumer_fallback"].as_object().expect("fallback") {
            let agent = observation.agent_on_pane(pane_id).expect("agent");
            assert_eq!(agent.facts.state, None, "{pane_id} has no projection");
            assert_eq!(agent.state().word(), state.as_str().expect("a state word"));
        }
    }

    #[test]
    fn malformed_parent_token_only_drops_the_ownership_link() {
        let output = snapshot_body(
            "[]",
            r#"[{
                "pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                "tokens": {
                    "pi_herdsman_session": "11111111-1111-4111-8111-111111111111",
                    "pi_herdsman_parent_session": "/home/dev/sessions/owner.jsonl"
                }
            }]"#,
        );
        let observation = decode_snapshot(&output).expect("decodes");
        let lineage = observation.agents[0]
            .lineage
            .clone()
            .expect("session lineage");
        assert_eq!(
            lineage.session,
            uuid("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(lineage.parent, None);
    }

    #[test]
    fn unreadable_values_are_unavailable_rather_than_fatal() {
        let output = snapshot_body(
            "[]",
            r#"[{
                "pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                "agent_status": "idle",
                "tokens": {
                    "pi_herdsman_state": "replaying",
                    "pi_herdsman_role": "worker",
                    "pi_herdsman_task": "",
                    "pi_herdsman_awaited": "agent:one, ",
                    "pi_bg_running": "two",
                    "pi_bg_tasks": "task-1,,task-2",
                    "radar_future_token": "ignored"
                }
            }]"#,
        );
        let observation =
            decode_snapshot(&output).expect("unknown values do not fail the inventory");
        let agent = &observation.agents[0];
        // A state this version does not recognise is preserved as a fact, and
        // the row's state is derived from the pane instead: idle with a child
        // still outstanding is waiting, whatever the owner called it.
        assert_eq!(
            agent.facts.state,
            Some(SemanticState::Other("replaying".into()))
        );
        assert_eq!(agent.state().word(), "waiting");
        // A count this version cannot read is absent, not a failed inventory.
        assert_eq!(agent.facts.background_running, None);
        assert_eq!(
            agent.facts.background_tasks,
            vec!["task-1".to_string(), "task-2".to_string()]
        );
        // A cleared key is unavailable, and unknown keys are ignored.
        assert_eq!(agent.facts.assignment, None);
        assert_eq!(agent.facts.awaited, vec!["agent:one".to_string()]);
        assert_eq!(agent.facts.role.as_deref(), Some("worker"));
    }

    #[test]
    fn an_absent_or_empty_list_token_yields_no_items() {
        let output = snapshot_body(
            "[]",
            r#"[{"pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                "tokens": {"pi_herdsman_awaited": "", "pi_bg_tasks": ""}}]"#,
        );
        let observation = decode_snapshot(&output).expect("decodes");
        let facts = &observation.agents[0].facts;
        assert!(facts.awaited.is_empty());
        assert!(facts.background_tasks.is_empty());
        assert!(!facts.is_awaited());
    }

    #[test]
    fn definition_and_assignment_start_come_from_their_own_sources() {
        // A real snapshot record: the display agent a pane declares is where
        // Herdsman publishes a worker's definition, and the assignment start
        // arrives as a decimal string. The age belongs to the collector, so the
        // decode leaves it unmeasured, and a start it cannot read is absent.
        let output = snapshot_body(
            "[]",
            r#"[{"pane_id": "wA:p1", "tab_id": "wA:t1", "workspace_id": "wA",
                 "display_agent": "worker",
                 "tokens": {"pi_herdsman_role": "worker",
                            "pi_herdsman_started": "1791204434043"}},
                {"pane_id": "wA:p2", "tab_id": "wA:t1", "workspace_id": "wA",
                 "tokens": {"pi_herdsman_started": "soon"}}]"#,
        );
        let observation = decode_snapshot(&output).expect("decodes");
        let worker = observation.agent_on_pane("wA:p1").expect("worker");
        assert_eq!(worker.facts.definition.as_deref(), Some("worker"));
        assert_eq!(
            worker.facts.assigned_started_unix_ms,
            Some(1_791_204_434_043)
        );
        assert_eq!(
            worker.facts.assigned_for, None,
            "the collector measures the age"
        );

        let unreadable = observation
            .agent_on_pane("wA:p2")
            .expect("unreadable start");
        assert_eq!(unreadable.facts.assigned_started_unix_ms, None);
        assert_eq!(unreadable.facts.definition, None);
    }

    #[test]
    fn a_pane_with_nothing_running_and_five_review_tasks_reads_waiting() {
        // The live shape: the count reports nothing running while five tasks
        // have exited into review, and their starts are published.
        let output = snapshot_body(
            "[]",
            r#"[{"pane_id":"wA:p1","tab_id":"wA:t1","workspace_id":"wA","agent_status":"idle","tokens":{"pi_bg_running":"0","pi_bg_tasks":"bg-2286:review,bg-2284:review,bg-2283:review,bg-2282:review,bg-2281:review","pi_bg_started":"2026-01-01T00:00:00.000Z"}}]"#,
        );
        let observation = decode_snapshot(&output).expect("decodes");
        let agent = observation.agent_on_pane("wA:p1").expect("agent");

        assert_eq!(agent.facts.background_running, Some(0));
        assert_eq!(agent.facts.background_task_ids().len(), 5);
        assert_eq!(agent.activity_state().word(), "waiting");
    }

    fn process_info(body: &str) -> String {
        format!(r#"{{"id":"cli:pane:process_info","result":{{"process_info":{body}}}}}"#)
    }

    #[test]
    fn shell_foreground_pid_evidence_is_shell() {
        let output = process_info(
            r#"{"pane_id":"wA:p1","shell_pid":100,"foreground_process_group_id":100,
                "foreground_processes":[{"pid":100,"name":"zsh","cmdline":"zsh"}]}"#,
        );
        assert_eq!(
            decode_process_info(&output).expect("decodes"),
            ForegroundEvidence::Shell
        );
    }

    #[test]
    fn non_shell_foreground_leader_is_positive_replacement_evidence() {
        let output = process_info(
            r#"{"pane_id":"wA:p1","shell_pid":100,"foreground_process_group_id":200,
                "foreground_processes":[{"pid":200,"name":"nvim","cmdline":"nvim file"},
                                        {"pid":201,"name":"node","cmdline":"node server.js"}]}"#,
        );
        assert_eq!(
            decode_process_info(&output).expect("decodes"),
            ForegroundEvidence::command(200, Some("nvim".into()), Some("nvim file".into()))
        );
    }

    #[test]
    fn the_foreground_command_reads_as_it_was_invoked() {
        let store_path = "/nix/store/rvgk-pi-1.0.0/libexec/pi/pi -c";
        let evidence = ForegroundEvidence::command(200, Some("pi".into()), Some(store_path.into()));
        // The executable's own path says where the program lives, not what it
        // is doing.
        assert_eq!(evidence.command_line().as_deref(), Some("pi -c"));
        assert_eq!(ForegroundEvidence::Shell.command_line(), None);
        // A script under an interpreter is the script.
        let wrapper = ForegroundEvidence::command(
            202,
            Some("lazyslurm".into()),
            Some("/path/uv/bin/python /home/u/.local/bin/lazyslurm --remote build".into()),
        );
        assert_eq!(
            wrapper.command_line().as_deref(),
            Some("lazyslurm --remote build")
        );
        // A bare program with no arguments is just its name.
        let bare = ForegroundEvidence::command(201, None, Some("/usr/bin/nvim".into()));
        assert_eq!(bare.command_line().as_deref(), Some("nvim"));
    }

    #[test]
    fn missing_or_inconsistent_pids_are_inconclusive() {
        let missing_shell = process_info(
            r#"{"pane_id":"wA:p1","foreground_process_group_id":200,
                "foreground_processes":[{"pid":200,"name":"nvim"}]}"#,
        );
        assert_eq!(
            decode_process_info(&missing_shell).expect("decodes"),
            ForegroundEvidence::Inconclusive
        );

        let missing_group = process_info(r#"{"pane_id":"wA:p1","shell_pid":100}"#);
        assert_eq!(
            decode_process_info(&missing_group).expect("decodes"),
            ForegroundEvidence::Inconclusive
        );

        // The group leader is already gone: nothing can be positively named.
        let leader_gone = process_info(
            r#"{"pane_id":"wA:p1","shell_pid":100,"foreground_process_group_id":300,
                "foreground_processes":[{"pid":99,"name":"tail"}]}"#,
        );
        assert_eq!(
            decode_process_info(&leader_gone).expect("decodes"),
            ForegroundEvidence::Inconclusive
        );
    }

    #[test]
    fn malformed_process_info_is_rejected() {
        assert!(decode_process_info("not json").is_err());
        assert!(decode_process_info(r#"{"id":"x","result":{}}"#).is_err());
    }
}
