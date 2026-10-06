//! Interaction state for the fleet tree: selection, folding, filtering, the
//! ordinary-pane toggle, and the bus data shown beside the tree.
//!
//! [`App`] owns the projected [`FleetTree`] plus the interaction state that
//! survives refreshes. Everything here is terminal-free except
//! [`App::handle_key`] and [`App::handle_mouse`], which consume crossterm
//! events so the keymap and the pointer gestures live with the view they drive,
//! and answer with the one [`Action`] the main loop has to carry out. The main loop decides quitting: it must send
//! keys to [`App::handle_key`] while [`App::is_filter_editing`] is true, and
//! treat `q` as quit only when filter editing is not active.
//!
//! Selection and fold state are keyed by [`RowId`], never by row index, so a
//! refresh that changes facts (or inserts/removes rows elsewhere) keeps the
//! user where they were. When the selected row disappears, selection moves to
//! the row that took its place, or to the last row, or becomes empty when no
//! rows remain.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};

use crate::bus::{BusEvent, BusState};
use crate::control::{self, ControlResult, Outcome};
use crate::lifecycle::{self, CloseRequest, Containment, TargetIdentity};
use crate::observation::{ObservationState, SourceFreshness};
use crate::runtime::{CloseTarget, Target};
use crate::theme;
use ratatui::layout::Rect;

use crate::model::{AgentState, FleetObservation};
use crate::tree::{AgentRow, FleetTree, PaneRow, RowId, RowKind, TaskRow, TreeNode};

/// How long a focus message stays up when no key clears it first.
const FOCUS_MESSAGE_TTL: Duration = Duration::from_secs(5);

/// The width of a row's disclosure marker in cells: `ui::fold_marker` draws it,
/// and a leaf keeps the same two blank columns, so the hit cell is the same
/// whether or not the row has children to show.
const DISCLOSURE_WIDTH: u16 = 2;

/// What a key asked the main loop to do outside the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Ask Herdr to focus this location.
    Focus(Target),
    /// `x`/`X`/`r` asked for confirmation of a lifecycle action. The caller
    /// opens it against the current observation, because eligibility is a
    /// property of the evidence, not of the cursor.
    BeginAction(Operation),
    /// Confirm was activated; the caller revalidates the frozen target and
    /// starts the close or manages the request when it still holds.
    ConfirmAction,
}

/// The lifecycle operation a confirmation is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// Close the selected row's pane.
    ClosePane,
    /// Close the selected row's tab.
    CloseTab,
    /// Restart the selected managed worker through its owner.
    Restart,
}

/// One row of the current view: the projected node plus its display depth,
/// fold state and branch prefix.
#[derive(Clone, Debug)]
pub struct VisibleRow<'a> {
    pub id: &'a RowId,
    pub depth: usize,
    pub node: &'a TreeNode,
    /// Whether the row has children that this view can show (a workspace whose
    /// only panes are hidden has none).
    pub has_children: bool,
    pub collapsed: bool,
    /// The branch prefix to draw before the row's own marks.
    pub connectors: Connectors,
}

impl VisibleRow<'_> {
    /// Where this row's disclosure marker begins, in cells from the panel
    /// content's left edge: exactly the branch prefix drawn before it. The draw
    /// and a later marker hit test read one layout through this.
    pub fn marker_column(&self) -> u16 {
        (self.connectors.continuation.len() as u16 + u16::from(self.depth > 0)) * 2
    }
}

/// How a visible row's branch prefix reads: the vertical continuation lines its
/// visible ancestry requires, and whether a visible sibling follows it.
///
/// Derived while flattening the ordered, filtered view, so folds and filtering
/// change it with the rows actually drawn; raw child indices cannot say this.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Connectors {
    /// One flag per nested ancestor from the workspace inward: `true` where that
    /// ancestor is followed by a visible sibling, so its branch line continues
    /// past this row.
    pub continuation: Vec<bool>,
    /// Whether a visible sibling follows this row: the branch connector when
    /// true, the last-child connector when false.
    pub has_following_sibling: bool,
}

/// The prefix a child row draws: its parent's continuation columns, then the
/// parent's own column when the parent is itself nested. A workspace root draws
/// no connector, so its children start with an empty prefix and it leaves no
/// continuation behind them.
fn child_connectors(
    parent: &Connectors,
    parent_depth: usize,
    has_following_sibling: bool,
) -> Connectors {
    let mut continuation = parent.continuation.clone();
    if parent_depth > 0 {
        continuation.push(parent.has_following_sibling);
    }
    Connectors {
        continuation,
        has_following_sibling,
    }
}

/// Which ordinary panes the fleet tree lists.
///
/// Agents are the fleet; panes are the places work is happening without one.
/// `Running` is the middle answer: the panes with a command actually in the
/// foreground, which is where a build, an editor or a merge tool is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneView {
    /// Agents only.
    #[default]
    Hidden,
    /// Panes with a command in the foreground.
    Running,
    /// Every pane, whatever it is doing.
    All,
}

impl PaneView {
    /// The view `p` moves to next.
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Running,
            Self::Running => Self::All,
            Self::All => Self::Hidden,
        }
    }

    /// A short name for the hint line and tests.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "agents",
            Self::Running => "running",
            Self::All => "all",
        }
    }
}

/// Which rows the tree lists: the pane view, plus whether finished sessions are
/// listed while ordinary panes are hidden and whether background tasks are
/// listed in this view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Visibility {
    view: PaneView,
    finished: bool,
    tasks: bool,
}

impl Visibility {
    /// Whether this view lists a row of this kind. A workspace, an agent and an
    /// ordinary pane follow the pane view; background tasks follow their own
    /// per-view choice.
    fn includes(self, kind: &RowKind) -> bool {
        match kind {
            RowKind::Task(_) => self.tasks,
            RowKind::Pane(pane) => match self.view {
                // A finished session is history rather than fleet, so only the
                // explicit toggle brings it into a view without panes — and
                // only while the pane is not in use again.
                PaneView::Hidden => {
                    self.finished && pane.exited.is_some() && pane.command().is_none()
                }
                PaneView::Running => pane.command().is_some(),
                PaneView::All => true,
            },
            RowKind::Workspace { .. } | RowKind::Agent(_) => true,
        }
    }
}

/// Whether background-task children are listed, per pane view, for this Radar
/// run.
///
/// A presentation choice rather than a fact: `agents` view is the fleet, so it
/// starts with the children hidden, while `running` and `all` are the operator's
/// view of outstanding work and start with them shown. Each view keeps its own
/// answer, so `b` in one view cannot change what another lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TaskRows {
    agents: bool,
    running: bool,
    all: bool,
}

impl Default for TaskRows {
    fn default() -> Self {
        Self {
            agents: false,
            running: true,
            all: true,
        }
    }
}

impl TaskRows {
    fn shown(self, view: PaneView) -> bool {
        match view {
            PaneView::Hidden => self.agents,
            PaneView::Running => self.running,
            PaneView::All => self.all,
        }
    }

    fn toggle(&mut self, view: PaneView) {
        match view {
            PaneView::Hidden => self.agents = !self.agents,
            PaneView::Running => self.running = !self.running,
            PaneView::All => self.all = !self.all,
        }
    }
}

/// The fleet-overview view state: tree, selection, folds, filter and which
/// panes are listed.
#[derive(Debug, Default)]
pub struct App {
    tree: FleetTree,
    collapsed: HashSet<RowId>,
    /// Filter query as typed; empty means "no filter".
    filter: String,
    filter_editing: bool,
    view: PaneView,
    /// Whether the details panel is hidden. Stored inverted so `Default` — the
    /// state a run starts in — is "details shown".
    details_hidden: bool,
    /// Whether finished sessions are listed while ordinary panes are hidden.
    /// Off by default: the agents view is the live fleet, and a fleet with its
    /// history in it stops being an overview.
    finished_shown: bool,
    /// Whether task children are listed, remembered per pane view for this run.
    task_rows: TaskRows,
    selected: Option<RowId>,
    /// Index of the selected row in the last visible-row list, used as the
    /// fallback position when the selected row disappears.
    anchor: usize,
    /// Bus data pushed by connected extensions, joined to rows when they are
    /// drawn. It sits beside the tree rather than inside it: bus facts are true
    /// only while their connection is open, so a refresh must never be able to
    /// change them or keep them alive.
    bus: BusState,
    /// Why the bus is not running, when the listener could not bind. Kept
    /// apart from source freshness: a bus that never bound says nothing about
    /// the fleet Radar observes.
    bus_diagnostic: Option<String>,
    /// Focus target per row, for the rows the displayed observation proves. A
    /// row missing here is one `Enter` must refuse and explain.
    focus_targets: HashMap<RowId, Target>,
    /// Whether the displayed inventory came from a successful collection. A
    /// stale inventory is last-good rather than current, so `Enter` refuses on
    /// every row of it.
    source_current: bool,
    /// One line about the last action Radar took or refused — a focus or a
    /// lifecycle close — and when it was set. It clears on the next key press or
    /// after [`FOCUS_MESSAGE_TTL`].
    focus_message: Option<(String, Instant)>,
    /// How siblings are ordered. Structure is never changed by it.
    order: RowOrder,
    /// Where the last draw put things, and how far the details are scrolled.
    layout: Geometry,
    details_scroll: u16,
    /// A wheel turn over the tree, waiting for the main loop to apply it to the
    /// list state it owns.
    scroll_request: Option<usize>,
    /// The open lifecycle confirmation, if any. Its target is frozen at the
    /// moment it opened; nothing is sent until it is revalidated and confirmed.
    confirmation: Option<Confirmation>,
    /// Lifecycle action outcomes, kept until dismissed. Separate from the
    /// transient focus message and from the source diagnostics, so an owner's
    /// answer cannot be mistaken for the fleet's freshness.
    notices: Vec<Notice>,
}

/// A lifecycle confirmation, frozen at the moment it opened.
///
/// The operation and target identity are held here, not read from the cursor:
/// moving the selection while the dialog is up must not retarget it.
#[derive(Clone, Debug)]
pub struct Confirmation {
    pub operation: Operation,
    pub target: CloseTarget,
    /// The observed target identity frozen when the dialog opened. Confirm
    /// revalidates against it, so a target replaced while the dialog is up is
    /// never acted on even when the replacement is equally eligible.
    pub identity: TargetIdentity,
    /// What the operator may lose, one short line each. Text from the pane,
    /// sanitized when it is drawn like every other runtime string.
    pub losses: Vec<String>,
    /// Whether Confirm is selected. Cancel is the default.
    pub confirm_selected: bool,
    /// The exact owner-routed action, when this confirmation is a managed close
    /// or restart rather than a direct runtime close.
    pub managed: Option<lifecycle::ManagedRequest>,
}

/// A confirmed action ready to run: a direct runtime close, or an owner-routed
/// managed request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmed {
    Direct(CloseRequest),
    Managed(lifecycle::ManagedRequest),
}

/// One lifecycle outcome, kept visible until it is dismissed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The request id, or the exact target key before one was minted. A later
    /// update with the same id refines this line rather than adding another.
    pub id: String,
    pub text: String,
    /// A refusal or invalid evidence, drawn as a failure rather than a result.
    pub failed: bool,
}

/// Where the last draw put the panels, so a mouse event can be mapped back to
/// the row it landed on. Radar binds nothing else to the pointer, so this is the whole
/// of what it has to remember about its own layout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    /// The tree panel, its frame included.
    pub tree_panel: Rect,
    /// The tree's content, where the rows are drawn.
    pub tree_content: Rect,
    /// The details panel, when it was drawn.
    pub details: Option<Rect>,
    /// How many lines the details hold, so a wheel over them can clamp.
    pub details_lines: usize,
    /// The first row the list drew.
    pub offset: usize,
    /// The confirmation dialog's Cancel button, when one was drawn.
    pub confirm_cancel: Option<Rect>,
    /// The confirmation dialog's Confirm button, when one was drawn.
    pub confirm_confirm: Option<Rect>,
}

/// How rows within a level are ordered. Ordering never moves a row out of its
/// parent, its group or its workspace — it only changes the sequence of
/// siblings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RowOrder {
    /// The order the observation arrived in. The default.
    #[default]
    Source,
    /// Rows that need a human first, then the rest by the state's own weight.
    /// A group ranks by its most urgent row.
    State,
    /// Alphabetically, by the label a row shows.
    Name,
}

impl RowOrder {
    /// The order's name, as the hint line states it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::State => "state",
            Self::Name => "name",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Source => Self::State,
            Self::State => Self::Name,
            Self::Name => Self::Source,
        }
    }
}

impl App {
    /// A fresh view: no tree, no selection, ordinary panes hidden, no bus data.
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuilds the projected tree and re-validates selection.
    ///
    /// Fold state is kept for every row identity that survives; the selection
    /// follows its row identity, and falls back to the nearest surviving row
    /// when that row is gone. The focus targets are re-derived here too: what
    /// `Enter` may focus is a property of the observation that was just drawn.
    pub fn refresh(&mut self, state: &ObservationState) {
        self.tree = FleetTree::build(state);
        self.tree.attach_tasks(&self.bus);
        self.source_current = matches!(state.source_freshness(), SourceFreshness::Current);
        self.focus_targets = if self.source_current {
            crate::focus::targets(&self.tree, state)
        } else {
            HashMap::new()
        };
        self.reconcile_selection();
    }

    /// The projected tree as of the last [`Self::refresh`].
    pub fn tree(&self) -> &FleetTree {
        &self.tree
    }

    /// Rows currently displayed, in source order, honouring folds, the
    /// ordinary-pane toggle and the filter.
    pub fn visible_rows(&self) -> Vec<VisibleRow<'_>> {
        let mut rows = Vec::new();
        match self.needle() {
            Some(needle) => {
                let roots = ordered(&self.tree.roots, self.order)
                    .into_iter()
                    .filter(|root| subtree_matches(root, &needle, self.visibility(), self.order));
                for root in roots {
                    collect_filtered(
                        root,
                        0,
                        &needle,
                        self.visibility(),
                        self.order,
                        Connectors::default(),
                        &mut rows,
                    );
                }
            }
            None => {
                for root in ordered(&self.tree.roots, self.order) {
                    collect_visible(
                        root,
                        0,
                        self.visibility(),
                        &self.collapsed,
                        self.order,
                        Connectors::default(),
                        &mut rows,
                    );
                }
            }
        }
        rows
    }

    /// Records where the draw just put the panels, and keeps the details scroll
    /// inside the content that is now there.
    pub fn note_layout(&mut self, layout: Geometry) {
        let visible = layout
            .details
            .map(|area| area.height.saturating_sub(2))
            .unwrap_or(0) as usize;
        let max = layout.details_lines.saturating_sub(visible) as u16;
        self.details_scroll = self.details_scroll.min(max);
        self.layout = layout;
    }

    /// How far the details are scrolled, in lines.
    pub fn details_scroll(&self) -> u16 {
        self.details_scroll
    }

    /// The scroll a wheel turn asked the tree for, taken once by the main loop.
    pub fn take_scroll(&mut self) -> Option<usize> {
        self.scroll_request.take()
    }

    /// Applies one mouse event: the wheel scrolls the panel under the pointer,
    /// a left click selects the row it lands on, folds a heading it lands on, or
    /// focuses a row that was already selected. A position holding no row does
    /// nothing.
    pub fn handle_mouse(&mut self, event: MouseEvent) -> Option<Action> {
        let at = (event.column, event.row);
        if self.confirmation.is_some() {
            // The dialog is modal: the tree behind it answers no pointer event.
            return match event.kind {
                MouseEventKind::Down(MouseButton::Left) => self.confirmation_click(at),
                _ => None,
            };
        }
        match event.kind {
            MouseEventKind::ScrollDown => {
                self.wheel(at, true);
                None
            }
            MouseEventKind::ScrollUp => {
                self.wheel(at, false);
                None
            }
            MouseEventKind::Down(MouseButton::Left) => self.click(at),
            _ => None,
        }
    }

    /// A wheel turn: the details panel scrolls its own lines, the tree moves the
    /// list's first drawn row, and a turn anywhere else does nothing.
    fn wheel(&mut self, at: (u16, u16), down: bool) {
        const STEP: usize = 3;
        if inside(self.layout.details, at) {
            let visible = self
                .layout
                .details
                .map(|area| area.height.saturating_sub(2))
                .unwrap_or(0) as usize;
            let max = self.layout.details_lines.saturating_sub(visible) as u16;
            let step = STEP as u16;
            self.details_scroll = if down {
                (self.details_scroll + step).min(max)
            } else {
                self.details_scroll.saturating_sub(step)
            };
            return;
        }
        if !inside(Some(self.layout.tree_panel), at) {
            return;
        }
        let visible = self.layout.tree_content.height as usize;
        let max = self.visible_rows().len().saturating_sub(visible);
        let offset = self.layout.offset;
        let next = if down {
            (offset + STEP).min(max)
        } else {
            offset.saturating_sub(STEP)
        };
        if next != offset {
            self.scroll_request = Some(next);
        }
    }

    /// A left click. A heading folds whether or not it was selected; an agent
    /// branch's disclosure cell folds that branch and sends nothing; a row that
    /// was already selected is the one the click acts on.
    fn click(&mut self, at: (u16, u16)) -> Option<Action> {
        let (id, disclosure) = self.row_under(at)?;
        if matches!(id, RowId::Workspace(_)) {
            self.selected = Some(id);
            self.toggle_fold();
            return None;
        }
        if disclosure {
            // The pointer said where, not which row to act on: the disclosure
            // cell folds this branch alone and leaves Herdr alone.
            self.toggle_branch(&id);
            return None;
        }
        let already = self.selected.as_ref() == Some(&id);
        self.selected = Some(id);
        if already {
            return self.focus_selected();
        }
        None
    }

    /// The row drawn at a position, and whether the position falls on its
    /// disclosure cell. A folded row's descendants are not drawn, so they
    /// cannot be under the pointer.
    fn row_under(&self, at: (u16, u16)) -> Option<(RowId, bool)> {
        let content = self.layout.tree_content;
        if !inside(Some(content), at) {
            return None;
        }
        let index = self.layout.offset + (at.1 - content.y) as usize;
        let row = self.visible_rows().into_iter().nth(index)?;
        let column = at.0.saturating_sub(content.x);
        let marker = row.marker_column();
        let disclosure = row.has_children && (marker..marker + DISCLOSURE_WIDTH).contains(&column);
        Some((row.id.clone(), disclosure))
    }

    /// Folds or unfolds one branch by identity, without moving the selection.
    /// Ignored while a filter is active, as the fold keys are: the filtered
    /// view shows matching ancestry on purpose, so a fold could not take effect.
    fn toggle_branch(&mut self, id: &RowId) {
        if self.needle().is_some() {
            return;
        }
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.clone());
        }
        self.reconcile_selection();
    }

    /// The order siblings are shown in, which `s` cycles.
    pub fn order(&self) -> RowOrder {
        self.order
    }

    /// `s`: the next order, keeping the same rows visible.
    pub fn cycle_order(&mut self) {
        self.order = self.order.next();
    }

    /// Index of the selected row within [`Self::visible_rows`], if any.
    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.visible_rows()
            .iter()
            .position(|row| row.id == selected)
    }

    /// The selected row, if the selection still resolves to a visible row.
    pub fn selected_row(&self) -> Option<VisibleRow<'_>> {
        let index = self.selected_index()?;
        self.visible_rows().into_iter().nth(index)
    }

    /// Whether the row is currently folded.
    pub fn is_collapsed(&self, id: &RowId) -> bool {
        self.collapsed.contains(id)
    }

    /// Whether anything on screen is animating, which is what makes a redraw
    /// worth doing: an agent's state mark, a pane whose foreground command is
    /// still running, and the mark beside background work that is still going.
    pub fn animates(&self) -> bool {
        let command = theme::command_frames().is_some();
        self.visible_rows()
            .iter()
            .any(|row| match &row.node.row.kind {
                RowKind::Agent(agent) => {
                    theme::animates(&agent.state, agent.retained.is_some())
                        || (command && agent.tasks.tasks.iter().any(TaskRow::is_running))
                }
                RowKind::Task(task) => command && task.is_running(),
                RowKind::Pane(pane) => command && pane.command().is_some(),
                RowKind::Workspace { .. } => false,
            })
    }

    /// Which ordinary panes are listed. Agents only by default.
    pub fn pane_view(&self) -> PaneView {
        self.view
    }

    /// Whether the details panel is shown. Shown by default, and toggleable
    /// because a small terminal or a glance at the tree alone wants the width.
    pub fn shows_details(&self) -> bool {
        !self.details_hidden
    }

    /// Whether panes whose label is a finished session are listed even while
    /// ordinary panes are hidden.
    pub fn shows_finished(&self) -> bool {
        self.finished_shown
    }

    /// Whether background-task children are listed in the current view.
    pub fn shows_tasks(&self) -> bool {
        self.task_rows.shown(self.view)
    }

    /// `b`: lists or hides background-task children in the current view only.
    ///
    /// Facts are untouched: the projection, the parent badges and the details
    /// stay current while the children are hidden, so revealing them later
    /// shows the latest report rather than a cached list. A task hidden under
    /// the selection falls back through [`Self::reconcile_selection`].
    pub fn toggle_tasks(&mut self) {
        self.task_rows.toggle(self.view);
        self.reconcile_selection();
    }

    /// Applies one event from the bus listener.
    ///
    /// A session's tasks arrive with its connection and leave with it. Bus data
    /// never changes an agent's state or its row: only the task children are
    /// rebuilt, because a publisher's list is not an observation fact. Nothing
    /// waits for the next poll — the rows and the selection are reconciled here
    /// and now.
    pub fn apply_bus_event(&mut self, event: BusEvent) {
        self.bus.apply(event);
        self.tree.attach_tasks(&self.bus);
        self.reconcile_selection();
    }

    /// Records why the bus is not running, or clears it.
    pub fn set_bus_diagnostic(&mut self, diagnostic: Option<String>) {
        self.bus_diagnostic = diagnostic;
    }

    /// Why the bus is not running, when it is not.
    pub fn bus_diagnostic(&self) -> Option<&str> {
        self.bus_diagnostic.as_deref()
    }

    /// Lists or hides finished sessions.
    pub fn toggle_finished(&mut self) {
        self.finished_shown = !self.finished_shown;
        self.reconcile_selection();
    }

    /// The rows the tree currently lists.
    fn visibility(&self) -> Visibility {
        Visibility {
            view: self.view,
            finished: self.finished_shown,
            tasks: self.task_rows.shown(self.view),
        }
    }

    /// The active filter query (empty when none is set).
    pub fn filter_query(&self) -> &str {
        &self.filter
    }

    /// Whether `/` has put the view into text-entry mode. While true, all
    /// printable keys edit the filter; the main loop must not treat them as
    /// commands (including `q`).
    pub fn is_filter_editing(&self) -> bool {
        self.filter_editing
    }

    /// Applies one key event, answering with what the main loop has to do.
    ///
    /// Bindings: `j`/Down and `k`/Up move the selection; `Space` (or
    /// Left/Right) folds and unfolds the selected branch; `Enter` focuses the
    /// selected row's location; `/` starts filter entry; `p` cycles the pane
    /// view, `d` shows or hides the details, `e` shows or hides finished
    /// sessions, `b` shows or hides background-task children in the current
    /// view and `s` cycles the order; `n`/`N` jump to the next or previous
    /// row needing attention and `w`/`W` to the next or previous working row;
    /// in filter entry, printable characters (with Backspace) edit the query,
    /// Enter applies it and Escape clears it and leaves entry. Escape outside
    /// entry clears an active filter.
    ///
    /// Any key the view handles drops a focus message: the user has moved on.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        // Terminals that report key releases would otherwise apply each press
        // twice.
        if key.kind == KeyEventKind::Release {
            return None;
        }
        // A confirmation is modal: its keys never move the tree selection, and
        // the view's own action message is not cleared out from under it.
        if self.confirmation.is_some() {
            return self.handle_confirmation_key(key);
        }
        self.focus_message = None;
        if self.filter_editing {
            self.handle_filter_key(key);
            return None;
        }
        match key.code {
            KeyCode::Char('x') => return Some(Action::BeginAction(Operation::ClosePane)),
            KeyCode::Char('X') => return Some(Action::BeginAction(Operation::CloseTab)),
            KeyCode::Char('r') => return Some(Action::BeginAction(Operation::Restart)),
            KeyCode::Char('c') => self.dismiss_notices(),
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char(' ') => self.toggle_fold(),
            KeyCode::Left => self.collapse_selected(),
            KeyCode::Right => self.expand_selected(),
            KeyCode::Char('p') => self.cycle_panes(),
            KeyCode::Char('s') => self.cycle_order(),
            KeyCode::Char('n') => self.jump(needs_attention, true),
            KeyCode::Char('N') => self.jump(needs_attention, false),
            KeyCode::Char('w') => self.jump(is_working, true),
            KeyCode::Char('W') => self.jump(is_working, false),
            KeyCode::Char('d') => self.toggle_details(),
            KeyCode::Char('e') => self.toggle_finished(),
            KeyCode::Char('b') => self.toggle_tasks(),
            KeyCode::Char('/') => self.filter_editing = true,
            KeyCode::Esc => {
                self.filter.clear();
                self.reconcile_selection();
            }
            KeyCode::Enter => return self.focus_selected(),
            _ => {}
        }
        None
    }

    /// `n`/`N` and `w`/`W`: move the selection to the nearest row of a kind,
    /// wrapping at the ends. A row hidden by a fold or excluded by the filter is
    /// not visible, so it cannot be jumped to; an empty view moves nothing.
    fn jump(&mut self, wanted: fn(&VisibleRow<'_>) -> bool, forward: bool) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            return;
        }
        let matching: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| wanted(row))
            .map(|(index, _)| index)
            .collect();
        let next = match (forward, self.selected_index()) {
            (true, Some(current)) => matching
                .iter()
                .copied()
                .find(|index| *index > current)
                .or_else(|| matching.first().copied()),
            (false, Some(current)) => matching
                .iter()
                .rev()
                .copied()
                .find(|index| *index < current)
                .or_else(|| matching.last().copied()),
            (true, None) => matching.first().copied(),
            (false, None) => matching.last().copied(),
        };
        if let Some(index) = next {
            self.selected = Some(rows[index].id.clone());
        }
    }

    /// `Enter`: the action that focuses the selected row, or a one-line message
    /// saying why it cannot be done.
    ///
    /// Nothing is sent for a row the observation does not place, and nothing is
    /// sent at all while the displayed inventory is last-good rather than
    /// current: a focus request moves the user's terminal, so it is only worth
    /// sending for a location Radar can still see.
    fn focus_selected(&mut self) -> Option<Action> {
        let id = self.selected_row()?.id.clone();
        if !self.source_current {
            self.set_focus_message(Some("the fleet is stale: not focusing".to_string()));
            return None;
        }
        // A task is not a location of its own: what to focus is the pane its
        // owner agent row names, and only while that row is still placed by
        // this observation. The owner row is already projected, so a task the
        // bus created focuses its parent without another runtime refresh.
        let target_id = match &id {
            RowId::Task(task) => RowId::Agent(task.owner.clone()),
            other => other.clone(),
        };
        match self.focus_targets.get(&target_id) {
            Some(target) => Some(Action::Focus(target.clone())),
            None => {
                let described = match &target_id {
                    RowId::Workspace(workspace_id) => format!("workspace {workspace_id}"),
                    RowId::Agent(pane_id) | RowId::Pane(pane_id) => format!("pane {pane_id}"),
                    RowId::Task(_) => unreachable!("a task is looked up through its owner row"),
                };
                self.set_focus_message(Some(format!("{described} is not observed")));
                None
            }
        }
    }

    /// Opens the confirmation for `operation` against the current observation,
    /// or explains why the selected row cannot be acted on.
    ///
    /// Eligibility is read from evidence, never from the cursor: a task or
    /// workspace row refuses rather than redirecting to its parent, a stale
    /// inventory refuses, a managed location is routed to its owner rather than
    /// the runtime, and only positive unmanaged evidence reaches the direct
    /// path. Nothing is sent here — Confirm is a separate act.
    pub fn begin_action(&mut self, operation: Operation, state: &ObservationState) {
        let Some(selected) = self.selected_action() else {
            self.set_focus_message(Some("nothing selected: no action target".to_string()));
            return;
        };
        let (pane_id, tab_id, losses, is_agent) = match selected {
            Selected::Agent {
                pane_id,
                tab_id,
                losses,
            } => (pane_id, tab_id, losses, true),
            Selected::Pane {
                pane_id,
                tab_id,
                losses,
            } => (pane_id, tab_id, losses, false),
            Selected::Refused(reason) => {
                let message = if operation == Operation::Restart {
                    "restart is only for a managed worker".to_string()
                } else {
                    reason.to_string()
                };
                self.set_focus_message(Some(message));
                return;
            }
        };
        if !matches!(state.source_freshness(), SourceFreshness::Current) {
            self.set_focus_message(Some("the fleet is stale: not acting".to_string()));
            return;
        }
        let Some(inventory) = state.inventory() else {
            self.set_focus_message(Some("no current inventory: not acting".to_string()));
            return;
        };

        // Restart is only ever a managed worker: an ordinary pane, a task or a
        // workspace row refuses rather than inferring a process to signal.
        if operation == Operation::Restart {
            if !is_agent {
                self.set_focus_message(Some("restart is only for a managed worker".to_string()));
                return;
            }
            let Some(agent) = inventory.agent_on_pane(&pane_id) else {
                self.set_focus_message(Some(format!(
                    "pane {pane_id} is not currently observed: nothing to restart"
                )));
                return;
            };
            if let Err(reason) = lifecycle::restartable(agent) {
                self.set_focus_message(Some(format!(
                    "pane {pane_id} cannot be restarted: {reason}"
                )));
                return;
            }
            match lifecycle::managed_request(inventory, &pane_id, control::Operation::Restart) {
                Ok(managed) => {
                    let mut losses = losses;
                    losses.push("the worker is restarted in place; its session is retained".into());
                    losses.push("asked of the worker's owner, not signalled directly".into());
                    self.open_managed(operation, managed, losses);
                }
                Err(reason) => self.set_focus_message(Some(reason)),
            }
            return;
        }

        let target = match operation {
            Operation::ClosePane => CloseTarget::Pane(pane_id.clone()),
            Operation::CloseTab => CloseTarget::Tab(tab_id),
            Operation::Restart => unreachable!("restart returned above"),
        };
        match containment_of(inventory, state, &target) {
            // A tab with any managed or uncertain member refuses whole, so a
            // container close never becomes a partial managed request.
            Containment::Managed if !matches!(operation, Operation::ClosePane) => {
                if let Some(reason) = containment_refusal(Containment::Managed, &target) {
                    self.set_focus_message(Some(reason));
                }
            }
            // A managed pane close is the owner's to run, with exact identity.
            Containment::Managed => {
                match lifecycle::managed_request(inventory, &pane_id, control::Operation::Close) {
                    Ok(managed) => {
                        let mut losses = losses;
                        losses.push(
                            "the owner decides; an active assignment may be abandoned".into(),
                        );
                        losses.push("asked of the worker's owner, not signalled directly".into());
                        self.open_managed(operation, managed, losses);
                    }
                    Err(reason) => self.set_focus_message(Some(reason)),
                }
            }
            Containment::Uncertain => {
                if let Some(reason) = containment_refusal(Containment::Uncertain, &target) {
                    self.set_focus_message(Some(reason));
                }
            }
            Containment::Unmanaged => self.open_direct(operation, target, losses, state),
        }
    }

    /// Opens the confirmation for a direct runtime close of unmanaged evidence.
    fn open_direct(
        &mut self,
        operation: Operation,
        target: CloseTarget,
        losses: Vec<String>,
        state: &ObservationState,
    ) {
        let Some(inventory) = state.inventory() else {
            return;
        };
        // Freeze the observed identity now, not the cursor at Confirm: moving
        // the selection while the dialog is up must not retarget it.
        let identity = lifecycle::identity(inventory, &target);
        let losses = match &target {
            // A tab names what it holds: the selected row's own losses plus
            // every member pane, so nothing closed with it is unnamed.
            CloseTarget::Tab(tab_id) => {
                let mut all = losses;
                all.extend(tab_losses(inventory, tab_id));
                all
            }
            CloseTarget::Pane(_) => losses,
        };
        self.confirmation = Some(Confirmation {
            operation,
            target,
            identity,
            losses,
            confirm_selected: false,
            managed: None,
        });
    }

    /// Opens the confirmation for an owner-routed managed action.
    fn open_managed(
        &mut self,
        operation: Operation,
        managed: lifecycle::ManagedRequest,
        losses: Vec<String>,
    ) {
        self.confirmation = Some(Confirmation {
            operation,
            target: managed.target.clone(),
            identity: managed.identity.clone(),
            losses,
            confirm_selected: false,
            managed: Some(managed),
        });
    }

    /// Revalidates an open confirmation against the latest current observation
    /// and hands back the frozen action when it still holds. A stale source,
    /// disappeared target or changed containment cancels it with a reason.
    pub fn confirm(&mut self, state: &ObservationState) -> Option<Confirmed> {
        let confirmation = self.confirmation.take()?;
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
        let refusal = match (confirmation.managed.is_some(), actual) {
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
        // A restart stays tied to the owner's current idle advertisement: a
        // worker that became busy between the dialog and Confirm is not asked.
        if let Some(managed) = &confirmation.managed
            && managed.operation == control::Operation::Restart
        {
            let pane_id = match &managed.target {
                CloseTarget::Pane(pane_id) => pane_id.clone(),
                CloseTarget::Tab(_) => String::new(),
            };
            let idle = inventory
                .agent_on_pane(&pane_id)
                .is_some_and(|agent| lifecycle::restartable(agent).is_ok());
            if !idle {
                self.set_focus_message(Some(format!(
                    "{described} is no longer idle: nothing was requested"
                )));
                return None;
            }
        }
        Some(match confirmation.managed {
            Some(managed) => Confirmed::Managed(managed),
            None => Confirmed::Direct(CloseRequest {
                target: confirmation.target,
                identity: confirmation.identity,
            }),
        })
    }

    /// Records a lifecycle outcome from the owner-control worker, refining the
    /// line for the same request rather than adding another.
    pub fn apply_managed_update(&mut self, update: lifecycle::Update) {
        let (failed, text) = lifecycle_notice(&update);
        match self
            .notices
            .iter_mut()
            .find(|notice| notice.id == update.id)
        {
            Some(notice) => {
                notice.failed = failed;
                notice.text = text;
            }
            None => self.notices.push(Notice {
                id: update.id,
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
    fn selected_action(&self) -> Option<Selected> {
        let row = self.selected_row()?;
        Some(match &row.node.row.kind {
            RowKind::Agent(agent) => Selected::Agent {
                pane_id: agent.pane_id.clone(),
                tab_id: agent.tab_id.clone(),
                losses: agent_losses(agent),
            },
            RowKind::Pane(pane) => Selected::Pane {
                pane_id: pane.pane_id.clone(),
                tab_id: pane.tab_id.clone(),
                losses: pane_losses(pane),
            },
            RowKind::Workspace { .. } => {
                Selected::Refused("a workspace is not a close target: close a pane or its tab")
            }
            RowKind::Task(_) => Selected::Refused("a background task is not a close target"),
        })
    }

    /// A confirmation's keys: Escape cancels, Tab and the arrows move between
    /// the drawn buttons, Enter activates the selected one. Everything else is
    /// swallowed, so nothing reaches the tree while the dialog is up.
    fn handle_confirmation_key(&mut self, key: KeyEvent) -> Option<Action> {
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
    fn confirmation_click(&mut self, at: (u16, u16)) -> Option<Action> {
        if inside(self.layout.confirm_confirm, at) {
            return Some(Action::ConfirmAction);
        }
        if inside(self.layout.confirm_cancel, at) {
            self.confirmation = None;
        }
        None
    }

    /// The one-line action message while one is up.
    pub fn focus_message(&self) -> Option<&str> {
        self.focus_message
            .as_ref()
            .map(|(message, _)| message.as_str())
    }
    /// Shows or clears the one-line action message.
    pub fn set_focus_message(&mut self, message: Option<String>) {
        self.focus_message = message.map(|message| (message, Instant::now()));
    }

    /// Drops a message that has been up for [`FOCUS_MESSAGE_TTL`]. Returns
    /// whether anything changed, so the caller can skip a redraw.
    pub fn expire_focus_message(&mut self, now: Instant) -> bool {
        match &self.focus_message {
            Some((_, set)) if now.duration_since(*set) >= FOCUS_MESSAGE_TTL => {
                self.focus_message = None;
                true
            }
            _ => false,
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Clearing the filter restores the unfiltered tree with its
                // pre-filter fold state (folds are never touched by filtering).
                self.filter.clear();
                self.filter_editing = false;
            }
            KeyCode::Enter => self.filter_editing = false,
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => return,
        }
        self.reconcile_selection();
    }

    /// Moves the selection by `delta` visible rows, clamped to the view.
    pub fn move_selection(&mut self, delta: i32) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            self.selected = None;
            self.anchor = 0;
            return;
        }
        let current = self
            .selected_index()
            .unwrap_or_else(|| self.anchor.min(rows.len() - 1));
        let next = (current as i64 + delta as i64).clamp(0, rows.len() as i64 - 1) as usize;
        self.selected = Some(rows[next].id.clone());
        self.anchor = next;
    }

    /// Folds the selected branch when it is open and unfolds it when it is
    /// closed: one key for the whole gesture, whichever way the branch points.
    pub fn toggle_fold(&mut self) {
        match self.selected_row().map(|row| row.collapsed) {
            Some(true) => self.expand_selected(),
            Some(false) => self.collapse_selected(),
            None => {}
        }
    }

    /// Folds the selected branch. No-op on a row without visible children.
    ///
    /// Folding is ignored while a filter is active: the filtered view keeps
    /// matching ancestors visible by design, so a fold could not take effect.
    pub fn collapse_selected(&mut self) {
        if self.needle().is_some() {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.has_children {
            self.collapsed.insert(row.id.clone());
            self.reconcile_selection();
        }
    }

    /// Unfolds the selected branch. No-op when it is already unfolded.
    ///
    /// Ignored while a filter is active, like [`Self::collapse_selected`].
    pub fn expand_selected(&mut self) {
        if self.needle().is_some() {
            return;
        }
        let Some(id) = self.selected_row().map(|row| row.id.clone()) else {
            return;
        };
        if self.collapsed.remove(&id) {
            self.reconcile_selection();
        }
    }

    /// Shows or hides the details panel.
    pub fn toggle_details(&mut self) {
        self.details_hidden = !self.details_hidden;
    }

    /// Moves to the next pane view: agents, then what is running, then every
    /// pane, and round again.
    pub fn cycle_panes(&mut self) {
        self.view = self.view.next();
        self.reconcile_selection();
    }

    fn needle(&self) -> Option<String> {
        let query = self.filter.trim();
        if query.is_empty() {
            return None;
        }
        Some(query.to_lowercase())
    }

    /// Keeps the selection on a real, visible row.
    fn reconcile_selection(&mut self) {
        let rows = self.visible_rows();
        if rows.is_empty() {
            self.selected = None;
            self.anchor = 0;
            return;
        }
        if let Some(selected) = &self.selected
            && let Some(index) = rows.iter().position(|row| row.id == selected)
        {
            self.anchor = index;
            return;
        }
        // A task its publisher stopped reporting falls back to the agent row it
        // hung under, which is still there for it to be read from. Only when
        // that row is gone too does the position fallback below apply.
        if let Some(RowId::Task(task)) = &self.selected
            && let Some(index) = rows
                .iter()
                .position(|row| matches!(&row.id, RowId::Agent(pane_id) if *pane_id == task.owner))
        {
            self.selected = Some(rows[index].id.clone());
            self.anchor = index;
            return;
        }
        // The selected row is gone: stay at the same position if the view has
        // one, otherwise at its end. Selection is never left dangling.
        let anchor = self.anchor.min(rows.len() - 1);
        self.selected = Some(rows[anchor].id.clone());
        self.anchor = anchor;
    }
}

/// A close target's containment in the current observation, retained
/// associations included.
fn containment_of(
    inventory: &FleetObservation,
    state: &ObservationState,
    target: &CloseTarget,
) -> Containment {
    match target {
        CloseTarget::Pane(pane_id) => lifecycle::pane(inventory, state.retained(), pane_id),
        CloseTarget::Tab(tab_id) => lifecycle::tab(inventory, state.retained(), tab_id),
    }
}

/// Why a target cannot be closed directly, when it cannot.
fn containment_refusal(containment: Containment, target: &CloseTarget) -> Option<String> {
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

/// The selected row as a lifecycle target: its pane and tab plus what acting on
/// it may lose, or the reason the row is not an action target.
enum Selected {
    Agent {
        pane_id: String,
        tab_id: String,
        losses: Vec<String>,
    },
    Pane {
        pane_id: String,
        tab_id: String,
        losses: Vec<String>,
    },
    Refused(&'static str),
}

/// One lifecycle line for the operator: a refusal or refusal-shaped answer is a
/// failure, an applied outcome is not. The text names the exact target and the
/// effects that were actually applied, never one inferred from the operation.
fn lifecycle_notice(update: &lifecycle::Update) -> (bool, String) {
    let verb = match update.operation {
        control::Operation::Close => "close",
        control::Operation::Restart => "restart",
    };
    let label = &update.label;
    match &update.kind {
        lifecycle::UpdateKind::Submitted => (
            false,
            format!("{verb} requested for {label}: waiting for its owner"),
        ),
        lifecycle::UpdateKind::Failed(message) => {
            (true, format!("{verb} not sent for {label}: {message}"))
        }
        lifecycle::UpdateKind::Started => (
            false,
            format!("{verb} started for {label}: outcome unknown, not retried"),
        ),
        lifecycle::UpdateKind::NotExecuted => (
            false,
            format!("{verb} not executed for {label}: the request expired unclaimed"),
        ),
        lifecycle::UpdateKind::Invalid(message) => (
            true,
            format!("{verb} evidence invalid for {label}: {message}"),
        ),
        lifecycle::UpdateKind::Answered(result) => answered_notice(verb, label, result),
    }
}

/// The line for an owner's answer. `effects` is read verbatim: a `closed` over a
/// lost generation reports only `process_ended`, so it must not claim a pane
/// was closed or that this request killed the process.
fn answered_notice(verb: &str, label: &str, result: &ControlResult) -> (bool, String) {
    let effects = if result.effects.is_empty() {
        "no effects".to_string()
    } else {
        format!("effects: {}", result.effects.join(", "))
    };
    match result.outcome {
        Outcome::Refused => (
            true,
            format!(
                "{verb} refused for {label}: [{}] {}",
                result.category.as_deref().unwrap_or("refused"),
                result.message
            ),
        ),
        Outcome::Closed => {
            let pane = if result.pane_closed() {
                "pane closed"
            } else {
                "no pane was closed"
            };
            (
                false,
                format!("{verb} applied for {label}: {effects} ({pane})"),
            )
        }
        Outcome::Restarted => (false, format!("{verb} applied for {label}: {effects}")),
        Outcome::Unknown => (
            true,
            format!("{verb} outcome unknown for {label}: {}", result.message),
        ),
    }
}

/// Whether the target still exists in this inventory.
fn target_present(inventory: &FleetObservation, target: &CloseTarget) -> bool {
    match target {
        CloseTarget::Pane(pane_id) => inventory.pane(pane_id).is_some(),
        CloseTarget::Tab(tab_id) => inventory.tabs.iter().any(|tab| tab.tab_id == *tab_id),
    }
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

/// Every pane a tab close would take with it, so nothing closed is unnamed.
fn tab_losses(inventory: &FleetObservation, tab_id: &str) -> Vec<String> {
    inventory
        .panes
        .iter()
        .filter(|pane| pane.location.tab_id == tab_id)
        .map(|pane| {
            let name = pane
                .display_name()
                .unwrap_or(pane.location.pane_id.as_str());
            format!("pane {} — {name}", pane.location.pane_id)
        })
        .collect()
}

/// Whether this node has children the current view can display.
fn has_visible_children(node: &TreeNode, visible: Visibility) -> bool {
    node.children
        .iter()
        .any(|child| visible.includes(&child.row.kind))
}

/// Collects the unfolded view, skipping hidden ordinary panes. A branch's
/// connector is decided from the siblings that survive filtering and folding,
/// not from the children the projection holds.
fn collect_visible<'a>(
    node: &'a TreeNode,
    depth: usize,
    visible: Visibility,
    collapsed: &HashSet<RowId>,
    order: RowOrder,
    connectors: Connectors,
    out: &mut Vec<VisibleRow<'a>>,
) {
    let folded = collapsed.contains(&node.row.id);
    out.push(VisibleRow {
        id: &node.row.id,
        depth,
        node,
        has_children: has_visible_children(node, visible),
        collapsed: folded,
        connectors: connectors.clone(),
    });
    if folded {
        return;
    }
    let children: Vec<&TreeNode> = ordered(&node.children, order)
        .into_iter()
        .filter(|child| visible.includes(&child.row.kind))
        .collect();
    let count = children.len();
    for (index, child) in children.into_iter().enumerate() {
        collect_visible(
            child,
            depth + 1,
            visible,
            collapsed,
            order,
            child_connectors(&connectors, depth, index + 1 < count),
            out,
        );
    }
}

/// Whether this subtree contributes a row to the filtered view: the row itself
/// matches, or a visible descendant does. What the filter keeps, so the
/// connectors are built from the rows that are drawn.
fn subtree_matches(node: &TreeNode, needle: &str, visible: Visibility, order: RowOrder) -> bool {
    node.row.matches(needle)
        || ordered(&node.children, order).into_iter().any(|child| {
            visible.includes(&child.row.kind) && subtree_matches(child, needle, visible, order)
        })
}

/// Whether a position falls inside an area. Radar's areas are half-open, as the
/// terminal's are: the edge cell belongs to the next panel.
fn inside(area: Option<Rect>, at: (u16, u16)) -> bool {
    area.is_some_and(|area| {
        at.0 >= area.x && at.0 < area.right() && at.1 >= area.y && at.1 < area.bottom()
    })
}

/// One level's children in the chosen order.
///
/// The sort is stable, so rows that compare equal keep the order the
/// observation gave them and a re-sort never shuffles them.
fn ordered(children: &[TreeNode], order: RowOrder) -> Vec<&TreeNode> {
    let mut ordered: Vec<&TreeNode> = children.iter().collect();
    match order {
        RowOrder::Source => {}
        RowOrder::State => ordered.sort_by_key(|node| state_rank(node)),
        RowOrder::Name => ordered.sort_by_key(|node| node.row.title().to_lowercase()),
    }
    ordered
}

/// How urgently a subtree wants a human: the lowest rank anywhere beneath it, so
/// a group ranks by its most urgent row rather than by a row of its own.
fn state_rank(node: &TreeNode) -> u8 {
    let own = match &node.row.kind {
        // A retained row holds the last status anyone saw, so it cannot be what
        // needs a human now: it ranks after everything observed.
        RowKind::Agent(agent) if agent.retained.is_some() => RETAINED_RANK,
        RowKind::Agent(agent) => agent_rank(&agent.state),
        RowKind::Task(task) => task_rank(task),
        // A pane carries no state of its own, and a workspace carries none
        // except through what is beneath it.
        RowKind::Pane(_) | RowKind::Workspace { .. } => u8::MAX,
    };
    node.children
        .iter()
        // A task's phase orders task rows among their siblings; it never raises
        // the rank of the agent or workspace they hang under, because an agent's
        // urgency is its own state and not its tasks'.
        .filter(|child| !matches!(child.row.kind, RowKind::Task(_)))
        .map(state_rank)
        .fold(own, std::cmp::min)
}

/// Where a task's phase sits in state order: a capture that has exited and is
/// still to be read or certified first, then a process still running, then a
/// word Radar cannot place. A display rank, and nothing else — no row's state is
/// derived from it.
fn task_rank(task: &TaskRow) -> u8 {
    match task.phase.as_deref() {
        Some("review") | Some("flushing") => 0,
        Some("running") => 3,
        _ => 5,
    }
}

/// Where a state sits in state order: what is stuck or unanswered first, then
/// what is moving, then the states Radar cannot name, then parked and finished
/// rows.
///
/// One list in one place: adding a state is a line here, and a state's rank
/// never depends on where it is read.
fn agent_rank(state: &AgentState) -> u8 {
    match state {
        AgentState::Blocked => 0,
        AgentState::Lost => 1,
        AgentState::Waiting => 2,
        AgentState::Working => 3,
        AgentState::Settling => 4,
        AgentState::Unknown => 5,
        AgentState::Other(_) => 6,
        AgentState::Idle => 7,
        AgentState::Done => 8,
    }
}

/// After every observed state: a retained row keeps the last facts anyone saw.
const RETAINED_RANK: u8 = 9;

/// Whether a row is one a human should look at: stuck, unanswered, or in a
/// state no source could name.
fn needs_attention(row: &VisibleRow<'_>) -> bool {
    matches!(
        &row.node.row.kind,
        RowKind::Agent(agent)
            if matches!(
                agent.state,
                AgentState::Blocked | AgentState::Lost | AgentState::Waiting | AgentState::Unknown
                    | AgentState::Other(_)
            )
    )
}

/// Whether a row is moving: an agent in the working state.
fn is_working(row: &VisibleRow<'_>) -> bool {
    matches!(
        &row.node.row.kind,
        RowKind::Agent(agent) if agent.state == AgentState::Working
    )
}

/// Collects the filtered view: matching rows plus the ancestry that explains
/// their placement. Fold state is deliberately ignored here. As in the
/// unfolded view, the connectors come from the rows the filter keeps.
fn collect_filtered<'a>(
    node: &'a TreeNode,
    depth: usize,
    needle: &str,
    visible: Visibility,
    order: RowOrder,
    connectors: Connectors,
    out: &mut Vec<VisibleRow<'a>>,
) {
    let children: Vec<&TreeNode> = ordered(&node.children, order)
        .into_iter()
        .filter(|child| {
            visible.includes(&child.row.kind) && subtree_matches(child, needle, visible, order)
        })
        .collect();
    out.push(VisibleRow {
        id: &node.row.id,
        depth,
        node,
        has_children: has_visible_children(node, visible),
        // Folding does not apply while filtering: the filtered view shows the
        // ancestry of matches on purpose, so no row reports as collapsed here.
        collapsed: false,
        connectors: connectors.clone(),
    });
    let count = children.len();
    for (index, child) in children.into_iter().enumerate() {
        collect_filtered(
            child,
            depth + 1,
            needle,
            visible,
            order,
            child_connectors(&connectors, depth, index + 1 < count),
            out,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr::decode_snapshot;
    use crate::model::{
        AgentObservation, FleetObservation, Location, Pane, RuntimeStatus, SessionIdentity, Tab,
        Workspace,
    };

    const REAL_SHAPED: &str = include_str!("../tests/fixtures/snapshot_real_shaped.json");

    fn row_ids(app: &App) -> Vec<RowId> {
        app.visible_rows()
            .iter()
            .map(|row| row.id.clone())
            .collect()
    }

    fn select(app: &mut App, id: &str) {
        // Drive selection through the real interface: move to the row's index.
        let index = row_ids(app)
            .iter()
            .position(|row| row == &RowId::Agent(id.into()))
            .expect("row visible");
        app.move_selection(index as i32 - app.selected_index().unwrap_or(0) as i32);
        assert_eq!(app.selected_index(), Some(index));
    }

    #[test]
    fn finished_sessions_are_opt_in() {
        let mut app = app_with_fixture();
        assert!(!app.shows_finished());
        app.handle_key(key(KeyCode::Char('e')));
        assert!(app.shows_finished());
        app.handle_key(key(KeyCode::Char('e')));
        assert!(!app.shows_finished());
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn app_with_fixture() -> App {
        fixture().1
    }

    /// The fixture observation and a view refreshed against it, for tests that
    /// need to revalidate against the same state the view drew.
    fn fixture() -> (ObservationState, App) {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        let mut app = App::new();
        app.refresh(&state);
        (state, app)
    }

    /// Cycles to the view that lists every pane, for tests about pane rows.
    fn show_all_panes(app: &mut App) {
        while app.pane_view() != PaneView::All {
            app.cycle_panes();
        }
    }

    #[test]
    fn pane_view_cycles_from_agents_to_running_to_all() {
        let mut app = app_with_fixture();
        assert_eq!(app.pane_view(), PaneView::Hidden);
        for row in row_ids(&app) {
            assert!(
                !matches!(row, RowId::Pane(_)),
                "no ordinary pane row before toggling: {row:?}"
            );
        }

        // The fixture reports no foreground process, so `running` has nothing
        // to list yet and `all` lists the panes.
        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.pane_view(), PaneView::Running);
        assert!(!row_ids(&app).iter().any(|id| matches!(id, RowId::Pane(_))));

        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.pane_view(), PaneView::All);
        let ids = row_ids(&app);
        assert!(ids.contains(&RowId::Pane("wA:p3".into())));
        assert!(ids.contains(&RowId::Pane("wA:p6".into())));

        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.pane_view(), PaneView::Hidden);
        assert!(!row_ids(&app).iter().any(|id| matches!(id, RowId::Pane(_))));
    }

    #[test]
    fn navigation_moves_within_visible_rows_and_clamps_at_the_edges() {
        let mut app = app_with_fixture();
        // First refresh selects the first row.
        assert_eq!(app.selected_index(), Some(0));

        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected_index(), Some(1));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_index(), Some(2));
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected_index(), Some(1));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_index(), Some(0));

        // Up at the top stays put; down to the end and beyond stays at the end.
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected_index(), Some(0));
        for _ in 0..50 {
            app.handle_key(key(KeyCode::Char('j')));
        }
        assert_eq!(app.selected_index(), Some(app.visible_rows().len() - 1));
    }

    #[test]
    fn collapsing_hides_descendants_and_keeps_selection_valid() {
        let mut app = app_with_fixture();
        select(&mut app, "wA:p1"); // the owner, with the worker nested under it
        assert!(row_ids(&app).contains(&RowId::Agent("wA:p2".into())));

        app.handle_key(key(KeyCode::Char(' ')));
        let ids = row_ids(&app);
        assert!(!ids.contains(&RowId::Agent("wA:p2".into())));
        assert_eq!(
            app.selected_row().expect("selection valid").id,
            &RowId::Agent("wA:p1".into())
        );

        app.handle_key(key(KeyCode::Char(' ')));
        assert!(row_ids(&app).contains(&RowId::Agent("wA:p2".into())));
        assert_eq!(
            app.selected_row().expect("selection valid").id,
            &RowId::Agent("wA:p1".into())
        );

        // Fold on a leaf is a no-op.
        select(&mut app, "wA:p2");
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(
            app.selected_row().expect("selection valid").id,
            &RowId::Agent("wA:p2".into())
        );
    }

    #[test]
    fn nested_filter_match_keeps_ancestors_and_is_case_insensitive() {
        let mut app = app_with_fixture();
        app.handle_key(key(KeyCode::Char('/')));
        assert!(app.is_filter_editing());
        for c in "WORKER TASK".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.filter_query(), "WORKER TASK");

        let ids = row_ids(&app);
        // The worker's label matches; its owner, workspace and ancestry remain.
        assert_eq!(
            ids,
            vec![
                RowId::Workspace("wA".into()),
                RowId::Agent("wA:p1".into()),
                RowId::Agent("wA:p2".into()),
            ]
        );
        // Rows without the match or a matching descendant are hidden.
        assert!(!ids.contains(&RowId::Agent("wA:p5".into())));
        assert!(!ids.contains(&RowId::Agent("wB:p1".into())));
        assert!(!ids.contains(&RowId::Workspace("wB".into())));

        // Backspace widens the match again through the real editing path.
        for _ in 0..5 {
            app.handle_key(key(KeyCode::Backspace));
        }
        assert_eq!(app.filter_query(), "WORKER");
        app.handle_key(key(KeyCode::Enter));
        assert!(!app.is_filter_editing());
        assert!(row_ids(&app).contains(&RowId::Agent("wA:p2".into())));
    }

    #[test]
    fn clearing_the_filter_restores_prior_fold_state() {
        let mut app = app_with_fixture();
        select(&mut app, "wA:p1");
        app.handle_key(key(KeyCode::Char(' ')));
        let folded = row_ids(&app);
        assert!(!folded.contains(&RowId::Agent("wA:p2".into())));

        // A filter would otherwise reveal the folded descendant.
        app.handle_key(key(KeyCode::Char('/')));
        for c in "worker".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        assert!(row_ids(&app).contains(&RowId::Agent("wA:p2".into())));

        app.handle_key(key(KeyCode::Esc));
        assert!(!app.is_filter_editing());
        assert_eq!(app.filter_query(), "");
        // Unfiltered tree with the pre-filter fold state, selection intact.
        assert_eq!(row_ids(&app), folded);
        assert!(!row_ids(&app).contains(&RowId::Agent("wA:p2".into())));
        assert_eq!(
            app.selected_row().expect("selection valid").id,
            &RowId::Agent("wA:p1".into())
        );
    }

    #[test]
    fn escape_clears_an_applied_filter_without_editing() {
        let mut app = app_with_fixture();
        app.handle_key(key(KeyCode::Char('/')));
        for c in "monitor".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert!(!row_ids(&app).contains(&RowId::Agent("wA:p1".into())));

        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.filter_query(), "");
        assert!(row_ids(&app).contains(&RowId::Agent("wA:p1".into())));
    }

    #[test]
    fn refresh_preserves_selection_and_fold_state_for_surviving_rows() {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        let mut app = App::new();
        app.refresh(&state);
        // Fold the owner's branch, select it, then hide panes so a refresh has
        // a second kind of change to survive.
        show_all_panes(&mut app);
        select(&mut app, "wA:p1");
        app.collapse_selected();

        // Same facts, plus a status change (the worker goes done).
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        for agent in &mut observation.agents {
            if agent.location.pane_id == "wA:p2" {
                agent.status = Some(RuntimeStatus::Done);
            }
        }
        state.apply_success(observation);
        app.refresh(&state);

        assert_eq!(
            app.selected_row().expect("selection survives refresh").id,
            &RowId::Agent("wA:p1".into())
        );
        assert!(app.is_collapsed(&RowId::Agent("wA:p1".into())));
        assert_eq!(app.pane_view(), PaneView::All);
        assert!(!row_ids(&app).contains(&RowId::Agent("wA:p2".into())));
    }

    #[test]
    fn running_view_lists_only_panes_with_a_command_in_the_foreground() {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        state.apply_evidence(
            "wA:p3",
            crate::model::ForegroundEvidence::command(
                7,
                Some("hunk".into()),
                Some("hunk diff".into()),
            ),
        );
        let mut app = App::new();
        app.refresh(&state);
        app.cycle_panes();
        assert_eq!(app.pane_view(), PaneView::Running);

        let ids = row_ids(&app);
        assert!(ids.contains(&RowId::Pane("wA:p3".into())));
        assert!(!ids.contains(&RowId::Pane("wA:p6".into())));
        // Agents are the fleet in every view.
        assert!(ids.contains(&RowId::Agent("wA:p2".into())));
    }

    #[test]
    fn selection_falls_back_when_the_selected_row_disappears() {
        let mut app = app_with_fixture();
        select(&mut app, "wA:p2");
        let before = row_ids(&app);
        let index = before
            .iter()
            .position(|id| id == &RowId::Agent("wA:p2".into()))
            .expect("worker row");

        // The worker's pane disappears from the source.
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        observation
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p2");
        observation
            .panes
            .retain(|pane| pane.location.pane_id != "wA:p2");
        let mut state = ObservationState::new();
        state.apply_success(observation);
        app.refresh(&state);

        // Selection is on a valid row: whatever now occupies that position.
        let after = row_ids(&app);
        let selected = app.selected_row().expect("selection never left dangling");
        assert!(after.contains(selected.id));
        assert_ne!(selected.id, &RowId::Agent("wA:p2".into()));
        let expected = index.min(after.len() - 1);
        assert_eq!(
            app.selected_index(),
            Some(expected),
            "selection holds its position"
        );
        assert_eq!(selected.id, &after[expected]);
    }

    #[test]
    fn selection_becomes_empty_when_no_rows_remain() {
        let mut app = app_with_fixture();
        assert!(app.selected_row().is_some());

        let mut state = ObservationState::new();
        state.apply_success(FleetObservation {
            workspaces: vec![],
            tabs: vec![],
            panes: vec![],
            agents: vec![],
        });
        app.refresh(&state);
        assert_eq!(app.selected_index(), None);
        assert!(app.selected_row().is_none());
        // Navigation over an empty view is safe and keeps nothing selected.
        app.handle_key(key(KeyCode::Char('j')));
        app.handle_key(key(KeyCode::Char(' ')));
        app.handle_key(key(KeyCode::Char('p')));
        assert!(app.selected_row().is_none());
    }

    #[test]
    fn filter_matches_reported_role_and_assignment_when_available() {
        let observation = FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wX".into(),
                label: Some("solo".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wX:t1".into(),
                workspace_id: "wX".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                label: None,
                title: None,
            }],
            agents: vec![AgentObservation {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                name: Some("pi".into()),
                label: Some("unrelated label".into()),
                status: Some(RuntimeStatus::Idle),
                session: Some(SessionIdentity::Reported {
                    source: None,
                    value: "session-1".into(),
                }),
                lineage: None,
                facts: crate::model::HerdsmanFacts {
                    role: Some("reviewer".into()),
                    assignment: Some("audit the parser".into()),
                    ..Default::default()
                },
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);

        for needle in ["REVIEWER", "audit the PARSER"] {
            app.handle_key(key(KeyCode::Char('/')));
            for c in needle.chars() {
                app.handle_key(key(KeyCode::Char(c)));
            }
            assert_eq!(
                row_ids(&app),
                vec![RowId::Workspace("wX".into()), RowId::Agent("wX:p1".into())],
                "filter {needle:?} matches reported metadata"
            );
            app.handle_key(key(KeyCode::Esc));
        }
    }

    #[test]
    fn key_releases_are_ignored() {
        let mut app = app_with_fixture();
        let mut release = KeyEvent::new(KeyCode::Char('j'), crossterm::event::KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        app.handle_key(release);
        assert_eq!(app.selected_index(), Some(0));
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Action> {
        app.handle_key(key(code))
    }

    /// Unwraps a confirmed direct close, panicking on a managed one.
    fn direct(confirmed: Confirmed) -> CloseRequest {
        match confirmed {
            Confirmed::Direct(request) => request,
            Confirmed::Managed(_) => panic!("expected a direct close"),
        }
    }

    fn select_row(app: &mut App, row: &RowId) {
        let index = app
            .visible_rows()
            .iter()
            .position(|visible| visible.id == row)
            .expect("row visible");
        app.move_selection(index as i32 - app.selected_index().unwrap_or(0) as i32);
        assert_eq!(app.selected_row().expect("selection").id, row);
    }

    #[test]
    fn enter_on_an_agent_row_asks_to_focus_its_pane() {
        let mut app = app_with_fixture();
        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Focus(Target::Pane("wA:p1".into())))
        );
        assert_eq!(app.focus_message(), None);
    }

    #[test]
    fn enter_on_a_workspace_row_asks_to_focus_the_workspace() {
        let mut app = app_with_fixture();
        select_row(&mut app, &RowId::Workspace("wA".into()));
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Focus(Target::Workspace("wA".into())))
        );
    }

    #[test]
    fn enter_on_a_retained_row_focuses_the_pane_it_was_last_seen_on() {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        // The owner stops being reported while its pane stays reported.
        let mut without_owner = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        without_owner
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p1");
        state.apply_success(without_owner);

        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        let retained = match &app.selected_row().expect("row").node.row.kind {
            RowKind::Agent(agent) => agent.retained.is_some(),
            other => panic!("expected a retained agent row, got {other:?}"),
        };
        assert!(retained, "the row is the retained one");
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Focus(Target::Pane("wA:p1".into())))
        );
    }

    #[test]
    fn enter_in_filter_entry_applies_the_filter_and_asks_for_nothing() {
        let mut app = app_with_fixture();
        assert_eq!(press(&mut app, KeyCode::Char('/')), None);
        assert_eq!(press(&mut app, KeyCode::Char('w')), None);
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(!app.is_filter_editing());
        assert_eq!(app.filter_query(), "w");
        assert_eq!(app.focus_message(), None);
    }

    #[test]
    fn a_stale_inventory_refuses_and_the_message_clears_on_the_next_key() {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        state.apply_failure("herdr timed out after 5s");
        let mut app = App::new();
        app.refresh(&state);

        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert_eq!(
            app.focus_message(),
            Some("the fleet is stale: not focusing")
        );

        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.focus_message(), None);
    }

    #[test]
    fn a_row_the_observation_cannot_place_refuses_with_a_message() {
        let mut app = app_with_fixture();
        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        // The state the rule guards against: a row kept on screen whose location
        // the current observation no longer reports.
        app.focus_targets.clear();
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert_eq!(app.focus_message(), Some("pane wA:p1 is not observed"));
    }

    #[test]
    fn enter_on_a_task_focuses_its_owners_pane_or_refuses_with_that_pane() {
        // One agent whose pane tokens name a task: the task row exists from the
        // projection alone, with no publisher and no second refresh.
        let observation = FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wX".into(),
                label: Some("solo".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wX:t1".into(),
                workspace_id: "wX".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                label: None,
                title: None,
            }],
            agents: vec![AgentObservation {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                name: Some("pi".into()),
                label: Some("owner".into()),
                status: Some(RuntimeStatus::Working),
                session: None,
                lineage: None,
                facts: crate::model::HerdsmanFacts {
                    background_tasks: vec!["bg-1:review".into()],
                    ..Default::default()
                },
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        // The subject is task behavior, so the agents view's default-hidden task
        // rows are explicitly listed.
        app.toggle_tasks();

        // A task is read through the pane its owner row names: `Enter` focuses
        // that pane and sends no task-consumption request.
        let task = RowId::Task(crate::tree::TaskId {
            owner: "wX:p1".into(),
            session: None,
            id: "bg-1".into(),
        });
        select_row(&mut app, &task);
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Focus(Target::Pane("wX:p1".into())))
        );
        assert_eq!(app.focus_message(), None);

        // The owner row still on screen with no location this observation
        // places: the refusal names the pane the task would be read through.
        app.focus_targets.clear();
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert_eq!(app.focus_message(), Some("pane wX:p1 is not observed"));
    }

    #[test]
    fn a_focus_message_expires_on_its_own() {
        let mut app = app_with_fixture();
        app.set_focus_message(Some("herdr refused to focus: pane_not_found".into()));
        assert!(!app.expire_focus_message(Instant::now()));
        assert!(app.expire_focus_message(Instant::now() + FOCUS_MESSAGE_TTL));
        assert_eq!(app.focus_message(), None);
    }

    fn mouse_at(at: (u16, u16)) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: at.0,
            row: at.1,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn close_on_a_managed_agent_refuses_and_opens_nothing() {
        let (state, mut app) = fixture();
        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('x')),
            Some(Action::BeginAction(Operation::ClosePane))
        );
        app.begin_action(Operation::ClosePane, &state);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message().expect("a reason").contains("managed"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn an_unmanaged_pane_opens_a_confirmation_that_defaults_to_cancel() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('x')),
            Some(Action::BeginAction(Operation::ClosePane))
        );
        // Opening the confirmation sends nothing: it is drawn, not acted on.
        app.begin_action(Operation::ClosePane, &state);
        let confirmation = app.confirmation().expect("a confirmation");
        assert_eq!(confirmation.target, CloseTarget::Pane("wA:p3".into()));
        assert!(!confirmation.confirm_selected, "Cancel is the default");

        // Enter activates the selected button, so the default Cancel cancels.
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.confirmation().is_none());

        // Move to Confirm and Enter hands the frozen target back for
        // revalidation rather than closing here.
        app.begin_action(Operation::ClosePane, &state);
        assert_eq!(press(&mut app, KeyCode::Tab), None);
        assert!(app.confirmation().expect("open").confirm_selected);
        assert_eq!(press(&mut app, KeyCode::Enter), Some(Action::ConfirmAction));
    }

    #[test]
    fn confirm_revalidates_against_the_latest_observation() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::ClosePane, &state);
        let request = direct(app.confirm(&state).expect("an unchanged target confirms"));
        assert_eq!(request.target, CloseTarget::Pane("wA:p3".into()));
        assert_eq!(
            request.identity,
            lifecycle::identity(
                state.inventory().expect("current inventory"),
                &request.target
            ),
            "the frozen identity is the observed one"
        );

        // A stale source cancels: the last-good inventory is not current
        // evidence, so nothing is closed on it.
        app.begin_action(Operation::ClosePane, &state);
        let mut stale = ObservationState::new();
        stale.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        stale.apply_failure("herdr exited with status 1");
        assert_eq!(app.confirm(&stale), None);
        assert!(app.focus_message().expect("a reason").contains("stale"));
    }

    #[test]
    fn moving_the_selection_does_not_redirect_the_frozen_target() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::ClosePane, &state);

        // The cursor moves to the managed owner while the dialog is up.
        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        let request = direct(
            app.confirm(&state)
                .expect("the frozen target still confirms"),
        );
        assert_eq!(
            request.target,
            CloseTarget::Pane("wA:p3".into()),
            "Confirm closes the reviewed target, not the current selection"
        );
    }

    #[test]
    fn confirm_refuses_a_replaced_unmanaged_occupant_on_the_same_pane() {
        // Same pane id, same agent kind, same containment — only the session
        // identity changed. The operator confirmed the first occupant, so
        // closing the second would act on something they never reviewed.
        let observation = |session: &str| FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wX".into(),
                label: Some("solo".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wX:t1".into(),
                workspace_id: "wX".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                label: None,
                title: None,
            }],
            agents: vec![AgentObservation {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                name: Some("claude".into()),
                label: None,
                status: Some(RuntimeStatus::Working),
                session: Some(SessionIdentity::Reported {
                    source: None,
                    value: session.into(),
                }),
                lineage: None,
                facts: crate::model::HerdsmanFacts::default(),
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation("s1"));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wX:p1".into()));
        app.begin_action(Operation::ClosePane, &state);
        assert!(app.confirmation().is_some(), "an unmanaged pane confirms");

        let mut replaced = ObservationState::new();
        replaced.apply_success(observation("s2"));
        assert_eq!(
            app.confirm(&replaced),
            None,
            "a replaced occupant is not the confirmed target"
        );
        assert!(
            app.focus_message().expect("a reason").contains("changed"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn a_target_that_became_managed_before_confirm_is_not_closed() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::ClosePane, &state);

        // A managed worker appears on the frozen pane before Confirm.
        let mut observation = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        observation.agents.push(AgentObservation {
            location: Location {
                workspace_id: "wA".into(),
                tab_id: "wA:t1".into(),
                pane_id: "wA:p3".into(),
            },
            name: Some("pi".into()),
            label: None,
            status: None,
            session: None,
            lineage: None,
            facts: crate::model::HerdsmanFacts {
                managed_metadata: true,
                ..Default::default()
            },
        });
        let mut changed = ObservationState::new();
        changed.apply_success(observation);

        assert_eq!(app.confirm(&changed), None);
        assert!(
            app.focus_message().expect("a reason").contains("managed"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn tab_close_is_refused_when_a_member_is_managed() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        // wA:t1 also holds the managed owner on wA:p1.
        assert_eq!(
            press(&mut app, KeyCode::Char('X')),
            Some(Action::BeginAction(Operation::CloseTab))
        );
        app.begin_action(Operation::CloseTab, &state);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message().expect("a reason").contains("managed"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn a_workspace_row_does_not_redirect_a_close_to_its_panes() {
        let (state, mut app) = fixture();
        select_row(&mut app, &RowId::Workspace("wA".into()));
        app.begin_action(Operation::ClosePane, &state);
        assert!(app.confirmation().is_none());
        assert!(app.focus_message().expect("a reason").contains("workspace"));
    }

    #[test]
    fn a_task_row_does_not_redirect_a_close_to_its_owner() {
        let observation = FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wX".into(),
                label: Some("solo".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wX:t1".into(),
                workspace_id: "wX".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                label: None,
                title: None,
            }],
            agents: vec![AgentObservation {
                location: Location {
                    workspace_id: "wX".into(),
                    tab_id: "wX:t1".into(),
                    pane_id: "wX:p1".into(),
                },
                name: Some("claude".into()),
                label: Some("worker".into()),
                status: Some(RuntimeStatus::Working),
                session: None,
                lineage: None,
                facts: crate::model::HerdsmanFacts {
                    background_tasks: vec!["bg-1:review".into()],
                    ..Default::default()
                },
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        app.toggle_tasks();
        let task = RowId::Task(crate::tree::TaskId {
            owner: "wX:p1".into(),
            session: None,
            id: "bg-1".into(),
        });
        select_row(&mut app, &task);
        app.begin_action(Operation::ClosePane, &state);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .expect("a reason")
                .contains("background task")
        );
    }

    #[test]
    fn a_retained_managed_association_forbids_a_direct_close() {
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        let mut without = decode_snapshot(REAL_SHAPED).expect("fixture decodes");
        // The managed owner stops being reported while its pane stays.
        without
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p1");
        state.apply_success(without);
        let mut app = App::new();
        app.refresh(&state);

        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        app.begin_action(Operation::ClosePane, &state);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message().expect("a reason").contains("managed"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn lifecycle_keys_typed_in_filter_entry_are_text() {
        let (_, mut app) = fixture();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(press(&mut app, KeyCode::Char('x')), None);
        assert_eq!(press(&mut app, KeyCode::Char('X')), None);
        assert_eq!(press(&mut app, KeyCode::Char('r')), None);
        assert!(app.is_filter_editing());
        assert_eq!(app.filter_query(), "xXr");
        assert!(app.confirmation().is_none());
    }

    #[test]
    fn mouse_only_the_drawn_buttons_answer_a_confirmation() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::ClosePane, &state);
        let cancel = Rect::new(20, 10, 10, 1);
        let confirm = Rect::new(34, 10, 11, 1);
        app.note_layout(Geometry {
            confirm_cancel: Some(cancel),
            confirm_confirm: Some(confirm),
            ..Geometry::default()
        });

        // A click on a tree row — even a second click that would focus it — is
        // not confirmation.
        assert_eq!(app.handle_mouse(mouse_at((5, 3))), None);
        assert!(app.confirmation().is_some());

        // The drawn Confirm button is.
        assert_eq!(
            app.handle_mouse(mouse_at((40, 10))),
            Some(Action::ConfirmAction)
        );

        // The drawn Cancel button closes without acting.
        app.cancel_confirmation();
        app.begin_action(Operation::ClosePane, &state);
        assert_eq!(app.handle_mouse(mouse_at((25, 10))), None);
        assert!(app.confirmation().is_none());
    }

    const MANAGED_OWNER: &str = "01a10c77-8a6b-7035-8a0e-b1fa607bb507";
    const MANAGED_RUN: &str = "8f2b1c34-5d6e-4f70-8a91-2b3c4d5e6f71";

    fn managed_facts() -> crate::model::HerdsmanFacts {
        crate::model::HerdsmanFacts {
            managed_metadata: true,
            label: Some("implementer-1".into()),
            run: Some(MANAGED_RUN.into()),
            state: Some(crate::model::SemanticState::Idle),
            ..Default::default()
        }
    }

    /// One managed worker on `wM:p1`, with `owner` as its published owner
    /// session (a parentless worker publishes none).
    fn managed_state(
        facts: crate::model::HerdsmanFacts,
        status: RuntimeStatus,
        owner: Option<&str>,
    ) -> ObservationState {
        let location = Location {
            workspace_id: "wM".into(),
            tab_id: "wM:t1".into(),
            pane_id: "wM:p1".into(),
        };
        let session = crate::model::SessionUuid::parse(MANAGED_OWNER).expect("a UUID");
        let lineage = Some(crate::model::Lineage {
            session,
            parent: owner.map(|owner| crate::model::SessionUuid::parse(owner).expect("a UUID")),
        });
        let mut state = ObservationState::new();
        state.apply_success(FleetObservation {
            workspaces: vec![Workspace {
                workspace_id: "wM".into(),
                label: Some("managed".into()),
                number: None,
            }],
            tabs: vec![Tab {
                tab_id: "wM:t1".into(),
                workspace_id: "wM".into(),
                label: None,
                number: None,
            }],
            panes: vec![Pane {
                location: location.clone(),
                label: None,
                title: None,
            }],
            agents: vec![AgentObservation {
                location,
                name: Some("pi".into()),
                label: None,
                status: Some(status),
                session: None,
                lineage,
                facts,
            }],
        });
        state
    }

    #[test]
    fn a_managed_worker_opens_an_owner_routed_confirmation() {
        let state = managed_state(managed_facts(), RuntimeStatus::Idle, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('x')),
            Some(Action::BeginAction(Operation::ClosePane))
        );
        app.begin_action(Operation::ClosePane, &state);
        let confirmation = app.confirmation().expect("a managed close confirmation");
        assert!(!confirmation.confirm_selected, "Cancel is the default");
        let managed = confirmation.managed.as_ref().expect("owner-routed");
        assert_eq!(managed.operation, control::Operation::Close);
        assert_eq!(managed.label, "implementer-1");
        assert_eq!(managed.owner_session, MANAGED_OWNER);
        assert_eq!(managed.run_id, MANAGED_RUN);

        // Confirm hands back the frozen owner request, never a direct close.
        assert_eq!(press(&mut app, KeyCode::Tab), None);
        match app.confirm(&state).expect("unchanged") {
            Confirmed::Managed(request) => assert_eq!(request.run_id, MANAGED_RUN),
            Confirmed::Direct(_) => panic!("a managed target is never a direct close"),
        }
    }

    #[test]
    fn restart_is_offered_only_for_an_idle_managed_worker() {
        let state = managed_state(managed_facts(), RuntimeStatus::Idle, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('r')),
            Some(Action::BeginAction(Operation::Restart))
        );
        app.begin_action(Operation::Restart, &state);
        let managed = app
            .confirmation()
            .expect("a restart confirmation")
            .managed
            .clone()
            .expect("owner-routed");
        assert_eq!(managed.operation, control::Operation::Restart);

        // A busy owner projection refuses without a mux fallback.
        let mut busy = managed_facts();
        busy.state = Some(crate::model::SemanticState::Working);
        let busy = managed_state(busy, RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&busy);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &busy);
        assert!(app.confirmation().is_none());
        assert!(app.focus_message().expect("a reason").contains("idle"));

        // A parentless worker (a lead or standalone session) has no owner.
        let orphan = managed_state(managed_facts(), RuntimeStatus::Idle, None);
        let mut app = App::new();
        app.refresh(&orphan);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &orphan);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .expect("a reason")
                .contains("no owner session"),
            "{:?}",
            app.focus_message()
        );

        // A workspace or task row never infers a process to signal.
        select_row(&mut app, &RowId::Workspace("wM".into()));
        app.begin_action(Operation::Restart, &orphan);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .expect("a reason")
                .contains("managed worker")
        );
    }

    #[test]
    fn confirm_refuses_a_managed_target_whose_run_changed() {
        let state = managed_state(managed_facts(), RuntimeStatus::Idle, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::ClosePane, &state);

        let mut replaced = managed_facts();
        replaced.run = Some("7e6d5c4b-3a29-4180-9f7e-6d5c4b3a2918".into());
        let replaced = managed_state(replaced, RuntimeStatus::Idle, Some(MANAGED_OWNER));
        assert_eq!(app.confirm(&replaced), None);
        assert!(
            app.focus_message().expect("a reason").contains("changed"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn a_busy_managed_worker_is_not_restarted_at_confirm() {
        let state = managed_state(managed_facts(), RuntimeStatus::Idle, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &state);
        assert!(app.confirmation().is_some());

        let mut busy = managed_facts();
        busy.state = Some(crate::model::SemanticState::Working);
        let busy = managed_state(busy, RuntimeStatus::Working, Some(MANAGED_OWNER));
        assert_eq!(app.confirm(&busy), None);
        assert!(
            app.focus_message().expect("a reason").contains("idle"),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn a_derived_unknown_worker_is_not_restartable_even_when_the_owner_projects_idle() {
        // The owner projects idle, but the pane reads unknown, so the derived
        // state is unknown: restart is neither offered nor confirmed.
        let unknown = managed_state(managed_facts(), RuntimeStatus::Unknown, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&unknown);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &unknown);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message().expect("a reason").contains("idle"),
            "{:?}",
            app.focus_message()
        );

        // The same worker read idle is offered, so the refusal above is the
        // derived unknown and not the owner projection.
        let idle = managed_state(managed_facts(), RuntimeStatus::Idle, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&idle);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &idle);
        assert!(app.confirmation().is_some());

        // Confirm revalidates: a worker that reads unknown by then is refused.
        assert_eq!(app.confirm(&unknown), None);
        assert!(
            app.focus_message().expect("a reason").contains("idle"),
            "{:?}",
            app.focus_message()
        );
    }

    fn answered(outcome: Outcome, category: Option<&str>, effects: &[&str]) -> ControlResult {
        ControlResult {
            version: 1,
            request_id: "a0000000-0000-4000-8000-000000000001".into(),
            operation: control::Operation::Close,
            outcome,
            category: category.map(str::to_string),
            message: "implementer-1 handled".into(),
            effects: effects.iter().map(|effect| effect.to_string()).collect(),
            completed_at: "2026-01-01T00:00:01.000Z".into(),
        }
    }

    #[test]
    fn lifecycle_outcomes_are_kept_separately_and_dismissed() {
        let (_, mut app) = fixture();
        let id = "a0000000-0000-4000-8000-000000000001".to_string();
        let update = |kind| lifecycle::Update {
            id: id.clone(),
            label: "implementer-1".into(),
            operation: control::Operation::Close,
            kind,
        };
        app.apply_managed_update(update(lifecycle::UpdateKind::Submitted));
        assert_eq!(app.lifecycle_notices().len(), 1);
        assert!(app.lifecycle_notices()[0].text.contains("requested"));
        assert!(
            app.focus_message().is_none(),
            "kept apart from source/focus"
        );

        // A later update about the same request refines its line.
        app.apply_managed_update(update(lifecycle::UpdateKind::Started));
        assert_eq!(app.lifecycle_notices().len(), 1);
        assert!(app.lifecycle_notices()[0].text.contains("started"));

        // An applied close shows the effects actually reported, and a lost
        // generation's process-only close never claims a pane was closed.
        app.apply_managed_update(update(lifecycle::UpdateKind::Answered(answered(
            Outcome::Closed,
            None,
            &["process_ended"],
        ))));
        assert_eq!(app.lifecycle_notices().len(), 1);
        let text = &app.lifecycle_notices()[0].text;
        assert!(text.contains("process_ended"), "{text}");
        assert!(text.contains("no pane was closed"), "{text}");
        assert!(!app.lifecycle_notices()[0].failed);

        // A refusal preserves the owner's category and is a failure.
        app.apply_managed_update(update(lifecycle::UpdateKind::Answered(answered(
            Outcome::Refused,
            Some("agent_busy"),
            &[],
        ))));
        assert!(app.lifecycle_notices()[0].failed);
        assert!(app.lifecycle_notices()[0].text.contains("agent_busy"));

        // A publication failure is its own line, since no request id exists.
        app.apply_managed_update(lifecycle::Update {
            id: "target-key".into(),
            label: "implementer-1".into(),
            operation: control::Operation::Close,
            kind: lifecycle::UpdateKind::Failed("the control directory is not available".into()),
        });
        assert_eq!(app.lifecycle_notices().len(), 2);

        press(&mut app, KeyCode::Char('c'));
        assert!(app.lifecycle_notices().is_empty(), "dismissed");
    }
}
