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

use crate::bus::{BusEvent, BusSession, BusState};
use crate::focus::Target;
use crate::observation::{ObservationState, SourceFreshness};
use crate::theme;
use ratatui::layout::Rect;

use crate::model::AgentState;
use crate::tree::{FleetTree, RowId, RowKind, TreeNode};

/// How long a focus message stays up when no key clears it first.
const FOCUS_MESSAGE_TTL: Duration = Duration::from_secs(5);

/// What a key asked the main loop to do outside the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Ask Herdr to focus this location.
    Focus(Target),
}

/// One row of the current view: the projected node plus its display depth and
/// fold state.
#[derive(Clone, Copy, Debug)]
pub struct VisibleRow<'a> {
    pub id: &'a RowId,
    pub depth: usize,
    pub node: &'a TreeNode,
    /// Whether the row has children that this view can show (a workspace whose
    /// only panes are hidden has none).
    pub has_children: bool,
    pub collapsed: bool,
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
/// listed while ordinary panes are hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Visibility {
    view: PaneView,
    finished: bool,
}

impl Visibility {
    /// Whether this view lists a row of this kind. Everything that is not an
    /// ordinary pane is always listed.
    fn includes(self, kind: &RowKind) -> bool {
        match kind {
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
            _ => true,
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
    /// One line about the last focus attempt, and when it was set. It clears on
    /// the next key press or after [`FOCUS_MESSAGE_TTL`].
    focus_message: Option<(String, Instant)>,
    /// How siblings are ordered. Structure is never changed by it.
    order: RowOrder,
    /// Where the last draw put things, and how far the details are scrolled.
    layout: Geometry,
    details_scroll: u16,
    /// A wheel turn over the tree, waiting for the main loop to apply it to the
    /// list state it owns.
    scroll_request: Option<usize>,
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
        if let Some(needle) = self.needle() {
            for root in ordered(&self.tree.roots, self.order) {
                collect_filtered(root, 0, &needle, self.visibility(), self.order, &mut rows);
            }
        } else {
            for root in ordered(&self.tree.roots, self.order) {
                collect_visible(
                    root,
                    0,
                    self.visibility(),
                    &self.collapsed,
                    self.order,
                    &mut rows,
                );
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

    /// A left click. A heading folds whether or not it was selected; a row that
    /// was already selected is the one the click acts on.
    fn click(&mut self, at: (u16, u16)) -> Option<Action> {
        let id = self.row_at(at)?;
        let heading = matches!(id, RowId::Workspace(_));
        let already = self.selected.as_ref() == Some(&id);
        self.selected = Some(id);
        if heading {
            self.toggle_fold();
            return None;
        }
        if already {
            return self.focus_selected();
        }
        None
    }

    /// The row drawn at a position, if the position holds one. A folded row is
    /// not drawn, so it cannot be under the pointer.
    fn row_at(&self, at: (u16, u16)) -> Option<RowId> {
        let content = self.layout.tree_content;
        if !inside(Some(content), at) {
            return None;
        }
        let index = self.layout.offset + (at.1 - content.y) as usize;
        self.visible_rows()
            .into_iter()
            .nth(index)
            .map(|row| row.id.clone())
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
                        || (command
                            && agent.retained.is_none()
                            && agent.facts.background_running.unwrap_or(0) > 0
                            && !agent.facts.background_task_ids().is_empty())
                }
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

    /// Applies one event from the bus listener.
    ///
    /// A session's tasks arrive with its connection and leave with it; nothing
    /// here touches the tree or the observation, so bus data can never change
    /// an agent's state.
    pub fn apply_bus_event(&mut self, event: BusEvent) {
        self.bus.apply(event);
    }

    /// Records why the bus is not running, or clears it.
    pub fn set_bus_diagnostic(&mut self, diagnostic: Option<String>) {
        self.bus_diagnostic = diagnostic;
    }

    /// Why the bus is not running, when it is not.
    pub fn bus_diagnostic(&self) -> Option<&str> {
        self.bus_diagnostic.as_deref()
    }

    /// The bus data held for a session, where a publisher is connected for it.
    pub fn bus_session(&self, session: &str) -> Option<&BusSession> {
        self.bus.get(session)
    }

    /// The bus data of a publisher whose `hello` named this pane. A fallback
    /// for a row that publishes no session UUID of its own, and for nothing
    /// else.
    pub fn bus_session_on_pane(&self, pane_id: &str) -> Option<&BusSession> {
        self.bus
            .sessions()
            .find(|(_, held)| held.pane.as_deref() == Some(pane_id))
            .map(|(_, held)| held)
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
    /// sessions and `s` cycles the order; `n`/`N` jump to the next or previous
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
        self.focus_message = None;
        if self.filter_editing {
            self.handle_filter_key(key);
            return None;
        }
        match key.code {
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
        match self.focus_targets.get(&id) {
            Some(target) => Some(Action::Focus(target.clone())),
            None => {
                let described = match &id {
                    RowId::Workspace(workspace_id) => format!("workspace {workspace_id}"),
                    RowId::Agent(pane_id) | RowId::Pane(pane_id) => format!("pane {pane_id}"),
                };
                self.set_focus_message(Some(format!("{described} is not observed")));
                None
            }
        }
    }

    /// The one-line focus message while one is up.
    pub fn focus_message(&self) -> Option<&str> {
        self.focus_message
            .as_ref()
            .map(|(message, _)| message.as_str())
    }

    /// Shows or clears the one-line focus message.
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
        // The selected row is gone: stay at the same position if the view has
        // one, otherwise at its end. Selection is never left dangling.
        let anchor = self.anchor.min(rows.len() - 1);
        self.selected = Some(rows[anchor].id.clone());
        self.anchor = anchor;
    }
}

/// Whether this node has children the current view can display.
fn has_visible_children(node: &TreeNode, visible: Visibility) -> bool {
    node.children
        .iter()
        .any(|child| visible.includes(&child.row.kind))
}

/// Collects the unfolded view, skipping hidden ordinary panes.
fn collect_visible<'a>(
    node: &'a TreeNode,
    depth: usize,
    visible: Visibility,
    collapsed: &HashSet<RowId>,
    order: RowOrder,
    out: &mut Vec<VisibleRow<'a>>,
) {
    let folded = collapsed.contains(&node.row.id);
    out.push(VisibleRow {
        id: &node.row.id,
        depth,
        node,
        has_children: has_visible_children(node, visible),
        collapsed: folded,
    });
    if folded {
        return;
    }
    for child in ordered(&node.children, order) {
        if !visible.includes(&child.row.kind) {
            continue;
        }
        collect_visible(child, depth + 1, visible, collapsed, order, out);
    }
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
        // A pane carries no state of its own, and a workspace carries none
        // except through what is beneath it.
        RowKind::Pane(_) | RowKind::Workspace { .. } => u8::MAX,
    };
    node.children
        .iter()
        .map(state_rank)
        .fold(own, std::cmp::min)
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
/// their placement. Fold state is deliberately ignored here. Returns whether
/// this subtree contributed any row.
fn collect_filtered<'a>(
    node: &'a TreeNode,
    depth: usize,
    needle: &str,
    visible: Visibility,
    order: RowOrder,
    out: &mut Vec<VisibleRow<'a>>,
) -> bool {
    let mut descendants = Vec::new();
    let mut descendant_matched = false;
    for child in ordered(&node.children, order) {
        if !visible.includes(&child.row.kind) {
            continue;
        }
        descendant_matched |=
            collect_filtered(child, depth + 1, needle, visible, order, &mut descendants);
    }
    if !node.row.matches(needle) && !descendant_matched {
        return false;
    }
    out.push(VisibleRow {
        id: &node.row.id,
        depth,
        node,
        has_children: has_visible_children(node, visible),
        // Folding does not apply while filtering: the filtered view shows the
        // ancestry of matches on purpose, so no row reports as collapsed here.
        collapsed: false,
    });
    out.extend(descendants);
    true
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
        let mut state = ObservationState::new();
        state.apply_success(decode_snapshot(REAL_SHAPED).expect("fixture decodes"));
        let mut app = App::new();
        app.refresh(&state);
        app
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
    fn a_focus_message_expires_on_its_own() {
        let mut app = app_with_fixture();
        app.set_focus_message(Some("herdr refused to focus: pane_not_found".into()));
        assert!(!app.expire_focus_message(Instant::now()));
        assert!(app.expire_focus_message(Instant::now() + FOCUS_MESSAGE_TTL));
        assert_eq!(app.focus_message(), None);
    }
}
