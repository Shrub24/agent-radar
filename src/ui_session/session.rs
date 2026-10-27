//! The shared UI session: one canonical navigation model, the attachments that
//! act on it, and the ordered publications they observe.
//!
//! This is a deterministic reducer with no threads, clocks or I/O. Everything
//! that waits or delivers lives in [`super::actor`]. Frontends send semantic
//! actions; the session applies each once, in order, and answers with the
//! outcome. Terminal geometry, details and local overlays never reach it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use serde_json::Value;

use super::gate::{Completion, Gate};
use super::navigation::{self, NavState, NavStep, Navigation, PaneView, RowOrder, TaskRows};
use crate::control_plane::public_contracts::ui_session as dto;
use crate::model::AgentState;
use crate::observation::ObservationState;
use crate::tasks::TaskBoard;
use crate::tree::{FleetTree, RowId, RowKind, TaskId};

/// Receipts kept per attachment. An older action id answers `receipt_expired`.
const RECEIPTS: usize = 64;
/// Superseded tree payloads kept besides the current one.
const RETAINED_TREES: usize = 16;

struct Receipt {
    action_id: u64,
    fingerprint: String,
    outcome: dto::ActionOutcome,
}

struct Attachment {
    role: dto::AttachmentRole,
    next_action: u64,
    receipts: VecDeque<Receipt>,
}

/// One state as published: ordered by `update_id`, its payloads shared.
#[derive(Clone, Debug)]
pub struct Published {
    pub update_id: u64,
    pub tree_hash: String,
    pub rows: Arc<Vec<dto::TreeRow>>,
    pub state_hash: String,
    pub state: dto::SessionState,
}

impl Published {
    /// The wire form, with rows only when the receiver lacks `known` content.
    pub fn publication(&self, known: Option<&str>) -> dto::Publication {
        dto::Publication {
            update_id: self.update_id,
            tree: dto::TreeProjection {
                hash: self.tree_hash.clone(),
                rows: (known != Some(self.tree_hash.as_str())).then(|| (*self.rows).clone()),
            },
            state_hash: self.state_hash.clone(),
            session: self.state.clone(),
        }
    }
}

pub struct Session {
    nav: Navigation,
    bus: TaskBoard,
    attachments: HashMap<String, Attachment>,
    editor: Option<String>,
    gate: Gate,
    operation: Option<dto::CommandOutcome>,
    interaction_notice: Option<String>,
    /// The inventory a command's target is derived from, as last observed. Kept
    /// so the session decides what a command acts on, not the calling frontend.
    state: Option<ObservationState>,
    /// Containers whose deletion the daemon held back: a repeat asks it to close
    /// anyway. A plain deletion never carries force.
    deletions_held: std::collections::HashSet<String>,
    /// The held key of each effect this session dispatched, by request id.
    held: HashMap<String, String>,
    current: Published,
    retained: VecDeque<(String, Arc<Vec<dto::TreeRow>>)>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Self {
        let mut session = Self {
            nav: Navigation::default(),
            bus: TaskBoard::default(),
            attachments: HashMap::new(),
            editor: None,
            gate: Gate::default(),
            operation: None,
            interaction_notice: None,
            state: None,
            deletions_held: std::collections::HashSet::new(),
            held: HashMap::new(),
            current: Published {
                update_id: 0,
                tree_hash: String::new(),
                rows: Arc::new(Vec::new()),
                state_hash: String::new(),
                state: dto::SessionState::default(),
            },
            retained: VecDeque::new(),
        };
        session.project();
        session
    }

    pub fn current(&self) -> &Published {
        &self.current
    }

    pub fn claim_attachment(
        &mut self,
        id: &str,
        interaction: &str,
    ) -> Result<(), dto::RefusalCode> {
        self.gate.claim(id, interaction).map_err(|(code, _)| code)?;
        self.project();
        Ok(())
    }

    pub fn attachments_contain(&self, id: &str) -> bool {
        self.attachments.contains_key(id)
    }

    pub fn attachments(&self) -> usize {
        self.attachments.len()
    }

    /// Folds a new observation into the tree. Returns whether anything a
    /// presentation shows changed.
    pub fn observe(&mut self, state: &ObservationState, tasks: Option<&Value>) -> bool {
        if let Some(tasks) = tasks
            && let Ok(events) = crate::control_plane::client::tasks_from_snapshot(tasks)
        {
            let mut bus = TaskBoard::default();
            for event in events {
                bus.apply(event);
            }
            self.bus = bus;
        }
        self.state = Some(state.clone());
        let mut tree = FleetTree::build(state);
        tree.attach_tasks(&self.bus);
        self.nav.replace_tree(tree);
        self.project()
    }

    pub fn attach(&mut self, id: &str, role: dto::AttachmentRole) -> Result<(), dto::RefusalCode> {
        if self.attachments.contains_key(id) {
            return Err(dto::RefusalCode::Conflict);
        }
        self.attachments.insert(
            id.to_string(),
            Attachment {
                role,
                next_action: 1,
                receipts: VecDeque::new(),
            },
        );
        Ok(())
    }

    /// Releases an attachment and its editor ownership; the accepted query stays.
    pub fn detach(&mut self, id: &str) -> bool {
        let known = self.attachments.remove(id).is_some();
        let released = self.editor.as_deref() == Some(id);
        if released {
            self.editor = None;
        }
        if self.gate.attachment_lost(id) || released {
            self.project();
        }
        known
    }

    pub fn content(&self, hash: &str) -> Option<Arc<Vec<dto::TreeRow>>> {
        if self.current.tree_hash == hash {
            return Some(Arc::clone(&self.current.rows));
        }
        self.retained
            .iter()
            .find(|(held, _)| held == hash)
            .map(|(_, rows)| Arc::clone(rows))
    }

    /// Applies action `action_id` of attachment `id` once and in order.
    pub fn act(&mut self, id: &str, action_id: u64, action: dto::UiAction) -> dto::ActionOutcome {
        self.act_stamped(id, action_id, None, action)
    }

    /// As [`Self::act`] for input interpreted under gate generation `gate`.
    /// Ordinary input from before the gate last changed is refused, so a key
    /// typed for one screen cannot land on another. Interaction actions name
    /// their interaction and are not stamped.
    pub fn act_stamped(
        &mut self,
        id: &str,
        action_id: u64,
        gate: Option<u64>,
        action: dto::UiAction,
    ) -> dto::ActionOutcome {
        use dto::ActionOutcome as Out;
        let fingerprint =
            serde_json::to_string(&(gate, &action)).expect("serialize action receipt");
        let Some(attachment) = self.attachments.get(id) else {
            return Out::Detached;
        };
        if action_id < attachment.next_action {
            return match attachment
                .receipts
                .iter()
                .find(|receipt| receipt.action_id == action_id)
            {
                None => Out::ReceiptExpired,
                Some(receipt) if receipt.fingerprint != fingerprint => Out::Refused {
                    code: dto::RefusalCode::Conflict,
                    message: format!("action {action_id} was already used for another action"),
                },
                Some(receipt) => match &receipt.outcome {
                    Out::Applied {
                        update_id,
                        interaction_id,
                        request_id,
                    } => Out::Replayed {
                        update_id: *update_id,
                        interaction_id: interaction_id.clone(),
                        request_id: request_id.clone(),
                    },
                    other => other.clone(),
                },
            };
        }
        if action_id > attachment.next_action {
            return Out::Gap {
                expected: attachment.next_action,
            };
        }
        let role = attachment.role;
        let resolving = matches!(action, dto::UiAction::ResolveInteraction { .. });
        let ordinary = action.is_ordinary();
        let outcome = match if ordinary && gate.is_some_and(|g| g != self.gate.generation()) {
            Err((
                dto::RefusalCode::StaleGate,
                "the screen changed under that input".to_string(),
            ))
        } else {
            self.apply(id, role, action)
        } {
            Ok(interaction_id) => Out::Applied {
                update_id: self.current.update_id,
                interaction_id,
                request_id: if resolving {
                    self.gate.request_id()
                } else {
                    None
                },
            },
            Err((code, message)) => Out::Refused { code, message },
        };
        let attachment = self
            .attachments
            .get_mut(id)
            .expect("the attachment was just read");
        attachment.next_action += 1;
        attachment.receipts.push_back(Receipt {
            action_id,
            fingerprint,
            outcome: outcome.clone(),
        });
        while attachment.receipts.len() > RECEIPTS {
            attachment.receipts.pop_front();
        }
        outcome
    }

    fn apply(
        &mut self,
        id: &str,
        role: dto::AttachmentRole,
        action: dto::UiAction,
    ) -> Result<Option<String>, (dto::RefusalCode, String)> {
        use dto::UiAction as A;
        let refuse = |code, message: &str| Err((code, message.to_string()));
        if role == dto::AttachmentRole::Presenter
            && !matches!(
                action,
                A::ResolveInteraction { .. }
                    | A::CancelInteraction { .. }
                    | A::ClaimPresentation { .. }
            )
        {
            return refuse(
                dto::RefusalCode::Role,
                "a presenter only resolves its interaction",
            );
        }
        // One shared interaction at a time, and while it is open nothing else
        // changes what the screens agree on.
        if self.gate.is_open()
            && !matches!(
                action,
                A::ResolveInteraction { .. }
                    | A::CancelInteraction { .. }
                    | A::ProvideChoices { .. }
                    | A::ClaimPresentation { .. }
                    | A::PresentationFailed { .. }
            )
        {
            return refuse(dto::RefusalCode::Blocked, "an interaction is open");
        }
        let owns = self.editor.as_deref() == Some(id);
        let foreign = self.editor.is_some() && !owns;
        match action {
            A::SelectRow { row } => {
                if !self.nav.apply(&NavStep::Select(row_id(&row))) {
                    return refuse(dto::RefusalCode::UnknownRow, "that row is not visible");
                }
            }
            A::Move { delta } => self.step(NavStep::Move(delta)),
            A::Collapse { row } => self.on_row(row, NavStep::Collapse)?,
            A::Expand { row } => self.on_row(row, NavStep::Expand)?,
            A::ToggleFold { row } => self.on_row(row, NavStep::ToggleFold)?,
            A::ToggleBranch { row } => self.step(NavStep::ToggleBranch(row_id(&row))),
            A::CyclePaneView => self.step(NavStep::CyclePanes),
            A::CycleOrder => self.step(NavStep::CycleOrder),
            A::ToggleFinished => self.step(NavStep::ToggleFinished),
            A::ToggleTasks => self.step(NavStep::ToggleTasks),
            A::ShowEverything => self.step(NavStep::ShowEverything),
            A::NextAttention { forward } => self.step(NavStep::JumpAttention(forward)),
            A::NextWorking { forward } => self.step(NavStep::JumpWorking(forward)),
            A::BeginFilterEdit => {
                if foreign {
                    return refuse(
                        dto::RefusalCode::NotEditor,
                        "another view is editing the filter",
                    );
                }
                self.editor = Some(id.to_string());
            }
            A::SetFilterQuery { query } => {
                if !owns {
                    return refuse(
                        dto::RefusalCode::NotEditor,
                        "begin editing the filter first",
                    );
                }
                self.nav.apply(&NavStep::SetFilter(query));
            }
            A::EndFilterEdit => {
                if foreign {
                    return refuse(
                        dto::RefusalCode::NotEditor,
                        "another view is editing the filter",
                    );
                }
                self.editor = None;
            }
            A::ClearFilter => {
                if foreign {
                    return refuse(
                        dto::RefusalCode::NotEditor,
                        "another view is editing the filter",
                    );
                }
                self.nav.apply(&NavStep::ClearFilter);
            }
            A::RunCommand {
                command: dto::SharedCommand::NewWorkspace,
                row,
            } => {
                let interaction =
                    self.gate
                        .begin_project(id, dto::SharedCommand::NewWorkspace, row)?;
                // Starting a blocking interaction ends editing: the query stays.
                self.editor = None;
                self.interaction_notice = None;
                self.project();
                return Ok(Some(interaction));
            }
            A::ProvideChoices {
                interaction_id,
                choices,
                note,
            } => self
                .gate
                .provide_choices(id, &interaction_id, choices, note)?,
            A::ClaimPresentation { interaction_id } => self.gate.claim(id, &interaction_id)?,
            A::ResolveInteraction {
                interaction_id,
                result,
            } => self.gate.resolve(id, &interaction_id, result)?,
            A::CancelInteraction { interaction_id } => self.gate.cancel(&interaction_id)?,
            A::PresentationFailed {
                interaction_id,
                message,
            } => {
                // A host that could not present reports a failure, which closes
                // the interaction exactly as any other view's cancellation does.
                self.gate.cancel(&interaction_id)?;
                self.interaction_notice = Some(message);
            }
            A::RunCommand { command, row }
                if matches!(
                    command,
                    dto::SharedCommand::Focus
                        | dto::SharedCommand::Delete
                        | dto::SharedCommand::Restart
                        | dto::SharedCommand::CreateContainer
                        | dto::SharedCommand::ReloadContainer
                ) =>
            {
                return self.begin_frozen_command(id, command, row);
            }
            // A configured launch is the one command that needs the invoking
            // frontend's environment; the session derives the rest.
            A::PrepareLaunch { spec } => {
                let Some(state) = self.state.as_ref() else {
                    self.interaction_notice = Some("no fleet inventory: nothing was sent".into());
                    return Ok(None);
                };
                let container = self
                    .nav
                    .selected_id()
                    .and_then(|id| self.nav.node(id))
                    .and_then(|node| super::targets::selected_container(&node.row.kind))
                    .cloned();
                let request_id = crate::control_plane::random_uuid();
                match super::targets::launch_plan(container.as_ref(), &spec, state, &request_id) {
                    Ok(plan) => self
                        .gate
                        .enqueue(plan, dto::SharedCommand::LaunchConfigured),
                    Err(message) => self.interaction_notice = Some(message),
                }
                self.project();
            }
            A::RunCommand { .. } => {
                return refuse(
                    dto::RefusalCode::Unavailable,
                    "that command is not served by the session yet",
                );
            }
        }
        self.project();
        Ok(None)
    }

    /// Opens the question a destructive command asks, or files the effect it
    /// needs no agreement for. The target is the row the invoking view displayed,
    /// looked up by identity, and it is frozen here: another view's selection
    /// cannot retarget it, and a row that is gone refuses instead of moving on.
    fn begin_frozen_command(
        &mut self,
        id: &str,
        command: dto::SharedCommand,
        row: Option<dto::UiRowId>,
    ) -> Result<Option<String>, (dto::RefusalCode, String)> {
        let Some(state) = self.state.as_ref() else {
            self.interaction_notice = Some("no fleet inventory: nothing was sent".into());
            return Ok(None);
        };
        let captured = row.or_else(|| self.nav.selected_id().map(super::session::ui_row_id));
        let Some(captured) = captured else {
            self.interaction_notice = Some("nothing selected: no action target".into());
            return Ok(None);
        };
        let Some(node) = self.nav.node(&super::session::row_id(&captured)) else {
            self.interaction_notice = Some("that row is gone; nothing was changed".into());
            return Ok(None);
        };
        if command == dto::SharedCommand::Focus {
            let row_id = super::session::row_id(&captured);
            let Some(target) = super::targets::focus(self.nav.tree(), state, &row_id) else {
                self.interaction_notice = Some(format!(
                    "{} is not observed: nothing to focus",
                    super::targets::describe(&row_id)
                ));
                return Ok(None);
            };
            // No question: moving a terminal takes nothing away. The session owns
            // where focus goes, so the frontend asks for a row and no more.
            self.gate.enqueue(
                super::targets::focus_plan(target, crate::control_plane::random_uuid()),
                command,
            );
            self.project();
            return Ok(None);
        }
        let operation = match command {
            dto::SharedCommand::Delete => crate::app::Operation::Delete,
            dto::SharedCommand::Restart => crate::app::Operation::Restart,
            dto::SharedCommand::CreateContainer => crate::app::Operation::CreateContainer,
            dto::SharedCommand::ReloadContainer => crate::app::Operation::ReloadContainer,
            _ => return Ok(None),
        };
        let kind = &node.row.kind;
        let container = super::targets::selected_container(kind).cloned();
        let frozen = super::targets::plan(
            operation,
            super::targets::selection(kind, super::targets::represents_tab(kind)),
            state,
            container.as_ref(),
            container
                .as_ref()
                .is_some_and(|container| self.deletions_held.contains(&container.container_id)),
            &crate::control_plane::random_uuid(),
        );
        match frozen {
            // Nothing to ask: the command goes ahead and is still one frozen effect.
            Ok(plan) if !plan.confirms => {
                self.gate.enqueue(plan, command);
                self.project();
                Ok(None)
            }
            Ok(plan) => {
                let interaction = self.gate.begin_confirmation(
                    id,
                    command,
                    Some(captured),
                    Some(plan.prompt.clone()),
                    plan.losses.clone(),
                    plan,
                )?;
                // Opening the question is a published change: the other views have
                // to see that the session is blocked.
                self.project();
                Ok(Some(interaction))
            }
            Err(message) => {
                self.interaction_notice = Some(message);
                self.project();
                Ok(None)
            }
        }
    }

    /// Records which target an effect may hold a repeat of.
    fn remember(&mut self, plan: &super::targets::Plan) {
        if let Some(key) = &plan.held_key {
            self.held.insert(plan.request_id.clone(), key.clone());
        }
    }

    /// What accepted interactions have set in motion, for the actor to dispatch.
    pub fn take_completions(&mut self) -> Vec<Completion> {
        let completions = self.gate.take_completions();
        // Correlation is recorded for effects that reach dispatch, never for a
        // question that may be cancelled: an asked-and-cancelled deletion would
        // otherwise leave an entry nothing will ever settle.
        for completion in &completions {
            if let Some(plan) = &completion.plan {
                self.remember(plan);
            }
        }
        completions
    }

    pub fn tick(&mut self, seconds: u64) {
        if self.gate.tick(seconds) {
            self.interaction_notice =
                Some("project picker abandoned: no presenter claimed within 30 seconds".into());
            self.project();
        }
    }

    pub fn operation(&mut self, outcome: dto::CommandOutcome) {
        use dto::CommandStatus as S;
        if outcome.status == S::DispatchRefused {
            // The effect never left this process, so there is no verdict to wait
            // for and nothing for a repeat to be armed against.
            self.held.remove(&outcome.request_id);
        }
        // Acceptance is not an answer: a dispatched deletion is still on its way
        // to the verdict that decides whether a repeat may force it.
        let terminal = matches!(outcome.status, S::Completed | S::Refused | S::Unknown);
        if terminal && let Some(key) = self.held.remove(&outcome.request_id) {
            let held = outcome.status == S::Refused
                && outcome
                    .answer
                    .as_ref()
                    .and_then(|answer| answer.pointer("/result/request/category"))
                    .and_then(serde_json::Value::as_str)
                    == Some(crate::control_plane::store::Category::AGENTS_ALIVE);
            if held {
                self.deletions_held.insert(key);
            } else if outcome.status == S::Completed {
                self.deletions_held.remove(&key);
            }
        }
        self.operation = Some(outcome);
        self.project();
    }

    fn step(&mut self, step: NavStep) {
        self.nav.apply(&step);
    }

    fn on_row(
        &mut self,
        row: Option<dto::UiRowId>,
        step: NavStep,
    ) -> Result<(), (dto::RefusalCode, String)> {
        if let Some(row) = row
            && !self.nav.apply(&NavStep::Select(row_id(&row)))
        {
            return Err((
                dto::RefusalCode::UnknownRow,
                "that row is not visible".into(),
            ));
        }
        self.nav.apply(&step);
        Ok(())
    }

    /// Rebuilds the published projection from the model. Content that did not
    /// change keeps its hash and its update id.
    fn project(&mut self) -> bool {
        let rows: Vec<dto::TreeRow> = self
            .nav
            .visible_rows()
            .iter()
            .map(|row| dto::TreeRow {
                id: ui_row_id(row.id),
                depth: row.depth as u32,
                label: row.node.row.title().to_string(),
                state: match &row.node.row.kind {
                    RowKind::Agent(agent) => Some(agent_state(&agent.state)),
                    _ => None,
                },
                has_children: row.has_children,
                collapsed: row.collapsed,
                needs_attention: navigation::needs_attention(row),
                working: navigation::is_working(row),
            })
            .collect();
        let tree_hash = hash(&rows);
        let nav = self.nav.state();
        let state = dto::SessionState {
            selected: nav.selected.as_ref().map(ui_row_id),
            order: match nav.order {
                RowOrder::Source => dto::RowOrderDto::Source,
                RowOrder::State => dto::RowOrderDto::State,
                RowOrder::Name => dto::RowOrderDto::Name,
            },
            pane_view: match nav.view {
                PaneView::Hidden => dto::PaneViewDto::Hidden,
                PaneView::Running => dto::PaneViewDto::Running,
                PaneView::All => dto::PaneViewDto::All,
            },
            show_finished: nav.finished_shown,
            tasks: dto::TaskRowsDto {
                agents: nav.task_rows.agents,
                running: nav.task_rows.running,
                all: nav.task_rows.all,
            },
            collapsed: nav.collapsed.iter().map(ui_row_id).collect(),
            gate: self.gate.generation(),
            blocking: self.gate.blocking(),
            operation: self.operation.clone(),
            interaction_notice: self.interaction_notice.clone(),
            filter: dto::FilterState {
                query: nav.filter,
                editor: self.editor.clone(),
            },
        };
        let state_hash = hash(&state);
        if tree_hash == self.current.tree_hash && state_hash == self.current.state_hash {
            return false;
        }
        if tree_hash != self.current.tree_hash {
            // Stored before it is published, so a reference never outruns its content.
            let old = std::mem::replace(&mut self.current.rows, Arc::new(rows));
            if !self.current.tree_hash.is_empty() {
                self.retained
                    .push_back((self.current.tree_hash.clone(), old));
            }
            self.retained.retain(|(held, _)| *held != tree_hash);
            while self.retained.len() > RETAINED_TREES {
                self.retained.pop_front();
            }
        }
        self.current = Published {
            update_id: self.current.update_id + 1,
            tree_hash,
            rows: Arc::clone(&self.current.rows),
            state_hash,
            state,
        };
        true
    }
}

fn hash<T: serde::Serialize>(value: &T) -> String {
    let bytes = serde_json::to_vec(value).expect("projection payloads serialize");
    blake3::hash(&bytes).to_hex().to_string()
}

fn agent_state(state: &AgentState) -> dto::AgentStateDto {
    match state {
        AgentState::Idle => dto::AgentStateDto::Idle,
        AgentState::Working => dto::AgentStateDto::Working,
        AgentState::Waiting => dto::AgentStateDto::Waiting,
        AgentState::Blocked => dto::AgentStateDto::Blocked,
        AgentState::Settling => dto::AgentStateDto::Settling,
        AgentState::Done => dto::AgentStateDto::Done,
        AgentState::Unknown => dto::AgentStateDto::Unknown,
        AgentState::Lost => dto::AgentStateDto::Lost,
        AgentState::Other(name) => dto::AgentStateDto::Other { name: name.clone() },
    }
}

pub fn ui_row_id(id: &RowId) -> dto::UiRowId {
    match id {
        RowId::Workspace(id) => dto::UiRowId::Workspace { id: id.clone() },
        RowId::Agent(pane_id) => dto::UiRowId::Agent {
            pane_id: pane_id.clone(),
        },
        RowId::Registered(agent_id) => dto::UiRowId::Registered {
            agent_id: agent_id.clone(),
        },
        RowId::Pane(pane_id) => dto::UiRowId::Pane {
            pane_id: pane_id.clone(),
        },
        RowId::Task(task) => dto::UiRowId::Task {
            owner: task.owner.clone(),
            session: task.session.clone(),
            id: task.id.clone(),
        },
        RowId::Lifecycle(subject_id) => dto::UiRowId::Lifecycle {
            subject_id: subject_id.clone(),
        },
    }
}

pub fn row_id(id: &dto::UiRowId) -> RowId {
    match id {
        dto::UiRowId::Workspace { id } => RowId::Workspace(id.clone()),
        dto::UiRowId::Agent { pane_id } => RowId::Agent(pane_id.clone()),
        dto::UiRowId::Registered { agent_id } => RowId::Registered(agent_id.clone()),
        dto::UiRowId::Pane { pane_id } => RowId::Pane(pane_id.clone()),
        dto::UiRowId::Task { owner, session, id } => RowId::Task(TaskId {
            owner: owner.clone(),
            session: session.clone(),
            id: id.clone(),
        }),
        dto::UiRowId::Lifecycle { subject_id } => RowId::Lifecycle(subject_id.clone()),
    }
}

/// The navigation state a published [`dto::SessionState`] describes.
pub fn nav_state(state: &dto::SessionState) -> NavState {
    NavState {
        selected: state.selected.as_ref().map(row_id),
        order: match state.order {
            dto::RowOrderDto::Source => RowOrder::Source,
            dto::RowOrderDto::State => RowOrder::State,
            dto::RowOrderDto::Name => RowOrder::Name,
        },
        view: match state.pane_view {
            dto::PaneViewDto::Hidden => PaneView::Hidden,
            dto::PaneViewDto::Running => PaneView::Running,
            dto::PaneViewDto::All => PaneView::All,
        },
        finished_shown: state.show_finished,
        task_rows: TaskRows {
            agents: state.tasks.agents,
            running: state.tasks.running,
            all: state.tasks.all,
        },
        filter: state.filter.query.clone(),
        collapsed: state.collapsed.iter().map(row_id).collect(),
    }
}

/// The action that makes `step` on the session.
pub fn action_of(step: &NavStep) -> dto::UiAction {
    use dto::UiAction as A;
    match step {
        NavStep::Select(id) => A::SelectRow { row: ui_row_id(id) },
        NavStep::Move(delta) => A::Move { delta: *delta },
        NavStep::Collapse => A::Collapse { row: None },
        NavStep::Expand => A::Expand { row: None },
        NavStep::ToggleFold => A::ToggleFold { row: None },
        NavStep::ToggleBranch(id) => A::ToggleBranch { row: ui_row_id(id) },
        NavStep::CyclePanes => A::CyclePaneView,
        NavStep::CycleOrder => A::CycleOrder,
        NavStep::ToggleFinished => A::ToggleFinished,
        NavStep::ToggleTasks => A::ToggleTasks,
        NavStep::ShowEverything => A::ShowEverything,
        NavStep::JumpAttention(forward) => A::NextAttention { forward: *forward },
        NavStep::JumpWorking(forward) => A::NextWorking { forward: *forward },
        NavStep::SetFilter(query) => A::SetFilterQuery {
            query: query.clone(),
        },
        NavStep::ClearFilter => A::ClearFilter,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dto::{ActionOutcome as Out, RefusalCode, UiAction as A};
    use std::time::Duration;

    fn seeded() -> Session {
        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        let mut session = Session::new();
        assert!(session.observe(&state, None));
        session.attach("a", dto::AttachmentRole::Dashboard).unwrap();
        session.attach("b", dto::AttachmentRole::Sidebar).unwrap();
        session
    }

    #[test]
    fn selection_moves_without_republishing_the_tree() {
        let mut session = seeded();
        let before = session.current().clone();
        assert!(matches!(
            session.act("a", 1, A::Move { delta: 1 }),
            Out::Applied { .. }
        ));
        let after = session.current();
        assert_eq!(after.tree_hash, before.tree_hash);
        assert_ne!(after.state_hash, before.state_hash);
        assert!(after.update_id > before.update_id);
        assert!(
            after
                .publication(Some(&before.tree_hash))
                .tree
                .rows
                .is_none()
        );
    }

    #[test]
    fn an_unchanged_observation_publishes_nothing() {
        let mut session = seeded();
        let id = session.current().update_id;
        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        assert!(!session.observe(&state, None));
        assert_eq!(session.current().update_id, id);
    }

    #[test]
    fn actions_apply_once_in_order_and_retries_replay() {
        let mut session = seeded();
        assert_eq!(session.act("a", 2, A::CycleOrder), Out::Gap { expected: 1 });
        let first = session.act("a", 1, A::CycleOrder);
        let Out::Applied { update_id, .. } = first else {
            panic!("applied, got {first:?}");
        };
        // A retry returns the original acceptance and does not cycle again.
        assert_eq!(
            session.act("a", 1, A::CycleOrder),
            Out::Replayed {
                update_id,
                interaction_id: None,
                request_id: None
            }
        );
        assert_eq!(session.current().state.order, dto::RowOrderDto::State);
        // The same id for a different action is refused, not executed.
        assert!(matches!(
            session.act("a", 1, A::ToggleTasks),
            Out::Refused {
                code: RefusalCode::Conflict,
                ..
            }
        ));
        // Each attachment orders independently.
        assert!(matches!(
            session.act("b", 1, A::ToggleTasks),
            Out::Applied { .. }
        ));
        assert_eq!(session.act("gone", 1, A::ToggleTasks), Out::Detached);
    }

    #[test]
    fn an_evicted_receipt_requires_reconciliation() {
        let mut session = seeded();
        for id in 1..=(RECEIPTS as u64 + 2) {
            session.act("a", id, A::CycleOrder);
        }
        assert_eq!(session.act("a", 1, A::CycleOrder), Out::ReceiptExpired);
    }

    #[test]
    fn one_editor_owns_the_filter_and_release_keeps_the_query() {
        let mut session = seeded();
        assert!(matches!(
            session.act("a", 1, A::BeginFilterEdit),
            Out::Applied { .. }
        ));
        assert!(matches!(
            session.act("b", 1, A::BeginFilterEdit),
            Out::Refused {
                code: RefusalCode::NotEditor,
                ..
            }
        ));
        assert!(matches!(
            session.act("b", 2, A::SetFilterQuery { query: "x".into() }),
            Out::Refused {
                code: RefusalCode::NotEditor,
                ..
            }
        ));
        session.act("a", 2, A::SetFilterQuery { query: "pi".into() });
        assert_eq!(session.current().state.filter.editor.as_deref(), Some("a"));
        session.detach("a");
        let filter = &session.current().state.filter;
        assert_eq!(
            (filter.query.as_str(), filter.editor.as_deref()),
            ("pi", None)
        );
    }

    #[test]
    fn replaced_content_is_retrievable_until_evicted() {
        let mut session = seeded();
        let first = session.current().tree_hash.clone();
        session.act("a", 1, A::ToggleTasks);
        session.act("a", 2, A::CyclePaneView);
        session.act("a", 3, A::CyclePaneView);
        session.act("a", 4, A::CyclePaneView);
        session.act("a", 5, A::ToggleTasks);
        // Returning to earlier content reuses its payload and hash.
        assert!(session.content(&first).is_some());
        assert!(session.content("unknown").is_none());
    }

    #[test]
    fn a_destructive_command_acts_on_the_row_its_view_displayed() {
        let mut session = seeded();
        // Every pane, every workspace: the shared action the dashboard uses.
        session.act("a", 1, A::ShowEverything);
        let panes: Vec<dto::UiRowId> = session
            .current()
            .rows
            .iter()
            .filter_map(|row| match &row.id {
                dto::UiRowId::Pane { pane_id } => Some(dto::UiRowId::Pane {
                    pane_id: pane_id.clone(),
                }),
                _ => None,
            })
            .collect();
        let (shown, other) = (panes[0].clone(), panes[1].clone());
        let mut id = 2;
        // The shared selection is elsewhere when the command is ordered: the row
        // the invoking view displayed is what it acts on, never this one.
        session.act("a", id, A::SelectRow { row: other.clone() });
        assert_eq!(
            session.current().state.selected.clone(),
            Some(other.clone())
        );
        id += 1;
        let out = session.act(
            "a",
            id,
            A::RunCommand {
                command: dto::SharedCommand::Delete,
                row: Some(shown.clone()),
            },
        );
        id += 1;
        let Out::Applied {
            interaction_id: Some(interaction),
            ..
        } = out
        else {
            panic!("a deletion asks first: {out:?}");
        };
        let blocking = session.current().state.blocking.clone().expect("blocking");
        assert_eq!(blocking.kind, dto::InteractionKind::Confirm);
        assert_eq!(blocking.stage, dto::InteractionStage::AwaitingResult);
        // The gate holds the session, so the other view cannot move the selection.
        assert!(matches!(
            session.act("b", 1, A::Move { delta: 1 }),
            Out::Refused {
                code: RefusalCode::Blocked,
                ..
            }
        ));
        session.act(
            "a",
            id,
            A::ClaimPresentation {
                interaction_id: interaction.clone(),
            },
        );
        id += 1;
        session.act(
            "a",
            id,
            A::ResolveInteraction {
                interaction_id: interaction,
                result: dto::InteractionResult::Confirm { accepted: true },
            },
        );
        let jobs = session.take_completions();
        let plan = jobs[0].plan.as_ref().expect("a frozen effect");
        let crate::ui_session::targets::Effect::Domain { method, params } = &plan.effect else {
            panic!("a deletion is a domain request");
        };
        let frozen = serde_json::to_string(params).unwrap();
        let (dto::UiRowId::Pane { pane_id: shown }, dto::UiRowId::Pane { pane_id: other }) =
            (&shown, &other)
        else {
            unreachable!("panes were selected")
        };
        assert!(frozen.contains(shown.as_str()), "{frozen}");
        assert!(!frozen.contains(other.as_str()), "{frozen}");
        assert_eq!(method, &"mux.close");
    }

    #[test]
    fn a_held_target_is_correlated_only_once_its_effect_reaches_dispatch() {
        let mut session = seeded();
        let held_plan = super::super::targets::Plan {
            effect: super::super::targets::Effect::Domain {
                method: "mux.close",
                params: serde_json::json!({}),
            },
            request_id: "r1".into(),
            prompt: "Close it".into(),
            losses: vec!["an agent is running".into()],
            confirms: true,
            held_key: Some("c1".into()),
        };
        let interaction = session
            .gate
            .begin_confirmation(
                "a",
                dto::SharedCommand::Delete,
                None,
                Some("Close it".into()),
                vec!["an agent is running".into()],
                held_plan,
            )
            .expect("the question opens");
        assert!(
            session.held.is_empty(),
            "a question that may be cancelled has dispatched nothing yet"
        );
        session
            .gate
            .claim("a", &interaction)
            .expect("the view that asked presents the question");
        session
            .gate
            .resolve(
                "a",
                &interaction,
                dto::InteractionResult::Confirm { accepted: true },
            )
            .expect("the answer is accepted");
        let jobs = session.take_completions();
        assert_eq!(jobs.len(), 1, "the answer dispatches one effect");
        assert_eq!(
            session.held.get("r1").map(String::as_str),
            Some("c1"),
            "the effect that reaches dispatch is the one correlated"
        );
        session.operation(dto::CommandOutcome {
            request_id: jobs[0].request_id.clone(),
            status: dto::CommandStatus::DispatchRefused,
            answer: None,
        });
        assert!(
            session.held.is_empty(),
            "an effect the scheduler refused has no verdict to wait for"
        );
    }

    #[test]
    fn a_held_deletion_is_settled_by_its_verdict_not_by_its_acceptance() {
        let mut session = Session::new();
        // The correlation a container deletion records when it is frozen.
        session.held.insert("request-1".into(), "c1".into());
        session.operation(dto::CommandOutcome {
            request_id: "request-1".into(),
            status: dto::CommandStatus::DispatcherAccepted,
            answer: None,
        });
        assert!(
            !session.deletions_held.contains("c1"),
            "an accepted dispatch has not said why a deletion was refused"
        );
        assert!(session.held.contains_key("request-1"), "the answer is owed");
        session.operation(dto::CommandOutcome {
            request_id: "request-1".into(),
            status: dto::CommandStatus::Refused,
            answer: Some(serde_json::json!({
                "result": {"request": {"category": "agents_alive"}}
            })),
        });
        assert!(
            session.deletions_held.contains("c1"),
            "the refusal arms the forced repeat"
        );
        assert!(
            !session.held.contains_key("request-1"),
            "the answer settled it"
        );
    }

    #[test]
    fn an_answered_confirmation_keeps_the_request_it_froze() {
        let mut session = seeded();
        session.act("a", 1, A::ShowEverything);
        let pane = session
            .current()
            .rows
            .iter()
            .find_map(|row| match &row.id {
                dto::UiRowId::Pane { pane_id } => Some(dto::UiRowId::Pane {
                    pane_id: pane_id.clone(),
                }),
                _ => None,
            })
            .expect("the fixture shows a pane");
        let out = session.act(
            "a",
            2,
            A::RunCommand {
                command: dto::SharedCommand::Delete,
                row: Some(pane),
            },
        );
        let Out::Applied {
            interaction_id: Some(interaction),
            ..
        } = out
        else {
            panic!("a deletion that loses work asks first: {out:?}");
        };
        // The view that presents it claims it, then answers: only the claimed
        // presenter resolves an interaction.
        session.act(
            "a",
            3,
            A::ClaimPresentation {
                interaction_id: interaction.clone(),
            },
        );
        session.act(
            "a",
            4,
            A::ResolveInteraction {
                interaction_id: interaction,
                result: dto::InteractionResult::Confirm { accepted: true },
            },
        );
        let jobs = session.take_completions();
        let frozen = jobs[0].plan.as_ref().expect("a frozen effect");
        // The answer runs the operation under the identity it was frozen with, so
        // its result correlates with the request it is recorded against.
        assert_eq!(jobs[0].request_id, frozen.request_id);
    }

    #[test]
    fn a_focus_asks_no_question_and_names_the_place_the_row_shows() {
        let mut session = seeded();
        session.act("a", 1, A::ShowEverything);
        let pane = session
            .current()
            .rows
            .iter()
            .find_map(|row| match &row.id {
                dto::UiRowId::Pane { pane_id } => Some(pane_id.clone()),
                _ => None,
            })
            .expect("the fixture shows a pane");
        let out = session.act(
            "a",
            2,
            A::RunCommand {
                command: dto::SharedCommand::Focus,
                row: Some(dto::UiRowId::Pane {
                    pane_id: pane.clone(),
                }),
            },
        );
        // Moving a terminal takes nothing away, so focus never opens a gate.
        assert!(
            matches!(
                out,
                Out::Applied {
                    interaction_id: None,
                    ..
                }
            ),
            "{out:?}"
        );
        assert!(session.current().state.blocking.is_none());
        let jobs = session.take_completions();
        assert_eq!(
            match &jobs[0].plan.as_ref().expect("a frozen effect").effect {
                crate::ui_session::targets::Effect::Focus(target) => Some(target.clone()),
                crate::ui_session::targets::Effect::Domain { .. } => None,
            },
            Some(crate::runtime::Target::Pane(pane))
        );
    }

    #[test]
    fn an_answer_that_removes_no_row_is_still_published() {
        let mut session = seeded();
        let before = session.current().clone();
        // A refused effect changes no row, and must still reach every view: the
        // answer is what tells the operator nothing happened.
        session.operation(dto::CommandOutcome {
            request_id: "request-1".into(),
            status: dto::CommandStatus::Refused,
            answer: Some(serde_json::json!({
                "error": {"code": "agents_alive", "message": "an agent inside did not exit"}
            })),
        });
        let after = session.current();
        assert_eq!(after.tree_hash, before.tree_hash, "no row moved");
        assert!(
            after.update_id > before.update_id,
            "the answer was published"
        );
        let outcome = after.state.operation.clone().expect("the answer");
        assert_eq!(outcome.request_id, "request-1");
        assert_eq!(outcome.status, dto::CommandStatus::Refused);
    }

    #[test]
    fn a_row_that_is_not_visible_is_refused_not_substituted() {
        let mut session = seeded();
        let selected = session.current().state.selected.clone();
        let out = session.act(
            "a",
            1,
            A::SelectRow {
                row: dto::UiRowId::Pane {
                    pane_id: "nope".into(),
                },
            },
        );
        assert!(matches!(
            out,
            Out::Refused {
                code: RefusalCode::UnknownRow,
                ..
            }
        ));
        assert_eq!(session.current().state.selected, selected);
    }

    #[test]
    fn the_shared_filter_is_fuzzy_and_keeps_the_ancestry_of_matches() {
        let mut session = seeded();
        let labels = |session: &Session| -> Vec<String> {
            session
                .current()
                .rows
                .iter()
                .map(|row| row.label.clone())
                .collect()
        };
        let all = labels(&session);
        let title = all
            .iter()
            .find(|label| label.len() >= 4 && label.is_ascii())
            .expect("a row with a usable label")
            .clone();
        // Every other letter of a label: not a substring, still a fuzzy match.
        let skipped: String = title.chars().step_by(2).collect();
        assert!(!title.to_lowercase().contains(&skipped.to_lowercase()));
        session.act("a", 1, A::BeginFilterEdit);
        session.act("a", 2, A::SetFilterQuery { query: skipped });
        let shown = labels(&session);
        assert!(shown.contains(&title), "{shown:?} should keep {title}");
        assert!(shown.len() < all.len() || all.len() == 1);
        // Ancestry stays: the first row of a filtered tree is a root.
        assert_eq!(session.current().rows[0].depth, 0);
    }

    /// The fixture fleet repeated under distinct ids: `copies` times its size.
    fn large_fleet(copies: usize) -> ObservationState {
        let text = serde_json::to_string(&crate::model::testing::fleet()).unwrap();
        let mut fleet: serde_json::Value = serde_json::from_str(&text).unwrap();
        let mut merged = fleet.clone();
        for copy in 1..copies {
            let renamed = text
                .replace("\"wA", &format!("\"w{copy}A"))
                .replace("\"wB", &format!("\"w{copy}B"));
            let part: serde_json::Value = serde_json::from_str(&renamed).unwrap();
            for key in ["workspaces", "tabs", "panes", "agents"] {
                merged[key]
                    .as_array_mut()
                    .unwrap()
                    .extend(part[key].as_array().unwrap().iter().cloned());
            }
        }
        let _ = &mut fleet;
        let mut state = ObservationState::new();
        state.apply_success(serde_json::from_value(merged).unwrap());
        state
    }

    /// The latency exercise of milestone B, as a bounded check: the work behind
    /// one keypress on a fleet well beyond a real one stays far below a frame.
    /// The numbers are printed so the exercise can be repeated and compared.
    #[test]
    fn the_work_behind_one_keypress_stays_small_on_a_large_fleet() {
        use std::time::Instant;
        let state = large_fleet(120);
        let mut session = Session::new();
        session.attach("a", dto::AttachmentRole::Dashboard).unwrap();
        let started = Instant::now();
        session.observe(&state, None);
        let build = started.elapsed();
        let rows = session.current().rows.len();
        assert!(
            rows > 500,
            "the fleet is large enough to mean something: {rows}"
        );

        let mut moves = Vec::new();
        for number in 1..=300u64 {
            let started = Instant::now();
            let delta = if number % 2 == 0 { 1 } else { -1 };
            session.act("a", number, A::Move { delta });
            moves.push(started.elapsed());
        }
        let mut filters = Vec::new();
        session.act("a", 301, A::BeginFilterEdit);
        let mut query = String::new();
        for (offset, letter) in "agentreviewer".chars().enumerate() {
            query.push(letter);
            let started = Instant::now();
            session.act(
                "a",
                302 + offset as u64,
                A::SetFilterQuery {
                    query: query.clone(),
                },
            );
            filters.push(started.elapsed());
        }
        moves.sort();
        filters.sort();
        let worst = |times: &[std::time::Duration]| times[times.len() - 1];
        let median = |times: &[std::time::Duration]| times[times.len() / 2];
        eprintln!(
            "rows={rows} observe={build:?} move median={:?} max={:?} filter median={:?} max={:?}",
            median(&moves),
            worst(&moves),
            median(&filters),
            worst(&filters),
        );
        // Debug builds are several times slower than release; this bound only
        // catches a hot path that has become the wrong shape.
        assert!(
            worst(&moves) < Duration::from_millis(100),
            "{:?}",
            worst(&moves)
        );
        assert!(
            worst(&filters) < Duration::from_millis(250),
            "{:?}",
            worst(&filters)
        );
    }

    fn open_gate(session: &mut Session, by: &str, number: u64) -> String {
        let Out::Applied {
            interaction_id: Some(interaction),
            ..
        } = session.act(
            by,
            number,
            A::RunCommand {
                command: dto::SharedCommand::NewWorkspace,
                row: None,
            },
        )
        else {
            panic!("the command should open an interaction");
        };
        interaction
    }

    fn offer(session: &mut Session, by: &str, number: u64, interaction: &str) {
        let offered = session.act(
            by,
            number,
            A::ProvideChoices {
                interaction_id: interaction.into(),
                choices: vec!["/p/one".into(), "/p/two".into(), "/p/one".into()],
                note: None,
            },
        );
        assert!(matches!(offered, Out::Applied { .. }), "{offered:?}");
    }

    fn refused_with(outcome: Out) -> RefusalCode {
        match outcome {
            Out::Refused { code, .. } => code,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn an_open_interaction_blocks_the_session_until_one_result_wins() {
        let mut session = seeded();
        // Editing ends when an interaction begins, and the query is kept.
        session.act("a", 1, A::BeginFilterEdit);
        session.act("a", 2, A::SetFilterQuery { query: "ag".into() });
        let interaction = open_gate(&mut session, "a", 3);
        assert_eq!(session.current().state.filter.editor, None);
        assert_eq!(session.current().state.filter.query, "ag");
        let blocking = session.current().state.blocking.clone().unwrap();
        assert_eq!(blocking.stage, dto::InteractionStage::Preparing);

        // Nothing the screens agree on changes while it is open, for anyone.
        for (by, number) in [("a", 4), ("b", 1)] {
            assert_eq!(
                refused_with(session.act(by, number, A::CycleOrder)),
                RefusalCode::Blocked
            );
        }
        // Another interaction cannot open beside it.
        assert_eq!(
            refused_with(session.act(
                "b",
                2,
                A::RunCommand {
                    command: dto::SharedCommand::NewWorkspace,
                    row: None
                }
            )),
            RefusalCode::Blocked
        );

        // A result before the choices exist, from the wrong view, or outside
        // the choices does not close it.
        let project = |path: &str| A::ResolveInteraction {
            interaction_id: interaction.clone(),
            result: dto::InteractionResult::Project { path: path.into() },
        };
        assert_eq!(
            refused_with(session.act("b", 3, project("/p/one"))),
            RefusalCode::Conflict
        );
        assert_eq!(
            refused_with(session.act(
                "b",
                4,
                A::ProvideChoices {
                    interaction_id: interaction.clone(),
                    choices: vec![],
                    note: None,
                }
            )),
            RefusalCode::Role
        );
        offer(&mut session, "a", 5, &interaction);
        assert_eq!(
            session.current().state.blocking.as_ref().unwrap().stage,
            dto::InteractionStage::AwaitingResult
        );
        assert!(matches!(
            session.act(
                "b",
                5,
                A::ClaimPresentation {
                    interaction_id: interaction.clone()
                }
            ),
            Out::Applied { .. }
        ));
        assert_eq!(
            refused_with(session.act("b", 6, project("/elsewhere"))),
            RefusalCode::BadResult
        );

        // The first valid result wins; the cancel that follows finds nothing.
        assert!(matches!(
            session.act("b", 7, project("/p/two")),
            Out::Applied { .. }
        ));
        assert_eq!(session.current().state.blocking, None);
        assert_eq!(
            refused_with(session.act(
                "a",
                6,
                A::CancelInteraction {
                    interaction_id: interaction.clone()
                }
            )),
            RefusalCode::NoInteraction
        );
        let done = session.take_completions();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0].interaction_id, interaction);
        assert_eq!(
            done[0].result,
            dto::InteractionResult::Project {
                path: "/p/two".into()
            }
        );
        // The session navigates again.
        assert!(matches!(
            session.act("b", 8, A::CycleOrder),
            Out::Applied { .. }
        ));
    }

    #[test]
    fn any_view_can_cancel_and_a_cancelled_result_never_dispatches() {
        let mut session = seeded();
        let interaction = open_gate(&mut session, "a", 1);
        offer(&mut session, "a", 2, &interaction);
        let cancel = A::CancelInteraction {
            interaction_id: interaction.clone(),
        };
        assert!(matches!(session.act("b", 1, cancel), Out::Applied { .. }));
        let late = session.act(
            "a",
            3,
            A::ResolveInteraction {
                interaction_id: interaction,
                result: dto::InteractionResult::Project {
                    path: "/p/one".into(),
                },
            },
        );
        assert_eq!(refused_with(late), RefusalCode::NoInteraction);
        assert!(session.take_completions().is_empty());
    }

    #[test]
    fn input_from_before_the_gate_changed_is_refused_but_current_input_is_not() {
        let mut session = seeded();
        let before = session.current().state.gate;
        let interaction = open_gate(&mut session, "a", 1);
        session.act(
            "a",
            2,
            A::CancelInteraction {
                interaction_id: interaction,
            },
        );
        assert!(session.current().state.gate > before);
        let stale = session.act_stamped("a", 3, Some(before), A::CycleOrder);
        assert_eq!(refused_with(stale), RefusalCode::StaleGate);
        let current = session.current().state.gate;
        assert!(matches!(
            session.act_stamped("a", 4, Some(current), A::CycleOrder),
            Out::Applied { .. }
        ));
    }

    #[test]
    fn one_presenter_holds_the_claim_and_losing_the_invoker_or_presenter_cannot_wedge_the_gate() {
        let mut session = seeded();
        session
            .attach("p1", dto::AttachmentRole::Presenter)
            .unwrap();
        session
            .attach("p2", dto::AttachmentRole::Presenter)
            .unwrap();
        let interaction = open_gate(&mut session, "a", 1);
        let claim = || A::ClaimPresentation {
            interaction_id: interaction.clone(),
        };
        // Nothing to present while the invoker is still preparing.
        assert_eq!(
            refused_with(session.act("p1", 1, claim())),
            RefusalCode::Conflict
        );
        offer(&mut session, "a", 2, &interaction);
        assert!(matches!(session.act("p1", 2, claim()), Out::Applied { .. }));
        assert_eq!(
            refused_with(session.act("p2", 1, claim())),
            RefusalCode::Conflict
        );
        assert!(
            session
                .current()
                .state
                .blocking
                .as_ref()
                .unwrap()
                .presenter
                .is_some()
        );

        // Presenter loss abandons the interaction rather than wedging the gate.
        session.detach("p1");
        assert!(session.current().state.blocking.is_none());
        assert_eq!(
            refused_with(session.act("p2", 2, claim())),
            RefusalCode::NoInteraction
        );

        // An invoker lost while still preparing ends its interaction.
        session.act(
            "p2",
            3,
            A::CancelInteraction {
                interaction_id: interaction,
            },
        );
        let second = open_gate(&mut session, "a", 3);
        assert_eq!(
            session
                .current()
                .state
                .blocking
                .as_ref()
                .unwrap()
                .interaction_id,
            second
        );
        session.detach("a");
        assert_eq!(session.current().state.blocking, None);
    }

    #[test]
    fn an_unclaimed_presentation_expires_without_cancelling_a_claimed_one() {
        let mut session = seeded();
        let id = open_gate(&mut session, "a", 1);
        offer(&mut session, "a", 2, &id);
        session.tick(29);
        assert!(session.current().state.blocking.is_some());
        session.tick(1);
        assert!(session.current().state.blocking.is_none());
        let id = open_gate(&mut session, "a", 3);
        offer(&mut session, "a", 4, &id);
        session.act("b", 1, A::ClaimPresentation { interaction_id: id });
        session.tick(120);
        assert!(
            session
                .current()
                .state
                .blocking
                .as_ref()
                .unwrap()
                .presenter
                .is_some()
        );
    }
}
