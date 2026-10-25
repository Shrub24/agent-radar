//! Lifecycle and container actions: the confirmation dialog, outcomes and notices.

use super::*;

impl App {
    /// Opens the confirmation for `operation` against the current observation,
    /// or explains why the selected row cannot be acted on.
    ///
    /// Eligibility is read from evidence, never from the cursor: a task or
    /// workspace row refuses rather than redirecting to its parent, a stale
    /// inventory refuses, a managed location is routed to its owner rather than
    /// the runtime, and only positive unmanaged evidence reaches the direct
    /// path. Nothing is sent here — Confirm is a separate act.
    pub fn begin_action(
        &mut self,
        operation: Operation,
        state: &ObservationState,
    ) -> Option<Confirmed> {
        // A row Radar records a container for is deleted as a container; every
        // other row is the place it shows, so the same key ends it.
        // An agent row names a process, so `D` on it is that process ending: its
        // owner's stop when Radar holds a subject, otherwise the pane close. Only a
        // place's own row is deleted as a container, and a managed worker's pane
        // record goes with the stop.
        let agent_row = matches!(
            self.selected_row().map(|row| &row.node.row.kind),
            Some(RowKind::Agent(_))
        );
        if operation.is_container()
            || (operation == Operation::Delete && !agent_row && self.selected_container().is_some())
        {
            return self.begin_container(operation, state);
        }
        if operation == Operation::PresenceStatus {
            self.show_presence_status(state);
            return None;
        }
        let Some(selected) = self.selected_action() else {
            self.set_focus_message(Some("nothing selected: no action target".to_string()));
            return None;
        };
        let (pane_id, tab, losses, is_agent, subject) = match selected {
            Selected::Agent {
                pane_id,
                tab,
                losses,
                subject,
                ..
            } => (pane_id, tab, losses, true, subject),
            Selected::Pane {
                pane_id,
                tab,
                losses,
                ..
            } => (pane_id, tab, losses, false, None),
            Selected::Refused(reason) => {
                let message = if operation == Operation::Restart {
                    "restart is only for a managed worker".to_string()
                } else {
                    reason.to_string()
                };
                self.set_focus_message(Some(message));
                return None;
            }
        };
        if !matches!(state.source_freshness(), SourceFreshness::Current) {
            self.set_focus_message(Some("the fleet is stale: not acting".to_string()));
            return None;
        }
        let Some(inventory) = state.inventory() else {
            self.set_focus_message(Some("no current inventory: not acting".to_string()));
            return None;
        };
        let subject = subject.map(|subject| *subject);
        if operation == Operation::Restart {
            if !is_agent {
                self.set_focus_message(Some("restart is only for a managed worker".to_string()));
                return None;
            }
            let Some(subject) = subject else {
                self.set_focus_message(Some(
                    "this agent has no daemon lifecycle subject".to_string(),
                ));
                return None;
            };
            if subject.state != crate::control_plane::lifecycle::State::Bound
                || subject.session.is_none()
            {
                self.set_focus_message(Some(
                    "this lifecycle subject is not restartable".to_string(),
                ));
                return None;
            }
            let target = CloseTarget::Pane(pane_id);
            let identity = lifecycle::identity(inventory, &target);
            let mut losses = losses;
            losses
                .push("the verified process is stopped and resumed in its recorded session".into());
            self.open_subject(operation, target, identity, subject, losses);
            return None;
        }

        // A row that stands for its tab ends the tab, the way deleting a workspace
        // ends what it holds; a nested row is its own pane.
        // A managed worker is stopped through its owner whenever it is the row
        // selected, so its exit is reported; a tab is only taken whole when the row
        // standing for it is not one Radar holds a subject for.
        let managed = containment_of(inventory, state, &CloseTarget::Pane(pane_id.clone()))
            == Containment::Managed;
        let target = match operation {
            Operation::Delete if !managed => match tab {
                Some(tab_id) => CloseTarget::Tab(tab_id),
                None => CloseTarget::Pane(pane_id.clone()),
            },
            Operation::Delete => CloseTarget::Pane(pane_id.clone()),
            Operation::Restart => unreachable!("restart returned above"),
            Operation::CreateContainer | Operation::ReloadContainer => {
                unreachable!("a container control returned above")
            }
            Operation::PresenceStatus => unreachable!("presence status returned above"),
        };
        // `D` ends what the row stands for. A tab is closed as a tab: its owner
        // settles whatever ran inside it from the panes' loss, and every pane goes
        // with it. A single managed pane is stopped through its owner, so its exit
        // is reported rather than merely observed. Everything else is the pane's
        // own close, including a pane Radar cannot classify: the operator asked
        // for this row to end.
        // A managed association with no subject has no owner to settle the exit, so
        // the pane closes like any other; the owner-held case stops through it.
        // The same walk at every level: what the row contains decides whether the
        // operator is asked, not which kind of row it is.
        let mut walked = losses_under(inventory, state, &target);
        let ask = !walked.is_empty();
        if let Some(subject) = subject.filter(|_| managed) {
            let identity = lifecycle::identity(inventory, &target);
            walked.push(
                "Radar cancels the exact process; a recorded child pane closes only after verified exit".into(),
            );
            self.open_subject(operation, target, identity, subject, walked);
        } else {
            self.open_direct(operation, target, walked, state);
        }
        if ask {
            return None;
        }
        // Nothing inside is mid-work, so this is not a question: the confirmation
        // keeps its own revalidation and is taken straight through it.
        self.confirm(state)
    }

    /// Opens the confirmation to enroll the selected registered lead.
    ///
    /// The lead must be joined to its pane, have a fresh session identity from its
    /// registration, and not already be a lifecycle subject. The daemon verifies
    /// the live process again before it records anything.
    pub(super) fn show_presence_status(&mut self, _state: &ObservationState) {
        let Some(row) = self.selected_row() else {
            self.set_focus_message(Some("nothing selected: no action target".to_string()));
            return;
        };
        let RowKind::Agent(agent) = &row.node.row.kind else {
            self.set_focus_message(Some("only a Pi session can be managed".to_string()));
            return;
        };
        let standing = agent
            .registry
            .as_ref()
            .and_then(|view| view.agent.claim_standing.as_ref());
        let message = match standing {
            Some(crate::control_plane::registry::presence::Standing::Granted) => {
                "this Pi session already has a Radar subject"
            }
            Some(crate::control_plane::registry::presence::Standing::Duplicate) => {
                "this session claim was rejected; reload or resume from its last live state"
            }
            Some(crate::control_plane::registry::presence::Standing::Reserved) => {
                "Radar reserved this session attachment; waiting for the launched process"
            }
            Some(crate::control_plane::registry::presence::Standing::Pending) => {
                "Radar is still verifying this session claim"
            }
            Some(crate::control_plane::registry::presence::Standing::Released) => {
                "this session claim was released; reload or resume to claim it again"
            }
            None => "Radar adopts Pi sessions through presence; waiting for the next report",
        };
        self.set_focus_message(Some(message.to_string()));
    }

    pub(super) fn open_subject(
        &mut self,
        operation: Operation,
        target: CloseTarget,
        identity: TargetIdentity,
        subject: crate::registry_observation::ManagedSubject,
        losses: Vec<String>,
    ) {
        self.confirmation = Some(Confirmation {
            operation,
            target,
            identity,
            losses,
            confirm_selected: false,
            subject: Some(subject),
            container_id: None,
        });
    }

    pub(super) fn open_direct(
        &mut self,
        operation: Operation,
        target: CloseTarget,
        mut losses: Vec<String>,
        state: &ObservationState,
    ) {
        let inventory = state.inventory().expect("current inventory");
        if let CloseTarget::Tab(tab) = &target {
            losses.extend(tab_losses(inventory, tab));
        }
        let identity = lifecycle::identity(inventory, &target);
        self.confirmation = Some(Confirmation {
            operation,
            target,
            identity,
            losses,
            confirm_selected: false,
            subject: None,
            container_id: None,
        });
    }

    /// The control that creates a new workspace, whatever row is selected.
    ///
    /// With a `project` it starts in that directory and is named for it; without
    /// one it is tmux's own empty session.
    pub fn new_workspace(&self, project: Option<&std::path::Path>) -> Confirmed {
        let (label, directory) = match project {
            Some(directory) => (
                Some(crate::project::project_name(directory)),
                Some(directory.to_string_lossy().into_owned()),
            ),
            None => (None, None),
        };
        Confirmed::Container(crate::topology::Control::Create(ContainerCreate {
            request_id: crate::control_plane::random_uuid(),
            kind: ContainerCreateKind::Workspace { directory },
            label,
            recipe: None,
        }))
    }

    /// The container the selected row shows, when Radar records one for it.
    ///
    /// A row without one is a place this daemon records nothing for: there is no
    /// container to address, and inventing one from the handle would name a place
    /// Radar does not own.
    pub fn selected_container(&self) -> Option<&ContainerRef> {
        let row = self.selected_row()?;
        match &row.node.row.kind {
            RowKind::Workspace { container, .. } => container.as_ref(),
            RowKind::Agent(agent) => agent.container.as_ref(),
            RowKind::Pane(pane) => pane.container.as_ref(),
            RowKind::Task(_) | RowKind::Lifecycle(_) => None,
        }
    }

    /// Build the configured managed-agent launch without a prompt or confirmation.
    pub fn launch_configured(
        &mut self,
        state: &ObservationState,
        command: &[String],
        shell: Option<&str>,
        path: Option<&std::ffi::OsStr>,
    ) -> Option<Confirmed> {
        if !matches!(state.source_freshness(), SourceFreshness::Current) {
            self.set_focus_message(Some("the fleet is stale: nothing was sent".into()));
            return None;
        }
        let Some(inventory) = state.inventory() else {
            self.set_focus_message(Some("no current inventory: nothing was sent".into()));
            return None;
        };
        let Some(container) = self.selected_container().cloned() else {
            self.set_focus_message(Some("select a workspace or pane to launch into".into()));
            return None;
        };
        let Some(recorded) = inventory
            .containers
            .iter()
            .find(|item| item.container_id == container.container_id && item.removed_at.is_none())
        else {
            self.set_focus_message(Some("the selected container is no longer current".into()));
            return None;
        };
        let Some(binding) = recorded
            .binding
            .as_ref()
            .filter(|binding| binding.handle == container.handle && recorded.unresolved.is_none())
        else {
            self.set_focus_message(Some("the selected container is unresolved".into()));
            return None;
        };
        // A launch always opens a new window: the agent gets its own tab in the
        // workspace the selected place belongs to, rather than a pane beside
        // whatever is already running there. The directory is the selected
        // place's own, so a launch from a pane lands in that pane's project.
        let (workspace_id, directory) = match recorded.kind {
            crate::control_plane::containers::Kind::Workspace => (
                binding.handle.clone(),
                inventory
                    .workspaces
                    .iter()
                    .find(|workspace| workspace.workspace_id == binding.handle)
                    .and_then(|workspace| workspace.session_path.as_deref()),
            ),
            crate::control_plane::containers::Kind::Pane => {
                let Some(pane) = inventory
                    .panes
                    .iter()
                    .find(|pane| pane.location.pane_id == binding.handle)
                else {
                    self.set_focus_message(Some(
                        "the selected pane is not in the current inventory".into(),
                    ));
                    return None;
                };
                (
                    pane.location.workspace_id.clone(),
                    pane.current_path.as_deref(),
                )
            }
            // A tab is location detail rather than a row, so it is never the
            // selected container; a place that is not one of the two above is
            // refused rather than guessed at.
            _ => {
                self.set_focus_message(Some(
                    "select a workspace or pane to launch an agent".into(),
                ));
                return None;
            }
        };
        // A tab names no directory of its own, so the launch starts in its
        // workspace's, which is the project the reader selected.
        let directory = directory.or_else(|| {
            inventory
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .and_then(|workspace| workspace.session_path.as_deref())
        });
        let kind = crate::control_plane::client::ContainerCreateKind::Tab { workspace_id };
        let Some(directory) = directory else {
            self.set_focus_message(Some(
                "the selected place has no current directory: nothing was sent".into(),
            ));
            return None;
        };
        let Some(shell) = shell.filter(|shell| !shell.trim().is_empty()) else {
            self.set_focus_message(Some("SHELL is unset; set it to fish, bash, or zsh".into()));
            return None;
        };
        let recipe = match crate::topology::target_shell_recipe(command, shell, path, directory) {
            Ok(recipe) => recipe,
            Err(message) => {
                self.set_focus_message(Some(format!("{message}: nothing was sent")));
                return None;
            }
        };
        Some(Confirmed::Container(crate::topology::Control::Create(
            crate::control_plane::client::ContainerCreate {
                request_id: crate::control_plane::random_uuid(),
                kind,
                label: None,
                recipe: Some(recipe),
            },
        )))
    }

    /// Opens the confirmation for one container control.
    ///
    /// The container, its place and the occupant of that place are frozen now, not
    /// read from the cursor at Confirm: moving the selection while the dialog is up
    /// must not retarget it, and the daemon revalidates the frozen place before it
    /// changes anything.
    pub(super) fn begin_container(
        &mut self,
        operation: Operation,
        state: &ObservationState,
    ) -> Option<Confirmed> {
        if !matches!(state.source_freshness(), SourceFreshness::Current) {
            self.set_focus_message(Some("the fleet is stale: not acting".to_string()));
            return None;
        }
        let Some(inventory) = state.inventory() else {
            self.set_focus_message(Some("no current inventory: not acting".to_string()));
            return None;
        };
        let Some(container) = self.selected_container().cloned() else {
            self.set_focus_message(Some(
                "this row is not a container Radar records: nothing to act on".to_string(),
            ));
            return None;
        };
        // A container whose place cannot be verified has nothing to freeze, so
        // every control but a deletion is refused here. Deleting it is the record
        // going: the daemon accepts that only when it cannot verify the place is
        // still served, so nothing live is forgotten around.
        if let Some(reason) = &container.unresolved {
            if operation != Operation::Delete {
                self.set_focus_message(Some(format!(
                    "{} is unresolved ({reason}): nothing to act on",
                    container_name(inventory, &container)
                )));
                return None;
            }
            let id = container.container_id.clone();
            return Some(Confirmed::Container(crate::topology::Control::Delete(
                ContainerDelete {
                    request_id: crate::control_plane::random_uuid(),
                    force: self.deletion_held.contains(&id),
                    container_id: id,
                    // No place to freeze: there is nothing recorded to close.
                    frozen: serde_json::Value::Null,
                },
            )));
        }
        let target = container.kind.target(&container.handle);
        let described = container_name(inventory, &container);
        let running = losses_under(inventory, state, &target);
        let mut losses = vec![described.clone()];
        match operation {
            Operation::Delete => {
                if running.is_empty() {
                    losses.push("nothing is running in it".to_string());
                } else {
                    losses.extend(running.iter().cloned());
                }
            }
            Operation::ReloadContainer => {
                if container.kind != crate::control_plane::containers::Kind::Pane {
                    self.set_focus_message(Some(
                        "only a pane slot runs a recipe: select a pane".to_string(),
                    ));
                    return None;
                }
                if !container.recipe {
                    self.set_focus_message(Some(format!(
                        "container {} records no recipe: nothing to reload",
                        container.container_id
                    )));
                    return None;
                }
                losses.push("the recorded recipe runs again in this pane".to_string());
                losses.push(
                    "a command of its own that has not exited refuses the reload".to_string(),
                );
            }
            Operation::CreateContainer => match container.kind {
                crate::control_plane::containers::Kind::Pane => {
                    losses.push("a new pane is split from this one".to_string())
                }
                crate::control_plane::containers::Kind::Workspace => {
                    losses.push("a new tab is created in this workspace".to_string())
                }
                // A tab is a place inside its workspace, and Radar creates tabs in a
                // workspace rather than in a tab: the operator picks the pane or the
                // workspace instead of this guessing.
                crate::control_plane::containers::Kind::Tab => {
                    self.set_focus_message(Some(
                        "select a pane in this tab to add a pane, or its workspace to add a tab"
                            .to_string(),
                    ));
                    return None;
                }
            },
            _ => return None,
        }

        let identity = lifecycle::identity(inventory, &target);
        let confirmation = Confirmation {
            operation,
            target,
            identity,
            losses,
            confirm_selected: false,
            subject: None,
            container_id: Some(container.container_id),
        };
        // Deleting a container nothing runs in is the standard case and loses no
        // work, so it is not a question. Anything running is confirmed by name.
        if operation == Operation::Delete && running.is_empty() {
            return self.confirm_container(confirmation, state);
        }
        self.confirmation = Some(confirmation);
        None
    }

    /// Revalidates a container control against the latest current observation and
    /// hands back the control to dispatch.
    ///
    /// The place must still be the one that was confirmed and still hold what was
    /// confirmed: a handle a later backend instance reused is not this container's,
    /// so the control is refused here rather than aimed at whatever holds it now.
    pub(super) fn confirm_container(
        &mut self,
        confirmation: Confirmation,
        state: &ObservationState,
    ) -> Option<Confirmed> {
        let described = confirmation.target.description();
        if !matches!(state.source_freshness(), SourceFreshness::Current) {
            self.set_focus_message(Some("the fleet is stale: nothing was sent".to_string()));
            return None;
        }
        let Some(inventory) = state.inventory() else {
            self.set_focus_message(Some("no current inventory: nothing was sent".to_string()));
            return None;
        };
        let Some(id) = confirmation.container_id.as_deref() else {
            self.set_focus_message(Some(
                "this control names no container: nothing was sent".to_string(),
            ));
            return None;
        };
        let recorded = inventory
            .containers
            .iter()
            .find(|container| container.container_id == id && container.removed_at.is_none());
        let Some(recorded) = recorded else {
            self.set_focus_message(Some(format!("container {id} is gone: nothing was sent")));
            return None;
        };
        if recorded
            .binding
            .as_ref()
            .is_none_or(|binding| binding.handle != *confirmation.target.id())
            || !lifecycle::matches(inventory, &confirmation.identity)
        {
            self.set_focus_message(Some(format!("{described} changed: nothing was sent")));
            return None;
        }
        let request_id = crate::control_plane::random_uuid();
        let control = match confirmation.operation {
            Operation::Delete => crate::topology::Control::Delete(ContainerDelete {
                request_id,
                force: self.deletion_held.contains(id),
                container_id: id.to_string(),
                // The frozen target and occupant travel with the request: the daemon
                // refuses the deletion when that place no longer holds them.
                frozen: serde_json::json!({
                    "target": confirmation.target,
                    "identity": confirmation.identity,
                }),
            }),
            Operation::ReloadContainer => crate::topology::Control::Reload(ContainerReload {
                request_id,
                container_id: id.to_string(),
            }),
            Operation::CreateContainer => {
                let kind = match recorded.kind {
                    crate::control_plane::containers::Kind::Workspace => ContainerCreateKind::Tab {
                        workspace_id: recorded
                            .binding
                            .as_ref()
                            .expect("binding checked above")
                            .handle
                            .clone(),
                    },
                    _ => ContainerCreateKind::PaneSplit {
                        pane_id: recorded
                            .binding
                            .as_ref()
                            .expect("binding checked above")
                            .handle
                            .clone(),
                    },
                };
                let recipe = None;
                crate::topology::Control::Create(ContainerCreate {
                    request_id,
                    kind,
                    label: None,
                    recipe,
                })
            }
            _ => return None,
        };
        Some(Confirmed::Container(control))
    }

    /// Revalidates an open confirmation against the latest current observation
    /// and hands back the frozen action when it still holds. A stale source,
    /// disappeared target or changed containment cancels it with a reason.
    pub fn confirm(&mut self, state: &ObservationState) -> Option<Confirmed> {
        let confirmation = self.confirmation.take()?;
        if confirmation.operation.is_container() {
            return self.confirm_container(confirmation, state);
        }
        let described = confirmation.target.description();
        if !matches!(state.source_freshness(), SourceFreshness::Current) {
            self.set_focus_message(Some("the fleet is stale: nothing was sent".to_string()));
            return None;
        }
        let Some(inventory) = state.inventory() else {
            self.set_focus_message(Some("no current inventory: nothing was sent".to_string()));
            return None;
        };
        if !target_present(inventory, &confirmation.target) {
            self.set_focus_message(Some(format!("{described} is gone: nothing was sent")));
            return None;
        }
        let actual = containment_of(inventory, state, &confirmation.target);
        let subject_managed = confirmation.subject.is_some();
        let refusal = match (subject_managed, actual) {
            // A managed confirmation is only ever an owner request; a direct
            // one is only ever positive unmanaged evidence.
            (false, Containment::Unmanaged) | (true, Containment::Managed) => None,
            (false, other) => containment_refusal(other, &confirmation.target),
            (true, Containment::Unmanaged) => Some(format!(
                "{described} is no longer managed: nothing was requested"
            )),
            (true, Containment::Uncertain) => Some(format!(
                "{described} can no longer be verified as managed: nothing was requested"
            )),
        };
        if let Some(reason) = refusal {
            self.set_focus_message(Some(reason));
            return None;
        }
        if !lifecycle::matches(inventory, &confirmation.identity) {
            self.set_focus_message(Some(format!("{described} changed: nothing was sent")));
            return None;
        }
        if let Some(expected) = &confirmation.subject {
            let mut matches = inventory
                .agents
                .iter()
                .filter_map(|agent| agent.registry.as_ref()?.managed.as_ref())
                .filter(|current| {
                    current.subject_id == expected.subject_id && *current == expected
                });
            if matches.next().is_none() || matches.next().is_some() {
                self.set_focus_message(Some(format!(
                    "{described} lifecycle binding changed: nothing was sent"
                )));
                return None;
            }
            return Some(Confirmed::Subject {
                operation: confirmation.operation,
                request_id: crate::control_plane::random_uuid(),
                subject_id: expected.subject_id.clone(),
                close_request_id: expected
                    .child_close
                    .as_ref()
                    .map(|_| crate::control_plane::random_uuid()),
                claim_id: expected.claim_id.clone(),
                child_close: expected.child_close.clone(),
            });
        }
        Some(Confirmed::Direct(CloseRequest {
            target: confirmation.target,
            identity: confirmation.identity,
        }))
    }

    /// Records what a confirmed container control reported.
    ///
    /// The line keeps the request id the control was dispatched under, so an
    /// answer that never arrived leaves an operation the operator can read back
    /// instead of an effect nobody can name.
    pub fn apply_container_outcome(&mut self, finished: crate::topology::Finished) {
        let crate::topology::Finished {
            verb,
            container_id,
            request_id,
            result,
        } = finished;
        // A deletion the daemon held back is the operator's to repeat: the next
        // `D` on the same container asks for it to close anyway. Any other answer
        // clears the hold, so a plain deletion never carries force.
        if let Some(id) = container_id.as_deref().filter(|_| verb != "delete") {
            self.deletion_held.remove(id);
        }
        match result {
            Ok(answer) => {
                let outcome = answer.outcome().to_string();
                let described = match (&answer.container, answer.removed.len()) {
                    (Some(container), _) => format!("container {}", container.container_id),
                    // A deletion names what it removed: one place, or a subtree.
                    (None, 1) => format!("container {}", answer.removed[0].container_id),
                    (None, count) if count > 1 => format!("{count} containers"),
                    (None, _) => "the container".to_string(),
                };
                let mut text = format!("{verb} {outcome} for {described} [{request_id}]");
                if let Some(message) = answer.message() {
                    text.push_str(&format!(": {message}"));
                }
                let failed = outcome != "completed";
                // The daemon files the hold under its own category, so what the
                // UI acts on is a word, not a sentence.
                if verb == "delete"
                    && let Some(id) = container_id
                {
                    let held = answer.category().is_some_and(|category| {
                        category == crate::control_plane::store::Category::AGENTS_ALIVE
                    });
                    if failed && held {
                        self.deletion_held.insert(id);
                    } else {
                        self.deletion_held.remove(&id);
                    }
                }
                self.record_notice(request_id, text, failed);
            }
            Err(message) => self.record_notice(
                request_id.clone(),
                format!("{verb} not sent [{request_id}]: {message}"),
                true,
            ),
        }
    }

    /// Records a daemon lifecycle outcome with its subject and request identity.
    pub fn apply_subject_outcome(&mut self, finished: crate::managed_control::Finished) {
        let crate::managed_control::Finished {
            operation,
            request_id,
            subject_id,
            result,
            close_request_id,
            close_result,
        } = finished;
        let verb = operation.verb();
        match result {
            Ok(answer) => {
                let outcome = answer.outcome().to_string();
                let mut text = format!("{verb} {outcome} for subject {subject_id} [{request_id}]");
                if let Some(message) = answer.message() {
                    text.push_str(&format!(": {message}"));
                }
                self.record_notice(request_id, text, outcome != "completed");
            }
            Err(message) => self.record_notice(
                request_id.clone(),
                format!(
                    "{verb} outcome unknown for subject {subject_id} [{request_id}]: {message}"
                ),
                true,
            ),
        }
        if let (Some(close_id), Some(close_result)) = (close_request_id, close_result) {
            match close_result {
                Ok(answer) => {
                    let outcome = answer.outcome().to_string();
                    let mut text = format!("pane cleanup {outcome} for subject {subject_id} [{close_id}]");
                    if let Some(message) = answer.message() {
                        text.push_str(&format!(": {message}"));
                    }
                    self.record_notice(close_id, text, outcome != "completed");
                }
                Err(message) => self.record_notice(
                    close_id.clone(),
                    format!("pane cleanup outcome unknown for subject {subject_id} [{close_id}]: {message}"),
                    true,
                ),
            }
        }
    }

    /// Records one outcome line, refining a notice for the same request id rather
    /// than adding another.
    pub(super) fn record_notice(&mut self, id: String, text: String, failed: bool) {
        // The footer shows one clipped line; the log keeps the whole text.
        if failed {
            tracing::warn!(request = %id, "{text}");
        } else {
            tracing::info!(request = %id, "{text}");
        }
        let at = Instant::now();
        match self.notices.iter_mut().find(|notice| notice.id == id) {
            Some(notice) => {
                notice.failed = failed;
                notice.text = text;
                notice.at = at;
            }
            None => self.notices.push(Notice {
                at,
                id,
                text,
                failed,
            }),
        }
    }

    /// Lifecycle outcomes, newest last, kept until dismissed.
    pub fn lifecycle_notices(&self) -> &[Notice] {
        &self.notices
    }

    /// Dismisses every lifecycle outcome.
    pub fn dismiss_notices(&mut self) {
        self.notices.clear();
        self.notice_scroll = 0;
    }

    /// Whether the outcomes panel is open, and where it is scrolled to.
    pub fn notices_open(&self) -> bool {
        self.notices_open
    }

    pub fn notice_scroll(&self) -> usize {
        self.notice_scroll
    }

    /// Opens or closes the outcomes panel. Opening starts at the top, where the
    /// newest outcome is.
    pub fn toggle_notices(&mut self) {
        self.notices_open = !self.notices_open;
        if self.notices_open {
            self.notice_scroll = 0;
        }
    }

    /// Moves the outcomes panel by whole lines. The panel clamps the offset to the
    /// text it has, so this may be called past either end.
    pub fn scroll_notices(&mut self, lines: isize) {
        self.notice_scroll = self.notice_scroll.saturating_add_signed(lines);
    }

    /// The open confirmation, if any.
    pub fn confirmation(&self) -> Option<&Confirmation> {
        self.confirmation.as_ref()
    }

    /// Closes the confirmation without acting.
    pub fn cancel_confirmation(&mut self) {
        self.confirmation = None;
    }

    /// The selected row as an action target, with what acting may lose.
    pub(super) fn selected_action(&self) -> Option<Selected> {
        let row = self.selected_row()?;
        Some(match &row.node.row.kind {
            RowKind::Agent(agent) if !agent.joined() => Selected::Refused(
                "an unresolved agent is not joined to a pane: its controls are refused",
            ),
            RowKind::Agent(agent) => Selected::Agent {
                pane_id: agent.pane_id.clone(),
                tab: self.tab_the_row_stands_for(&row),
                losses: agent_losses(agent),
                subject: agent
                    .registry
                    .as_ref()
                    .and_then(|view| view.managed.clone())
                    .map(Box::new),
            },
            RowKind::Pane(pane) => Selected::Pane {
                pane_id: pane.pane_id.clone(),
                tab: self.tab_the_row_stands_for(&row),
                losses: pane_losses(pane),
            },
            RowKind::Workspace { .. } => {
                Selected::Refused("a workspace is not a close target: close a pane or its tab")
            }
            RowKind::Task(_) => Selected::Refused("a background task is not a close target"),
            RowKind::Lifecycle(_) => {
                Selected::Refused("a lifecycle status entry is not a pane close target")
            }
        })
    }

    /// The tab a row stands for, when it is the tab's own root row.
    ///
    /// A tab is drawn as its root pane or agent, so ending that row ends the tab
    /// the way deleting a workspace ends what it holds. Every other row is the
    /// pane it shows, however deep it sits.
    pub(super) fn tab_the_row_stands_for(&self, row: &VisibleRow<'_>) -> Option<String> {
        match &row.node.row.kind {
            RowKind::Agent(agent) if agent.joined() && agent.represents_tab => {
                Some(agent.tab_id.clone())
            }
            RowKind::Pane(pane) if pane.represents_tab => Some(pane.tab_id.clone()),
            _ => None,
        }
    }

    /// A confirmation's keys: Escape cancels, Tab and the arrows move between
    /// the drawn buttons, Enter activates the selected one. Everything else is
    /// swallowed, so nothing reaches the tree while the dialog is up.
    pub(super) fn handle_confirmation_key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => self.confirmation = None,
            KeyCode::Tab
            | KeyCode::BackTab
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down => {
                if let Some(confirmation) = self.confirmation.as_mut() {
                    confirmation.confirm_selected = !confirmation.confirm_selected;
                }
            }
            KeyCode::Enter => {
                // Enter activates the selected button: Confirm only when the
                // operator moved to it, otherwise the default Cancel cancels.
                if self
                    .confirmation
                    .as_ref()
                    .is_some_and(|confirmation| confirmation.confirm_selected)
                {
                    return Some(Action::ConfirmAction);
                }
                self.confirmation = None;
            }
            _ => {}
        }
        None
    }

    /// A left click while the confirmation is up. Only the two drawn buttons
    /// answer: a click anywhere else, including on a tree row behind it, does
    /// nothing, so a row's second-click focus can never confirm.
    pub(super) fn confirmation_click(&mut self, at: (u16, u16)) -> Option<Action> {
        if inside(self.layout.confirm_confirm, at) {
            return Some(Action::ConfirmAction);
        }
        if inside(self.layout.confirm_cancel, at) {
            self.confirmation = None;
        }
        None
    }
}
