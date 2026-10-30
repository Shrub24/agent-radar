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

mod actions;
mod commands;
mod details;
mod mouse;
mod process;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};

use self::commands::CommandId;
use crate::control_plane::public_contracts::ui_session::SharedCommand;
use crate::control_plane::public_contracts::ui_session::UiAction;
use crate::observation::{ObservationState, SourceFreshness};
use crate::picker::{Picker, Step};
use crate::tasks::{Task, TaskBoard, TaskEvent};
use crate::theme;
use crate::ui_session::client::UiClient;
pub use crate::ui_session::navigation::{Connectors, PaneView, RowOrder, VisibleRow};
use crate::ui_session::{NavStep, Navigation};
use ratatui::layout::Rect;

use crate::model::{
    CpuPercent, DescendantSample, ForegroundEvidence, ProcessIdentity, ProcessResources,
};
use crate::tree::{ContainerRef, FleetTree, RowId, RowKind, TaskId, TaskRow, TaskSource};

/// How long a focus message stays up when no key clears it first.
const FOCUS_MESSAGE_TTL: Duration = Duration::from_secs(5);

/// The width of a row's disclosure marker in cells: `ui::fold_marker` draws it,
/// and a leaf keeps the same two blank columns, so the hit cell is the same
/// whether or not the row has children to show.
const DISCLOSURE_WIDTH: u16 = 2;

/// What a key asked the main loop to do outside the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Ask the backend to focus this location.
    /// Confirm was activated on the session's question; the answer names the
    /// interaction, whose effect the session already froze.
    ConfirmAction,
    /// `L` launches the configured managed agent: the caller supplies its own
    /// shell and `PATH`, and the session derives the place.
    LaunchConfigured,
    /// `E` shows Radar's automatic Pi presence standing. An informational
    /// overlay: it asks nothing and gates nothing.
    PresenceStatus,
    /// The sidebar's hide/unhide key; the caller owns the host commands.
    ToggleSidebar,
    /// The sidebar's zoom key; the caller owns the host commands.
    ToggleZoom,
    /// Open a sidebar beside this dashboard; the caller owns the host command.
    LaunchSidebar,
}

/// Which page of the selected row's details the panel shows.
///
/// The pages split the same facts by what they are about, so the identity a
/// reader wants is not buried under the long published text a task list brings
/// with it. Every page stays reachable: none of them is the only place a fact
/// about the row lives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailPage {
    /// What the row is: location, the live PID, activity and model.
    #[default]
    Overview,
    /// The process holding the row's location.
    Processes,
    /// The background work published for the row.
    Tasks,
    /// Where the facts came from and how current they are.
    Source,
}

impl DetailPage {
    /// Every page, in the order the panel offers them.
    pub const ALL: [Self; 4] = [Self::Overview, Self::Processes, Self::Tasks, Self::Source];

    /// How many pages there are.
    pub const COUNT: usize = Self::ALL.len();

    /// The page's name, as its tab and the hint line state it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Processes => "Processes",
            Self::Tasks => "Tasks",
            Self::Source => "Source",
        }
    }

    /// This page's slot in the per-page scroll and in the drawn tabs.
    pub fn index(self) -> usize {
        match self {
            Self::Overview => 0,
            Self::Processes => 1,
            Self::Tasks => 2,
            Self::Source => 3,
        }
    }

    /// The page after this one, wrapping at the end.
    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::COUNT]
    }

    /// The page before this one, wrapping at the start.
    pub fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::COUNT - 1) % Self::COUNT]
    }
}

/// A block of long content the details start collapsed.
///
/// Only text long enough to bury the facts around it is behind a disclosure: a
/// row's assignment, a task's command and directory, and the verbose half of a
/// live process's identity. The short forms of what a block holds stay on the
/// page, so a marker is never the only thing saying a fact exists.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Disclosure {
    /// The selected row's assignment text.
    Assignment,
    /// The verbose half of the selected row's process identity: the roots a
    /// freshness comparison was made between, the whole executable path, the
    /// birth identity a sample belongs to, what a state letter means and what a
    /// descendant sum is.
    Process,
    /// The table of processes the scan observed beneath the selected row's own
    /// process: one row each, in the hierarchy their parent links reported.
    ProcessTable,
    /// One published task's long text, named by the task's identity so a
    /// refresh that keeps the task keeps its expansion.
    Task(TaskId),
}

/// The foreground of a row whose Processes page describes a process: a pane with
/// none reported, an inconclusive foreground, a shell, a retained row and a
/// container row all leave the page with no process of its own.
fn described_foreground<'row>(row: &'row VisibleRow<'_>) -> Option<&'row ForegroundEvidence> {
    let foreground = match &row.node.row.kind {
        RowKind::Pane(pane) => pane.foreground.as_ref(),
        RowKind::Agent(agent) if agent.retained.is_none() => agent.foreground.as_ref(),
        _ => None,
    };
    foreground.filter(|foreground| matches!(foreground, ForegroundEvidence::NonShell { .. }))
}

/// Whether the selected row's Processes page describes a live process at all.
///
/// A shell, an inconclusive foreground, a pane with no process, a retained row
/// and a task's published PID all leave the page with nothing verbose to hold,
/// and a block that opens onto nothing is worse than no block: the page offers
/// one exactly where it draws the facts the block repeats in full.
fn process_detail_is_drawn(row: &VisibleRow<'_>) -> bool {
    described_foreground(row).is_some()
}

/// Whether the selected row's Processes page has a verified process table to
/// offer: a sampled process whose descendants the same scan enumerated.
///
/// A scan that could not be read, a root with no sample of its own and a row
/// whose process is not current all leave the page with no tree to draw, and the
/// page says which instead of opening onto a table of nothing.
fn process_table_is_drawn(row: &VisibleRow<'_>) -> bool {
    let Some(ForegroundEvidence::NonShell { local, .. }) = described_foreground(row) else {
        return false;
    };
    local
        .resources
        .as_ref()
        .is_some_and(|resources| resources.descendants.observed.is_some())
}

/// One row of the process table: a process and the branch it hangs from.
///
/// A row is its process's birth identity. It is never a pid and never a position
/// in the drawn table, so the same process observed again is the same row and a
/// pid the kernel handed on is another row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProcessRow {
    pub identity: ProcessIdentity,
    /// The process this row was read beneath; `None` for the row's own process.
    pub parent: Option<ProcessIdentity>,
    /// The kernel's name for the process (`comm`), as the read carried it. The
    /// row's own process has none: the runtime reports that name, and the page
    /// draws it from there rather than from a kernel read of a descendant.
    pub name: Option<String>,
    /// Interval CPU for this process, `None` until an interval exists.
    pub cpu: Option<CpuPercent>,
    pub rss_bytes: Option<u64>,
    /// One flag per branch above this row, true where that branch has a process
    /// after it — all the vertical guides are drawn from.
    pub guides: Vec<bool>,
    /// Whether this row is the last of its own branch.
    pub last: bool,
    /// Whether the scan verified processes beneath this one, so the table can
    /// fold them away.
    pub has_children: bool,
    /// Whether a folded branch above this row hides it. A folded branch's own row
    /// stays visible: the fold is what it shows instead of its children.
    pub hidden: bool,
}

/// The process table of one fleet row: the row's own process and every process
/// one scan verified beneath it, in the order the page draws them — the root
/// first, then depth-first with the children of one parent in pid order.
///
/// One projection serves the drawing and the reader's cursor both, so the rows a
/// key moves through are the rows on the page. A folded branch is still a row
/// (its process is still observed) but not a drawn one.
pub(crate) struct ProcessTable {
    /// The row's own process: the anchor a vanished selection falls back to, and
    /// the identity every fold and the cursor are kept against.
    pub root: ProcessIdentity,
    /// Every confirmed row, in drawn order, the folded ones included.
    pub rows: Vec<ProcessRow>,
}

impl ProcessTable {
    /// The rows the page draws, in order.
    pub fn visible(&self) -> impl Iterator<Item = &ProcessRow> {
        self.rows.iter().filter(|row| !row.hidden)
    }

    /// The row for a process, when this table has one.
    pub fn row(&self, identity: &ProcessIdentity) -> Option<&ProcessRow> {
        self.rows.iter().find(|row| &row.identity == identity)
    }

    /// The nearest drawn row at or above `identity`: a process that a fold put
    /// out of sight is selected as the branch that folded it, so the cursor is
    /// never left on a row the page does not draw.
    pub fn anchor(&self, identity: &ProcessIdentity) -> &ProcessIdentity {
        let mut row = self.row(identity);
        while let Some(hidden) = row.filter(|row| row.hidden) {
            match hidden.parent.as_ref().and_then(|parent| self.row(parent)) {
                Some(parent) => row = Some(parent),
                None => return &self.root,
            }
        }
        row.map_or(&self.root, |row| &row.identity)
    }

    /// Projects the table one fleet row's scan verified, or `None` where the page
    /// draws none: a scan that could not enumerate what is beneath the process is
    /// not a tree of nothing, and the sums beside it already say so.
    ///
    /// The members are the ones the same scan measured, taken as they came: this
    /// projects the order, the depth and the folds, and reads no process of its
    /// own.
    pub(crate) fn of(
        folded: &HashSet<ProcessIdentity>,
        resources: &ProcessResources,
    ) -> Option<Self> {
        resources.descendants.observed?;
        let mut children: HashMap<&ProcessIdentity, Vec<&DescendantSample>> = HashMap::new();
        for member in &resources.descendants.members {
            children.entry(&member.parent).or_default().push(member);
        }
        let root = resources.identity.clone();
        let table_root = ProcessRow {
            identity: root.clone(),
            parent: None,
            name: None,
            cpu: resources.cpu,
            rss_bytes: resources.rss_bytes,
            guides: Vec::new(),
            last: true,
            has_children: children.contains_key(&root),
            hidden: false,
        };
        let mut rows = vec![table_root];
        descend(
            &mut rows,
            &children,
            &root,
            &mut Vec::new(),
            folded,
            folded.contains(&root),
        );
        Some(Self { root, rows })
    }
}

/// Adds the processes verified beneath `parent`, each followed by its own:
/// depth-first, the children of one parent in pid order.
///
/// `guides` carries, for each branch above the one being drawn, whether that
/// branch has a process after it. `hidden` says whether a folded branch is
/// already above this level: the branch that was folded stays drawn, and the
/// processes under it do not.
fn descend(
    rows: &mut Vec<ProcessRow>,
    children: &HashMap<&ProcessIdentity, Vec<&DescendantSample>>,
    parent: &ProcessIdentity,
    guides: &mut Vec<bool>,
    folded: &HashSet<ProcessIdentity>,
    hidden: bool,
) {
    let Some(siblings) = children.get(parent) else {
        return;
    };
    let mut siblings = siblings.clone();
    siblings.sort_by_key(|sample| sample.identity.pid);
    for (index, sample) in siblings.iter().enumerate() {
        let last = index + 1 == siblings.len();
        rows.push(ProcessRow {
            identity: sample.identity.clone(),
            parent: Some(sample.parent.clone()),
            name: Some(sample.name.clone()),
            cpu: sample.cpu,
            rss_bytes: sample.rss_bytes,
            guides: guides.clone(),
            last,
            has_children: children.contains_key(&sample.identity),
            hidden,
        });
        guides.push(!last);
        descend(
            rows,
            children,
            &sample.identity,
            guides,
            folded,
            hidden || folded.contains(&sample.identity),
        );
        guides.pop();
    }
}

/// How long an assignment may be before the details collapse it: past the width
/// of a panel line, it buries the facts that follow it.
const ASSIGNMENT_COLLAPSED_OVER: usize = 80;

/// Whether an assignment is long enough to start collapsed.
fn assignment_is_long(text: &str) -> bool {
    text.chars().count() > ASSIGNMENT_COLLAPSED_OVER
}

/// Whether a published task has long text worth hiding.
fn task_has_long_text(task: &Task) -> bool {
    task.command.is_some() || task.cwd.is_some()
}

/// The tasks whose long text a row's Tasks page holds behind a disclosure, in
/// the order it draws them. A matched publisher's list is authoritative over the
/// pane's own token ids, and a task the publisher sent no detail for has nothing
/// to hide.
pub fn collapsible_tasks(row: &VisibleRow<'_>) -> Vec<TaskId> {
    fn collapsible(task: &TaskRow) -> Option<TaskId> {
        task.published
            .as_ref()
            .filter(|published| task_has_long_text(published))
            .map(|_| task.id.clone())
    }
    match &row.node.row.kind {
        RowKind::Agent(agent) if matches!(agent.tasks.source, Some(TaskSource::Bus)) => {
            agent.tasks.tasks.iter().filter_map(collapsible).collect()
        }
        RowKind::Task(task) => collapsible(task).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// The lifecycle operation a confirmation is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// End the selected row: its container when Radar records one, otherwise the
    /// pane the row is. A managed worker is stopped through its owner, so the
    /// exit is reported rather than merely observed.
    Delete,
    /// Restart the selected managed worker through its owner.
    Restart,
    /// Add a place inside the selected container: a tab in a workspace, or a pane
    /// slot split from a pane.
    CreateContainer,
    /// Run the selected pane container’s recorded recipe again in its own slot.
    ReloadContainer,
    /// Show Radar's current Pi session-presence standing.
    PresenceStatus,
}

impl Operation {
    /// Whether this operation is a container control rather than an owner request:
    /// a container control goes to the daemon, which records the container.
    pub fn is_container(self) -> bool {
        matches!(self, Self::CreateContainer | Self::ReloadContainer)
    }
}

/// Below this width the sidebar is drawn as a one-column rail of state marks.
pub const SIDEBAR_RAIL_BELOW: u16 = 16;

/// Whether a sidebar given `width` columns is drawn as the collapsed rail.
pub fn sidebar_collapsed(width: u16) -> bool {
    width < SIDEBAR_RAIL_BELOW
}

/// The fleet-overview view state: tree, selection, folds, filter and which
/// panes are listed.
/// The shared session an [`App`] navigates through, and the last state it took
/// from it.
struct SessionLink {
    client: UiClient,
    mirror: Option<std::sync::Arc<crate::ui_session::client::Mirror>>,
    connected: bool,
}

impl std::fmt::Debug for SessionLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLink")
            .field("connected", &self.client.connected())
            .finish()
    }
}

#[derive(Debug, Default)]
pub struct App {
    /// The shared navigation: the tree, folds, filter, pane view and selection.
    nav: Navigation,
    /// The shared UI session this view navigates through, when the daemon
    /// serves one. Without it the view navigates alone, on the same model.
    session: Option<SessionLink>,
    filter_editing: bool,
    /// Whether the details panel is hidden. Stored inverted so `Default` — the
    /// state a run starts in — is "details shown".
    details_hidden: bool,
    /// Drawn as `radar sidebar`: compressed rows while the split is narrow, and
    /// the whole dashboard once the host has zoomed it.
    sidebar: bool,
    /// The sidebar has been expanded into the whole dashboard. An explicit mode,
    /// not a width: the toggle asks the host to give it the window, and the host
    /// cannot take the mode back by resizing.
    expanded: bool,
    /// The width the last draw was given. The sidebar's own layout follows it:
    /// the host terminal owns the split, so its width is the state.
    width: u16,
    /// The command palette.
    palette: Option<Picker<CommandId>>,
    /// The workspace chooser, open while a new workspace waits for its directory.
    workspace_picker: Option<Picker<PathBuf>>,
    interaction_picker: Option<String>,
    /// The question a shared confirmation asks, when one is presented here.
    session_confirm: Option<crate::ui_session::confirm::Confirm>,
    /// The row the last focus asked for, so a server with no client attached can
    /// open a terminal for that place and ask again.
    focus_request: Option<crate::control_plane::public_contracts::ui_session::UiRowId>,
    /// A focus that failed for want of a client, with the place to open one for.
    focus_recovery: Option<(
        crate::control_plane::public_contracts::ui_session::UiRowId,
        crate::runtime::Target,
    )>,
    /// Bus data pushed by connected extensions, joined to rows when they are
    /// drawn. It sits beside the tree rather than inside it: bus facts are true
    /// only while their connection is open, so a refresh must never be able to
    /// change them or keep them alive.
    bus: TaskBoard,
    /// Why the bus is not running, when the listener could not bind. Kept
    /// apart from source freshness: a bus that never bound says nothing about
    /// the fleet Radar observes.
    task_diagnostic: Option<String>,
    /// Focus target per row, for the rows the displayed observation proves. A
    /// row missing here is one `Enter` must refuse and explain.
    /// Whether the displayed inventory came from a successful collection. A
    /// stale inventory is last-good rather than current, so `Enter` refuses on
    /// every row of it.
    source_current: bool,
    /// Containers whose deletion was held back because an agent inside them did
    /// not exit. The next `D` on one of them asks the daemon to close anyway, so
    /// a wedged agent cannot make a place undeletable for good.
    /// One line about the last action Radar took or refused — a focus or a
    /// lifecycle close — and when it was set. It clears on the next key press or
    /// after [`FOCUS_MESSAGE_TTL`].
    focus_message: Option<(String, Instant)>,
    /// Where the last draw put things, and how far each page is scrolled.
    layout: Geometry,
    /// Which page of the selected row the panel shows.
    detail_page: DetailPage,
    /// Whether the details panel has the keyboard. Tab hands it over and back;
    /// a run starts with the tree holding it.
    details_focused: bool,
    /// How far each page is scrolled, for the current selection. Reset when the
    /// selection moves, because an offset belongs to the row it was scrolled in.
    detail_scroll: [u16; DetailPage::COUNT],
    /// Which long blocks the reader has opened. Keyed by the block rather than
    /// by position, so a refresh that keeps a task keeps its expansion, and one
    /// that drops a task drops it.
    expanded_disclosures: HashSet<Disclosure>,
    /// Which of the page's blocks the keyboard is on.
    disclosure_target: usize,
    /// The reader's place in the Processes page's process table: which process
    /// they picked and which branches they folded. It is local to that page and
    /// to the row whose table it was made in.
    process: ProcessCursor,
    /// A wheel turn over the tree, waiting for the main loop to apply it to the
    /// list state it owns.
    scroll_request: Option<usize>,
    /// Whether the outcomes panel is open, and how far it is scrolled. The footer
    /// shows one clipped line per outcome; this is where the whole text of a
    /// refusal is readable, since a daemon's answer names the method, the code and
    /// the reason and is longer than any footer line.
    notices_open: bool,
    notice_scroll: usize,
}

/// The reader's place in one fleet row's process table.
///
/// It belongs to the processes it was made in: another row, another process of
/// the row's own, or a process that is gone drops it, rather than carrying a fold
/// or a cursor onto a process it never described.
#[derive(Clone, Debug, Default)]
struct ProcessCursor {
    /// Whether process-row navigation is on. A mode of the Processes page, not a
    /// binding of the fleet: every key that leaves the page leaves the mode.
    navigating: bool,
    /// The row's own process the folds and the cursor belong to.
    root: Option<ProcessIdentity>,
    /// The process the reader picked, when it is not the row's own process.
    selected: Option<ProcessIdentity>,
    /// The branches the reader folded, by birth identity.
    folded: HashSet<ProcessIdentity>,
    /// Whether a selection asked to be shown. A selection changes the page —
    /// the picked process's own compact block replaces the root's — so the row
    /// to scroll to is the one the *next* draw reports, not the one the last
    /// layout still describes.
    reveal: bool,
}

/// Where the last draw put the panels, so a mouse event can be mapped back to
/// the row it landed on. Radar binds nothing else to the pointer, so this is the whole
/// of what it has to remember about its own layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Geometry {
    /// The tree panel, its frame included.
    pub tree_panel: Rect,
    /// The tree's content, where the rows are drawn.
    pub tree_content: Rect,
    /// The details panel, when it was drawn.
    pub details: Option<Rect>,
    /// The cells the page's lines are drawn in: the panel inside its frame and
    /// below the page tabs.
    pub details_content: Rect,
    /// Every process-table row the draw laid out, in order: the row's birth
    /// identity, the page row it starts on (before the scroll), and where it was
    /// drawn — a row the scroll has taken off the panel has no rectangle, and no
    /// click lands on it.
    pub process_rows: Vec<(ProcessIdentity, usize, Option<Rect>)>,
    /// Where each foldable process row's marker was drawn: the cell that folds
    /// that branch and does nothing else.
    pub process_folds: Vec<(ProcessIdentity, Rect)>,
    /// The rows the details page occupies at the width it was just drawn at: a
    /// line long enough to wrap is drawn over several rows, and the panel
    /// scrolls in the rows a reader can see, so this is the unit of the clamp.
    pub details_rows: usize,
    /// How many of those rows fit beside the page tabs and inside the frame,
    /// so a scroll to one screenful knows what a screenful is.
    pub details_viewport: u16,
    /// Where each page's tab was drawn, indexed by [`DetailPage::index`], so a
    /// click is answered by the tab actually on screen.
    pub detail_tabs: [Option<Rect>; DetailPage::COUNT],
    /// Where each openable block's marker was drawn, so a click answers with
    /// the block it labels rather than with a position in a list.
    pub disclosure_markers: Vec<(Disclosure, Rect)>,
    /// The first row the list drew.
    pub offset: usize,
    /// The confirmation dialog's Cancel button, when one was drawn.
    pub confirm_cancel: Option<Rect>,
    /// The confirmation dialog's Confirm button, when one was drawn.
    pub confirm_confirm: Option<Rect>,
}

impl App {
    /// A fresh view: no tree, no selection, ordinary panes hidden, no bus data.
    pub fn new() -> Self {
        Self::default()
    }

    /// A view for `radar sidebar`.
    pub fn sidebar() -> Self {
        Self {
            sidebar: true,
            ..Self::default()
        }
    }

    pub fn is_sidebar(&self) -> bool {
        self.sidebar
    }

    /// The selected row's identity, which a popup selects again by value.
    pub fn selected_id(&self) -> Option<RowId> {
        self.nav.selected_id().cloned()
    }

    /// Lists every row, so a row another view could see is one this can select.
    pub fn show_everything(&mut self) {
        self.step(NavStep::ShowEverything);
    }

    /// Selects `id` exactly, or nothing: a row that is gone is not replaced by a
    /// neighbour, because the action it was chosen for must not land elsewhere.
    pub fn select_exact(&mut self, id: &RowId) -> bool {
        self.step(NavStep::Select(id.clone()))
    }

    /// Records the width this draw was given, so the sidebar's layout can follow
    /// the split the host terminal owns.
    pub fn note_width(&mut self, width: u16) {
        self.width = width;
    }

    /// Whether the sidebar is drawn compressed: narrow rows and no details
    /// panel. False for the dashboard, and false once the sidebar is expanded.
    pub fn sidebar_compact(&self) -> bool {
        self.sidebar && !self.expanded
    }

    /// Whether the sidebar is drawn as the collapsed rail.
    pub fn sidebar_railed(&self) -> bool {
        self.sidebar_compact() && sidebar_collapsed(self.width)
    }

    /// Whether the sidebar has been expanded into the whole dashboard.
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Flips between the compressed sidebar and the whole dashboard.
    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    /// Rebuilds the projected tree and re-validates selection.
    ///
    /// Fold state is kept for every row identity that survives; the selection
    /// follows its row identity, and falls back to the nearest surviving row
    /// when that row is gone. Freshness remains a local judgement of the facts
    /// this frontend drew; focus targets are derived only by the session.
    pub fn refresh(&mut self, state: &ObservationState) {
        let mut tree = FleetTree::build(state);
        tree.attach_tasks(&self.bus);
        self.navigate(|nav| nav.replace_tree(tree));
        self.readopt_session_state();
        self.source_current = matches!(state.source_freshness(), SourceFreshness::Current);
        self.prune_disclosures();
        self.reconcile_process();
    }

    /// The projected tree as of the last [`Self::refresh`].
    pub fn tree(&self) -> &FleetTree {
        self.nav.tree()
    }

    /// Rows currently displayed, in source order, honouring folds, the
    /// ordinary-pane toggle and the filter.
    pub fn visible_rows(&self) -> Vec<VisibleRow<'_>> {
        self.nav.visible_rows()
    }

    /// The rows to draw: the session's projection when this view is attached, its
    /// own navigation otherwise.
    ///
    /// The projection decides which rows exist and in what order; this view's
    /// observation only explains them. A row it cannot explain is still drawn,
    /// because the session says the row is there.
    pub fn draw_rows(&self) -> Vec<crate::ui_session::navigation::DrawRow<'_>> {
        Self::drawn(self.mirror_ref().map(|mirror| &**mirror), &self.nav)
    }

    /// The shared state this view has adopted, if it is attached.
    fn mirror_ref(&self) -> Option<&std::sync::Arc<crate::ui_session::client::Mirror>> {
        self.session.as_ref()?.mirror.as_ref()
    }

    /// Records where the draw just put the panels, and keeps the page on screen
    /// scrolled inside the rows that are now there.
    pub fn note_layout(&mut self, layout: Geometry) {
        let max = layout
            .details_rows
            .saturating_sub(layout.details_viewport as usize) as u16;
        let page = self.detail_page.index();
        self.detail_scroll[page] = self.detail_scroll[page].min(max);
        self.layout = layout;
        // A selection that asked to be shown does it against the page this draw
        // produced, so the cursor is scrolled to with the rows it is drawn among.
        if self.process.reveal {
            self.process.reveal = false;
            self.reveal_process();
        }
    }

    /// Moves the selection to a row identity.
    fn select(&mut self, id: Option<RowId>) {
        match id {
            Some(id) => {
                self.step(NavStep::Select(id));
            }
            None => self.navigate(|nav| nav.select(None)),
        }
    }

    /// Makes one shared navigation step here at once and asks the session to
    /// make it too. The session's answer arrives as the next published state and
    /// replaces this view's copy, so both run [`Navigation::apply`] and agree.
    /// Returns whether a selection step found its row.
    fn step(&mut self, step: NavStep) -> bool {
        if self.shared_blocking() {
            return false;
        }
        let found = self.navigate(|nav| nav.apply(&step));
        // A step that found nothing changed nothing, and is not worth ordering.
        if found && let Some(link) = &self.session {
            link.client
                .submit(crate::ui_session::session::action_of(&step));
        }
        found
    }

    /// Submits a command whose target and question belong to the shared session.
    /// The row this view shows travels with it, so the session freezes what the
    /// operator was looking at rather than whatever is selected later.
    fn shared_command(&mut self, command: SharedCommand) -> Option<Action> {
        if self.session.is_none() {
            self.set_focus_message(Some("this command requires the shared UI session".into()));
            return None;
        }
        let shared = self.command_row()?;
        self.submit_session(UiAction::RunCommand {
            command,
            row: shared,
        });
        None
    }

    pub fn command_row(
        &mut self,
    ) -> Option<Option<crate::control_plane::public_contracts::ui_session::UiRowId>> {
        match Self::shared_row(&self.nav, self.mirror_ref().map(|mirror| &**mirror)) {
            Ok(row) => Some(row),
            Err(refusal) => {
                self.set_focus_message(Some(refusal.into()));
                None
            }
        }
    }

    fn shared_row(
        nav: &crate::ui_session::navigation::Navigation,
        mirror: Option<&crate::ui_session::client::Mirror>,
    ) -> Result<Option<crate::control_plane::public_contracts::ui_session::UiRowId>, &'static str>
    {
        if mirror
            .and_then(|mirror| mirror.state.selected.as_ref())
            .is_some_and(|shared| {
                nav.node(&crate::ui_session::session::row_id(shared))
                    .is_none()
            })
        {
            return Err("that row is selected in the session but not described by this view yet");
        }
        Ok(nav.selected_id().map(crate::ui_session::session::ui_row_id))
    }

    pub async fn shutdown_session(&mut self) {
        if let Some(mut link) = self.session.take() {
            link.client.shutdown().await;
        }
    }

    /// Joins the shared session through `client`.
    pub fn attach_session(&mut self, client: UiClient) {
        self.session = Some(SessionLink {
            client,
            mirror: None,
            connected: false,
        });
    }

    /// Takes the session's newest state, once everything this view sent has been
    /// answered. Returns whether anything changed.
    pub fn apply_session_update(&mut self) -> bool {
        let Some(link) = &mut self.session else {
            return false;
        };
        let connected = link.client.connected();
        let health_changed = connected != link.connected;
        link.connected = connected;
        let Some(mirror) = link.client.poll() else {
            if !connected {
                self.source_current = false;
            }
            return health_changed;
        };
        let observation_changed = link
            .mirror
            .as_ref()
            .is_none_or(|last| !std::sync::Arc::ptr_eq(&last.observation, &mirror.observation));
        let gate_changed = link
            .mirror
            .as_ref()
            .is_some_and(|last| last.state.gate != mirror.state.gate);
        let changed_notice = link
            .mirror
            .as_ref()
            .is_none_or(|last| last.state.interaction_notice != mirror.state.interaction_notice);
        let changed_operation = link
            .mirror
            .as_ref()
            .is_none_or(|last| last.state.operation != mirror.state.operation);
        let state = crate::ui_session::session::nav_state(&mirror.state);
        link.mirror = Some(std::sync::Arc::clone(&mirror));
        if gate_changed {
            self.filter_editing = false;
            self.palette = None;
            self.workspace_picker = None;
            self.interaction_picker = None;
        }
        // The editor is the session's to grant: a view that asked while another
        // holds it must not stay in an editor it does not own.
        if self.session_editing_allowed() {
            self.filter_editing =
                mirror.state.filter.editor.as_deref() == Some(&mirror.attachment_id);
        }
        let blocking = mirror.state.blocking.as_ref();
        let presented = blocking.filter(|b| b.presenter.as_deref() == Some(&mirror.attachment_id));
        if let Some(b) = presented
            && b.kind
                == crate::control_plane::public_contracts::ui_session::InteractionKind::Confirm
        {
            self.present_session_confirm(b);
        } else if let Some(b) = presented {
            if self.interaction_picker.as_deref() != Some(&b.interaction_id) {
                let hint = match &b.note {
                    Some(note) => format!("{note} · enter open · esc cancel"),
                    None => "enter open · esc cancel".into(),
                };
                self.workspace_picker = Some(crate::project::picker(&b.choices, hint));
                self.interaction_picker = Some(b.interaction_id.clone());
            }
        } else if self.interaction_picker.is_some() || self.session_confirm.is_some() {
            self.workspace_picker = None;
            self.interaction_picker = None;
            self.session_confirm = None;
        }
        if observation_changed {
            if let Some(tasks) = &mirror.observation.tasks {
                self.replace_task_snapshot(crate::control_plane::client::tasks_from_snapshot(
                    tasks,
                ));
            }
            self.refresh(&mirror.observation.observation);
        }
        self.source_current = connected
            && mirror.diagnostic.is_none()
            && matches!(
                mirror.observation.observation.source_freshness(),
                SourceFreshness::Current
            );
        if let Some(diagnostic) = &mirror.diagnostic {
            self.set_focus_message(Some(diagnostic.to_string()));
        }
        let before = self.nav.selected_id().cloned();
        self.nav.adopt(state);
        self.reset_reading_if_moved(before);
        if changed_notice && let Some(note) = &mirror.state.interaction_notice {
            self.set_focus_message(Some(note.clone()));
        }
        if changed_operation && let Some(op) = &mirror.state.operation {
            // The session publishes the effect it dispatched and its answer; this
            // view shows that one, and keeps no record of its own: the daemon
            // already retains every request, readable by the id named here.
            let failed = !matches!(
                op.status,
                crate::control_plane::public_contracts::ui_session::CommandStatus::Completed
            );
            if failed {
                tracing::warn!(request = %op.request_id, "{}", op.status);
            } else {
                tracing::info!(request = %op.request_id, "{}", op.status);
            }
            self.set_focus_message(Some(format!("{} · request {}", op.status, op.request_id)));
            // A focus with no client attached names the place: this view opens a
            // terminal for it and asks the session again once one is attached.
            if let Some(crate::control_plane::public_contracts::ui_session::OutcomeError {
                code: crate::control_plane::protocol::Code::NoClient,
                target: Some(target),
                ..
            }) = op.failure()
                && let Some(row) = self.focus_request.clone()
            {
                self.focus_recovery = Some((row, target.clone()));
            }
        }
        true
    }

    /// Whether this view may believe the published editor owner: only when
    /// nothing of its own is in flight, so an answer still on its way is not
    /// mistaken for a refusal.
    fn session_editing_allowed(&self) -> bool {
        self.session.as_ref().is_some_and(|link| link.client.idle())
    }

    /// Re-takes the last adopted state after the local tree changed, so a
    /// selection the session named before this view had that row is restored
    /// once the row exists. Only when nothing of this view's own is in flight.
    fn readopt_session_state(&mut self) {
        let Some(link) = &self.session else {
            return;
        };
        if link.client.idle()
            && let Some(mirror) = &link.mirror
        {
            let before = self.nav.selected_id().cloned();
            self.nav
                .adopt(crate::ui_session::session::nav_state(&mirror.state));
            self.reset_reading_if_moved(before);
        }
    }

    pub fn session_mirror(&self) -> Option<std::sync::Arc<crate::ui_session::client::Mirror>> {
        self.mirror_ref().cloned()
    }

    pub fn submit_session(&self, action: UiAction) {
        if let Some(link) = &self.session {
            link.client.submit(action);
        }
    }

    /// Asks the session to focus `row`, whatever this view's freshness.
    pub fn submit_focus_row(
        &self,
        row: crate::control_plane::public_contracts::ui_session::UiRowId,
    ) {
        self.submit_session(
            crate::control_plane::public_contracts::ui_session::UiAction::RunCommand {
                command: crate::control_plane::public_contracts::ui_session::SharedCommand::Focus,
                row: Some(row),
            },
        );
    }

    /// A focus that failed because no client was attached: the place to open a
    /// terminal for, and the row to ask again once one is.
    pub fn take_focus_recovery(
        &mut self,
    ) -> Option<(
        crate::control_plane::public_contracts::ui_session::UiRowId,
        crate::runtime::Target,
    )> {
        self.focus_recovery.take()
    }

    /// The effect the session last dispatched and the answer it has, if any.
    /// This view keeps no record of its own: the daemon retains every request.
    pub fn session_outcome(
        &self,
    ) -> Option<&crate::control_plane::public_contracts::ui_session::CommandOutcome> {
        self.session
            .as_ref()
            .and_then(|link| link.mirror.as_ref())
            .and_then(|mirror| mirror.state.operation.as_ref())
    }

    /// Presents the question a shared interaction asks here. The highlighted
    /// button survives a republication of the same question.
    pub(crate) fn present_session_confirm(
        &mut self,
        blocking: &crate::control_plane::public_contracts::ui_session::Blocking,
    ) {
        // A republication of the same question carries what this view already
        // did with it: the highlighted button, and whether it was answered.
        let carried = self
            .session_confirm
            .as_ref()
            .filter(|confirm| confirm.interaction_id == blocking.interaction_id)
            .cloned();
        let mut confirm = crate::ui_session::confirm::Confirm::new(
            blocking.interaction_id.clone(),
            title_of(blocking.command),
            blocking.prompt.clone().unwrap_or_default(),
            blocking.losses.clone(),
        );
        if let Some(carried) = carried {
            confirm.confirm_selected = carried.confirm_selected;
            confirm.answered = carried.answered;
        }
        self.session_confirm = Some(confirm);
        self.workspace_picker = None;
        self.interaction_picker = None;
    }

    /// Submits the operator's answer to the session that asked the question.
    pub fn answer_session_confirm(&mut self, answer: crate::ui_session::confirm::Answer) {
        use crate::ui_session::confirm::Answer;
        let Some(confirm) = self.session_confirm.as_mut() else {
            return;
        };
        if confirm.answered {
            return;
        }
        // The question stays up, inert, until the session reports it settled: a
        // publication arriving before then must not re-ask it.
        confirm.answered = true;
        let action = match answer {
            Answer::Confirm => UiAction::ResolveInteraction {
                interaction_id: confirm.interaction_id.clone(),
                result:
                    crate::control_plane::public_contracts::ui_session::InteractionResult::Confirm {
                        accepted: true,
                    },
            },
            Answer::Cancel => UiAction::CancelInteraction {
                interaction_id: confirm.interaction_id.clone(),
            },
        };
        self.submit_session(action);
    }

    /// The question a shared confirmation asks, when one is presented here.
    pub fn session_confirm(&self) -> Option<&crate::ui_session::confirm::Confirm> {
        self.session_confirm.as_ref()
    }

    pub fn shared_blocking(&self) -> bool {
        self.session_mirror()
            .is_some_and(|m| m.state.blocking.is_some())
    }

    pub fn session_observation(
        &self,
    ) -> Option<std::sync::Arc<crate::ui_session::publication::Observation>> {
        self.mirror_ref()
            .map(|m| std::sync::Arc::clone(&m.observation))
    }

    pub fn wants_processes(&self, wanted: bool) {
        if let Some(link) = &self.session {
            link.client.wants_processes(wanted);
        }
    }

    pub fn session_diagnostic(&self) -> Option<&str> {
        let link = self.session.as_ref()?;
        if !link.client.connected() {
            return Some("daemon disconnected · showing last coherent view");
        }
        let Some(mirror) = &link.mirror else {
            return Some("waiting for a coherent baseline");
        };
        mirror
            .diagnostic
            .as_ref()
            .map(|failure| failure.description())
    }

    /// Whether the shared session is attached.
    pub fn session_connected(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|link| link.client.connected())
    }

    /// Runs one navigation step. A step that moves the selection starts the new
    /// row's reading from the top: its scroll, opened blocks and process cursor
    /// belong to the row they were made in.
    fn navigate<R>(&mut self, step: impl FnOnce(&mut Navigation) -> R) -> R {
        let before = self.nav.selected_id().cloned();
        let outcome = step(&mut self.nav);
        self.reset_reading_if_moved(before);
        outcome
    }

    fn reset_reading_if_moved(&mut self, before: Option<RowId>) {
        if self.nav.selected_id() == before.as_ref() {
            return;
        }
        self.detail_scroll = [0; DetailPage::COUNT];
        self.expanded_disclosures.clear();
        self.disclosure_target = 0;
        // The process table belongs to the row it was drawn for: another row is
        // another process, and folds and a cursor are not carried onto it.
        self.process = ProcessCursor::default();
    }

    /// Attaches the task bus to the projected tree. The bus sits beside the tree
    /// as App state, so this borrows both fields instead of going through
    /// [`Self::navigate`].
    fn attach_tasks(&mut self) {
        let before = self.nav.selected_id().cloned();
        self.nav.attach_tasks(&self.bus);
        self.reset_reading_if_moved(before);
    }

    /// The order siblings are shown in, which `s` cycles.
    pub fn order(&self) -> RowOrder {
        self.nav.order()
    }

    /// `s`: the next order, keeping the same rows visible.
    pub fn cycle_order(&mut self) {
        self.step(NavStep::CycleOrder);
    }

    /// Index of the selected row within [`Self::visible_rows`], if any.
    /// The rows to draw for an adopted state, or for a view with none.
    ///
    /// An attached view draws what the session projects, **including nothing**: an
    /// empty canonical tree is an answer, not a reason to resurrect the rows this
    /// process happens to remember.
    fn drawn<'a>(
        mirror: Option<&'a crate::ui_session::client::Mirror>,
        nav: &'a Navigation,
    ) -> Vec<crate::ui_session::navigation::DrawRow<'a>> {
        match mirror {
            Some(mirror) => crate::ui_session::navigation::projected(&mirror.rows, nav),
            None => nav
                .visible_rows()
                .into_iter()
                .map(crate::ui_session::navigation::DrawRow::Known)
                .collect(),
        }
    }

    /// Index of the selected row in the drawn list: the list the pointer maps
    /// onto and the list the highlight is placed in.
    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.nav.selected_id()?;
        self.draw_rows().iter().position(|row| row.id() == selected)
    }

    /// The selected row, if the selection still resolves to a visible row.
    pub fn selected_row(&self) -> Option<VisibleRow<'_>> {
        self.nav.selected_row()
    }

    /// Whether the row is currently folded.
    pub fn is_collapsed(&self, id: &RowId) -> bool {
        self.nav.is_collapsed(id)
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
                RowKind::Lifecycle(row) => {
                    command
                        && matches!(
                            row.subject.state,
                            crate::control_plane::lifecycle::State::Pending
                                | crate::control_plane::lifecycle::State::Captured
                        )
                }
                RowKind::Workspace { .. } => false,
            })
    }

    /// Which ordinary panes are listed. Running panes by default.
    pub fn pane_view(&self) -> PaneView {
        self.nav.pane_view()
    }

    /// Whether the details panel is shown. Shown by default, and toggleable
    /// because a small terminal or a glance at the tree alone wants the width.
    pub fn shows_details(&self) -> bool {
        !self.sidebar_compact() && !self.details_hidden
    }

    /// Whether panes whose label is a finished session are listed even while
    /// ordinary panes are hidden.
    pub fn shows_history(&self) -> bool {
        self.nav.shows_history()
    }

    /// Whether background-task children are listed in the current view.
    pub fn shows_tasks(&self) -> bool {
        self.nav.shows_tasks()
    }

    /// `b`: lists or hides background-task children in the current view only.
    ///
    /// Facts are untouched: the projection, the parent badges and the details
    /// stay current while the children are hidden, so revealing them later
    /// shows the latest report rather than a cached list. A task hidden under
    /// the selection falls back to its owner.
    pub fn toggle_tasks(&mut self) {
        self.step(NavStep::ToggleTasks);
    }

    /// Applies one event from the bus listener.
    ///
    /// A session's tasks arrive with its connection and leave with it. Bus data
    /// never changes an agent's state or its row: only the task children are
    /// rebuilt, because a publisher's list is not an observation fact. Nothing
    /// waits for the next poll — the rows and the selection are reconciled here
    /// and now.
    pub fn apply_task_event(&mut self, event: TaskEvent) {
        self.bus.apply(event);
        self.attach_tasks();
        self.prune_disclosures();
    }

    pub fn replace_task_snapshot(&mut self, events: Vec<TaskEvent>) {
        let mut bus = TaskBoard::default();
        for event in events {
            bus.apply(event);
        }
        self.bus = bus;
        self.attach_tasks();
        self.prune_disclosures();
    }

    /// Records why the bus is not running, or clears it.
    pub fn set_task_diagnostic(&mut self, diagnostic: Option<String>) {
        self.task_diagnostic = diagnostic;
    }

    /// Why the bus is not running, when it is not.
    pub fn task_diagnostic(&self) -> Option<&str> {
        self.task_diagnostic.as_deref()
    }

    /// Lists or hides finished sessions.
    pub fn toggle_history(&mut self) {
        self.step(NavStep::ToggleFinished);
    }

    /// History hidden by the current view, excluding a private filter draft.
    pub fn hidden_history(&self) -> usize {
        self.nav.hidden_history()
    }

    /// The active filter query (empty when none is set).
    pub fn filter_query(&self) -> &str {
        self.nav.filter_query()
    }

    /// Whether `/` has put the view into text-entry mode. While true, all
    /// printable keys edit the filter; the main loop must not treat them as
    /// commands (including `q`).
    pub fn is_filter_editing(&self) -> bool {
        self.filter_editing
    }

    /// The open command palette, if any.
    pub fn palette(&self) -> Option<&Picker<CommandId>> {
        self.palette.as_ref()
    }

    /// The open workspace chooser, if any.
    pub fn workspace_picker(&self) -> Option<&Picker<PathBuf>> {
        self.workspace_picker.as_ref()
    }

    /// Starts the shared new-workspace interaction.
    pub fn new_workspace(&mut self) {
        if self.session.is_some() {
            self.submit_session(UiAction::RunCommand {
                command:
                    crate::control_plane::public_contracts::ui_session::SharedCommand::NewWorkspace,
                row: None,
            });
        } else {
            self.set_focus_message(Some(
                "workspace creation requires the shared UI session".into(),
            ));
        }
    }

    /// Whether a picker holds the keyboard.
    pub fn picker_open(&self) -> bool {
        self.palette.is_some() || self.workspace_picker.is_some()
    }

    /// One picker key. Answers with the effect of a chosen command or workspace,
    /// so the caller keeps the effect.
    fn handle_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        if let Some(mut picker) = self.palette.take() {
            return match picker.handle_key(key) {
                Step::Continue => {
                    self.palette = Some(picker);
                    None
                }
                Step::Close => None,
                Step::Choose(command) => self.run(command),
            };
        }
        let mut picker = self.workspace_picker.take()?;
        match picker.handle_key(key) {
            Step::Continue => {
                self.workspace_picker = Some(picker);
                None
            }
            Step::Close => {
                if let Some(id) = &self.interaction_picker {
                    self.submit_session(UiAction::CancelInteraction {
                        interaction_id: id.clone(),
                    });
                }
                None
            }
            Step::Choose(directory) => {
                if let Some(id) = &self.interaction_picker {
                    self.submit_session(UiAction::ResolveInteraction {
                        interaction_id: id.clone(),
                        result: crate::control_plane::public_contracts::ui_session::InteractionResult::Project { path: directory.to_string_lossy().into_owned() },
                    });
                    None
                } else {
                    // The picker only exists for a shared interaction; without
                    // one there is nothing to answer.
                    None
                }
            }
        }
    }

    /// Applies one key event, answering with what the main loop has to do.
    ///
    /// Bindings: `j`/Down and `k`/Up move the selection; `Tab`/`Shift-Tab` hand
    /// the keyboard between the tree and the details; `Space` (or Left/Right)
    /// folds and unfolds the selected branch; `Enter` focuses the selected row's
    /// location; `/` starts filter entry; `p` cycles the pane view, `d` shows or
    /// hides the details, `e` shows or hides finished sessions, `b` shows or
    /// hides background-task children in the current view and `s` cycles the
    /// order; `n`/`N` jump to the next or previous row needing attention and
    /// `w`/`W` to the next or previous working row; in filter entry, printable
    /// characters (with Backspace) edit the query, Enter applies it and Escape
    /// clears it and leaves entry. Escape outside entry clears an active filter.
    ///
    /// While the details hold the keyboard its own keys answer instead: see
    /// [`Self::handle_details_key`].
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
        if self.session_confirm().is_some() {
            return self.handle_session_confirm_key(key);
        }
        self.focus_message = None;
        // The command palette is modal: it owns the keyboard until it closes,
        // so its letters filter commands rather than moving the tree.
        if self.picker_open() {
            return self.handle_picker_key(key);
        }
        // The outcomes panel is modal: it shows what the session last reported and
        // acts on nothing, so while it is open no other key reaches the tree.
        if self.notices_open {
            match key.code {
                KeyCode::Char('!') | KeyCode::Esc | KeyCode::Char('c') => {
                    self.notices_open = false;
                }
                KeyCode::Char('j') | KeyCode::Down => self.scroll_notices(1),
                KeyCode::Char('k') | KeyCode::Up => self.scroll_notices(-1),
                KeyCode::PageDown | KeyCode::Char(' ') => self.scroll_notices(10),
                KeyCode::PageUp => self.scroll_notices(-10),
                KeyCode::Home => self.notice_scroll = 0,
                KeyCode::End => self.scroll_notices(isize::MAX / 2),
                _ => {}
            }
            return None;
        }
        if let Some(confirm) = self.session_confirm.as_mut()
            && let Some(answer) = confirm.handle_key(key)
        {
            self.answer_session_confirm(answer);
            return None;
        }
        if self.shared_blocking() {
            if key.code == KeyCode::Esc
                && let Some(m) = self.session_mirror()
                && let Some(b) = &m.state.blocking
            {
                self.submit_session(UiAction::CancelInteraction {
                    interaction_id: b.interaction_id.clone(),
                });
            }
            return None;
        }
        if self.filter_editing {
            self.handle_filter_key(key);
            return None;
        }
        if self.details_focused && self.handle_details_key(key) {
            return None;
        }
        // The sidebar's own keys ask the host to resize the split it lives in,
        // whether it is compressed or zoomed.
        if self.sidebar {
            match key.code {
                KeyCode::Char('B') => return Some(Action::ToggleSidebar),
                KeyCode::Char('Z') => return Some(Action::ToggleZoom),
                _ => {}
            }
            // Presentation changes which panels exist, not the authority to
            // manage the selected row. A sidebar cannot open another sidebar.
            if key.code == KeyCode::Char('S')
                || (self.sidebar_compact()
                    && matches!(
                        key.code,
                        KeyCode::Char('d') | KeyCode::Tab | KeyCode::BackTab
                    ))
            {
                return None;
            }
        }
        if let Some(command) = commands::resolve(key.code) {
            return self.run(command);
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('?') => {
                self.palette = Some(Picker::new(
                    "Commands",
                    "enter run · esc close",
                    commands::items(self.sidebar, self.sidebar_compact()),
                ))
            }
            KeyCode::Esc => {
                self.step(NavStep::ClearFilter);
            }
            _ => {}
        }
        None
    }

    /// Runs a command by its identity. A key and a palette choice both arrive
    /// here, so a command means the same wherever it was chosen.
    fn run(&mut self, command: CommandId) -> Option<Action> {
        match command {
            // Focus is the session's: the frontend says which row, the session
            // decides where that lands and the daemon moves the client.
            CommandId::Focus => {
                let Some(row) = self.selected_row() else {
                    self.set_focus_message(Some("nothing selected: nothing to focus".into()));
                    return None;
                };
                // Staleness is this view's own reading of the observation it drew;
                // where the row focuses is the session's decision.
                if !self.source_current {
                    self.set_focus_message(Some("the fleet is stale: not focusing".into()));
                    return None;
                }
                let row = crate::ui_session::session::ui_row_id(&row.id.clone());
                self.focus_request = Some(row.clone());
                self.submit_session(
                    crate::control_plane::public_contracts::ui_session::UiAction::RunCommand {
                        command:
                            crate::control_plane::public_contracts::ui_session::SharedCommand::Focus,
                        row: Some(row),
                    },
                );
                return None;
            }
            CommandId::Filter => self.begin_filter_edit(),
            CommandId::NextAttention => {
                self.step(NavStep::JumpAttention(true));
            }
            CommandId::PreviousAttention => {
                self.step(NavStep::JumpAttention(false));
            }
            CommandId::NextWorking => {
                self.step(NavStep::JumpWorking(true));
            }
            CommandId::PreviousWorking => {
                self.step(NavStep::JumpWorking(false));
            }
            CommandId::Fold => self.toggle_fold(),
            CommandId::Collapse => self.collapse_selected(),
            CommandId::Expand => self.expand_selected(),
            CommandId::PaneView => self.cycle_panes(),
            CommandId::TreeOrder => self.cycle_order(),
            CommandId::FinishedSessions => self.toggle_history(),
            CommandId::BackgroundTasks => self.toggle_tasks(),
            CommandId::Details => self.toggle_details(),
            CommandId::FocusSwap => self.toggle_detail_focus(),
            // `!` opens the whole text of what was reported: the footer shows one
            // clipped line, and a refusal is longer than that.
            CommandId::Notices => self.toggle_notices(),
            // `S` and `B` are the sidebar's host controls; a dashboard has no sidebar
            // to hide, so its `B` does nothing.
            CommandId::OpenSidebar => return Some(Action::LaunchSidebar),
            CommandId::HideSidebar => return self.sidebar.then_some(Action::ToggleSidebar),
            // `A` starts a workspace from nowhere: nothing is selected, replaced or
            // confirmed, because an addition cannot take anything away.
            CommandId::NewWorkspace => self.new_workspace(),
            // `L` launches the configured managed agent in a new tab or pane.
            CommandId::LaunchConfigured => return Some(Action::LaunchConfigured),
            CommandId::Restart => {
                return self.shared_command(SharedCommand::Restart);
            }
            // Container controls act on a container Radar records rather than on an
            // owner: `a` adds a tab to its workspace, `D` deletes it, `R` reloads its slot.
            CommandId::CreateContainer => {
                return self.shared_command(SharedCommand::CreateContainer);
            }
            CommandId::ReloadContainer => {
                return self.shared_command(SharedCommand::ReloadContainer);
            }
            // `E` reports Radar's automatic Pi presence standing.
            CommandId::PresenceStatus => {
                return Some(Action::PresenceStatus);
            }
            CommandId::Delete => return self.shared_command(SharedCommand::Delete),
        }
        None
    }

    /// The keys the details answer while they hold the keyboard, and whether the
    /// key was theirs.
    ///
    /// `j`/`k` and the arrows scroll, `PageUp`/`PageDown` move a viewport,
    /// `Home`/`End` reach the limits, `←`/`→` cycle the pages and Escape hands
    /// the keyboard back to the tree without touching the filter. Space moves to
    /// the page's next openable block and Enter opens or closes the one the
    /// keyboard is on. `t` enters or leaves process-row navigation on the
    /// Processes page, where the arrows and `j`/`k` move through the process
    /// table, `Home`/`End` reach its ends and Enter or Space folds the selected
    /// branch; every other key below keeps its meaning in that mode too.
    ///
    /// A key that would act on the selected row is the panel's too, and does
    /// nothing: `Enter` focuses a pane and `x`, `X` and `r` open lifecycle
    /// confirmations, none of which a reader of a page asked for. So does a
    /// process row: entering its navigation is not selecting a fleet row and
    /// never reaches the mux or the owner. Every other key is left to the tree's
    /// own map, so the view keys — `/`, `d`, `s`, `p`, `e`, `b`, `c` and the
    /// jumps — keep working from either panel.
    fn handle_details_key(&mut self, key: KeyEvent) -> bool {
        // While the reader is moving through the process table, the keys that
        // move are the table's: the page's scrolling, the disclosures and the
        // tree's selection are not what the reader asked for. Every other key
        // keeps its own meaning below.
        if self.process.navigating {
            match key.code {
                KeyCode::Char('t') => {
                    self.process.navigating = false;
                    return true;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_process(1);
                    return true;
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_process(-1);
                    return true;
                }
                KeyCode::Home => {
                    self.reach_process(false);
                    return true;
                }
                KeyCode::End => {
                    self.reach_process(true);
                    return true;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    self.fold_selected_process();
                    return true;
                }
                _ => {}
            }
        }
        let page = self.page_step();
        match key.code {
            KeyCode::Char('t') => self.toggle_process_navigation(),
            KeyCode::Char('j') | KeyCode::Down => self.scroll_page(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll_page(-1),
            KeyCode::PageDown => self.scroll_page(page),
            KeyCode::PageUp => self.scroll_page(-page),
            KeyCode::Home => self.scroll_page_to(0),
            KeyCode::End => self.scroll_page_to(u16::MAX),
            KeyCode::Left => self.cycle_page(false),
            KeyCode::Right => self.cycle_page(true),
            KeyCode::Esc => {
                self.process.navigating = false;
                self.details_focused = false;
            }
            KeyCode::Char(' ') => self.cycle_disclosure(),
            KeyCode::Enter => self.toggle_disclosure(),
            // The lifecycle keys are the panel's while it has the keyboard, so
            // nothing acts on the fleet from here.
            KeyCode::Char('D') | KeyCode::Char('r') => {}
            _ => return false,
        }
        true
    }

    /// The one-line action message while one is up.
    pub fn focus_message(&self) -> Option<&str> {
        self.focus_message
            .as_ref()
            .map(|(message, _)| message.as_str())
    }
    /// Shows or clears the one-line action message.
    pub fn set_focus_message(&mut self, message: Option<String>) {
        if let Some(message) = &message {
            tracing::warn!("{message}");
        }
        self.focus_message = message.map(|message| (message, Instant::now()));
    }

    /// Drops a message that has been up for [`FOCUS_MESSAGE_TTL`]. Returns
    /// whether anything changed, so the caller can skip a redraw.
    ///
    /// A completed outcome ages out with it, because the result was the operator's
    /// own action and the tree already shows it. Nothing is retained beyond it: a
    /// longer record is the daemon's, read by the request id the line names.
    pub fn expire_focus_message(&mut self, now: Instant) -> bool {
        match &self.focus_message {
            Some((_, set)) if now.duration_since(*set) >= FOCUS_MESSAGE_TTL => {
                self.focus_message = None;
                true
            }
            _ => false,
        }
    }

    /// Reads one key of the filter editor.
    fn handle_filter_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                // Clearing the filter restores the unfiltered tree with its
                // pre-filter fold state (folds are never touched by filtering).
                self.step(NavStep::ClearFilter);
                self.end_filter_edit();
            }
            KeyCode::Enter => self.end_filter_edit(),
            KeyCode::Backspace => {
                let mut query = self.nav.filter_query().to_string();
                query.pop();
                self.step(NavStep::SetFilter(query));
            }
            KeyCode::Char(c) => {
                let mut query = self.nav.filter_query().to_string();
                query.push(c);
                self.step(NavStep::SetFilter(query));
            }
            _ => {}
        }
    }

    fn begin_filter_edit(&mut self) {
        self.filter_editing = true;
        if let Some(link) = &self.session {
            link.client.submit(UiAction::BeginFilterEdit);
        }
    }

    fn end_filter_edit(&mut self) {
        self.filter_editing = false;
        if let Some(link) = &self.session {
            link.client.submit(UiAction::EndFilterEdit);
        }
    }

    /// Moves the selection by `delta` visible rows, clamped to the view.
    pub fn move_selection(&mut self, delta: i32) {
        self.step(NavStep::Move(delta));
    }

    /// Folds the selected branch when it is open and unfolds it when it is
    /// closed: one key for the whole gesture, whichever way the branch points.
    pub fn toggle_fold(&mut self) {
        self.step(NavStep::ToggleFold);
    }

    /// Folds the selected branch. No-op on a row without visible children, and
    /// while a filter is active: the filtered view keeps matching ancestors
    /// visible by design, so a fold could not take effect.
    pub fn collapse_selected(&mut self) {
        self.step(NavStep::Collapse);
    }

    /// Unfolds the selected branch. No-op when it is already unfolded, and while
    /// a filter is active, like [`Self::collapse_selected`].
    pub fn expand_selected(&mut self) {
        self.step(NavStep::Expand);
    }

    /// Moves to the next pane view: agents, then what is running, then every
    /// pane, and round again.
    pub fn cycle_panes(&mut self) {
        self.step(NavStep::CyclePanes);
    }
}

/// Whether a position falls inside an area. Radar's areas are half-open, as the
/// terminal's are: the edge cell belongs to the next panel.
fn inside(area: Option<Rect>, at: (u16, u16)) -> bool {
    area.is_some_and(|area| {
        at.0 >= area.x && at.0 < area.right() && at.1 >= area.y && at.1 < area.bottom()
    })
}
/// The dialog title for a shared command.
fn title_of(command: SharedCommand) -> &'static str {
    match command {
        SharedCommand::Delete => "Delete",
        SharedCommand::Restart => "Restart worker",
        SharedCommand::CreateContainer => "Add a tab",
        SharedCommand::ReloadContainer => "Reload pane",
        SharedCommand::NewWorkspace => "New workspace",
        SharedCommand::Focus | SharedCommand::LaunchConfigured => "Confirm",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AgentObservation, FleetObservation, Location, Pane, RuntimeStatus, SessionIdentity, Tab,
        Workspace,
    };

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
        assert!(!app.shows_history());
        app.handle_key(key(KeyCode::Char('e')));
        assert!(app.shows_history());
        app.handle_key(key(KeyCode::Char('e')));
        assert!(!app.shows_history());
    }

    #[test]
    fn tab_moves_the_keyboard_between_the_panels_and_escape_hands_it_back() {
        let (_state, mut app) = fixture();
        assert!(!app.details_focused(), "the tree starts with the keyboard");
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focused());
        press(&mut app, KeyCode::BackTab);
        assert!(!app.details_focused());

        // Escape leaves the panel without clearing a filter the tree set.
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('w'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focused());
        press(&mut app, KeyCode::Esc);
        assert!(!app.details_focused(), "Escape returns to the tree");
        assert_eq!(app.filter_query(), "w", "the filter survives the panel");
    }

    #[test]
    fn reading_the_pages_leaves_the_selection_and_its_folds_alone() {
        let (_state, mut app) = fixture();
        let workspace = RowId::Workspace("wA".into());
        select_row(&mut app, &workspace);
        press(&mut app, KeyCode::Char(' '));
        assert!(app.is_collapsed(&workspace), "the branch folds");
        let rows = row_ids(&app);

        press(&mut app, KeyCode::Tab);
        assert!(app.details_focused());
        assert_eq!(app.detail_page(), DetailPage::Overview);
        for code in [
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Left,
            KeyCode::Char('j'),
            KeyCode::Char('k'),
            KeyCode::Char(' '),
            KeyCode::End,
        ] {
            assert_eq!(press(&mut app, code), None, "{code:?} acts on the fleet");
        }
        assert_eq!(
            app.detail_page(),
            DetailPage::Source,
            "the pages cycle, wrapping at the start"
        );
        assert_eq!(
            app.selected_row().map(|row| row.id.clone()),
            Some(workspace.clone())
        );
        assert!(app.is_collapsed(&workspace), "and the fold holds");
        assert_eq!(row_ids(&app), rows, "no panel key moved the tree");
    }

    #[test]
    fn filter_entry_keeps_its_keys_while_the_details_are_focused() {
        let (_state, mut app) = fixture();
        press(&mut app, KeyCode::Tab);
        let page = app.detail_page();
        press(&mut app, KeyCode::Char('/'));

        // Typed text is text: the panel's own keys are characters here, and
        // Enter ends entry rather than acting on the row.
        for code in [KeyCode::Char('j'), KeyCode::Char('r'), KeyCode::Enter] {
            assert_eq!(press(&mut app, code), None, "{code:?} is filter entry");
        }
        assert_eq!(app.filter_query(), "jr");
        assert_eq!(app.detail_page(), page, "the pages did not move");
        assert_eq!(app.details_scroll(), 0, "nor did the page scroll");
    }

    #[test]
    fn hiding_the_details_returns_the_keyboard_to_the_tree() {
        let (_state, mut app) = fixture();
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focused());

        // `d` is the tree's own key and still works from the panel; a hidden
        // panel cannot hold a keyboard.
        press(&mut app, KeyCode::Char('d'));
        assert!(!app.shows_details());
        assert!(!app.details_focused());
        press(&mut app, KeyCode::Tab);
        assert!(!app.details_focused(), "Tab has nowhere to hand it to");
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    /// Connects `session` and publishes `tasks` as its complete list, as the
    fn app_with_fixture() -> App {
        fixture().1
    }

    /// The fixture with the worker moved into its owner's window, so it nests
    /// under the owner: the shape that folds and filters act on.
    fn nested_fixture() -> (ObservationState, App) {
        let mut observation = crate::model::testing::fleet();
        for agent in &mut observation.agents {
            if agent.location.pane_id == "wA:p2" {
                agent.location.tab_id = "wA:t1".into();
            }
        }
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        (state, app)
    }

    /// The fixture observation and a view refreshed against it, for tests that
    /// need to revalidate against the same state the view drew.
    /// The fixture with a command in a pane's foreground: the case that loses
    fn fixture() -> (ObservationState, App) {
        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        let mut app = App::new();
        app.refresh(&state);
        (state, app)
    }

    fn show_all_panes(app: &mut App) {
        while app.pane_view() != PaneView::All {
            app.cycle_panes();
        }
    }

    #[test]
    fn pane_view_starts_at_running_and_cycles_to_all_then_agents() {
        let mut app = app_with_fixture();
        // Without foreground evidence, ordinary panes are unresolved.
        assert_eq!(app.pane_view(), PaneView::Running);
        for row in row_ids(&app) {
            assert!(
                !matches!(row, RowId::Pane(_)),
                "no ordinary pane row before toggling: {row:?}"
            );
        }

        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.pane_view(), PaneView::All);
        let ids = row_ids(&app);
        assert!(ids.contains(&RowId::Pane("wA:p3".into())));
        assert!(ids.contains(&RowId::Pane("wA:p6".into())));

        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.pane_view(), PaneView::Hidden);
        assert!(!row_ids(&app).iter().any(|id| matches!(id, RowId::Pane(_))));

        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.pane_view(), PaneView::Running);
    }

    #[test]
    fn workspaces_survive_pane_modes_and_a_filter_with_no_matches() {
        let mut observation = crate::model::testing::fleet();
        observation.agents.clear();
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        let workspaces = vec![RowId::Workspace("wA".into()), RowId::Workspace("wB".into())];
        for mode in [PaneView::Running, PaneView::All, PaneView::Hidden] {
            assert_eq!(app.pane_view(), mode);
            let ids = row_ids(&app);
            assert!(workspaces.iter().all(|id| ids.contains(id)));
            app.handle_key(key(KeyCode::Char('/')));
            for c in "this-query-matches-no-existing-row-123456789".chars() {
                app.handle_key(key(KeyCode::Char(c)));
            }
            app.handle_key(key(KeyCode::Enter));
            assert_eq!(
                row_ids(&app),
                workspaces,
                "workspace rows survive in {mode:?}"
            );
            app.handle_key(key(KeyCode::Esc));
            app.handle_key(key(KeyCode::Char('p')));
        }
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
        let mut app = nested_fixture().1;
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
        let mut app = nested_fixture().1;
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
                RowId::Workspace("wB".into()),
            ]
        );
        // Non-workspace rows without a matching descendant are hidden.
        assert!(!ids.contains(&RowId::Agent("wA:p5".into())));
        assert!(!ids.contains(&RowId::Agent("wB:p1".into())));
        assert!(ids.contains(&RowId::Workspace("wB".into())));

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
        let mut app = nested_fixture().1;
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
        let (mut state, mut app) = nested_fixture();
        // Fold the owner's branch, select it, then hide panes so a refresh has
        // a second kind of change to survive.
        show_all_panes(&mut app);
        select(&mut app, "wA:p1");
        app.collapse_selected();

        // Same facts, plus a status change (the worker goes done).
        let mut observation = crate::model::testing::fleet();
        for agent in &mut observation.agents {
            if agent.location.pane_id == "wA:p2" {
                agent.status = Some(RuntimeStatus::Done);
                agent.location.tab_id = "wA:t1".into();
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
    fn running_view_lists_live_shells_and_commands_but_not_unresolved_panes() {
        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        state.apply_evidence(
            "wA:p3",
            crate::model::ForegroundEvidence::command(
                7,
                Some("hunk".into()),
                Some("hunk diff".into()),
            ),
        );
        state.apply_evidence("wA:p6", crate::model::ForegroundEvidence::Shell);
        state.apply_evidence("wA:p4", crate::model::ForegroundEvidence::Inconclusive);
        let mut app = App::new();
        app.refresh(&state);
        assert_eq!(app.pane_view(), PaneView::Running);

        let ids = row_ids(&app);
        assert!(ids.contains(&RowId::Pane("wA:p3".into())));
        assert!(ids.contains(&RowId::Pane("wA:p6".into())));
        assert!(!ids.contains(&RowId::Pane("wA:p4".into())));
        assert!(ids.contains(&RowId::Agent("wA:p2".into())));

        state.apply_evidence(
            "wA:p4",
            crate::model::ForegroundEvidence::NonShell {
                pid: 9,
                name: None,
                command: None,
                local: Default::default(),
            },
        );
        app.refresh(&state);
        assert!(
            row_ids(&app).contains(&RowId::Pane("wA:p4".into())),
            "a live process needs no command label"
        );
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
        let mut observation = crate::model::testing::fleet();
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
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
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
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
            workspaces: vec![Workspace {
                workspace_id: "wX".into(),
                label: Some("solo".into()),
                number: None,
                session_path: None,
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
                current_path: None,
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
                registry: None,
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

    /// A destructive command's target belongs to the shared session now. Without
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
        state.apply_success(crate::model::testing::fleet());
        state.apply_failure("tmux timed out after 5s");
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
    fn a_focus_message_expires_on_its_own() {
        let mut app = app_with_fixture();
        app.set_focus_message(Some("tmux refused to focus: pane_not_found".into()));
        assert!(!app.expire_focus_message(Instant::now()));
        assert!(app.expire_focus_message(Instant::now() + FOCUS_MESSAGE_TTL));
        assert_eq!(app.focus_message(), None);
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
        managed_state_ending(facts, status, owner, false)
    }

    /// The same worker, with the daemon's recorded exit decided by the caller.
    fn managed_state_ending(
        facts: crate::model::HerdsmanFacts,
        status: RuntimeStatus,
        owner: Option<&str>,
        ended: bool,
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
            metadata_agents: Vec::new(),
            containers: Vec::new(),
            lifecycle_subjects: Vec::new(),
            workspaces: vec![Workspace {
                workspace_id: "wM".into(),
                label: Some("managed".into()),
                number: None,
                            session_path: None,
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
                            current_path: None,
            }],
            agents: vec![AgentObservation {
                location,
                name: Some("pi".into()),
                label: None,
                status: Some(status),
                session: None,
                lineage,
                facts,
                registry: Some(crate::registry_observation::RegistryView {
                    agent: serde_json::from_value(serde_json::json!({
                        "registration": {
                            "version": 1,
                            "registration_id": "managed-agent",
                            "registered_at": "2026-01-01T00:00:00Z",
                            "source": "test",
                            "incarnation": "11111111-1111-4111-8111-111111111111",
                            "location": {"backend":"tmux", "workspace":"wM", "tab":"wM:t1", "pane":"wM:p1"},
                            "process_claimed": true,
                            "launch": {"available": false}
                        },
                        "process": {"claim": {"boot_id":"boot", "pid":1234, "start_ticks":55}},
                        "execution": null,
                        "assignment": null,
                        "facts": null
                    })).expect("registry agent"),
                    binding: crate::registry_observation::Binding::Joined,
                    spawn: None,
                    managed: Some(crate::registry_observation::ManagedSubject {
                        subject_id: "subject-managed".into(),
                    claim_id: "claim-managed".into(),
                        process: crate::model::ProcessIdentity { boot_id: "boot".into(), pid: 1234, start_ticks: 55 },
                        metadata: crate::control_plane::lifecycle::ManagedMetadata::default(),
                        child_close: None,
                        state: crate::control_plane::lifecycle::State::Bound,
                        session: Some(MANAGED_OWNER.into()),
                        ended,
                    }),
                }),
            }],
        });
        state
    }

    /// The agent rows a view lists, ignoring workspace and pane rows. An agent
    /// that joins no pane is keyed by its registration, so both names count.
    fn agent_rows(app: &App) -> Vec<RowId> {
        row_ids(app)
            .into_iter()
            .filter(|id| matches!(id, RowId::Agent(_) | RowId::Registered(_)))
            .collect()
    }

    #[test]
    fn an_exited_worker_is_history_until_the_toggle_asks_for_it() {
        let state = managed_state_ending(
            managed_facts(),
            RuntimeStatus::Idle,
            Some(MANAGED_OWNER),
            true,
        );
        let mut app = App::new();
        app.refresh(&state);

        // The daemon recorded this subject's process as ended, so the live fleet
        // does not list it — and the count says one row is behind the toggle
        // rather than losing it silently.
        assert!(agent_rows(&app).is_empty(), "{:?}", row_ids(&app));
        assert_eq!(app.hidden_history(), 1);

        app.toggle_history();
        assert_eq!(agent_rows(&app).len(), 1, "{:?}", row_ids(&app));
        assert_eq!(app.hidden_history(), 0);

        app.toggle_history();
        assert!(agent_rows(&app).is_empty(), "{:?}", row_ids(&app));
    }

    #[test]
    fn a_command_is_refused_rather_than_retargeted_when_the_shared_selection_is_unexplained() {
        use crate::control_plane::public_contracts::ui_session as dto;
        use crate::ui_session::client::Mirror;

        let app = app_with_fixture();
        let local = app.nav.selected_id().cloned().expect("a selection");
        let agreed = Mirror {
            observation: std::sync::Arc::new(crate::ui_session::publication::Observation::default()),
            diagnostic: None,
            attachment_id: "a".into(),
            update_id: 1,
            state: dto::SessionState {
                selected: Some(crate::ui_session::session::ui_row_id(&local)),
                ..dto::SessionState::default()
            },
            rows: std::sync::Arc::new(Vec::new()),
        };
        assert_eq!(
            App::shared_row(&app.nav, Some(&agreed)),
            Ok(Some(crate::ui_session::session::ui_row_id(&local))),
            "the session and this view agree, so the command names that row"
        );

        let other = app
            .visible_rows()
            .iter()
            .find(|row| *row.id != local)
            .expect("another row")
            .id
            .clone();
        let mut previous = agreed.clone();
        previous.state.selected = Some(crate::ui_session::session::ui_row_id(&other));
        assert_eq!(
            App::shared_row(&app.nav, Some(&previous)),
            Ok(Some(crate::ui_session::session::ui_row_id(&local))),
            "a known older selection must not override the displayed optimistic row"
        );

        // The session selects a row this view's observation cannot describe.
        let elsewhere = Mirror {
            observation: std::sync::Arc::new(crate::ui_session::publication::Observation::default()),
            diagnostic: None,
            attachment_id: "a".into(),
            update_id: 2,
            state: dto::SessionState {
                selected: Some(dto::UiRowId::Pane {
                    pane_id: "elsewhere:p1".into(),
                }),
                ..dto::SessionState::default()
            },
            rows: std::sync::Arc::new(Vec::new()),
        };
        assert_eq!(
            App::shared_row(&app.nav, Some(&elsewhere)),
            Err("that row is selected in the session but not described by this view yet"),
            "nothing acts on the stale local row"
        );
        assert_eq!(
            App::shared_row(&app.nav, None),
            Ok(Some(crate::ui_session::session::ui_row_id(&local))),
            "a view with no session names its own row"
        );
    }

    #[test]
    fn an_empty_canonical_tree_draws_nothing_rather_than_the_rows_remembered_here() {
        use crate::control_plane::public_contracts::ui_session as dto;
        use crate::ui_session::client::Mirror;

        let app = app_with_fixture();
        assert!(
            app.visible_rows().len() > 1,
            "the fixture gives this view rows of its own"
        );
        let attached = Mirror {
            observation: std::sync::Arc::new(crate::ui_session::publication::Observation::default()),
            diagnostic: None,
            attachment_id: "a".into(),
            update_id: 1,
            state: dto::SessionState::default(),
            rows: std::sync::Arc::new(Vec::new()),
        };
        assert!(
            App::drawn(Some(&attached), &app.nav).is_empty(),
            "a session that projects no rows is an answer, not a gap to fill locally"
        );
        assert_eq!(
            App::drawn(None, &app.nav).len(),
            app.visible_rows().len(),
            "a view with no session draws its own rows"
        );
    }

    #[test]
    fn an_agent_that_joins_no_pane_is_history_too() {
        let mut state = managed_state(managed_facts(), RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut observation = state.inventory().expect("current inventory").clone();
        observation.agents[0]
            .registry
            .as_mut()
            .expect("registry view")
            .binding = crate::registry_observation::Binding::Unjoined(
            crate::registry_observation::Unjoined::PaneAbsent,
        );
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);

        assert_eq!(row_ids(&app), vec![RowId::Workspace("wM".into())]);
        assert_eq!(app.hidden_history(), 1);

        app.toggle_history();
        assert_eq!(agent_rows(&app).len(), 1, "{:?}", row_ids(&app));
    }

    #[test]
    fn a_palette_choice_is_a_tree_command_even_while_details_hold_the_keyboard() {
        let (_, mut app) = fixture();
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focused, "Tab hands the keyboard to details");
        // The details panel keeps its own `r`, so the key itself does nothing.
        assert_eq!(press(&mut app, KeyCode::Char('r')), None);

        press(&mut app, KeyCode::Char('?'));
        for ch in "restart".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            None,
            "the palette runs Restart on the tree, not through the details panel"
        );
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("shared UI session")),
            "the palette's command reached the tree"
        );
    }

    #[test]
    fn the_palette_filters_commands_and_runs_the_chosen_one_by_its_identity() {
        let (_, mut app) = fixture();
        assert_eq!(press(&mut app, KeyCode::Char('?')), None);
        assert!(app.palette().is_some(), "? opens the palette");

        // Keys reach the query, not the tree: `e` would otherwise show finished
        // sessions, and `q` would be a quit the main loop must not see.
        let selected = app.selected_index();
        for ch in "show or hide history".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        assert_eq!(app.selected_index(), selected);
        let palette = app.palette().expect("open");
        assert_eq!(palette.query(), "show or hide history");
        assert_eq!(
            palette.rows(),
            [(
                "Show or hide history: exited, finished and unresolved rows",
                Some("e")
            )],
            "only the matching command is offered"
        );

        // Enter answers with that command's key, which does what the key does.
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.palette().is_none(), "choosing closes the palette");
        assert!(app.shows_history(), "the chosen command ran by identity");

        press(&mut app, KeyCode::Char('?'));
        assert_eq!(press(&mut app, KeyCode::Esc), None);
        assert!(app.palette().is_none(), "Escape closes it");
    }
}
