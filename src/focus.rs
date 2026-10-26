//! Where a row would focus, derived from the tree and the observation.
//!
//! This is a decision, not an effect: the session asks it when a focus command
//! names a row, and the daemon performs the move. How a request is transported —
//! the CLI invocation, socket discovery, the wire request, its deadline and its
//! child cleanup — belongs to the runtime adapter behind
//! [`RuntimeProvider`](crate::runtime::RuntimeProvider); this module holds none
//! of it and passes a normalized [`Target`] across the seam.

use std::collections::{HashMap, HashSet};

use crate::observation::ObservationState;
use crate::runtime::Target;
use crate::tree::{FleetTree, RowId, RowKind, TreeNode};

/// The focus target of every row the current observation proves.
///
/// A row keeps its own location: agent and pane rows focus their pane,
/// workspace rows focus their workspace. A row whose location the observation
/// no longer places — a retained row whose pane is gone, a workspace only
/// retained facts name — gets no entry, which is what makes `Enter` refuse
/// instead of focusing something Radar cannot point at.
pub fn targets(tree: &FleetTree, state: &ObservationState) -> HashMap<RowId, Target> {
    let Some(inventory) = state.inventory() else {
        return HashMap::new();
    };
    let panes: HashSet<&str> = inventory
        .panes
        .iter()
        .map(|pane| pane.location.pane_id.as_str())
        .chain(
            inventory
                .agents
                .iter()
                .map(|agent| agent.location.pane_id.as_str()),
        )
        .collect();
    let workspaces: HashSet<&str> = inventory
        .workspaces
        .iter()
        .map(|workspace| workspace.workspace_id.as_str())
        .chain(
            inventory
                .panes
                .iter()
                .map(|pane| pane.location.workspace_id.as_str()),
        )
        .chain(
            inventory
                .agents
                .iter()
                .map(|agent| agent.location.workspace_id.as_str())
                // An agent that reports no workspace names none to focus.
                .filter(|workspace_id| !workspace_id.is_empty()),
        )
        .collect();

    fn walk(
        nodes: &[TreeNode],
        panes: &HashSet<&str>,
        workspaces: &HashSet<&str>,
        targets: &mut HashMap<RowId, Target>,
    ) {
        for node in nodes {
            let target = match &node.row.kind {
                RowKind::Workspace { workspace_id, .. } if workspaces.contains(&**workspace_id) => {
                    Some(Target::Workspace(workspace_id.clone()))
                }
                RowKind::Agent(agent) if agent.joined() && panes.contains(&*agent.pane_id) => {
                    Some(Target::Pane(agent.pane_id.clone()))
                }
                RowKind::Pane(pane) if panes.contains(&*pane.pane_id) => {
                    Some(Target::Pane(pane.pane_id.clone()))
                }
                _ => None,
            };
            if let Some(target) = target {
                targets.insert(node.row.id.clone(), target);
            }
            walk(&node.children, panes, workspaces, targets);
        }
    }

    let mut targets = HashMap::new();
    walk(&tree.roots, &panes, &workspaces, &mut targets);
    targets
}
