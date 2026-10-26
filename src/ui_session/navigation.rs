//! The shared navigation model: the projected fleet tree, its folds, the filter,
//! the pane view, the background-task toggle and the selected row. Every
//! presentation of the dashboard reads and changes these through here.
//!
//! Terminal geometry, details pages and process readings are not part of it.
//! The frontend that shows them keeps them, and resets them when the selected
//! row changes.

use std::cell::RefCell;
use std::collections::HashSet;

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};

use crate::model::AgentState;
use crate::tasks::TaskBoard;
use crate::tree::{FleetTree, RowId, RowKind, TaskRow, TreeNode};

/// The navigation state a frontend reads and changes. Its selection follows a
/// row identity; its anchor is the position it last held, for when that row
/// disappears.
#[derive(Debug, Default)]
pub struct Navigation {
    tree: FleetTree,
    collapsed: HashSet<RowId>,
    /// Filter query as typed; empty means "no filter".
    filter: String,
    view: PaneView,
    /// Whether finished sessions are listed while ordinary panes are hidden.
    finished_shown: bool,
    task_rows: TaskRows,
    order: RowOrder,
    selected: Option<RowId>,
    /// Index of the selected row in the last visible-row list, used as the
    /// fallback position when the selected row disappears.
    anchor: usize,
    /// Scratch space for the fuzzy matcher, reused across every row and call.
    matcher: RefCell<Matcher>,
}

/// One change to the shared navigation state. The session applies these for
/// every attachment, and a frontend applies the same step to its own replica
/// before the published state arrives, so both run one implementation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavStep {
    Select(RowId),
    Move(i32),
    Collapse,
    Expand,
    ToggleFold,
    /// A fold toggle for one named branch, as a click on its marker makes.
    ToggleBranch(RowId),
    CyclePanes,
    CycleOrder,
    ToggleFinished,
    ToggleTasks,
    ShowEverything,
    JumpAttention(bool),
    JumpWorking(bool),
    SetFilter(String),
    ClearFilter,
}

/// Everything about the navigation that two views must agree on, apart from the
/// tree itself: what a published session state carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NavState {
    pub selected: Option<RowId>,
    pub order: RowOrder,
    pub view: PaneView,
    pub finished_shown: bool,
    pub task_rows: TaskRows,
    pub filter: String,
    /// Sorted, so equal states compare and serialize equally.
    pub collapsed: Vec<RowId>,
}

/// A compiled filter query and the matcher that applies it.
pub(crate) struct Needle<'a> {
    pattern: Pattern,
    matcher: &'a RefCell<Matcher>,
}

impl Needle<'_> {
    /// Whether `text` matches every word of the query, fuzzily and without
    /// regard to case, as the substring filter it replaces was.
    pub(crate) fn hit(&self, text: &str) -> bool {
        let mut buffer = Vec::new();
        self.pattern
            .score(
                Utf32Str::new(text, &mut buffer),
                &mut self.matcher.borrow_mut(),
            )
            .is_some()
    }
}

impl Navigation {
    /// The projected tree as of the last replacement.
    pub fn tree(&self) -> &FleetTree {
        &self.tree
    }

    pub fn order(&self) -> RowOrder {
        self.order
    }

    pub fn pane_view(&self) -> PaneView {
        self.view
    }

    pub fn shows_history(&self) -> bool {
        self.finished_shown
    }

    pub fn hidden_history(&self) -> usize {
        if self.finished_shown || self.needle().is_some() {
            return 0;
        }
        let listed = self.visibility();
        let with_history = Visibility {
            finished: true,
            ..listed
        };
        let mut count = 0;
        let mut pending: Vec<&TreeNode> = self.tree.roots.iter().collect();
        while let Some(node) = pending.pop() {
            if with_history.includes(&node.row.kind) && !listed.includes(&node.row.kind) {
                count += 1;
            }
            pending.extend(node.children.iter());
        }
        count
    }

    pub fn shows_tasks(&self) -> bool {
        self.task_rows.shown(self.view)
    }

    pub fn filter_query(&self) -> &str {
        &self.filter
    }

    pub fn selected_id(&self) -> Option<&RowId> {
        self.selected.as_ref()
    }

    pub fn is_collapsed(&self, id: &RowId) -> bool {
        self.collapsed.contains(id)
    }

    /// The projected node for a row identity, whatever the current view shows.
    /// A captured target is looked up by identity, never by position.
    pub fn node(&self, id: &RowId) -> Option<&TreeNode> {
        fn find<'a>(nodes: &'a [TreeNode], id: &RowId) -> Option<&'a TreeNode> {
            nodes.iter().find_map(|node| {
                if &node.row.id == id {
                    Some(node)
                } else {
                    find(&node.children, id)
                }
            })
        }
        find(&self.tree.roots, id)
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
                    if !subtree_has_visible(root, self.visibility()) {
                        continue;
                    }
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

    /// Applies one step. Returns whether a [`NavStep::Select`] found its row;
    /// every other step reports `true`.
    pub fn apply(&mut self, step: &NavStep) -> bool {
        match step {
            NavStep::Select(id) => return self.select_exact(id),
            NavStep::Move(delta) => self.move_selection(*delta),
            NavStep::Collapse => self.collapse_selected(),
            NavStep::Expand => self.expand_selected(),
            NavStep::ToggleFold => self.toggle_fold(),
            NavStep::ToggleBranch(id) => self.toggle_branch(id),
            NavStep::CyclePanes => self.cycle_panes(),
            NavStep::CycleOrder => self.cycle_order(),
            NavStep::ToggleFinished => self.toggle_history(),
            NavStep::ToggleTasks => self.toggle_tasks(),
            NavStep::ShowEverything => self.show_everything(),
            NavStep::JumpAttention(forward) => self.jump_attention(*forward),
            NavStep::JumpWorking(forward) => self.jump_working(*forward),
            NavStep::SetFilter(query) => self.set_filter(query),
            NavStep::ClearFilter => self.clear_filter(),
        }
        true
    }

    /// The agreed state, for publication.
    pub fn state(&self) -> NavState {
        let mut collapsed: Vec<RowId> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        NavState {
            selected: self.selected.clone(),
            order: self.order,
            view: self.view,
            finished_shown: self.finished_shown,
            task_rows: self.task_rows,
            filter: self.filter.clone(),
            collapsed,
        }
    }

    /// Takes on a published state wholesale. The selection is kept as published
    /// when it is visible; otherwise the usual fallback picks a row.
    pub fn adopt(&mut self, state: NavState) {
        self.order = state.order;
        self.view = state.view;
        self.finished_shown = state.finished_shown;
        self.task_rows = state.task_rows;
        self.filter = state.filter;
        self.collapsed = state.collapsed.into_iter().collect();
        self.selected = state.selected;
        self.reconcile();
    }

    /// Replaces the projected tree and keeps the selection on a visible row.
    pub fn replace_tree(&mut self, tree: FleetTree) {
        self.tree = tree;
        self.reconcile();
    }

    /// Attaches the task bus to the projected tree and re-validates selection.
    pub fn attach_tasks(&mut self, bus: &TaskBoard) {
        self.tree.attach_tasks(bus);
        self.reconcile();
    }

    /// Sets the selected row by identity. The caller owns anchoring; this is
    /// the plain move a click makes.
    pub fn select(&mut self, id: Option<RowId>) {
        self.selected = id;
    }

    /// Selects `id` exactly, or nothing: a row that is gone is not replaced by a
    /// neighbour, because the action it was chosen for must not land elsewhere.
    pub fn select_exact(&mut self, id: &RowId) -> bool {
        let Some(index) = self.visible_rows().iter().position(|row| row.id == id) else {
            return false;
        };
        self.selected = Some(id.clone());
        self.anchor = index;
        true
    }

    /// Lists every row, so a row another view could see is one this can select.
    pub fn show_everything(&mut self) {
        self.view = PaneView::All;
        self.finished_shown = true;
        self.task_rows.all = true;
        self.reconcile();
    }

    /// `s`: the next order, keeping the same rows visible.
    pub fn cycle_order(&mut self) {
        self.order = self.order.next();
    }

    /// Moves to the next pane view: agents, then what is running, then every
    /// pane, and round again.
    pub fn cycle_panes(&mut self) {
        self.view = self.view.next();
        self.reconcile();
    }

    /// `b`: lists or hides background-task children in the current view only.
    pub fn toggle_tasks(&mut self) {
        self.task_rows.toggle(self.view);
        self.reconcile();
    }

    /// Lists or hides finished sessions.
    pub fn toggle_history(&mut self) {
        self.finished_shown = !self.finished_shown;
        self.reconcile();
    }

    pub fn push_filter(&mut self, ch: char) {
        self.filter.push(ch);
        self.reconcile();
    }

    /// Replaces the whole query: the shared filter is edited by complete
    /// strings, never by character.
    pub fn set_filter(&mut self, query: &str) {
        self.filter.clear();
        self.filter.push_str(query);
        self.reconcile();
    }

    pub fn pop_filter(&mut self) {
        self.filter.pop();
        self.reconcile();
    }

    /// Clearing the filter restores the unfiltered tree with its pre-filter fold
    /// state: folds are never touched by filtering.
    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.reconcile();
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

    /// Folds the selected branch. No-op on a row without visible children, and
    /// while a filter is active: the filtered view keeps matching ancestors
    /// visible by design, so a fold could not take effect.
    pub fn collapse_selected(&mut self) {
        if self.needle().is_some() {
            return;
        }
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.has_children {
            self.collapsed.insert(row.id.clone());
            self.reconcile();
        }
    }

    /// Unfolds the selected branch. No-op when it is already unfolded, and while
    /// a filter is active, like [`Self::collapse_selected`].
    pub fn expand_selected(&mut self) {
        if self.needle().is_some() {
            return;
        }
        let Some(id) = self.selected_row().map(|row| row.id.clone()) else {
            return;
        };
        if self.collapsed.remove(&id) {
            self.reconcile();
        }
    }

    /// Folds or unfolds one branch by identity, without moving the selection.
    /// Ignored while a filter is active, as the fold keys are.
    pub fn toggle_branch(&mut self, id: &RowId) {
        if self.needle().is_some() {
            return;
        }
        if !self.collapsed.remove(id) {
            self.collapsed.insert(id.clone());
        }
        self.reconcile();
    }

    /// `n`/`N`: the nearest row needing attention, wrapping at the ends.
    pub fn jump_attention(&mut self, forward: bool) {
        self.jump(needs_attention, forward);
    }

    /// `w`/`W`: the nearest working row, wrapping at the ends.
    pub fn jump_working(&mut self, forward: bool) {
        self.jump(is_working, forward);
    }

    /// Moves the selection to the nearest visible row of a kind. A row hidden by
    /// a fold or excluded by the filter is not visible, so it cannot be jumped
    /// to; an empty view moves nothing.
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

    fn needle(&self) -> Option<Needle<'_>> {
        let query = self.filter.trim();
        if query.is_empty() {
            return None;
        }
        Some(Needle {
            pattern: Pattern::new(
                query,
                CaseMatching::Ignore,
                Normalization::Smart,
                AtomKind::Fuzzy,
            ),
            matcher: &self.matcher,
        })
    }

    /// The rows the tree currently lists.
    fn visibility(&self) -> Visibility {
        Visibility {
            view: self.view,
            finished: self.finished_shown,
            tasks: self.task_rows.shown(self.view),
        }
    }

    /// Keeps the selection on a real, visible row. In order of preference: the
    /// selected row itself, a task's owner when the task is gone, then the
    /// position the selection last held, or the end of the list.
    fn reconcile(&mut self) {
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
        let task_owner = match &self.selected {
            Some(RowId::Task(task)) => Some(task.owner.clone()),
            _ => None,
        };
        if let Some(owner) = task_owner
            && let Some(index) = rows
                .iter()
                .position(|row| matches!(&row.id, RowId::Agent(pane_id) if *pane_id == owner))
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

/// One row of the tree as drawn.
///
/// The session's projection decides which rows exist and in what order; this
/// view's own observation explains them. A projected row it cannot explain is
/// still drawn, because the session says the row is there — it is only without
/// the detail that lives on this side of the wire.
pub enum DrawRow<'a> {
    Known(VisibleRow<'a>),
    Structure {
        /// This view's own row identity for the projected row. Owned, because a
        /// row the local tree does not hold has nothing to borrow it from.
        id: RowId,
        /// The projected row, for the little this side of the wire can say.
        row: &'a crate::control_plane::public_contracts::ui_session::TreeRow,
        connectors: Connectors,
    },
}

impl<'a> DrawRow<'a> {
    pub fn id(&self) -> &RowId {
        match self {
            Self::Known(row) => row.id,
            Self::Structure { id, .. } => id,
        }
    }

    pub fn depth(&self) -> usize {
        match self {
            Self::Known(row) => row.depth,
            Self::Structure { row, .. } => row.depth as usize,
        }
    }

    /// Whether the row has children this view can show.
    pub fn has_children(&self) -> bool {
        match self {
            Self::Known(row) => row.has_children,
            Self::Structure { row, .. } => row.has_children,
        }
    }

    /// Where the disclosure marker begins, in cells from the panel's left edge.
    pub fn marker_column(&self) -> u16 {
        match self {
            Self::Known(row) => row.marker_column(),
            Self::Structure { connectors, .. } => {
                (connectors.continuation.len() as u16 + u16::from(self.depth() > 0)) * 2
            }
        }
    }
}

/// The session's ordered rows, joined to the local tree for everything the wire
/// does not carry.
///
/// The join is by row identity, never by position: a row the local observation
/// lacks is drawn from the projection alone, and one the projection dropped is
/// not drawn at all, however this view's tree happens to look.
pub fn projected<'a>(
    rows: &'a [crate::control_plane::public_contracts::ui_session::TreeRow],
    nav: &'a Navigation,
) -> Vec<DrawRow<'a>> {
    let depths: Vec<usize> = rows.iter().map(|row| row.depth as usize).collect();
    let ids: Vec<RowId> = rows
        .iter()
        .map(|row| super::session::row_id(&row.id))
        .collect();
    let mut drawn = Vec::with_capacity(rows.len());
    for (index, wire) in rows.iter().enumerate() {
        let connectors = flat_connectors(&depths, index);
        match nav.node(&ids[index]) {
            Some(node) => drawn.push(DrawRow::Known(VisibleRow {
                id: &node.row.id,
                depth: depths[index],
                node,
                has_children: wire.has_children,
                collapsed: wire.collapsed,
                connectors,
            })),
            None => drawn.push(DrawRow::Structure {
                id: ids[index].clone(),
                row: wire,
                connectors,
            }),
        }
    }
    drawn
}

/// The branch prefix of one row of a flat, depth-annotated listing.
///
/// A row's own connector and each ancestor's continuation are read from the
/// depths around it: a sibling follows when the next row of the same depth comes
/// before any shallower one, and an ancestor's line continues when that ancestor
/// has a following sibling of its own.
fn flat_connectors(depths: &[usize], index: usize) -> Connectors {
    fn sibling_follows(depths: &[usize], from: usize, depth: usize) -> bool {
        for &next in depths.iter().skip(from) {
            if next < depth {
                return false;
            }
            if next == depth {
                return true;
            }
        }
        false
    }
    let depth = depths[index];
    let mut continuation = Vec::new();
    // Each ancestor column, outermost first: find that ancestor by walking back.
    for level in 1..depth {
        let mut ancestor = index;
        while ancestor > 0 && depths[ancestor] > level {
            ancestor -= 1;
        }
        continuation.push(
            depths[ancestor] >= level && sibling_follows(depths, index.min(ancestor) + 1, level),
        );
    }
    Connectors {
        continuation,
        has_following_sibling: sibling_follows(depths, index + 1, depth),
    }
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
/// The fleet starts at `Running`: a pane with a command in the foreground is
/// where a build, an editor or a merge tool is, and an exited pane is history
/// rather than part of it. `All` is the explicit answer, and `Hidden` the quiet
/// one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneView {
    /// Agents only.
    Hidden,
    /// Panes with a command in the foreground.
    #[default]
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
                // explicit toggle brings it into a view — and only while the
                // pane is not in use again.
                PaneView::Hidden => {
                    self.finished && pane.exited.is_some() && pane.command().is_none()
                }
                PaneView::Running => {
                    pane.command().is_some()
                        || (self.finished && pane.exited.is_some() && pane.command().is_none())
                }
                PaneView::All => true,
            },
            RowKind::Agent(agent) => !agent.history() || self.finished,
            RowKind::Lifecycle(subject) => !subject.history() || self.finished,
            RowKind::Workspace { .. } => true,
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
pub struct TaskRows {
    pub agents: bool,
    pub running: bool,
    pub all: bool,
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

/// Whether this node has children the current view can display.
fn subtree_has_visible(node: &TreeNode, visible: Visibility) -> bool {
    let below = |node: &TreeNode| {
        node.children
            .iter()
            .any(|child| subtree_has_visible(child, visible))
    };
    match &node.row.kind {
        // A workspace the projection holds with nothing under it stays: it is a
        // place that exists. One emptied by this view's own toggles does not.
        RowKind::Workspace { .. } => node.children.is_empty() || below(node),
        _ => visible.includes(&node.row.kind) || below(node),
    }
}

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
fn subtree_matches(
    node: &TreeNode,
    needle: &Needle<'_>,
    visible: Visibility,
    order: RowOrder,
) -> bool {
    node.row.matches(|text| needle.hit(text))
        || ordered(&node.children, order).into_iter().any(|child| {
            visible.includes(&child.row.kind) && subtree_matches(child, needle, visible, order)
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
        RowKind::Lifecycle(_) => u8::MAX - 1,
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
pub(crate) fn needs_attention(row: &VisibleRow<'_>) -> bool {
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
pub(crate) fn is_working(row: &VisibleRow<'_>) -> bool {
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
    needle: &Needle<'_>,
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
mod projected_tests {
    use super::*;
    use crate::control_plane::public_contracts::ui_session as dto;

    fn wire(id: dto::UiRowId, depth: u32, has_children: bool) -> dto::TreeRow {
        dto::TreeRow {
            id,
            depth,
            label: "row".into(),
            state: None,
            has_children,
            collapsed: false,
            needs_attention: false,
            working: false,
        }
    }

    fn nav_with_fixture() -> Navigation {
        let mut state = crate::observation::ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        let mut nav = Navigation::default();
        nav.replace_tree(FleetTree::build(&state));
        nav
    }

    #[test]
    fn the_projection_decides_order_and_nesting_and_the_tree_only_explains_it() {
        let nav = nav_with_fixture();
        let local: Vec<dto::UiRowId> = nav
            .visible_rows()
            .iter()
            .take(3)
            .map(|row| crate::ui_session::session::ui_row_id(row.id))
            .collect();
        assert_eq!(local.len(), 3, "the fixture has rows");
        let mut rows: Vec<dto::TreeRow> =
            local.iter().cloned().map(|id| wire(id, 0, false)).collect();
        // A row this view's observation does not hold: the session says it is
        // there, so it is drawn as structure.
        let stranger = dto::UiRowId::Pane {
            pane_id: "gone:p9".into(),
        };
        rows.push(wire(stranger, 0, false));
        // A nested row, to read the branch prefix from the flat listing.
        rows.push(wire(local[0].clone(), 1, false));

        let drawn = super::projected(&rows, &nav);
        assert_eq!(drawn.len(), rows.len(), "every projected row is drawn");
        match &drawn[0] {
            // The next row is at the same depth, so this branch continues.
            DrawRow::Known(row) => assert!(row.connectors.has_following_sibling),
            DrawRow::Structure { .. } => panic!("a fixture row is in this view's tree"),
        }
        assert_eq!(
            drawn[0].id(),
            &crate::ui_session::session::row_id(&local[0]),
            "order is the projection's"
        );
        match &drawn[3] {
            DrawRow::Structure { id, .. } => {
                assert_eq!(id, &RowId::Pane("gone:p9".into()), "drawn by identity");
            }
            DrawRow::Known(_) => panic!("a row the tree lacks must not borrow detail"),
        }
        match &drawn[4] {
            DrawRow::Known(row) => {
                assert_eq!(row.depth, 1, "nesting comes from the projection");
                assert!(
                    row.connectors.continuation.is_empty(),
                    "a child of a root draws no continuation column"
                );
            }
            DrawRow::Structure { .. } => panic!("a fixture row is in this view's tree"),
        }
    }

    #[test]
    fn a_deeper_row_keeps_the_ancestor_column_that_continues_past_it() {
        let nav = nav_with_fixture();
        let local =
            |index: usize| crate::ui_session::session::ui_row_id(nav.visible_rows()[index].id);
        // root, child, grandchild, then another child: the first child's line
        // continues past the grandchild.
        let rows = vec![
            wire(local(0), 0, true),
            wire(local(0), 1, true),
            wire(local(0), 2, false),
            wire(local(1), 1, false),
        ];
        let drawn = super::projected(&rows, &nav);
        match &drawn[2] {
            DrawRow::Known(row) => {
                assert_eq!(row.connectors.continuation, vec![true]);
                assert!(!row.connectors.has_following_sibling);
            }
            DrawRow::Structure { .. } => panic!("a fixture row is in this view's tree"),
        }
    }

    #[test]
    fn a_row_the_projection_dropped_is_not_drawn() {
        let nav = nav_with_fixture();
        let first = crate::ui_session::session::ui_row_id(nav.visible_rows()[0].id);
        let rows = vec![wire(first, 0, false)];
        let drawn = super::projected(&rows, &nav);
        assert_eq!(drawn.len(), 1, "the tree's extra rows are not drawn");
    }
}
