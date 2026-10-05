//! Workspace and ownership projection: normalized observations → display rows.
//!
//! [`FleetTree::build`] turns the observation surface into display-ready
//! rows: one group per Herdr workspace, agent rows nested under unambiguous
//! same-workspace owners (any tab), and at most one ordinary-pane row per
//! pane. Tabs stay location detail on the rows rather than a tree level.
//!
//! Ownership fallbacks (spec: *Workspace-grouped fleet tree*): links that are
//! missing, ambiguous (two agents claiming one session UUID), cyclic, or
//! resolved outside the workspace leave the affected agent visible at its
//! physical workspace root instead of nesting unsafely. Lineage is consulted
//! here only as already-decoded UUIDs — presentation never sees tokens.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::model::{AgentObservation, ForegroundEvidence, Lineage, Pane, TerminalMode};
use crate::observation::{ObservationState, RetentionBasis};

/// A finished session, as its title survives on a pane's label.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExitedSession {
    /// The session title as displayed: the runtime latched it, the provider's
    /// prefix is the mark's job, and the row draws the mark.
    pub title: String,
    /// The vendor mark that prefix carried, when Radar recognises the provider.
    pub mark: Option<&'static str>,
}

/// Stable identity of a projected row.
///
/// Selection and fold state are keyed by this identity so refreshes that
/// change facts — or insert/remove rows elsewhere — preserve them wherever
/// the row survives.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowId {
    Workspace(String),
    /// Agent row, keyed by its connector-scoped pane id.
    Agent(String),
    /// Ordinary pane row, keyed by its connector-scoped pane id.
    Pane(String),
}

/// A display-ready row: everything the tree needs to place it and the detail
/// panel needs to explain it. Carries normalized facts only — no source
/// shapes, no metadata tokens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub id: RowId,
    pub kind: RowKind,
}

impl TreeRow {
    /// Displayed label: workspace label → id, otherwise the row's own title.
    pub fn title(&self) -> &str {
        match &self.kind {
            RowKind::Workspace {
                workspace_id,
                label,
                ..
            } => label.as_deref().unwrap_or(workspace_id),
            RowKind::Agent(agent) => &agent.title,
            RowKind::Pane(pane) => &pane.title,
        }
    }

    /// Case-insensitive match against the displayed label and any available
    /// role/assignment text. `needle_lower` must already be lowercase.
    pub fn matches(&self, needle_lower: &str) -> bool {
        let role_and_assignment = match &self.kind {
            RowKind::Agent(agent) => [
                agent.facts.role.as_deref(),
                agent.facts.assignment.as_deref(),
            ],
            _ => [None, None],
        };
        std::iter::once(Some(self.title()))
            .chain(role_and_assignment)
            .flatten()
            .any(|text| text.to_lowercase().contains(needle_lower))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowKind {
    Workspace {
        workspace_id: String,
        label: Option<String>,
        number: Option<u32>,
    },
    /// Both row kinds are boxed: an agent row carries every fact the source
    /// published, and the tree moves rows by value.
    Agent(Box<AgentRow>),
    Pane(Box<PaneRow>),
}

/// An agent row: current facts, or the last-observed facts of a retained
/// association (marked via `retained`, never presented as current).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentRow {
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_label: Option<String>,
    pub tab_id: String,
    pub tab_label: Option<String>,
    /// The name the row shows: a managed worker's published runtime label, a
    /// lead's published session name, or the reported title when the source
    /// publishes neither.
    pub title: String,
    /// Reported agent name (for example `pi`), if any.
    pub name: Option<String>,
    /// Runtime lifecycle as reported; never a semantic control state.
    pub status: Option<crate::model::RuntimeStatus>,
    /// The state this row presents: the activity state derived from the
    /// pane's own facts, per Herdsman's contract.
    pub state: crate::model::AgentState,
    /// `Some` when this row shows retained last-observed facts; the basis
    /// records why the association is believed to be at rest (or not).
    pub retained: Option<RetentionBasis>,
    pub session: Option<crate::model::SessionIdentity>,
    /// The exact session UUID the source published as this agent's ownership
    /// lineage (`pi_herdsman_session`), when it published one. It is the
    /// identity a bus publisher's `hello` is joined by, and is never derived
    /// from a path, a human name or a pane's position.
    pub session_uuid: Option<String>,
    /// What Herdsman publishes about this agent, as decoded. Presentation
    /// draws these as they are: a fact the source does not publish is absent,
    /// never defaulted and never derived here.
    pub facts: crate::model::HerdsmanFacts,
}

/// An ordinary (non-agent) pane row, shown only when the user toggles
/// ordinary panes on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRow {
    pub pane_id: String,
    pub workspace_id: String,
    pub workspace_label: Option<String>,
    pub tab_id: String,
    pub tab_label: Option<String>,
    /// What the pane is now: its own label, or the terminal title when the
    /// label is a session that has gone. Falls back to the pane id when the
    /// title only repeats the workspace it already sits under.
    pub title: String,
    /// `Some` when the pane's own label is a finished session's title — a
    /// provider title on a pane the runtime reports no agent for. The row is
    /// that session, marked `exited`, because the name is the only thing
    /// linking the pane to the work that was done in it; `title` says what the
    /// pane is instead.
    pub exited: Option<ExitedSession>,
    /// What the source last reported about the pane's foreground. `None` until
    /// a refresh asked, which is why an absent command reads as unavailable
    /// rather than as "nothing is running".
    pub foreground: Option<ForegroundEvidence>,
}

impl PaneRow {
    /// The command occupying the pane, when one is: the difference between a
    /// pane someone is working in and a shell nobody is using.
    pub fn command(&self) -> Option<String> {
        self.foreground
            .as_ref()
            .and_then(ForegroundEvidence::command_line)
    }

    /// How long the foreground command has been running, when this machine knows.
    pub fn running_for(&self) -> Option<Duration> {
        self.foreground
            .as_ref()
            .and_then(|evidence| evidence.local().running_for)
    }

    /// Whether the foreground program has taken the terminal over.
    pub fn terminal(&self) -> TerminalMode {
        self.foreground
            .as_ref()
            .map_or(TerminalMode::Unknown, |evidence| evidence.local().terminal)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeNode {
    pub row: TreeRow,
    pub children: Vec<TreeNode>,
}

/// The projected fleet tree: workspace roots in source order, agent children
/// nested by unambiguous ownership, ordinary panes appended per workspace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FleetTree {
    pub roots: Vec<TreeNode>,
}

impl FleetTree {
    /// Projects the current observation surface into rows.
    ///
    /// Pending or unavailable sources (no inventory) project to an empty
    /// tree. Ordinary-pane rows are always projected here; hiding them by
    /// default is the presentation/interaction layer's decision.
    pub fn build(state: &ObservationState) -> FleetTree {
        let Some(inventory) = state.inventory() else {
            return FleetTree::default();
        };

        let workspace_labels: HashMap<&str, &str> = inventory
            .workspaces
            .iter()
            .filter_map(|w| Some((w.workspace_id.as_str(), w.label.as_deref()?)))
            .collect();
        let tab_labels: HashMap<&str, &str> = inventory
            .tabs
            .iter()
            .filter_map(|t| Some((t.tab_id.as_str(), t.label.as_deref()?)))
            .collect();
        let workspace_numbers: HashMap<&str, u32> = inventory
            .workspaces
            .iter()
            .filter_map(|w| Some((w.workspace_id.as_str(), w.number?)))
            .collect();

        // First agent record per pane wins, matching FleetObservation::agent_on_pane.
        let mut agent_by_pane: HashMap<&str, &AgentObservation> = HashMap::new();
        for agent in &inventory.agents {
            agent_by_pane
                .entry(agent.location.pane_id.as_str())
                .or_insert(agent);
        }

        let workspace_ids = workspace_id_order(state);

        // Phase 1: workspace roots with their agent rows (current, then
        // retained). `claimed` is global so a pane yields exactly one row in
        // the whole tree, whatever workspace its facts place it in.
        let mut claimed: HashSet<&str> = HashSet::new();
        let mut roots: Vec<TreeNode> = Vec::new();
        let mut workspace_index: HashMap<&str, usize> = HashMap::new();

        for workspace_id in &workspace_ids {
            let label = workspace_labels.get(workspace_id.as_str()).copied();
            let mut agents: Vec<(TreeNode, Option<Lineage>)> = Vec::new();

            for agent in &inventory.agents {
                if agent.location.workspace_id != *workspace_id {
                    continue;
                }
                let pane_id = agent.location.pane_id.as_str();
                if !claimed.insert(pane_id) {
                    continue; // duplicate agent record for an already-rowed pane
                }
                let pane = inventory.pane(pane_id);
                agents.push((
                    agent_node(agent, None, pane, &workspace_labels, &tab_labels),
                    agent.lineage.clone(),
                ));
            }

            for (pane_id, retained) in state.retained() {
                let agent = &retained.observation;
                if agent.location.workspace_id != *workspace_id {
                    continue;
                }
                if !claimed.insert(pane_id.as_str()) {
                    continue; // a current row already represents this pane
                }
                let pane = inventory.pane(pane_id);
                agents.push((
                    agent_node(
                        agent,
                        Some(retained.basis.clone()),
                        pane,
                        &workspace_labels,
                        &tab_labels,
                    ),
                    agent.lineage.clone(),
                ));
            }

            workspace_index.insert(workspace_id.as_str(), roots.len());
            roots.push(TreeNode {
                row: TreeRow {
                    id: RowId::Workspace(workspace_id.clone()),
                    kind: RowKind::Workspace {
                        workspace_id: workspace_id.clone(),
                        label: label.map(str::to_string),
                        number: workspace_numbers.get(workspace_id.as_str()).copied(),
                    },
                },
                children: nest(agents),
            });
        }

        // Phase 2: ordinary pane rows for panes no agent row claimed. Each
        // pane record appears at most once (first record wins).
        let mut pane_seen: HashSet<&str> = HashSet::new();
        for pane in &inventory.panes {
            let pane_id = pane.location.pane_id.as_str();
            if claimed.contains(pane_id) || !pane_seen.insert(pane_id) {
                continue;
            }
            let Some(&root_index) = workspace_index.get(pane.location.workspace_id.as_str()) else {
                continue; // workspace_id_order covers every pane workspace
            };
            roots[root_index].children.push(TreeNode {
                row: TreeRow {
                    id: RowId::Pane(pane_id.to_string()),
                    kind: RowKind::Pane(Box::new(PaneRow {
                        pane_id: pane_id.to_string(),
                        workspace_id: pane.location.workspace_id.clone(),
                        workspace_label: workspace_labels
                            .get(pane.location.workspace_id.as_str())
                            .map(|label| (*label).to_string()),
                        tab_id: pane.location.tab_id.clone(),
                        tab_label: tab_labels
                            .get(pane.location.tab_id.as_str())
                            .map(|label| (*label).to_string()),
                        // Herdr keeps the last title an agent set as the pane's
                        // label. When the pane reports no agent, that label is a
                        // finished session, not what the pane is now.
                        exited: pane
                            .label
                            .as_deref()
                            .filter(|label| crate::title::has_provider_prefix(label))
                            .map(|label| ExitedSession {
                                title: crate::title::display(label, None),
                                mark: crate::title::session_vendor(label),
                            }),
                        title: crate::title::pane_title(
                            pane.label.as_deref(),
                            pane.title.as_deref(),
                            workspace_labels
                                .get(pane.location.workspace_id.as_str())
                                .copied(),
                            pane_id,
                        ),
                        // The process view reads this; it is absent until the
                        // source has been asked about this pane.
                        foreground: state.foreground(pane_id).cloned(),
                    })),
                },
                children: Vec::new(),
            });
        }

        FleetTree { roots }
    }
}

/// Workspace groups in source order: reported workspaces first, then any
/// workspace id referenced only by panes, agents or retained observations.
fn workspace_id_order(state: &ObservationState) -> Vec<String> {
    let Some(inventory) = state.inventory() else {
        return Vec::new();
    };
    let mut order: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |id: &str, order: &mut Vec<String>, seen: &mut HashSet<String>| {
        if seen.insert(id.to_string()) {
            order.push(id.to_string());
        }
    };
    for workspace in &inventory.workspaces {
        push(&workspace.workspace_id, &mut order, &mut seen);
    }
    for pane in &inventory.panes {
        push(&pane.location.workspace_id, &mut order, &mut seen);
    }
    for agent in &inventory.agents {
        push(&agent.location.workspace_id, &mut order, &mut seen);
    }
    for retained in state.retained().values() {
        push(
            &retained.observation.location.workspace_id,
            &mut order,
            &mut seen,
        );
    }
    order
}

fn agent_node(
    agent: &AgentObservation,
    retained: Option<RetentionBasis>,
    pane: Option<&Pane>,
    workspace_labels: &HashMap<&str, &str>,
    tab_labels: &HashMap<&str, &str>,
) -> TreeNode {
    let location = &agent.location;
    // The name a row shows is what Herdsman publishes for this agent — a
    // managed worker's runtime label, a lead's session name — and only then
    // the reported title. Herdr writes the vendor's own mark and name into
    // that title and the row already draws the mark beside it, so only the
    // words are kept.
    let raw = agent
        .facts
        .display_name()
        .map(str::to_string)
        .or_else(|| agent.label.clone())
        .or_else(|| agent.name.clone())
        .or_else(|| pane.and_then(|pane| pane.display_name().map(str::to_string)))
        .unwrap_or_else(|| location.pane_id.clone());
    let displayed = crate::title::display(&raw, agent.name.as_deref());
    let title = crate::title::strip_workspace_suffix(
        &displayed,
        workspace_labels
            .get(location.workspace_id.as_str())
            .copied(),
    )
    .to_string();
    TreeNode {
        row: TreeRow {
            id: RowId::Agent(location.pane_id.clone()),
            kind: RowKind::Agent(Box::new(AgentRow {
                pane_id: location.pane_id.clone(),
                workspace_id: location.workspace_id.clone(),
                workspace_label: workspace_labels
                    .get(location.workspace_id.as_str())
                    .map(|label| (*label).to_string()),
                tab_id: location.tab_id.clone(),
                tab_label: tab_labels
                    .get(location.tab_id.as_str())
                    .map(|label| (*label).to_string()),
                title,
                name: agent.name.clone(),
                status: agent.status.clone(),
                state: agent.state(),
                retained,
                session: agent.session.clone(),
                session_uuid: agent
                    .lineage
                    .as_ref()
                    .map(|lineage| lineage.session.as_str().to_string()),
                facts: agent.facts.clone(),
            })),
        },
        children: Vec::new(),
    }
}

/// Nests agent rows under unambiguous same-workspace owners and returns the
/// rest at the workspace root, in original row order.
///
/// A link is used only when exactly one agent in the workspace claims the
/// parent session UUID and the link is not self-referential; cycles are then
/// broken by dropping every edge inside the cycle (all members stay visible
/// as roots, per the spec fallback).
fn nest(mut agents: Vec<(TreeNode, Option<Lineage>)>) -> Vec<TreeNode> {
    let count = agents.len();
    if count == 0 {
        return Vec::new();
    }

    // Session UUIDs identify agents uniquely; the map is keyed by their
    // canonical text so no ordering/hashing derive is needed on the id type.
    let mut claims: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, (_, lineage)) in agents.iter().enumerate() {
        if let Some(lineage) = lineage {
            claims
                .entry(lineage.session.as_str())
                .or_default()
                .push(index);
        }
    }

    let mut parent: Vec<Option<usize>> = vec![None; count];
    for (index, (_, lineage)) in agents.iter().enumerate() {
        let Some(parent_session) = lineage.as_ref().and_then(|lineage| lineage.parent.as_ref())
        else {
            continue;
        };
        if let Some(owners) = claims.get(parent_session.as_str())
            && owners.len() == 1
            && owners[0] != index
        {
            parent[index] = Some(owners[0]);
        }
    }
    remove_cycles(&mut parent);

    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut roots: Vec<usize> = Vec::new();
    for (index, owner) in parent.iter().enumerate() {
        match owner {
            Some(owner) => children_of[*owner].push(index),
            None => roots.push(index),
        }
    }

    let mut slots: Vec<Option<TreeNode>> = agents.drain(..).map(|(node, _)| Some(node)).collect();
    fn assemble(
        index: usize,
        slots: &mut [Option<TreeNode>],
        children_of: &mut [Vec<usize>],
    ) -> TreeNode {
        let mut node = slots[index].take().expect("each node assembled once");
        for child in std::mem::take(&mut children_of[index]) {
            node.children.push(assemble(child, slots, children_of));
        }
        node
    }
    roots
        .into_iter()
        .map(|root| assemble(root, &mut slots, &mut children_of))
        .collect()
}

/// Drops every parent edge that lies on a cycle (DFS gray-edge detection).
///
/// Iterative rather than recursive: a pathological source could report a very
/// long ownership chain.
fn remove_cycles(parent: &mut [Option<usize>]) {
    const UNVISITED: u8 = 0;
    const ON_WALK: u8 = 1;
    const DONE: u8 = 2;

    let mut color = vec![UNVISITED; parent.len()];
    let mut path: Vec<usize> = Vec::new();
    for start in 0..parent.len() {
        if color[start] != UNVISITED {
            continue;
        }
        // Walk the parent chain, colouring as we go.
        let mut node = start;
        loop {
            color[node] = ON_WALK;
            path.push(node);
            let Some(owner) = parent[node] else { break };
            match color[owner] {
                UNVISITED => node = owner,
                ON_WALK => {
                    // `owner` is on this walk: every edge from it back to here
                    // closes a cycle. Drop them so those members stay visible
                    // as flat rows instead of nesting unsafely.
                    let start = path
                        .iter()
                        .position(|&entry| entry == owner)
                        .expect("owner is on the walk");
                    for &member in &path[start..] {
                        parent[member] = None;
                    }
                    break;
                }
                _ => break,
            }
        }
        for &member in &path {
            color[member] = DONE;
        }
        path.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::decode_snapshot;
    use crate::model::{
        FleetObservation, Location, RuntimeStatus, SessionIdentity, SessionUuid, Tab, Workspace,
    };

    const REAL_SHAPED: &str = include_str!("../tests/fixtures/snapshot_real_shaped.json");

    /// A canonical UUID derived from a small integer, so tests never embed
    /// UUID-shaped literals (which secret scanners mangle on write).
    fn u(n: u32) -> SessionUuid {
        SessionUuid::parse(&format!("{n:08x}-{n:04x}-4{n:03x}-8{n:03x}-{n:012x}"))
            .expect("synthetic canonical UUID")
    }

    fn fixture_state() -> ObservationState {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        state
    }

    /// (depth, row id, title) for every node in source order.
    fn flatten(tree: &FleetTree) -> Vec<(usize, RowId, String)> {
        fn walk(nodes: &[TreeNode], depth: usize, out: &mut Vec<(usize, RowId, String)>) {
            for node in nodes {
                out.push((depth, node.row.id.clone(), node.row.title().to_string()));
                walk(&node.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&tree.roots, 0, &mut out);
        out
    }

    fn node<'a>(tree: &'a FleetTree, id: &RowId) -> &'a TreeNode {
        fn find<'a>(nodes: &'a [TreeNode], id: &RowId) -> Option<&'a TreeNode> {
            for node in nodes {
                if &node.row.id == id {
                    return Some(node);
                }
                if let Some(found) = find(&node.children, id) {
                    return Some(found);
                }
            }
            None
        }
        find(&tree.roots, id).expect("row present")
    }

    #[test]
    fn nests_owner_across_tabs_and_keeps_tab_detail() {
        let tree = FleetTree::build(&fixture_state());

        let rows = flatten(&tree);
        // Two workspace roots.
        assert_eq!(
            rows.iter()
                .filter(|(depth, ..)| *depth == 0)
                .map(|(_, id, _)| id.clone())
                .collect::<Vec<_>>(),
            vec![RowId::Workspace("wA".into()), RowId::Workspace("wB".into())]
        );

        // The worker sits under the owner even though they occupy different
        // tabs, and its own tab stays on the row as location detail.
        let owner = node(&tree, &RowId::Agent("wA:p1".into()));
        assert!(
            owner
                .children
                .iter()
                .any(|child| child.row.id == RowId::Agent("wA:p2".into())),
            "worker nests under owner"
        );
        let worker = node(&tree, &RowId::Agent("wA:p2".into()));
        match &worker.row.kind {
            RowKind::Agent(agent) => {
                assert_eq!(agent.workspace_id, "wA");
                assert_eq!(agent.tab_id, "wA:t2");
                assert_eq!(agent.tab_label.as_deref(), Some("agent tab"));
            }
            other => panic!("expected agent row, got {other:?}"),
        }
        // The owner is not itself nested: its parent link is absent.
        assert_eq!(
            tree.roots[0].children[0].row.id,
            RowId::Agent("wA:p1".into())
        );
    }

    #[test]
    fn unavailable_ownership_stays_at_workspace_root() {
        let tree = FleetTree::build(&fixture_state());

        // The monitor agent reports no lineage: visible, flat, no fabrication.
        let monitor_parent = tree
            .roots
            .iter()
            .find_map(|root| {
                root.children
                    .iter()
                    .find(|child| child.row.id == RowId::Agent("wA:p5".into()))
            })
            .expect("monitor row exists");
        assert_eq!(monitor_parent.row.id, RowId::Agent("wA:p5".into()));
    }

    #[test]
    fn unsafe_ownership_links_fall_back_to_flat_rows() {
        // One workspace, four unsafe links plus one valid link for contrast.
        let session = |id: u32, parent: Option<u32>| Lineage {
            session: u(id),
            parent: parent.map(u),
        };
        let agent = |pane: &str, lineage: Option<Lineage>| AgentObservation {
            location: Location {
                workspace_id: "wX".into(),
                tab_id: "wX:t1".into(),
                pane_id: pane.into(),
            },
            name: Some("pi".into()),
            label: Some(format!("agent {pane}")),
            status: Some(RuntimeStatus::Idle),
            session: Some(SessionIdentity::Reported {
                source: None,
                value: format!("session-{pane}"),
            }),
            lineage,
            facts: crate::model::HerdsmanFacts::default(),
        };
        let observation = FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wX".into(),
                label: Some("mixed".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wX:t1".into(),
                workspace_id: "wX".into(),
                label: None,
                number: None,
            }],
            panes: ["p1", "p2", "p3", "p4", "p5", "p6", "p7"]
                .iter()
                .map(|pane| crate::model::Pane {
                    location: Location {
                        workspace_id: "wX".into(),
                        tab_id: "wX:t1".into(),
                        pane_id: format!("wX:{pane}"),
                    },
                    label: None,
                    title: None,
                })
                .collect(),
            agents: vec![
                // p1: parent UUID that no agent claims (absent owner).
                agent("wX:p1", Some(session(11, Some(90)))),
                // p2: parent claimed by two agents (ambiguous owner).
                agent("wX:p2", Some(session(12, Some(33)))),
                // p3 + p4: duplicate owner claimants for 33.
                agent("wX:p3", Some(session(33, None))),
                agent("wX:p4", Some(session(33, None))),
                // p5 <-> p6: ownership cycle.
                agent("wX:p5", Some(session(55, Some(66)))),
                agent("wX:p6", Some(session(66, Some(55)))),
                // p7: valid link onto p1's session — must still nest.
                agent("wX:p7", Some(session(77, Some(11)))),
            ],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let tree = FleetTree::build(&state);

        let root_ids: Vec<RowId> = tree.roots[0]
            .children
            .iter()
            .map(|child| child.row.id.clone())
            .collect();
        // Unsafe links stay visible without fabricated nesting…
        for flat in ["wX:p1", "wX:p2", "wX:p3", "wX:p4", "wX:p5", "wX:p6"] {
            assert!(
                root_ids.contains(&RowId::Agent(flat.into())),
                "{flat} must be a flat workspace-root row"
            );
        }
        // …while the one unambiguous link still nests (p7 under p1).
        let p1 = node(&tree, &RowId::Agent("wX:p1".into()));
        assert!(
            p1.children
                .iter()
                .any(|child| child.row.id == RowId::Agent("wX:p7".into()))
        );
    }

    #[test]
    fn owner_in_another_workspace_is_not_nested() {
        let worker_lineage = Lineage {
            session: u(2),
            parent: Some(u(99)),
        };
        let mut state = fixture_state();
        // Move the worker's ownership link to a session owned in workspace wB
        // (the reviewer's), which must not nest across workspaces.
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        let worker = observation
            .agents
            .iter_mut()
            .find(|agent| agent.location.pane_id == "wA:p2")
            .expect("worker");
        worker.lineage = Some(worker_lineage);
        // The reviewer in wB carries session 9999… as identity, not lineage;
        // give it lineage so an owner physically exists — in another workspace.
        let reviewer = observation
            .agents
            .iter_mut()
            .find(|agent| agent.location.pane_id == "wB:p1")
            .expect("reviewer");
        reviewer.lineage = Some(Lineage {
            session: u(99),
            parent: None,
        });
        state.apply_success(observation);
        let tree = FleetTree::build(&state);

        let w_a = tree
            .roots
            .iter()
            .find(|root| root.row.id == RowId::Workspace("wA".into()))
            .expect("wA");
        assert!(
            w_a.children
                .iter()
                .any(|child| child.row.id == RowId::Agent("wA:p2".into())),
            "worker with an out-of-workspace owner stays flat in its own workspace"
        );
        let w_b = tree
            .roots
            .iter()
            .find(|root| root.row.id == RowId::Workspace("wB".into()))
            .expect("wB");
        assert!(
            !w_b.children
                .iter()
                .any(|child| child.row.id == RowId::Agent("wA:p2".into()))
        );
    }

    #[test]
    fn ordinary_panes_project_once_and_never_alongside_agent_rows() {
        let tree = FleetTree::build(&fixture_state());
        let rows = flatten(&tree);

        let pane_rows: Vec<&(usize, RowId, String)> = rows
            .iter()
            .filter(|(_, id, _)| matches!(id, RowId::Pane(_)))
            .collect();
        // Only p3, p4 and p6 lack agent associations.
        let mut pane_ids: Vec<String> = pane_rows
            .iter()
            .map(|(_, id, _)| match id {
                RowId::Pane(id) => id.clone(),
                _ => unreachable!(),
            })
            .collect();
        pane_ids.sort();
        assert_eq!(pane_ids, vec!["wA:p3", "wA:p4", "wA:p6"]);

        // Every pane-backed row in the whole tree is unique: no pane ever
        // yields both an agent row and an ordinary-pane row.
        let mut seen_panes: HashSet<&str> = HashSet::new();
        for (_, id, _) in &rows {
            let pane = match id {
                RowId::Agent(pane) | RowId::Pane(pane) => Some(pane.as_str()),
                RowId::Workspace(_) => None,
            };
            if let Some(pane) = pane {
                assert!(seen_panes.insert(pane), "duplicate row for pane {pane}");
            }
        }
    }

    #[test]
    fn retained_association_projects_a_marked_agent_row_without_a_pane_duplicate() {
        let mut state = fixture_state();
        // The owner stops being reported while its pane stays.
        let mut without_owner = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        without_owner
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p1");
        state.apply_success(without_owner);
        state.apply_evidence("wA:p1", crate::model::ForegroundEvidence::Shell);

        let tree = FleetTree::build(&state);
        let retained = node(&tree, &RowId::Agent("wA:p1".into()));
        match &retained.row.kind {
            RowKind::Agent(agent) => {
                assert_eq!(agent.retained, Some(RetentionBasis::ShellForeground));
                assert_eq!(agent.status, Some(RuntimeStatus::Idle));
                assert_eq!(agent.title, "fleet owner task");
            }
            other => panic!("expected agent row, got {other:?}"),
        }
        // The worker still nests under the retained owner (uniform join).
        assert!(
            retained
                .children
                .iter()
                .any(|child| child.row.id == RowId::Agent("wA:p2".into()))
        );
        // And the pane yields no second, ordinary row.
        assert!(
            !flatten(&tree)
                .iter()
                .any(|(_, id, _)| *id == RowId::Pane("wA:p1".into()))
        );
    }

    #[test]
    fn duplicate_source_records_yield_single_rows() {
        let mut state = fixture_state();
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        // Duplicate pane record and duplicate agent record for the same pane.
        let pane = observation.panes[0].clone();
        observation.panes.push(pane);
        let agent = observation.agents[0].clone();
        observation.agents.push(agent);
        state.apply_success(observation);

        let rows = flatten(&FleetTree::build(&state));
        assert_eq!(
            rows.iter()
                .filter(|(_, id, _)| *id == RowId::Agent("wA:p1".into()))
                .count(),
            1
        );
        assert_eq!(
            rows.iter()
                .filter(|(_, id, _)| *id == RowId::Pane("wA:p3".into()))
                .count(),
            1
        );
    }

    #[test]
    fn pending_source_projects_an_empty_tree() {
        let state = ObservationState::new();
        assert_eq!(FleetTree::build(&state), FleetTree::default());
    }
}
