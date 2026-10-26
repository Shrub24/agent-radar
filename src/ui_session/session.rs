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

use super::navigation::{self, Navigation, PaneView, RowOrder};
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
        if self.editor.as_deref() == Some(id) {
            self.editor = None;
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
        use dto::ActionOutcome as Out;
        let fingerprint = serde_json::to_string(&action).unwrap_or_default();
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
                    Out::Applied { update_id } => Out::Replayed {
                        update_id: *update_id,
                        interaction_id: None,
                        request_id: None,
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
        let outcome = match self.apply(id, role, action) {
            Ok(()) => Out::Applied {
                update_id: self.current.update_id,
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
    ) -> Result<(), (dto::RefusalCode, String)> {
        use dto::UiAction as A;
        let refuse = |code, message: &str| Err((code, message.to_string()));
        if role == dto::AttachmentRole::Presenter
            && !matches!(
                action,
                A::ResolveInteraction { .. }
                    | A::CancelInteraction { .. }
                    | A::ClaimPresentation { .. }
                    | A::AcknowledgePresentation { .. }
            )
        {
            return refuse(
                dto::RefusalCode::Role,
                "a presenter only resolves its interaction",
            );
        }
        let owns = self.editor.as_deref() == Some(id);
        let foreign = self.editor.is_some() && !owns;
        match action {
            A::SelectRow { row } => {
                if !self.nav.select_exact(&row_id(&row)) {
                    return refuse(dto::RefusalCode::UnknownRow, "that row is not visible");
                }
            }
            A::Move { delta } => self.nav.move_selection(delta),
            A::Collapse { row } => self.on_row(row, Navigation::collapse_selected)?,
            A::Expand { row } => self.on_row(row, Navigation::expand_selected)?,
            A::ToggleFold { row } => self.on_row(row, Navigation::toggle_fold)?,
            A::CyclePaneView => self.nav.cycle_panes(),
            A::CycleOrder => self.nav.cycle_order(),
            A::ToggleFinished => self.nav.toggle_finished(),
            A::ToggleTasks => self.nav.toggle_tasks(),
            A::ShowEverything => self.nav.show_everything(),
            A::NextAttention { forward } => self.nav.jump_attention(forward),
            A::NextWorking { forward } => self.nav.jump_working(forward),
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
                self.nav.set_filter(&query);
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
                self.nav.clear_filter();
            }
            A::RunCommand { .. }
            | A::ResolveInteraction { .. }
            | A::CancelInteraction { .. }
            | A::ClaimPresentation { .. }
            | A::AcknowledgePresentation { .. } => {
                return refuse(
                    dto::RefusalCode::Unavailable,
                    "interactions are not served yet",
                );
            }
        }
        self.project();
        Ok(())
    }

    fn on_row(
        &mut self,
        row: Option<dto::UiRowId>,
        op: fn(&mut Navigation),
    ) -> Result<(), (dto::RefusalCode, String)> {
        if let Some(row) = row
            && !self.nav.select_exact(&row_id(&row))
        {
            return Err((
                dto::RefusalCode::UnknownRow,
                "that row is not visible".into(),
            ));
        }
        op(&mut self.nav);
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
        let state = dto::SessionState {
            selected: self.nav.selected_id().map(ui_row_id),
            order: match self.nav.order() {
                RowOrder::Source => dto::RowOrderDto::Source,
                RowOrder::State => dto::RowOrderDto::State,
                RowOrder::Name => dto::RowOrderDto::Name,
            },
            pane_view: match self.nav.pane_view() {
                PaneView::Hidden => dto::PaneViewDto::Hidden,
                PaneView::Running => dto::PaneViewDto::Running,
                PaneView::All => dto::PaneViewDto::All,
            },
            show_finished: self.nav.shows_finished(),
            show_tasks: self.nav.shows_tasks(),
            filter: dto::FilterState {
                query: self.nav.filter_query().to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use dto::{ActionOutcome as Out, RefusalCode, UiAction as A};

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
        let Out::Applied { update_id } = first else {
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
}
