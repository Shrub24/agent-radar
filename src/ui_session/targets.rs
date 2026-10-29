//! What a destructive command acts on, decided once and frozen.
//!
//! The session derives the target from the row the invoking view displayed, so a
//! different selection in another view can never retarget the operation. The
//! frozen plan carries the request identity and the exact envelope the daemon
//! dispatches: the same one a control-plane client sends, so a retried answer can
//! never become a second effect.
//!
//! Eligibility reads evidence, never the cursor: a stale inventory or a
//! disappeared target refuses before a question is asked, a managed location is
//! routed to its owner rather than the runtime, and only positive unmanaged
//! evidence reaches a direct close.

use crate::control_plane::command::{Action as DomainAction, Command, Subject};
use crate::control_plane::containers::{DeleteRequest, ReloadRequest};
use crate::runtime::CreateRequest;

use crate::control_plane::containers::Kind;
use crate::lifecycle::{self, Containment};
use crate::model::{FleetObservation, Pane};
use crate::observation::{ObservationState, SourceFreshness};
use crate::registry_observation::ManagedSubject;
use crate::runtime::CloseTarget;
use crate::tree::{AgentRow, ContainerRef, PaneRow, RowKind};

const REQUESTER: &str = "radar";

/// The selected row as a lifecycle target, with what acting on it may lose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Selected {
    Agent {
        pane_id: String,
        /// The tab the row is the root of, when it is: ending that row ends the
        /// whole tab, because the row stands for it.
        tab: Option<String>,
        losses: Vec<String>,
        subject: Option<Box<ManagedSubject>>,
    },
    Pane {
        pane_id: String,
        /// The tab this row stands for, when it is the row's root.
        tab: Option<String>,
        losses: Vec<String>,
    },
    Refused(&'static str),
}

/// One frozen effect: what the daemon dispatches, under which request identity,
/// and what the operator is asked before it happens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// What the effect does. A focus is a runtime command the daemon runs; a
    /// domain request goes through admission under `request_id`. The two are
    /// mutually exclusive, so they are one value rather than a method and a
    /// possible focus beside it.
    pub effect: Effect,
    pub request_id: String,
    /// The question, naming the exact target.
    pub prompt: String,
    pub losses: Vec<String>,
    /// Whether the operator must agree first. A deletion that would lose no work
    /// is the one control that goes ahead without asking.
    pub confirms: bool,
}

/// What a frozen effect does when it reaches the daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A domain request, dispatched through admission under the plan's identity.
    Domain(Box<Command>),
    /// A runtime command that moves the client serving a location. Not a request
    /// record: nothing about it is durable, and the daemon is the only place the
    /// location is known.
    Focus(crate::runtime::Target),
}

/// Where a row is focused, from the tree and observation the session holds.
///
/// The map comes from the same derivation the dashboard used to run for itself:
/// focus is a decision about what the observation places, so it is taken where
/// that observation lives, and the frontend only asks for a row.
pub(crate) fn focus(
    tree: &crate::tree::FleetTree,
    state: &ObservationState,
    row: &crate::tree::RowId,
) -> Option<crate::runtime::Target> {
    // A task is not a location: what to focus is the pane its owner row names.
    let row = match row {
        crate::tree::RowId::Task(task) => &crate::tree::RowId::Agent(task.owner.clone()),
        other => other,
    };
    crate::focus::targets(tree, state).get(row).cloned()
}

/// What a row is, for a one-line message: `pane wA:p1`, `the tab wA:t1`.
pub(crate) fn describe(row: &crate::tree::RowId) -> String {
    match row {
        crate::tree::RowId::Workspace(id) => format!("workspace {id}"),
        crate::tree::RowId::Agent(pane) | crate::tree::RowId::Pane(pane) => format!("pane {pane}"),
        crate::tree::RowId::Registered(id) => format!("unresolved agent {id}"),
        crate::tree::RowId::Task(_) => "that task".to_string(),
        crate::tree::RowId::Lifecycle(id) => format!("lifecycle subject {id}"),
    }
}

/// The effect that focuses `target`: no question, because moving a terminal
/// takes nothing away.
pub(crate) fn focus_plan(target: crate::runtime::Target, request_id: String) -> Plan {
    Plan {
        effect: Effect::Focus(target.clone()),
        request_id,
        prompt: format!("Focus {}", target.description()),
        losses: Vec::new(),
        confirms: false,
    }
}

/// The selected row as an action target.
pub(crate) fn selection(kind: &RowKind, represents_tab: Option<&str>) -> Selected {
    match kind {
        RowKind::Agent(agent) if !agent.joined() => Selected::Refused(
            "an unresolved agent is not joined to a pane: its controls are refused",
        ),
        RowKind::Agent(agent) => Selected::Agent {
            pane_id: agent.pane_id.clone(),
            tab: represents_tab.map(str::to_string),
            losses: agent_losses(agent),
            subject: agent
                .registry
                .as_ref()
                .and_then(|view| view.managed.clone())
                .map(Box::new),
        },
        RowKind::Pane(pane) => Selected::Pane {
            pane_id: pane.pane_id.clone(),
            tab: represents_tab.map(str::to_string),
            losses: pane_losses(pane),
        },
        RowKind::Workspace { .. } => {
            Selected::Refused("a workspace is not a close target: close a pane or its tab")
        }
        RowKind::Task(_) => Selected::Refused("a background task is not a close target"),
        RowKind::Lifecycle(_) => {
            Selected::Refused("a lifecycle status entry is not a pane close target")
        }
    }
}

/// The container the selected row shows, when the daemon records one for it.
pub(crate) fn selected_container(kind: &RowKind) -> Option<&ContainerRef> {
    match kind {
        RowKind::Workspace { container, .. } => container.as_ref(),
        RowKind::Agent(agent) => agent.container.as_ref(),
        RowKind::Pane(pane) => pane.container.as_ref(),
        RowKind::Task(_) | RowKind::Lifecycle(_) => None,
    }
}

/// The tab a row stands for, when it is that tab's own root row.
pub(crate) fn represents_tab(kind: &RowKind) -> Option<&str> {
    match kind {
        RowKind::Agent(agent) if agent.joined() && agent.represents_tab => Some(&agent.tab_id),
        RowKind::Pane(pane) if pane.represents_tab => Some(&pane.tab_id),
        _ => None,
    }
}

/// Freezes the effect `operation` has on `selected`, or explains why it cannot be
/// acted on. Nothing is dispatched here.
pub(crate) fn plan(
    operation: crate::app::Operation,
    selected: Selected,
    state: &ObservationState,
    container: Option<&ContainerRef>,
    request_id: &str,
) -> Result<Plan, String> {
    use crate::app::Operation;
    let inventory = current(state)?;
    // A row the daemon records a container for is acted on as a container; every
    // other row is the place it shows.
    if operation.is_container()
        || (operation == Operation::Delete
            && !matches!(selected, Selected::Agent { .. })
            && container.is_some())
    {
        return container_plan(
            operation,
            container.ok_or("this row is not a container Radar records: nothing to act on")?,
            inventory,
            state,
            request_id,
        );
    }
    let described = match &selected {
        Selected::Agent { pane_id, .. } | Selected::Pane { pane_id, .. } => {
            CloseTarget::Pane(pane_id.clone()).description()
        }
        Selected::Refused(_) => String::new(),
    };
    match selected {
        Selected::Refused(reason) => Err(match operation {
            Operation::Restart => "restart is only for a managed worker".to_string(),
            _ => reason.to_string(),
        }),
        Selected::Agent {
            pane_id,
            tab,
            losses,
            subject,
        } => {
            if operation == Operation::Restart {
                let subject = subject.ok_or("this agent has no daemon lifecycle subject")?;
                if subject.state != crate::control_plane::lifecycle::State::Bound
                    || subject.session.is_none()
                {
                    return Err("this lifecycle subject is not restartable".into());
                }
                let mut losses = losses;
                losses.push(
                    "the verified process is stopped and resumed in its recorded session".into(),
                );
                return subject_plan(
                    Operation::Restart,
                    CloseTarget::Pane(pane_id),
                    subject,
                    losses,
                    inventory,
                    request_id,
                );
            }
            let managed = containment_of(inventory, state, &CloseTarget::Pane(pane_id.clone()))
                == Containment::Managed;
            let target = match tab {
                Some(tab_id) if !managed => CloseTarget::Tab(tab_id),
                _ => CloseTarget::Pane(pane_id),
            };
            close_plan(
                operation, target, losses, subject, inventory, state, request_id,
            )
        }
        Selected::Pane {
            pane_id,
            tab,
            losses,
        } => {
            let managed = containment_of(inventory, state, &CloseTarget::Pane(pane_id.clone()))
                == Containment::Managed;
            let target = match tab {
                Some(tab_id) if !managed => CloseTarget::Tab(tab_id),
                _ => CloseTarget::Pane(pane_id),
            };
            close_plan(
                operation, target, losses, None, inventory, state, request_id,
            )
        }
    }
    .map(|plan| Plan {
        prompt: format!(
            "{}{}?",
            verb(operation),
            if described.is_empty() {
                String::new()
            } else {
                described
            }
        ),
        ..plan
    })
}

fn current(state: &ObservationState) -> Result<&FleetObservation, String> {
    if !matches!(state.source_freshness(), SourceFreshness::Current) {
        return Err("the fleet is stale: nothing was sent".into());
    }
    state
        .inventory()
        .ok_or_else(|| "no current inventory: nothing was sent".to_string())
}

fn verb(operation: crate::app::Operation) -> &'static str {
    use crate::app::Operation;
    match operation {
        Operation::Delete => "Delete ",
        Operation::Restart => "Restart ",
        Operation::CreateContainer => "Create in ",
        Operation::ReloadContainer => "Reload ",
        Operation::PresenceStatus => "Set presence for ",
    }
}

/// The workspace a container lives in, which is where an addition goes: a new
/// tab belongs to a workspace whichever row the operator was on.
fn workspace_of(inventory: &FleetObservation, container: &ContainerRef) -> Option<String> {
    match container.kind {
        Kind::Workspace => Some(container.handle.clone()),
        Kind::Tab => inventory
            .tabs
            .iter()
            .find(|tab| tab.tab_id == container.handle)
            .map(|tab| tab.workspace_id.clone()),
        Kind::Pane => inventory
            .panes
            .iter()
            .find(|pane| pane.location.pane_id == container.handle)
            .map(|pane| pane.location.workspace_id.clone()),
    }
}

fn close_plan(
    operation: crate::app::Operation,
    target: CloseTarget,
    mut losses: Vec<String>,
    subject: Option<Box<ManagedSubject>>,
    inventory: &FleetObservation,
    state: &ObservationState,
    request_id: &str,
) -> Result<Plan, String> {
    let described = target.description();
    if !target_present(inventory, &target) {
        return Err(format!("{described} is gone: nothing was sent"));
    }
    let actual = containment_of(inventory, state, &target);
    if let Some(subject) = subject {
        match actual {
            Containment::Managed => {}
            Containment::Unmanaged => {
                return Err(format!(
                    "{described} is no longer managed: nothing was requested"
                ));
            }
            Containment::Uncertain => {
                return Err(format!(
                    "{described} can no longer be verified as managed: nothing was requested"
                ));
            }
        }
        return subject_plan(operation, target, subject, losses, inventory, request_id);
    }
    if let Some(reason) = containment_refusal(actual, &target) {
        return Err(reason);
    }
    losses.extend(losses_under(inventory, state, &target));
    let request = lifecycle::CloseRequest {
        target: target.clone(),
        identity: lifecycle::identity(inventory, &target),
    };
    Ok(Plan {
        effect: Effect::Domain(Box::new(Command::new(
            REQUESTER,
            DomainAction::Close(request),
        ))),
        request_id: request_id.to_string(),
        prompt: described,
        losses,
        confirms: true,
    })
}

fn subject_plan(
    operation: crate::app::Operation,
    target: CloseTarget,
    subject: Box<ManagedSubject>,
    losses: Vec<String>,
    inventory: &FleetObservation,
    request_id: &str,
) -> Result<Plan, String> {
    let described = target.description();
    if !lifecycle::matches(inventory, &lifecycle::identity(inventory, &target)) {
        return Err(format!("{described} changed: nothing was sent"));
    }
    let subject = Subject {
        subject_id: subject.subject_id,
        claim_id: subject.claim_id,
    };
    let action = match operation {
        crate::app::Operation::Restart => DomainAction::Restart(subject),
        _ => DomainAction::Cancel(subject),
    };
    Ok(Plan {
        effect: Effect::Domain(Box::new(Command::new(REQUESTER, action))),
        request_id: request_id.to_string(),
        prompt: described,
        losses,
        confirms: true,
    })
}

fn container_plan(
    operation: crate::app::Operation,
    container: &ContainerRef,
    inventory: &FleetObservation,
    state: &ObservationState,
    request_id: &str,
) -> Result<Plan, String> {
    use crate::app::Operation;
    let described = container_name(inventory, container);
    // A container whose place cannot be verified has nothing to freeze, so every
    // control but a deletion is refused. Deleting it is the record going, and the
    // daemon accepts that only when it cannot verify the place is still served.
    if let Some(reason) = &container.unresolved {
        if operation != Operation::Delete {
            return Err(format!(
                "{described} is unresolved ({reason}): nothing to act on"
            ));
        }
        return Ok(Plan {
            effect: Effect::Domain(Box::new(Command::new(
                REQUESTER,
                DomainAction::Delete(DeleteRequest {
                    container_id: container.container_id.clone(),
                    frozen: None,
                }),
            ))),
            request_id: request_id.to_string(),
            prompt: described,
            losses: Vec::new(),
            confirms: true,
        });
    }
    let target = container.kind.target(&container.handle);
    let running = losses_under(inventory, state, &target);
    let mut losses = vec![described.clone()];
    let request = match operation {
        Operation::Delete => {
            if running.is_empty() {
                losses.push("nothing is running in it".to_string());
            } else {
                losses.extend(running.iter().cloned());
            }
            DomainAction::Delete(DeleteRequest {
                container_id: container.container_id.clone(),
                frozen: Some(lifecycle::CloseRequest {
                    target: target.clone(),
                    identity: lifecycle::identity(inventory, &target),
                }),
            })
        }
        Operation::ReloadContainer => {
            if container.kind != Kind::Pane {
                return Err("only a pane slot runs a recipe: select a pane".into());
            }
            if !container.recipe {
                return Err(format!(
                    "container {} records no recipe: nothing to reload",
                    container.container_id
                ));
            }
            losses.push("the recorded recipe runs again in this pane".to_string());
            losses.push("a command of its own that has not exited refuses the reload".to_string());
            DomainAction::Reload(ReloadRequest {
                container_id: container.container_id.clone(),
            })
        }
        Operation::CreateContainer => {
            let workspace_id = workspace_of(inventory, container)
                .ok_or_else(|| format!("{described} is gone: nothing was sent"))?;
            DomainAction::Create {
                request: CreateRequest::Tab {
                    workspace_id,
                    focus: Some(false),
                },
                label: None,
                recipe: None,
            }
        }
        _ => return Err("that is not a container control".into()),
    };
    Ok(Plan {
        effect: Effect::Domain(Box::new(Command::new(REQUESTER, request))),
        request_id: request_id.to_string(),
        prompt: described,
        // Only an action that can end work asks. A deletion nothing is running
        // in, and an addition, lose nothing.
        confirms: match operation {
            Operation::Delete => !running.is_empty(),
            Operation::CreateContainer => false,
            _ => true,
        },
        losses,
    })
}

/// The launch a prepared spec asks for, against the selected container.
///
/// The frontend supplies its own shell and `PATH`; the place, the directory and
/// the created tab all come from the shared inventory, so the launch lands in the
/// workspace the invoking view showed.
pub(crate) fn launch_plan(
    container: Option<&ContainerRef>,
    spec: &crate::control_plane::public_contracts::ui_session::LaunchSpec,
    state: &ObservationState,
    request_id: &str,
) -> Result<Plan, String> {
    let inventory = current(state)?;
    let container = container.ok_or("select a workspace or pane to launch into")?;
    let recorded = inventory
        .containers
        .iter()
        .find(|item| item.container_id == container.container_id && item.removed_at.is_none())
        .ok_or("the selected container is no longer current")?;
    let binding = recorded
        .binding
        .as_ref()
        .filter(|binding| binding.handle == container.handle && recorded.unresolved.is_none())
        .ok_or("the selected container is unresolved")?;
    let (workspace_id, directory) = match recorded.kind {
        Kind::Workspace => (
            binding.handle.clone(),
            inventory
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == binding.handle)
                .and_then(|workspace| workspace.session_path.clone()),
        ),
        Kind::Pane => {
            let pane = inventory
                .panes
                .iter()
                .find(|pane| pane.location.pane_id == binding.handle)
                .ok_or("the selected pane is not in the current inventory")?;
            (
                pane.location.workspace_id.clone(),
                pane.current_path.clone(),
            )
        }
        Kind::Tab => return Err("select a workspace or pane to launch an agent".into()),
    };
    // A tab names no directory of its own, so the launch starts in its
    // workspace's, which is the project the reader selected.
    let directory = directory
        .or_else(|| {
            inventory
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .and_then(|workspace| workspace.session_path.clone())
        })
        .ok_or("the selected place has no current directory: nothing was sent")?;
    if spec.shell.trim().is_empty() {
        return Err("SHELL is unset; set it to fish, bash, or zsh".into());
    }
    let path = spec.path.as_deref().map(std::ffi::OsStr::new);
    let recipe =
        crate::topology::target_shell_recipe(&spec.command, &spec.shell, path, directory.as_str())?;
    Ok(Plan {
        effect: Effect::Domain(Box::new(Command::new(
            REQUESTER,
            DomainAction::Create {
                request: CreateRequest::Tab {
                    workspace_id,
                    focus: Some(false),
                },
                label: None,
                recipe: Some(recipe),
            },
        ))),
        request_id: request_id.to_string(),
        prompt: format!("Launch in {directory}"),
        losses: Vec::new(),
        // A launch adds a place; it takes nothing away, so it is not a question.
        confirms: false,
    })
}

/// Whether the target still exists in this inventory.
pub(crate) fn target_present(inventory: &FleetObservation, target: &CloseTarget) -> bool {
    match target {
        CloseTarget::Pane(pane_id) => inventory.pane(pane_id).is_some(),
        CloseTarget::Tab(tab_id) => inventory.tabs.iter().any(|tab| tab.tab_id == *tab_id),
        CloseTarget::Workspace(workspace_id) => inventory
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == *workspace_id),
    }
}

/// A close target's containment in the current observation, retained
/// associations included.
pub(crate) fn containment_of(
    inventory: &FleetObservation,
    state: &ObservationState,
    target: &CloseTarget,
) -> Containment {
    match target {
        CloseTarget::Pane(pane_id) => {
            lifecycle::pane(inventory, state.retained_containment(), pane_id)
        }
        CloseTarget::Tab(tab_id) => lifecycle::tab(inventory, state.retained_containment(), tab_id),
        CloseTarget::Workspace(workspace_id) => {
            lifecycle::workspace(inventory, state.retained_containment(), workspace_id)
        }
    }
}

/// The refusal a containment state gives a direct close, when it gives one.
pub(crate) fn containment_refusal(
    containment: Containment,
    target: &CloseTarget,
) -> Option<String> {
    let described = target.description();
    match containment {
        Containment::Unmanaged => None,
        Containment::Managed => Some(format!(
            "{described} is managed by its owner: direct close is not available"
        )),
        Containment::Uncertain => Some(format!(
            "{described} cannot be verified as unmanaged: not closing"
        )),
    }
}

/// A container named for a person: its label, or what the place is called. Its
/// record id is never shown, because it names nothing an operator recognises.
pub(crate) fn container_name(inventory: &FleetObservation, container: &ContainerRef) -> String {
    let kind = match container.kind {
        Kind::Workspace => "workspace",
        Kind::Tab => "tab",
        Kind::Pane => "pane",
    };
    let place = match container.kind {
        Kind::Workspace => inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == container.handle)
            .and_then(|workspace| workspace.label.clone()),
        Kind::Tab => inventory
            .tabs
            .iter()
            .find(|tab| tab.tab_id == container.handle)
            .and_then(|tab| tab.label.clone()),
        Kind::Pane => inventory
            .pane(&container.handle)
            .and_then(|pane| pane.display_name().map(str::to_string)),
    };
    match place.filter(|place| !place.is_empty()) {
        Some(place) => format!("{kind} {place}"),
        None => format!("this {kind}"),
    }
}

/// Everything ending this target would take with it, walked down from the level
/// asked about so one rule applies at every level. An agent mid-task, a non-shell
/// command in a pane, and a running background task are losses; idle shells,
/// idle agents and finished work are not.
pub(crate) fn losses_under(
    inventory: &FleetObservation,
    state: &ObservationState,
    target: &CloseTarget,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for pane in panes_in_scope(inventory, target) {
        let pane_id = pane.location.pane_id.as_str();
        if let Some(agent) = inventory
            .agents
            .iter()
            .find(|agent| agent.location.pane_id == pane_id)
        {
            if agent_busy(agent) {
                let name = agent
                    .label
                    .as_deref()
                    .or(agent.name.as_deref())
                    .unwrap_or("an agent");
                let state = agent
                    .status
                    .as_ref()
                    .map(|status| format!("{status:?}").to_lowercase())
                    .unwrap_or_else(|| "unknown".to_string());
                lines.push(format!("{name} is {state}"));
            }
            continue;
        }
        if let Some(crate::model::ForegroundEvidence::NonShell { name, command, .. }) =
            state.foreground(pane_id)
        {
            let what = name
                .as_deref()
                .or(command.as_deref())
                .unwrap_or("a command");
            lines.push(format!("{what} is running"));
        }
    }
    lines.extend(task_losses(inventory, target));
    lines.sort();
    lines.dedup();
    lines
}

/// Whether an agent is in the middle of work its row's end would lose. Only a
/// reported idle or finished agent holds nothing: a source that reported no state
/// has not said the work is done, so it is asked about.
fn agent_busy(agent: &crate::model::AgentObservation) -> bool {
    !matches!(
        agent.status,
        Some(crate::model::RuntimeStatus::Idle) | Some(crate::model::RuntimeStatus::Done)
    )
}

/// Background work still running in the target's panes: those processes go with
/// the place, so they are named in the same question.
fn task_losses(inventory: &FleetObservation, target: &CloseTarget) -> Vec<String> {
    let panes: Vec<&str> = panes_in_scope(inventory, target)
        .into_iter()
        .map(|pane| pane.location.pane_id.as_str())
        .collect();
    inventory
        .agents
        .iter()
        .filter(|agent| panes.contains(&agent.location.pane_id.as_str()))
        .filter_map(|agent| {
            let running = agent.facts.background_running.filter(|count| *count > 0)?;
            let name = agent
                .label
                .as_deref()
                .or(agent.name.as_deref())
                .unwrap_or("an agent");
            Some(format!("{name} has {running} background task(s) running"))
        })
        .collect()
}

/// The panes a target covers, at whatever depth they sit.
fn panes_in_scope<'a>(inventory: &'a FleetObservation, target: &CloseTarget) -> Vec<&'a Pane> {
    inventory
        .panes
        .iter()
        .filter(|pane| match target {
            CloseTarget::Pane(pane_id) => pane.location.pane_id == *pane_id,
            CloseTarget::Tab(tab_id) => pane.location.tab_id == *tab_id,
            CloseTarget::Workspace(workspace_id) => pane.location.workspace_id == *workspace_id,
        })
        .collect()
}

/// What closing an agent's pane may lose: the agent, its assignment and any
/// outstanding background work.
fn agent_losses(agent: &AgentRow) -> Vec<String> {
    let mut lines = vec![format!("agent: {} ({})", agent.title, agent.state.word())];
    if let Some(assignment) = agent.facts.assignment.as_deref() {
        lines.push(format!("assignment: {assignment}"));
    }
    if !agent.tasks.tasks.is_empty() {
        lines.push(format!(
            "outstanding: {} background task(s)",
            agent.tasks.tasks.len()
        ));
    }
    lines
}

/// What closing a pane row may lose: its foreground process or finished
/// session, as the row reports it.
fn pane_losses(pane: &PaneRow) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(exited) = &pane.exited {
        lines.push(format!("finished session: {}", exited.title));
    }
    match pane.command() {
        Some(command) => lines.push(format!("process: {command}")),
        None if pane.exited.is_none() => {
            lines.push("no process in the foreground".to_string());
        }
        None => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon sees a plain guarded close: the operator's confirmation, not a
    /// flag on the request, is what authorises ending the agents inside.
    #[test]
    fn a_container_deletion_is_a_plain_close_with_no_consent_flag() {
        use crate::app::Operation;
        use crate::tree::ContainerRef;

        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        let container = ContainerRef {
            container_id: "c1".into(),
            kind: Kind::Workspace,
            provenance: crate::control_plane::containers::Provenance::Created,
            handle: "wA".into(),
            recipe: false,
            unresolved: None,
        };
        let selected = Selected::Pane {
            pane_id: "wA:p1".into(),
            tab: None,
            losses: Vec::new(),
        };
        let delete = plan(Operation::Delete, selected, &state, Some(&container), "r1")
            .expect("a container deletion is freezable");
        let Effect::Domain(command) = &delete.effect else {
            panic!("a deletion is a domain request");
        };
        let request = command.record_request(&delete.request_id);
        let params = &request.params;
        assert_eq!(request.method, "topology.delete");
        assert_eq!(
            params.pointer("/request/container_id"),
            Some(&serde_json::json!("c1"))
        );
        assert!(
            params.pointer("/request/force").is_none(),
            "consent is the confirmation the operator gave, never a request field"
        );
    }

    fn container(kind: Kind, handle: &str) -> crate::tree::ContainerRef {
        crate::tree::ContainerRef {
            container_id: "c1".into(),
            kind,
            provenance: crate::control_plane::containers::Provenance::Created,
            handle: handle.into(),
            recipe: false,
            unresolved: None,
        }
    }

    /// Adding is a tab in the row's workspace and asks nothing: it loses no work,
    /// and a pane row is no reason to split instead.
    #[test]
    fn adding_from_any_row_is_an_unquestioned_tab_in_its_workspace() {
        use crate::app::Operation;

        let mut state = ObservationState::new();
        let fleet = crate::model::testing::fleet();
        let pane = fleet.panes[0].location.clone();
        state.apply_success(fleet);
        for container in [
            container(Kind::Workspace, &pane.workspace_id),
            container(Kind::Tab, &pane.tab_id),
            container(Kind::Pane, &pane.pane_id),
        ] {
            let selected = Selected::Pane {
                pane_id: pane.pane_id.clone(),
                tab: None,
                losses: Vec::new(),
            };
            let add = plan(
                Operation::CreateContainer,
                selected,
                &state,
                Some(&container),
                "r1",
            )
            .expect("an addition is freezable");
            assert!(!add.confirms, "{:?} asks nothing", container.kind);
            assert!(add.losses.iter().all(|loss| !loss.contains("new")));
            let Effect::Domain(command) = &add.effect else {
                panic!("an addition is a domain request");
            };
            let request = command.record_request(&add.request_id);
            assert_eq!(
                request.params.pointer("/request/kind"),
                Some(&serde_json::json!("tab")),
                "{:?} adds a tab, never a split",
                container.kind
            );
            assert_eq!(
                request.params.pointer("/request/workspace_id"),
                Some(&serde_json::json!(pane.workspace_id))
            );
        }
    }
}
