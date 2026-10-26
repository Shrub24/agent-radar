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

use crate::control_plane::client::{
    ContainerCreate, ContainerCreateKind, ContainerDelete, ContainerReload,
};
use crate::lifecycle::{self, CloseRequest, Containment, TargetIdentity};
use crate::observation::{ObservationState, SourceFreshness};
use crate::picker::{Item, Picker, Step};
use crate::runtime::{CloseTarget, Target};
use crate::tasks::{Task, TaskBoard, TaskEvent};
use crate::theme;
use ratatui::layout::Rect;

use crate::model::{
    AgentState, CpuPercent, DescendantSample, FleetObservation, ForegroundEvidence,
    ProcessIdentity, ProcessResources,
};
use crate::tree::{
    AgentRow, ContainerRef, FleetTree, PaneRow, RowId, RowKind, TaskId, TaskRow, TaskSource,
    TreeNode,
};

/// How long a focus message stays up when no key clears it first.
const FOCUS_MESSAGE_TTL: Duration = Duration::from_secs(5);

/// How long a completed outcome stays in the footer. A failure is kept until `c`.
const NOTICE_TTL: Duration = Duration::from_secs(8);

/// The width of a row's disclosure marker in cells: `ui::fold_marker` draws it,
/// and a leaf keeps the same two blank columns, so the hit cell is the same
/// whether or not the row has children to show.
const DISCLOSURE_WIDTH: u16 = 2;

/// What a key asked the main loop to do outside the view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Ask the backend to focus this location.
    Focus(Target),
    /// `x`/`X`/`r` asked for confirmation of a lifecycle action. The caller
    /// opens it against the current observation, because eligibility is a
    /// property of the evidence, not of the cursor.
    BeginAction(Operation),
    /// Create a new workspace. It needs no selected row, so it works when every
    /// row on screen is unresolved.
    NewWorkspace,
    /// Confirm was activated; the caller revalidates the frozen target and
    /// starts the close or manages the request when it still holds.
    ConfirmAction,
    /// Immediately create a managed-agent pane using `[tui].launch_command`.
    LaunchConfigured,
    /// The sidebar's hide/unhide key; the caller owns the host commands.
    ToggleSidebar,
    /// The palette chose a command: run this key and use whatever it returns.
    Replay(KeyCode),
    /// The sidebar's zoom key; the caller owns the host commands.
    ToggleZoom,
    /// Open a sidebar beside this dashboard; the caller owns the host command.
    LaunchSidebar,
    /// Open this key's picker or confirmation in the workspace popup, over the
    /// row that was selected here.
    Modal { key: KeyCode, row: Option<RowId> },
    /// The workspace chooser took this directory; open a workspace there.
    NewWorkspaceIn(PathBuf),
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
            RowKind::Workspace { .. } | RowKind::Agent(_) | RowKind::Lifecycle(_) => true,
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

/// Below this width the sidebar is drawn as a one-column rail of state marks.
pub const SIDEBAR_RAIL_BELOW: u16 = 16;

/// At this width a zoomed sidebar is drawn as the whole dashboard. The host
/// terminal sets the split, so the width is the sidebar's only layout state.
pub const SIDEBAR_FULL_AT: u16 = 100;

/// Whether a sidebar given `width` columns is drawn as the collapsed rail.
pub fn sidebar_collapsed(width: u16) -> bool {
    width < SIDEBAR_RAIL_BELOW
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
    /// Drawn as `radar sidebar`: compressed rows while the split is narrow, and
    /// the whole dashboard once the host has zoomed it.
    sidebar: bool,
    /// Drawn as the popup behind a sidebar's picker or confirmation: only that
    /// modal, over a row chosen by identity, and gone once it is answered.
    modal_only: bool,
    /// A host can show a popup over the whole workspace, so a sidebar's pickers
    /// and confirmations go there instead of into its narrow pane.
    modal_host: bool,
    /// The width the last draw was given. The sidebar's own layout follows it:
    /// the host terminal owns the split, so its width is the state.
    width: u16,
    /// The command palette.
    palette: Option<Picker<KeyCode>>,
    /// The workspace chooser, open while a new workspace waits for its directory.
    workspace_picker: Option<Picker<PathBuf>>,
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
    bus: TaskBoard,
    /// Why the bus is not running, when the listener could not bind. Kept
    /// apart from source freshness: a bus that never bound says nothing about
    /// the fleet Radar observes.
    task_diagnostic: Option<String>,
    /// Focus target per row, for the rows the displayed observation proves. A
    /// row missing here is one `Enter` must refuse and explain.
    focus_targets: HashMap<RowId, Target>,
    /// Whether the displayed inventory came from a successful collection. A
    /// stale inventory is last-good rather than current, so `Enter` refuses on
    /// every row of it.
    source_current: bool,
    /// Containers whose deletion was held back because an agent inside them did
    /// not exit. The next `D` on one of them asks the daemon to close anyway, so
    /// a wedged agent cannot make a place undeletable for good.
    deletion_held: HashSet<String>,
    /// One line about the last action Radar took or refused — a focus or a
    /// lifecycle close — and when it was set. It clears on the next key press or
    /// after [`FOCUS_MESSAGE_TTL`].
    focus_message: Option<(String, Instant)>,
    /// How siblings are ordered. Structure is never changed by it.
    order: RowOrder,
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
    /// The open lifecycle confirmation, if any. Its target is frozen at the
    /// moment it opened; nothing is sent until it is revalidated and confirmed.
    confirmation: Option<Confirmation>,
    /// Lifecycle action outcomes, kept until dismissed. Separate from the
    /// transient focus message and from the source diagnostics, so an owner's
    /// answer cannot be mistaken for the fleet's freshness.
    notices: Vec<Notice>,
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
    /// Daemon-owned subject target for a managed lifecycle operation.
    pub subject: Option<crate::registry_observation::ManagedSubject>,
    /// The container this confirmation acts on, when it is a container control:
    /// the daemon revalidates that exact record before it changes anything.
    pub container_id: Option<String>,
}

/// A confirmed action ready to run: direct unmanaged close, daemon lifecycle
/// operation, or daemon topology control.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirmed {
    Direct(CloseRequest),
    /// A daemon-owned lifecycle control, targeted by stable subject id.
    Subject {
        operation: Operation,
        request_id: String,
        subject_id: String,
        claim_id: String,
        child_close: Option<crate::registry_observation::ChildCloseTarget>,
        close_request_id: Option<String>,
    },
    /// A container control, dispatched to the daemon under its own request id.
    Container(crate::topology::Control),
}

/// One lifecycle outcome, kept visible until it is dismissed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// When this line was recorded or last refined, so a success can age out.
    pub at: Instant,
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

    /// A view for the workspace popup behind a sidebar's picker or confirmation.
    pub fn modal() -> Self {
        Self {
            modal_only: true,
            ..Self::default()
        }
    }

    pub fn is_modal_only(&self) -> bool {
        self.modal_only
    }

    /// Records that a workspace popup is available to this sidebar.
    pub fn set_modal_host(&mut self, available: bool) {
        self.modal_host = available;
    }

    /// The selected row's identity, which a popup selects again by value.
    pub fn selected_id(&self) -> Option<RowId> {
        self.selected.clone()
    }

    /// Lists every row, so a row another view could see is one this can select.
    pub fn show_everything(&mut self) {
        self.view = PaneView::All;
        self.finished_shown = true;
        self.task_rows.all = true;
        self.reconcile_selection();
    }

    /// Selects `id` exactly, or nothing: a row that is gone is not replaced by a
    /// neighbour, because the action it was chosen for must not land elsewhere.
    pub fn select_exact(&mut self, id: &RowId) -> bool {
        let Some(index) = self.visible_rows().iter().position(|row| row.id == id) else {
            return false;
        };
        self.select(Some(id.clone()));
        self.anchor = index;
        true
    }

    /// Records the width this draw was given, so the sidebar's layout can follow
    /// the split the host terminal owns.
    pub fn note_width(&mut self, width: u16) {
        self.width = width;
    }

    /// Whether the sidebar is drawn compressed: narrow rows, no details and no
    /// lifecycle actions. False for the dashboard, and false for a sidebar the
    /// host has zoomed to the full width.
    pub fn sidebar_compact(&self) -> bool {
        self.sidebar && self.width < SIDEBAR_FULL_AT
    }

    /// Whether the sidebar is drawn as the collapsed rail.
    pub fn sidebar_railed(&self) -> bool {
        self.sidebar && sidebar_collapsed(self.width)
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
        self.prune_disclosures();
        self.reconcile_process();
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

    /// Moves the selection to a row identity. Another row starts every page at
    /// its top: a scroll belongs to the row it was scrolled in, and so does an
    /// opened block.
    fn select(&mut self, id: Option<RowId>) {
        if self.selected != id {
            self.detail_scroll = [0; DetailPage::COUNT];
            self.expanded_disclosures.clear();
            self.disclosure_target = 0;
            // The process table belongs to the row it was drawn for: another row
            // is another process, and folds and a cursor are not carried onto it.
            self.process = ProcessCursor::default();
        }
        self.selected = id;
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
        self.view
    }

    /// Whether the details panel is shown. Shown by default, and toggleable
    /// because a small terminal or a glance at the tree alone wants the width.
    pub fn shows_details(&self) -> bool {
        !self.sidebar_compact() && !self.details_hidden
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
    pub fn apply_task_event(&mut self, event: TaskEvent) {
        self.bus.apply(event);
        self.tree.attach_tasks(&self.bus);
        self.reconcile_selection();
        self.prune_disclosures();
    }

    pub fn replace_task_snapshot(&mut self, events: Vec<TaskEvent>) {
        let mut bus = TaskBoard::default();
        for event in events {
            bus.apply(event);
        }
        self.bus = bus;
        self.tree.attach_tasks(&self.bus);
        self.reconcile_selection();
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

    /// The open command palette, if any.
    pub fn palette(&self) -> Option<&Picker<KeyCode>> {
        self.palette.as_ref()
    }

    /// The open workspace chooser, if any.
    pub fn workspace_picker(&self) -> Option<&Picker<PathBuf>> {
        self.workspace_picker.as_ref()
    }

    /// Whether a picker holds the keyboard.
    pub fn picker_open(&self) -> bool {
        self.palette.is_some() || self.workspace_picker.is_some()
    }

    /// Opens the workspace chooser over `directories`. The first entry starts as
    /// the highlight, so a bare `Enter` takes the directory Radar was started in.
    pub fn open_workspace_picker(&mut self, directories: Vec<PathBuf>) {
        let items = directories
            .into_iter()
            .map(|directory| {
                let label = directory.to_string_lossy().into_owned();
                Item::new(label, None::<String>, directory)
            })
            .collect();
        self.workspace_picker = Some(Picker::new("Workspace", "enter open · esc cancel", items));
    }

    /// One picker key. Answers with the replay of a chosen command, or with the
    /// chosen workspace, so the caller keeps the effect.
    fn handle_picker_key(&mut self, key: KeyEvent) -> Option<Action> {
        if let Some(mut picker) = self.palette.take() {
            return match picker.handle_key(key) {
                Step::Continue => {
                    self.palette = Some(picker);
                    None
                }
                Step::Close => None,
                Step::Choose(code) => Some(Action::Replay(code)),
            };
        }
        let mut picker = self.workspace_picker.take()?;
        match picker.handle_key(key) {
            Step::Continue => {
                self.workspace_picker = Some(picker);
                None
            }
            Step::Close => None,
            Step::Choose(directory) => Some(Action::NewWorkspaceIn(directory)),
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
        if self.confirmation.is_some() {
            return self.handle_confirmation_key(key);
        }
        self.focus_message = None;
        // The command palette is modal: it owns the keyboard until it closes,
        // so its letters filter commands rather than moving the tree.
        if self.picker_open() {
            return self.handle_picker_key(key);
        }
        // The outcomes panel is modal: it shows what was reported and acts on
        // nothing, so while it is open no other key reaches the tree.
        if self.notices_open {
            match key.code {
                KeyCode::Char('!') | KeyCode::Esc | KeyCode::Char('c') => {
                    if key.code == KeyCode::Char('c') {
                        self.dismiss_notices();
                    }
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
                // Where the host can float a popup over the whole workspace, the
                // pickers and confirmations go there; this pane is too narrow.
                KeyCode::Char(ch @ ('r' | 'a' | 'A' | 'E' | 'D' | 'R' | '?'))
                    if self.modal_host =>
                {
                    let row = (ch != 'A').then(|| self.selected.clone()).flatten();
                    return Some(Action::Modal {
                        key: KeyCode::Char(ch),
                        row,
                    });
                }
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
        match key.code {
            // `S` asks the host to open a sidebar beside this dashboard; the
            // sidebar itself cannot, so the key is gated above.
            KeyCode::Char('S') => return Some(Action::LaunchSidebar),
            KeyCode::Char('r') => return Some(Action::BeginAction(Operation::Restart)),
            // Container controls act on a container Radar records rather than on an
            // owner: `a` adds a place inside it, `D` deletes it, `R` reloads its slot.
            KeyCode::Char('a') => return Some(Action::BeginAction(Operation::CreateContainer)),
            // `A` starts a workspace from nowhere: nothing is selected, replaced or
            // confirmed, because an addition cannot take anything away.
            KeyCode::Char('A') => return Some(Action::NewWorkspace),
            // `L` launches the configured managed agent in a new tab or pane.
            // `E` reports Radar's automatic Pi presence standing.
            KeyCode::Char('E') => return Some(Action::BeginAction(Operation::PresenceStatus)),
            KeyCode::Char('L') => return Some(Action::LaunchConfigured),
            KeyCode::Char('D') => return Some(Action::BeginAction(Operation::Delete)),
            KeyCode::Char('R') => return Some(Action::BeginAction(Operation::ReloadContainer)),
            KeyCode::Char('c') => self.dismiss_notices(),
            // `!` opens the whole text of what was reported: the footer shows one
            // clipped line, and a refusal is longer than that.
            KeyCode::Char('!') => self.toggle_notices(),
            KeyCode::Tab | KeyCode::BackTab => self.toggle_detail_focus(),
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
            KeyCode::Char('?') => {
                self.palette = Some(Picker::new(
                    "Commands",
                    "enter run · esc close",
                    commands::items(self.sidebar, self.sidebar_compact(), self.modal_only),
                ))
            }
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
            self.select(Some(rows[index].id.clone()));
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
                    RowId::Registered(registration_id) => {
                        format!("unresolved agent {registration_id}")
                    }
                    RowId::Task(_) => unreachable!("a task is looked up through its owner row"),
                    RowId::Lifecycle(subject_id) => format!("lifecycle subject {subject_id}"),
                };
                self.set_focus_message(Some(format!("{described} is not observed")));
                None
            }
        }
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
    /// own action and the tree already shows it. A failure stays until dismissed.
    pub fn expire_focus_message(&mut self, now: Instant) -> bool {
        let before = self.notices.len();
        self.notices
            .retain(|notice| notice.failed || now.duration_since(notice.at) < NOTICE_TTL);
        let aged = self.notices.len() != before;
        match &self.focus_message {
            Some((_, set)) if now.duration_since(*set) >= FOCUS_MESSAGE_TTL => {
                self.focus_message = None;
                true
            }
            _ => aged,
        }
    }

    /// Reads one key of the filter editor.
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
            self.select(None);
            self.anchor = 0;
            return;
        }
        let current = self
            .selected_index()
            .unwrap_or_else(|| self.anchor.min(rows.len() - 1));
        let next = (current as i64 + delta as i64).clamp(0, rows.len() as i64 - 1) as usize;
        self.select(Some(rows[next].id.clone()));
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
            self.select(None);
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
            self.select(Some(rows[index].id.clone()));
            self.anchor = index;
            return;
        }
        // The selected row is gone: stay at the same position if the view has
        // one, otherwise at its end. Selection is never left dangling.
        let anchor = self.anchor.min(rows.len() - 1);
        self.select(Some(rows[anchor].id.clone()));
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
        CloseTarget::Pane(pane_id) => {
            lifecycle::pane(inventory, state.retained_containment(), pane_id)
        }
        CloseTarget::Tab(tab_id) => lifecycle::tab(inventory, state.retained_containment(), tab_id),
        CloseTarget::Workspace(workspace_id) => {
            lifecycle::workspace(inventory, state.retained_containment(), workspace_id)
        }
    }
}

/// Why a target cannot be closed directly, when it cannot.
/// A container named for a person: its label, or what the place is called. Its
/// record id is never shown, because it names nothing an operator recognises.
fn container_name(inventory: &crate::model::FleetObservation, container: &ContainerRef) -> String {
    let kind = match container.kind {
        crate::control_plane::containers::Kind::Workspace => "workspace",
        crate::control_plane::containers::Kind::Tab => "tab",
        crate::control_plane::containers::Kind::Pane => "pane",
    };
    let place = match container.kind {
        crate::control_plane::containers::Kind::Workspace => inventory
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == container.handle)
            .and_then(|workspace| workspace.label.clone()),
        crate::control_plane::containers::Kind::Tab => inventory
            .tabs
            .iter()
            .find(|tab| tab.tab_id == container.handle)
            .and_then(|tab| tab.label.clone()),
        crate::control_plane::containers::Kind::Pane => inventory
            .pane(&container.handle)
            .and_then(|pane| pane.display_name().map(str::to_string)),
    };
    match place.filter(|place| !place.is_empty()) {
        Some(place) => format!("{kind} {place}"),
        None => format!("this {kind}"),
    }
}

/// Everything ending this target would take with it, walked down from the level
/// asked about so one rule applies at every level and the answer is collated
/// there: an agent mid-task, a non-shell command in a pane, a running background
/// task. Idle shells, idle agents and finished work are not losses.
fn losses_under(
    inventory: &crate::model::FleetObservation,
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
/// the place, so they are named in the same question. The count is the pane's own
/// publisher's, and it speaks only for the running ones.
fn task_losses(inventory: &crate::model::FleetObservation, target: &CloseTarget) -> Vec<String> {
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
fn panes_in_scope<'a>(
    inventory: &'a crate::model::FleetObservation,
    target: &CloseTarget,
) -> Vec<&'a crate::model::Pane> {
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

/// The refusal a containment state gives a direct close, when it gives one.
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
        /// The tab the row is the root of, when it is: `D` on that row ends the
        /// whole tab, because the row stands for it.
        tab: Option<String>,
        losses: Vec<String>,
        subject: Option<Box<crate::registry_observation::ManagedSubject>>,
    },
    Pane {
        pane_id: String,
        /// The tab this row stands for, when it is the row's root.
        tab: Option<String>,
        losses: Vec<String>,
    },
    Refused(&'static str),
}

/// Whether the target still exists in this inventory.
fn target_present(inventory: &FleetObservation, target: &CloseTarget) -> bool {
    match target {
        CloseTarget::Pane(pane_id) => inventory.pane(pane_id).is_some(),
        CloseTarget::Tab(tab_id) => inventory.tabs.iter().any(|tab| tab.tab_id == *tab_id),
        CloseTarget::Workspace(workspace_id) => inventory
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == *workspace_id),
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
    use crate::control_plane::containers::{
        Binding as ContainerBinding, Kind as ContainerKind, Provenance as ContainerProvenance,
        PublicContainer,
    };
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
        assert!(!app.shows_finished());
        app.handle_key(key(KeyCode::Char('e')));
        assert!(app.shows_finished());
        app.handle_key(key(KeyCode::Char('e')));
        assert!(!app.shows_finished());
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
    fn the_details_issue_no_lifecycle_or_focus_action() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        let pane = RowId::Pane("wA:p3".into());
        select_row(&mut app, &pane);
        press(&mut app, KeyCode::Tab);
        assert!(app.details_focused());

        // Enter focuses a pane and x, X and r open lifecycle confirmations; from
        // a page they are the panel's keys and do nothing at all.
        for code in [
            KeyCode::Enter,
            KeyCode::Char('D'),
            KeyCode::Char('D'),
            KeyCode::Char('r'),
        ] {
            assert_eq!(press(&mut app, code), None, "{code:?} is the panel's");
            assert!(app.confirmation().is_none(), "{code:?} opened a dialog");
        }
        assert_eq!(
            app.selected_row().map(|row| row.id.clone()),
            Some(pane),
            "the selected row is the same one"
        );
        assert!(app.details_focused(), "the panel still has the keyboard");

        // Back in the tree the same keys act again, so the panel is what
        // suppressed them.
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            press(&mut app, KeyCode::Char('D')),
            Some(Action::BeginAction(Operation::Delete))
        );
        // The pane is idle, so the action is handed back rather than drawn.
        assert!(matches!(
            app.begin_action(Operation::Delete, &state),
            Some(Confirmed::Direct(_))
        ));
    }

    #[test]
    fn space_picks_the_pages_next_block_and_enter_opens_only_that_one() {
        let mut observation = crate::model::testing::fleet();
        let owner = observation
            .agents
            .iter_mut()
            .find(|agent| agent.location.pane_id == "wA:p1")
            .expect("owner");
        // The session the fixture owner was launched in, which is what its
        // published tasks must name to join to it.
        let session = owner
            .lineage
            .as_ref()
            .expect("the owner publishes its session")
            .session
            .as_str()
            .to_string();
        // Publisher task snapshots join by fresh registry context, not by the
        // legacy lineage fact in this fixture.
        let registration_id = "task-owner";
        let mut registry_agent: crate::registry_observation::RegistryAgent =
            serde_json::from_value(serde_json::json!({
                "registration": {
                    "version": 1,
                    "registration_id": registration_id,
                    "registered_at": "2026-01-01T00:00:00Z",
                    "source": "herdsman",
                    "incarnation": "11111111-1111-4111-8111-111111111111",
                    "session": session,
                    "label": "owner",
                    "location": {"backend":"test", "workspace":"wA", "tab":"wA:t1", "pane":"wA:p1"},
                    "process_claimed": false,
                    "launch": {"available":false}
                },
                "process": {"claimed":false,"claim":null,"verification":"unavailable"},
                "execution": null,
                "assignment": null,
                "facts": null
            }))
            .expect("task owner registration fixture");
        registry_agent.context = Some(
            serde_json::from_value(serde_json::json!({
                "version": 1,
                "registration_id": registration_id,
                "writer": {
                    "source":"herdsman",
                    "incarnation":"22222222-2222-4222-8222-222222222222",
                    "generation":1,
                    "sequence":1
                },
                "context": {
                    "source":"herdsman",
                    "incarnation":"22222222-2222-4222-8222-222222222222",
                    "generation":1,
                    "sequence":1,
                    "lease_ms":30000,
                    "received_at":"2026-01-01T00:00:00Z",
                    "expires_at":"2026-01-01T00:00:30Z",
                    "restored":false,
                    "freshness":"fresh",
                    "session":session
                }
            }))
            .expect("fresh task session context fixture"),
        );
        owner.registry = Some(crate::registry_observation::RegistryView {
            agent: registry_agent,
            binding: crate::registry_observation::Binding::Joined,
            spawn: None,
            managed: None,
        });
        // Long enough that the panel puts it behind a marker rather than
        // drawing it over everything below it.
        owner.facts.assignment = Some("x".repeat(200));
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        publish_tasks(
            &mut app,
            &session,
            ["bg-1", "bg-2"]
                .iter()
                .map(|id| task_with_command(id, "nix build .#radar"))
                .collect(),
        );
        let owner_row = RowId::Agent("wA:p1".into());
        select_row(&mut app, &owner_row);
        press(&mut app, KeyCode::Tab);

        // Overview's one block is the long assignment, and Enter opens it
        // without focusing a pane or raising a dialog.
        assert_eq!(app.disclosures(), vec![Disclosure::Assignment]);
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.disclosure_open(&Disclosure::Assignment));
        assert!(app.confirmation().is_none());
        assert_eq!(
            app.selected_row().map(|row| row.id.clone()),
            Some(owner_row.clone())
        );

        // Processes has nothing to open, so its keys move nothing at all.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.detail_page(), DetailPage::Processes);
        assert!(app.disclosures().is_empty());
        assert_eq!(press(&mut app, KeyCode::Char(' ')), None);
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.disclosure_open(&Disclosure::Assignment));

        // Tasks offers one block per task with text behind it. Space cycles
        // them, and Enter opens the one the keyboard is on — the other task's
        // block and the assignment stay as they were.
        press(&mut app, KeyCode::Right);
        assert_eq!(app.detail_page(), DetailPage::Tasks);
        let blocks = app.disclosures();
        assert_eq!(blocks.len(), 2, "one block per task: {blocks:?}");
        assert_eq!(app.disclosure_target().as_ref(), Some(&blocks[0]));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.disclosure_target().as_ref(), Some(&blocks[1]));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.disclosure_target().as_ref(),
            Some(&blocks[0]),
            "Space wraps"
        );
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.disclosure_open(&blocks[0]));
        assert!(!app.disclosure_open(&blocks[1]));
        assert!(app.disclosure_open(&Disclosure::Assignment));
        assert!(app.confirmation().is_none());
        assert!(
            app.details_focused(),
            "opening a block is not leaving the panel"
        );
        assert_eq!(
            app.selected_row().map(|row| row.id.clone()),
            Some(owner_row)
        );

        // Back on a page with one block, the keyboard is on that block rather
        // than at a position the page does not have, so the marker drawn is the
        // one Enter would open.
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.detail_page(), DetailPage::Overview);
        assert_eq!(
            app.disclosure_target().as_ref(),
            Some(&Disclosure::Assignment)
        );
    }

    #[test]
    fn a_confirmation_keeps_precedence_over_the_details_keys() {
        let (state, _) = fixture();
        let state = with_running_pane(state, "wA:p3");
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        let page = app.detail_page();
        app.begin_action(Operation::Delete, &state);

        // The dialog answers the navigation keys: Right moves the dialog's
        // cursor rather than the panel's page, and Enter activates the button
        // the cursor is on.
        assert_eq!(press(&mut app, KeyCode::Right), None);
        assert_eq!(app.detail_page(), page, "the page did not move");
        assert!(app.confirmation().expect("open").confirm_selected);
        assert_eq!(press(&mut app, KeyCode::Left), None);
        assert!(
            !app.confirmation().expect("open").confirm_selected,
            "and back to Cancel"
        );
        assert_eq!(app.detail_page(), page);
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.confirmation().is_none(), "Cancel is the default");
        assert!(app.details_focused(), "the panel kept the keyboard");
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

    /// A running task carrying the long text a disclosure holds.
    fn task_with_command(id: &str, command: &str) -> Task {
        Task {
            id: id.into(),
            state: crate::tasks::TaskState::Running,
            command: Some(command.into()),
            cwd: None,
            pid: None,
            started_at: None,
            last_output_at: None,
            output_bytes: None,
            exit_code: None,
        }
    }

    /// Connects `session` and publishes `tasks` as its complete list, as the
    /// listener would report it.
    fn publish_tasks(app: &mut App, session: &str, tasks: Vec<Task>) {
        app.apply_task_event(TaskEvent::Connected {
            session: session.into(),
            pane: None,
        });
        app.apply_task_event(TaskEvent::Tasks {
            session: session.into(),
            tasks,
        });
    }

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
    /// The fixture with an agent reported mid-task: work its row's end would lose.
    fn with_busy_agent(mut state: ObservationState, pane_id: &str) -> ObservationState {
        let mut observation = state.inventory().expect("current inventory").clone();
        for agent in &mut observation.agents {
            if agent.location.pane_id == pane_id {
                agent.status = Some(crate::model::RuntimeStatus::Working);
            }
        }
        state.apply_success(observation);
        state
    }

    /// The fixture with a command in a pane's foreground: the case that loses
    /// work, so the case that is asked about before it ends.
    fn with_running_pane(mut state: ObservationState, pane_id: &str) -> ObservationState {
        state.apply_evidence(
            pane_id,
            crate::model::ForegroundEvidence::NonShell {
                pid: 4242,
                name: Some("nvim".to_string()),
                command: Some("nvim src/main.rs".to_string()),
                local: crate::model::LocalFacts::default(),
            },
        );
        state
    }

    fn fixture() -> (ObservationState, App) {
        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        let mut app = App::new();
        app.refresh(&state);
        (state, app)
    }

    /// Cycles to the view that lists every pane, for tests about pane rows.
    #[test]
    fn a_new_workspace_needs_no_selected_container() {
        // A plain pane row records no container, so `a` has nothing to act on, but
        // `A` still asks for a workspace.
        let (_state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        assert!(app.selected_container().is_none());
        assert_eq!(
            press(&mut app, KeyCode::Char('A')),
            Some(Action::NewWorkspace)
        );
        match app.new_workspace(None) {
            Confirmed::Container(crate::topology::Control::Create(create)) => {
                assert_eq!(
                    create.kind,
                    ContainerCreateKind::Workspace { directory: None }
                );
                assert!(create.recipe.is_none());
            }
            _ => panic!("a workspace creation"),
        }
    }

    fn show_all_panes(app: &mut App) {
        while app.pane_view() != PaneView::All {
            app.cycle_panes();
        }
    }

    #[test]
    fn pane_view_starts_at_running_and_cycles_to_all_then_agents() {
        let mut app = app_with_fixture();
        // The fixture reports no foreground process, so the default running view
        // lists the agents but no ordinary pane.
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
    fn running_view_lists_only_panes_with_a_command_in_the_foreground() {
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
        let mut app = App::new();
        app.refresh(&state);
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

    /// Unwraps a confirmed direct close, panicking on a managed one.
    fn direct(confirmed: Confirmed) -> CloseRequest {
        match confirmed {
            Confirmed::Direct(request) => request,
            Confirmed::Subject { .. } | Confirmed::Container(_) => {
                panic!("expected a direct close")
            }
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
    fn a_workspace_sidebar_sends_its_modals_to_the_popup_with_the_exact_row() {
        let (state, _) = fixture();
        let row = RowId::Agent("wA:p1".into());
        let mut sidebar = App::sidebar();
        sidebar.refresh(&state);
        sidebar.show_everything();
        select_row(&mut sidebar, &row);

        // Without a host that can float a popup, the sidebar handles the key itself.
        assert!(matches!(
            press(&mut sidebar, KeyCode::Char('D')),
            Some(Action::BeginAction(Operation::Delete))
        ));

        sidebar.set_modal_host(true);
        for ch in ['r', 'D', '?'] {
            assert_eq!(
                press(&mut sidebar, KeyCode::Char(ch)),
                Some(Action::Modal {
                    key: KeyCode::Char(ch),
                    row: Some(row.clone())
                }),
                "{ch} opens in the popup, over the row selected here"
            );
        }
        assert_eq!(
            press(&mut sidebar, KeyCode::Char('A')),
            Some(Action::Modal {
                key: KeyCode::Char('A'),
                row: None
            }),
            "a new workspace belongs to no row"
        );
        assert!(sidebar.confirmation().is_none() && !sidebar.picker_open());
    }

    #[test]
    fn the_popup_selects_by_identity_in_any_view_and_refuses_a_row_that_is_gone() {
        let (state, _) = fixture();
        let mut popup = App::modal();
        popup.refresh(&state);
        popup.show_everything();
        let row = RowId::Agent("wA:p1".into());
        assert!(popup.select_exact(&row));
        assert_eq!(popup.selected_id(), Some(row));

        let gone = RowId::Agent("wZ:gone".into());
        assert!(!popup.select_exact(&gone));
        assert_ne!(
            popup.selected_id(),
            Some(gone),
            "no neighbour stands in for it"
        );
        assert!(matches!(
            press(&mut popup, KeyCode::Char('D')),
            Some(Action::BeginAction(Operation::Delete))
        ));
        assert_eq!(
            serde_json::from_str::<RowId>(
                &serde_json::to_string(&RowId::Agent("x".into())).unwrap()
            )
            .unwrap(),
            RowId::Agent("x".into()),
            "the identity survives the argument it travels in"
        );
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
        state.apply_success(crate::model::testing::fleet());
        // The owner stops being reported while its pane stays reported.
        let mut without_owner = crate::model::testing::fleet();
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
                label: Some("owner".into()),
                status: Some(RuntimeStatus::Working),
                session: None,
                lineage: None,
                facts: crate::model::HerdsmanFacts {
                    background_tasks: vec!["bg-1:review".into()],
                    ..Default::default()
                },
                registry: None,
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        // The subject is task behavior, and the default running view lists them.
        assert!(app.shows_tasks());

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
        app.set_focus_message(Some("tmux refused to focus: pane_not_found".into()));
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
    fn an_idle_pane_ends_without_a_question_and_a_running_one_asks() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('D')),
            Some(Action::BeginAction(Operation::Delete))
        );
        // An idle shell holds nothing, so the standard case is not a question:
        // the same key hands the close straight back for dispatch.
        let dismissed = app.begin_action(Operation::Delete, &state);
        assert!(app.confirmation().is_none(), "nothing to ask about");
        assert!(
            matches!(dismissed, Some(Confirmed::Direct(_))),
            "{dismissed:?}"
        );

        // A pane with a command in the foreground is asked about, and the
        // question starts on Cancel.
        let mut state = state;
        state.apply_evidence(
            "wA:p3",
            crate::model::ForegroundEvidence::NonShell {
                pid: 4242,
                name: Some("nvim".to_string()),
                command: Some("nvim src/main.rs".to_string()),
                local: crate::model::LocalFacts::default(),
            },
        );
        app.begin_action(Operation::Delete, &state);
        let confirmation = app.confirmation().expect("a confirmation");
        assert_eq!(confirmation.target, CloseTarget::Pane("wA:p3".into()));
        assert!(!confirmation.confirm_selected, "Cancel is the default");
        assert!(
            confirmation
                .losses
                .iter()
                .any(|line| line == "nvim is running"),
            "{:?}",
            confirmation.losses
        );

        // Enter activates the selected button, so the default Cancel cancels.
        assert_eq!(press(&mut app, KeyCode::Enter), None);
        assert!(app.confirmation().is_none());

        // Move to Confirm and Enter hands the frozen target back for
        // revalidation rather than closing here.
        app.begin_action(Operation::Delete, &state);
        assert_eq!(press(&mut app, KeyCode::Tab), None);
        assert!(app.confirmation().expect("open").confirm_selected);
        assert_eq!(press(&mut app, KeyCode::Enter), Some(Action::ConfirmAction));
    }

    /// A container as the daemon reads one out: a pane Radar created, with the
    /// command its slot runs held privately.
    fn pane_container(id: &str, handle: &str, removed: Option<&str>) -> PublicContainer {
        PublicContainer {
            container_id: id.to_string(),
            kind: ContainerKind::Pane,
            parent: None,
            label: Some("work".to_string()),
            created_by: None,
            recipe: true,
            provenance: ContainerProvenance::Created,
            binding: Some(ContainerBinding {
                backend: "fake".into(),
                incarnation: None,
                handle: handle.to_string(),
            }),
            created_at: "2026-01-01T00:00:00.000Z".into(),
            observed_at: None,
            unresolved: None,
            removed_at: removed.map(str::to_string),
        }
    }

    /// The fixture with one container Radar created for a pane, every pane listed,
    /// and that pane's row selected.
    fn fixture_with_container(removed: Option<&str>) -> (ObservationState, App) {
        let mut observation = crate::model::testing::fleet();
        observation.containers = vec![pane_container("c1", "wA:p3", removed)];
        if let Some(workspace) = observation.workspaces.first_mut() {
            workspace.session_path = Some("/home/saurabhj/Projects/dev/agent-radar".into());
        }
        if let Some(pane) = observation
            .panes
            .iter_mut()
            .find(|pane| pane.location.pane_id == "wA:p3")
        {
            pane.current_path = Some("/home/saurabhj/Projects/dev/agent-radar".into());
        }
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        (state, app)
    }

    #[test]
    fn a_container_row_offers_a_container_control_and_a_plain_pane_does_not() {
        // A creation is always a question: it makes a place rather than ending one.
        let (state, mut app) = fixture_with_container(None);
        app.begin_action(Operation::CreateContainer, &state);
        let confirmation = app.confirmation().expect("a container confirmation");
        assert_eq!(confirmation.container_id.as_deref(), Some("c1"));
        assert_eq!(confirmation.target, CloseTarget::Pane("wA:p3".into()));
        assert!(!confirmation.confirm_selected, "Cancel is the default");
        app.cancel_confirmation();

        // A pane Radar records nothing for has no container to address, and none is
        // invented from the handle the backend knows the pane by.
        let (plain, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        // The same key on a pane Radar records nothing for ends that pane: there
        // is no container to address and none is invented from its handle. An idle
        // pane is not a question, so the close is handed straight back.
        match app.begin_action(Operation::Delete, &plain) {
            Some(Confirmed::Direct(request)) => {
                assert_eq!(request.target, CloseTarget::Pane("wA:p3".into()))
            }
            other => panic!("a pane close, not a container control: {other:?}"),
        }

        // A reload of a pane with no recipe refuses for the same reason: there is
        // nothing recorded to run again.
        let (no_recipe, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::ReloadContainer, &no_recipe);
        assert!(app.confirmation().is_none());
    }

    /// Deleting a container something is running in is a question, and the
    /// question names what would die rather than the record that holds it.
    #[test]
    fn deleting_a_running_container_asks_first_and_names_what_is_running() {
        let (mut state, mut app) = fixture_with_container(None);
        state.apply_evidence(
            "wA:p3",
            crate::model::ForegroundEvidence::NonShell {
                pid: 4242,
                name: Some("nvim".to_string()),
                command: Some("nvim src/main.rs".to_string()),
                local: crate::model::LocalFacts::default(),
            },
        );
        assert!(app.begin_action(Operation::Delete, &state).is_none());
        let confirmation = app.confirmation().expect("a confirmation");
        assert!(
            confirmation
                .losses
                .iter()
                .any(|line| line == "nvim is running"),
            "the losses name the command: {:?}",
            confirmation.losses
        );
        assert!(
            confirmation
                .losses
                .iter()
                .all(|line| !line.contains("c1") && !line.contains("wA:p3")),
            "no record id or pane handle is shown: {:?}",
            confirmation.losses
        );
    }

    /// A container whose handle no backend reports has no place to freeze, so its
    /// deletion is the record going — and the TUI must not refuse it before the
    /// daemon can decide.
    #[test]
    fn an_unresolved_container_can_be_forgotten_but_not_otherwise_acted_on() {
        let mut observation = crate::model::testing::fleet();
        let mut container = pane_container("c1", "wA:p3", None);
        container.unresolved = Some("no backend reports this container's handle".to_string());
        observation.containers = vec![container];
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));

        // A reload needs a place to run in, so it still refuses.
        assert!(
            app.begin_action(Operation::ReloadContainer, &state)
                .is_none()
        );
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("is unresolved")),
            "{:?}",
            app.focus_message()
        );

        // A deletion freezes nothing and asks nothing: it is the record's removal.
        match app.begin_action(Operation::Delete, &state) {
            Some(Confirmed::Container(crate::topology::Control::Delete(request))) => {
                assert_eq!(request.container_id, "c1");
                assert_eq!(request.frozen, serde_json::Value::Null);
            }
            other => panic!("a records-only deletion: {other:?}"),
        }
        assert!(app.confirmation().is_none(), "nothing to ask");
    }

    /// Nothing running is the standard case: it loses no work, so it is not asked
    /// about.
    #[test]
    fn deleting_an_idle_container_asks_nothing() {
        let (state, mut app) = fixture_with_container(None);
        let confirmed = app.begin_action(Operation::Delete, &state);
        assert!(app.confirmation().is_none(), "no question to ask");
        assert!(matches!(
            confirmed,
            Some(Confirmed::Container(crate::topology::Control::Delete(_)))
        ));
    }

    fn launch_test_shell(name: &str) -> (std::path::PathBuf, std::ffi::OsString) {
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(format!("radar-app-launch-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let shell = root.join("bash");
        std::fs::write(&shell, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&root]).unwrap();
        (root, path)
    }

    #[test]
    fn l_launches_the_configured_agent_immediately_in_the_selected_pane_directory() {
        let (state, mut app) = fixture_with_container(None);
        let (root, path) = launch_test_shell("pane");
        let shell = root.join("bash").to_string_lossy().into_owned();
        assert_eq!(
            press(&mut app, KeyCode::Char('L')),
            Some(Action::LaunchConfigured)
        );
        assert!(
            app.confirmation().is_none(),
            "L has no prompt or confirmation"
        );

        let confirmed = app
            .launch_configured(
                &state,
                &["pi".into(), "an argument with spaces".into()],
                Some("bash"),
                Some(path.as_os_str()),
            )
            .unwrap_or_else(|| panic!("launch refused: {:?}", app.focus_message()));
        match confirmed {
            Confirmed::Container(crate::topology::Control::Create(request)) => {
                // A launch from a pane opens its own window in that pane's
                // workspace, rather than a pane beside what already runs there.
                assert_eq!(
                    request.kind,
                    ContainerCreateKind::Tab {
                        workspace_id: "wA".to_string()
                    }
                );
                let recipe = request.recipe.expect("launch uses a private recipe");
                assert_eq!(
                    recipe.cwd.as_deref(),
                    Some("/home/saurabhj/Projects/dev/agent-radar")
                );
                assert_eq!(recipe.executable, shell);
                assert_eq!(recipe.argv[0], "-ilc");
                assert!(recipe.argv[1].contains("exec \"$@\""));
                assert_eq!(recipe.argv[3], "pi");
                assert_eq!(recipe.argv[4], "an argument with spaces");
                assert!(recipe.env.is_empty(), "TUI environment is not copied");
            }
            other => panic!("a launch: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn l_on_a_workspace_uses_its_recorded_directory_and_creates_a_tab() {
        let (mut state, mut app) = fixture_with_container(None);
        let (root, path) = launch_test_shell("workspace");
        let shell = root.join("bash").to_string_lossy().into_owned();
        let mut observation = state.inventory().expect("inventory").clone();
        let mut workspace = pane_container("c2", "wA", None);
        workspace.kind = ContainerKind::Workspace;
        workspace.recipe = false;
        observation.containers.push(workspace);
        state.apply_success(observation);
        app.refresh(&state);
        select_row(&mut app, &RowId::Workspace("wA".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('L')),
            Some(Action::LaunchConfigured)
        );
        let confirmed = app
            .launch_configured(&state, &["pi".into()], Some("bash"), Some(path.as_os_str()))
            .expect("workspace launch");
        match confirmed {
            Confirmed::Container(crate::topology::Control::Create(request)) => {
                assert!(matches!(request.kind, ContainerCreateKind::Tab { .. }));
                let recipe = request.recipe.unwrap();
                assert_eq!(recipe.executable, shell);
                assert_eq!(
                    recipe.cwd.as_deref(),
                    Some("/home/saurabhj/Projects/dev/agent-radar")
                );
            }
            other => panic!("workspace launch: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_empty_configured_launch_command_refuses_before_creating_a_pane() {
        let (state, mut app) = fixture_with_container(None);
        assert!(
            app.launch_configured(
                &state,
                &[],
                Some("/bin/bash"),
                Some(std::ffi::OsStr::new("/bin:/usr/bin")),
            )
            .is_none()
        );
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("configured launch command is empty"))
        );
    }

    #[test]
    fn presence_status_action_does_not_dispatch_enrollment() {
        // The key reports the daemon's current standing; the presence reporter owns
        // admission and session changes.
        let (state, mut app) = fixture();
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('E')),
            Some(Action::BeginAction(Operation::PresenceStatus))
        );
        app.begin_action(Operation::PresenceStatus, &state);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("only a Pi session can be managed")),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn a_container_control_freezes_its_place_and_occupant_at_confirm() {
        let (state, mut app) = fixture_with_container(None);
        // Nothing runs in this container, so the deletion is not a question: the
        // frozen control comes straight back.
        let confirmed = app.begin_action(Operation::Delete, &state);
        assert!(app.confirmation().is_none(), "nothing to ask");
        match confirmed.expect("the container still holds") {
            Confirmed::Container(crate::topology::Control::Delete(request)) => {
                assert_eq!(request.container_id, "c1");
                assert_eq!(
                    request.frozen["target"],
                    serde_json::json!({"pane": "wA:p3"}),
                    "{}",
                    request.frozen
                );
                assert_eq!(
                    request.frozen["identity"]["pane"]["pane_id"],
                    serde_json::json!("wA:p3"),
                    "{}",
                    request.frozen
                );
                // The caller's own id is the operation's key: a lost answer is read
                // back under it rather than sent again.
                assert!(
                    crate::model::SessionUuid::parse(&request.request_id).is_some(),
                    "{}",
                    request.request_id
                );
            }
            other => panic!("a container deletion: {other:?}"),
        }

        // A reload needs the recipe the record says is held, and hands back a
        // control naming the container rather than the place.
        app.begin_action(Operation::ReloadContainer, &state);
        match app.confirm(&state).expect("the container still holds") {
            Confirmed::Container(crate::topology::Control::Reload(request)) => {
                assert_eq!(request.container_id, "c1")
            }
            other => panic!("a container reload: {other:?}"),
        }
    }

    #[test]
    fn a_container_changed_before_confirm_is_not_acted_on() {
        let (mut state, mut app) = fixture_with_container(None);
        app.begin_action(Operation::ReloadContainer, &state);
        assert!(app.confirmation().is_some());

        // The container is removed while the dialog is up: the place may now belong
        // to whatever the backend put there, so nothing is sent.
        let mut observation = crate::model::testing::fleet();
        observation.containers = vec![pane_container(
            "c1",
            "wA:p3",
            Some("2026-01-01T00:00:02.000Z"),
        )];
        state.apply_success(observation);
        assert!(app.confirm(&state).is_none(), "nothing was sent");
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("is gone")),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn confirm_revalidates_against_the_latest_observation() {
        let (state, _) = fixture();
        let state = with_running_pane(state, "wA:p3");
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::Delete, &state);
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
        app.begin_action(Operation::Delete, &state);
        let mut stale = ObservationState::new();
        stale.apply_success(crate::model::testing::fleet());
        stale.apply_failure("tmux exited with status 1");
        assert_eq!(app.confirm(&stale), None);
        assert!(app.focus_message().expect("a reason").contains("stale"));
    }

    /// A deletion the daemon held because an agent inside the place did not exit
    /// is the operator's to repeat: the next `D` asks for the place to close.
    #[test]
    fn a_deletion_held_by_a_living_agent_is_repeated_with_force() {
        let mut observation = crate::model::testing::fleet();
        let mut container = pane_container("c1", "wA:p3", None);
        container.unresolved = Some("no backend reports this container's handle".to_string());
        observation.containers = vec![container];
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));

        let answer = crate::control_plane::client::ContainerAnswer {
            request: serde_json::json!({
                "request_id": "held",
                "outcome": "refused",
                "category": crate::control_plane::store::Category::AGENTS_ALIVE,
                "message": "1 agent(s) inside pane wA:p3 did not exit",
            }),
            container: None,
            removed: Vec::new(),
        };
        app.apply_container_outcome(crate::topology::Finished {
            verb: "delete",
            container_id: Some("c1".to_string()),
            request_id: "held".to_string(),
            result: Ok(answer),
        });

        match app.begin_action(Operation::Delete, &state) {
            Some(Confirmed::Container(crate::topology::Control::Delete(request))) => {
                assert!(request.force, "the repeat closes anyway")
            }
            other => panic!("a delete control: {other:?}"),
        }
    }

    #[test]
    fn moving_the_selection_does_not_redirect_the_frozen_target() {
        // A command in the pane makes the close a question, which is what the
        // frozen target belongs to.
        let (state, _) = fixture();
        let state = with_running_pane(state, "wA:p3");
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::Delete, &state);

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
                name: Some("claude".into()),
                label: None,
                status: Some(RuntimeStatus::Working),
                session: Some(SessionIdentity::Reported {
                    source: None,
                    value: session.into(),
                }),
                lineage: None,
                facts: crate::model::HerdsmanFacts::default(),
                registry: None,
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation("s1"));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wX:p1".into()));
        app.begin_action(Operation::Delete, &state);
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
        // The pane holds a command, so the close is a question worth asking.
        let (state, _) = fixture();
        let state = with_running_pane(state, "wA:p3");
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::Delete, &state);

        // A managed worker appears on the frozen pane before Confirm.
        let mut observation = crate::model::testing::fleet();
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
            registry: None,
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
    fn the_row_standing_for_a_tab_ends_the_tab_and_a_nested_row_ends_its_pane() {
        let (state, mut app) = fixture();
        show_all_panes(&mut app);

        // The row a tab is drawn as stands for the whole tab, so ending it ends
        // every pane inside — including one whose owner holds a subject there.
        let root = app
            .visible_rows()
            .into_iter()
            .find(|row| {
                matches!(
                    &row.node.row.kind,
                    RowKind::Pane(pane) if pane.represents_tab
                )
            })
            .map(|row| row.id.clone());
        let Some(root) = root else {
            // The fixture draws its tab as an agent row instead, mid-task.
            let state = with_busy_agent(state, "wA:p1");
            app.refresh(&state);
            show_all_panes(&mut app);
            select_row(&mut app, &RowId::Agent("wA:p1".into()));
            app.begin_action(Operation::Delete, &state);
            let confirmation = app.confirmation().expect("a confirmation");
            // The fixture gives this row no owner-held subject, so it is its own
            // pane rather than the tab.
            assert_eq!(confirmation.target, CloseTarget::Pane("wA:p1".into()));
            return;
        };
        // The tab's own pane holds the work, so the tab is asked about.
        let pane_of_root = match &root {
            RowId::Pane(pane) => Some(pane.clone()),
            RowId::Agent(pane) | RowId::Registered(pane) => Some(pane.clone()),
            _ => None,
        }
        .expect("a pane row");
        let state = with_running_pane(state, &pane_of_root);
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &root);
        app.begin_action(Operation::Delete, &state);
        let confirmation = app.confirmation().expect("a confirmation");
        assert!(
            matches!(confirmation.target, CloseTarget::Tab(_)),
            "{:?}",
            confirmation.target
        );
        assert!(confirmation.subject.is_none(), "a tab is not one subject");
        assert!(
            confirmation
                .losses
                .iter()
                .any(|line| line == "nvim is running"),
            "{:?}",
            confirmation.losses
        );
    }

    #[test]
    fn a_workspace_row_does_not_redirect_a_close_to_its_panes() {
        let (state, mut app) = fixture();
        select_row(&mut app, &RowId::Workspace("wA".into()));
        app.begin_action(Operation::Delete, &state);
        assert!(app.confirmation().is_none());
        assert!(app.focus_message().expect("a reason").contains("workspace"));
    }

    #[test]
    fn a_task_row_does_not_redirect_a_close_to_its_owner() {
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
                name: Some("claude".into()),
                label: Some("worker".into()),
                status: Some(RuntimeStatus::Working),
                session: None,
                lineage: None,
                facts: crate::model::HerdsmanFacts {
                    background_tasks: vec!["bg-1:review".into()],
                    ..Default::default()
                },
                registry: None,
            }],
        };
        let mut state = ObservationState::new();
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        assert!(app.shows_tasks(), "the default running view lists tasks");
        let task = RowId::Task(crate::tree::TaskId {
            owner: "wX:p1".into(),
            session: None,
            id: "bg-1".into(),
        });
        select_row(&mut app, &task);
        app.begin_action(Operation::Delete, &state);
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .expect("a reason")
                .contains("background task")
        );
    }

    #[test]
    fn a_retained_managed_association_is_ended_like_any_other_row() {
        let mut state = ObservationState::new();
        state.apply_success(crate::model::testing::fleet());
        let mut without = crate::model::testing::fleet();
        // The managed owner stops being reported while its pane stays.
        without
            .agents
            .retain(|agent| agent.location.pane_id != "wA:p1");
        state.apply_success(without);
        let mut app = App::new();
        app.refresh(&state);

        select_row(&mut app, &RowId::Agent("wA:p1".into()));
        // The pane is kept alive by a retained association, so Radar cannot verify
        // its occupant and has no owner to ask: the direct close is refused with
        // that reason rather than acted on behind an owner's back.
        assert!(app.begin_action(Operation::Delete, &state).is_none());
        assert!(app.confirmation().is_none());
        assert!(
            app.focus_message()
                .is_some_and(|message| message.contains("managed by its owner")),
            "{:?}",
            app.focus_message()
        );
    }

    #[test]
    fn lifecycle_keys_typed_in_filter_entry_are_text() {
        let (_, mut app) = fixture();
        press(&mut app, KeyCode::Char('/'));
        assert_eq!(press(&mut app, KeyCode::Char('D')), None);
        assert_eq!(press(&mut app, KeyCode::Char('D')), None);
        assert_eq!(press(&mut app, KeyCode::Char('r')), None);
        assert!(app.is_filter_editing());
        assert_eq!(app.filter_query(), "DDr");
        assert!(app.confirmation().is_none());
    }

    #[test]
    fn mouse_only_the_drawn_buttons_answer_a_confirmation() {
        let (state, _) = fixture();
        let state = with_running_pane(state, "wA:p3");
        let mut app = App::new();
        app.refresh(&state);
        show_all_panes(&mut app);
        select_row(&mut app, &RowId::Pane("wA:p3".into()));
        app.begin_action(Operation::Delete, &state);
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
        app.begin_action(Operation::Delete, &state);
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
                    }),
                }),
            }],
        });
        state
    }

    #[test]
    fn a_managed_worker_opens_a_daemon_subject_confirmation() {
        let mut state = managed_state(managed_facts(), RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut observation = state.inventory().expect("current inventory").clone();
        observation.agents[0]
            .registry
            .as_mut()
            .and_then(|view| view.managed.as_mut())
            .expect("managed subject")
            .child_close = Some(crate::registry_observation::ChildCloseTarget {
            spawn_request_id: "spawn-edge".into(),
            source: "worker".into(),
            incarnation: "11111111-1111-4111-8111-111111111111".into(),
        });
        state.apply_success(observation);
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Delete, &state);
        let confirmation = app.confirmation().expect("a managed close confirmation");
        assert!(!confirmation.confirm_selected, "Cancel is the default");
        assert_eq!(
            confirmation
                .subject
                .as_ref()
                .map(|subject| subject.subject_id.as_str()),
            Some("subject-managed")
        );
        match app.confirm(&state).expect("unchanged") {
            Confirmed::Subject {
                operation: Operation::Delete,
                subject_id,
                child_close,
                close_request_id,
                ..
            } => {
                assert_eq!(subject_id, "subject-managed");
                assert_eq!(
                    child_close,
                    Some(crate::registry_observation::ChildCloseTarget {
                        spawn_request_id: "spawn-edge".into(),
                        source: "worker".into(),
                        incarnation: "11111111-1111-4111-8111-111111111111".into(),
                    })
                );
                assert!(close_request_id.is_some(), "pane cleanup has its own id");
            }
            Confirmed::Direct(_) | Confirmed::Container(_) | Confirmed::Subject { .. } => {
                panic!("managed control must target its daemon subject")
            }
        }
    }

    #[test]
    fn restart_is_offered_for_a_bound_daemon_subject_with_a_session() {
        let state = managed_state(managed_facts(), RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        assert_eq!(
            press(&mut app, KeyCode::Char('r')),
            Some(Action::BeginAction(Operation::Restart))
        );
        app.begin_action(Operation::Restart, &state);
        let subject = app
            .confirmation()
            .expect("a restart confirmation")
            .subject
            .as_ref()
            .expect("daemon subject");
        assert_eq!(subject.subject_id, "subject-managed");

        // The daemon, not owner projection or locally inferred status, decides
        // whether the exact process can be restarted.
        let mut busy = managed_facts();
        busy.state = Some(crate::model::SemanticState::Working);
        let busy = managed_state(busy, RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&busy);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &busy);
        assert!(app.confirmation().is_some());

        // An enrolled lead's session does not require a parent owner.
        let orphan = managed_state(managed_facts(), RuntimeStatus::Idle, None);
        let mut app = App::new();
        app.refresh(&orphan);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &orphan);
        assert!(app.confirmation().is_some());
        app.handle_key(key(KeyCode::Esc));

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
    fn confirm_refuses_a_managed_target_when_process_generation_changes() {
        let mut state = managed_state(managed_facts(), RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Delete, &state);
        assert!(app.confirmation().is_some());

        let mut replacement = state.inventory().expect("current inventory").clone();
        replacement.agents[0]
            .registry
            .as_mut()
            .and_then(|view| view.managed.as_mut())
            .expect("managed subject")
            .process
            .start_ticks += 1;
        state.apply_success(replacement);
        assert!(app.confirm(&state).is_none());
        assert!(
            app.focus_message()
                .expect("refusal")
                .contains("binding changed")
        );
    }

    #[test]
    fn confirm_refuses_a_managed_target_whose_run_changed() {
        let state = managed_state(managed_facts(), RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Delete, &state);

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
    fn a_busy_worker_is_still_subject_targeted_for_daemon_refusal() {
        let state = managed_state(managed_facts(), RuntimeStatus::Working, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&state);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &state);
        assert!(app.confirmation().is_some());

        let mut busy = managed_facts();
        busy.state = Some(crate::model::SemanticState::Working);
        let busy = managed_state(busy, RuntimeStatus::Working, Some(MANAGED_OWNER));
        assert!(matches!(
            app.confirm(&busy),
            Some(Confirmed::Subject { .. })
        ));
        assert!(app.focus_message().is_none());
    }

    #[test]
    fn unknown_runtime_state_does_not_change_the_daemon_subject_target() {
        // Process eligibility belongs to the daemon's exact subject contract,
        // not to a local reconstruction of owner state.
        let unknown = managed_state(managed_facts(), RuntimeStatus::Unknown, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&unknown);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &unknown);
        assert!(app.confirmation().is_some());

        // The same subject still targets the same daemon record at confirmation.
        let idle = managed_state(managed_facts(), RuntimeStatus::Idle, Some(MANAGED_OWNER));
        let mut app = App::new();
        app.refresh(&idle);
        select_row(&mut app, &RowId::Agent("wM:p1".into()));
        app.begin_action(Operation::Restart, &idle);
        assert!(app.confirmation().is_some());

        assert!(matches!(
            app.confirm(&unknown),
            Some(Confirmed::Subject { .. })
        ));
    }

    #[test]
    fn the_outcomes_panel_is_modal_and_shows_what_was_reported() {
        let (state, mut app) = fixture_with_container(None);
        app.record_notice("bad".into(), "delete refused: something long".into(), true);

        assert_eq!(press(&mut app, KeyCode::Char('!')), None);
        assert!(app.notices_open());

        // While it is open nothing reaches the tree: `d` would toggle the details
        // panel and `j` would move the selection, and neither happens.
        let selected = app.selected_index();
        let details = app.shows_details();
        assert_eq!(press(&mut app, KeyCode::Char('d')), None);
        assert_eq!(press(&mut app, KeyCode::Char('j')), None);
        assert_eq!(app.shows_details(), details, "the tree kept its keys");
        assert_eq!(app.selected_index(), selected);
        assert_eq!(app.notice_scroll(), 1, "the panel took the key");

        // Escape closes it and leaves the outcome to be read again.
        assert_eq!(press(&mut app, KeyCode::Esc), None);
        assert!(!app.notices_open());
        assert_eq!(app.lifecycle_notices().len(), 1);

        // `c` in the panel clears the outcomes and closes it.
        assert_eq!(press(&mut app, KeyCode::Char('!')), None);
        assert_eq!(press(&mut app, KeyCode::Char('c')), None);
        assert!(!app.notices_open());
        assert!(app.lifecycle_notices().is_empty());
        let _ = state;
    }

    #[test]
    fn a_completed_outcome_ages_out_and_a_failure_stays_until_dismissed() {
        let mut app = App::new();
        app.record_notice("ok".into(), "create completed".into(), false);
        app.record_notice("bad".into(), "delete refused".into(), true);
        assert!(!app.expire_focus_message(Instant::now()));
        assert_eq!(app.lifecycle_notices().len(), 2);
        assert!(app.expire_focus_message(Instant::now() + NOTICE_TTL));
        let left: Vec<_> = app
            .lifecycle_notices()
            .iter()
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(left, ["bad"]);
        assert!(!app.expire_focus_message(Instant::now() + NOTICE_TTL * 10));
        app.dismiss_notices();
        assert!(app.lifecycle_notices().is_empty());
    }

    #[test]
    fn the_palette_filters_commands_and_runs_the_chosen_one_by_its_key() {
        let (_, mut app) = fixture();
        assert_eq!(press(&mut app, KeyCode::Char('?')), None);
        assert!(app.palette().is_some(), "? opens the palette");

        // Keys reach the query, not the tree: `e` would otherwise show finished
        // sessions, and `q` would be a quit the main loop must not see.
        let selected = app.selected_index();
        for ch in "show or hide finished".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        assert_eq!(app.selected_index(), selected);
        let palette = app.palette().expect("open");
        assert_eq!(palette.query(), "show or hide finished");
        assert_eq!(
            palette.rows(),
            [("Show or hide finished sessions", Some("e"))],
            "only the matching command is offered"
        );

        // Enter answers with that command's key, which does what the key does.
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::Replay(KeyCode::Char('e')))
        );
        assert!(app.palette().is_none(), "choosing closes the palette");

        // The caller replays the key; here that lands on the real binding.
        press(&mut app, KeyCode::Char('e'));
        assert!(app.shows_finished());

        press(&mut app, KeyCode::Char('?'));
        assert_eq!(press(&mut app, KeyCode::Esc), None);
        assert!(app.palette().is_none(), "Escape closes it");
    }

    #[test]
    fn the_workspace_picker_answers_with_its_directory() {
        let (_, mut app) = fixture();
        let directories = vec![
            std::path::PathBuf::from("/home/one/agent-radar"),
            std::path::PathBuf::from("/home/one/notes"),
        ];
        app.open_workspace_picker(directories.clone());
        let picker = app.workspace_picker().expect("open");
        assert_eq!(picker.rows().len(), 2);
        assert!(
            picker.rows()[0].0.contains("agent-radar"),
            "the paths are what a query matches"
        );

        // A query narrows it, and Enter answers with the highlighted directory.
        for ch in "notes".chars() {
            assert_eq!(press(&mut app, KeyCode::Char(ch)), None);
        }
        assert_eq!(
            press(&mut app, KeyCode::Enter),
            Some(Action::NewWorkspaceIn(PathBuf::from("/home/one/notes")))
        );
        assert!(app.workspace_picker().is_none());

        app.open_workspace_picker(directories);
        assert_eq!(press(&mut app, KeyCode::Esc), None);
        assert!(app.workspace_picker().is_none(), "Escape cancels");
        assert!(app.focus_message().is_none());
    }

    #[test]
    fn managed_cancel_and_spawn_close_are_separate_operation_notices() {
        let (_, mut app) = fixture();
        app.apply_subject_outcome(crate::managed_control::Finished {
            operation: crate::managed_control::Operation::Cancel,
            request_id: "cancel-id".into(),
            subject_id: "subject-1".into(),
            result: Ok(crate::control_plane::client::LifecycleAnswer {
                request: serde_json::json!({"outcome":"completed"}),
            }),
            close_request_id: Some("close-id".into()),
            close_result: Some(Ok(crate::control_plane::client::LifecycleAnswer {
                request: serde_json::json!({"outcome":"refused", "message":"pane generation changed"}),
            })),
        });
        assert_eq!(app.lifecycle_notices().len(), 2);
        assert!(app.lifecycle_notices()[0].text.contains("cancel completed"));
        assert!(
            app.lifecycle_notices()[1]
                .text
                .contains("pane cleanup refused")
        );
        assert!(app.lifecycle_notices()[1].text.contains("close-id"));
        assert!(app.lifecycle_notices()[1].failed);
    }

    #[test]
    fn lifecycle_outcomes_are_kept_separately_and_dismissed() {
        let (_, mut app) = fixture();
        let id = "a0000000-0000-4000-8000-000000000001".to_string();
        let finished = |outcome: &str, message: Option<&str>| crate::managed_control::Finished {
            operation: crate::managed_control::Operation::Stop,
            request_id: id.clone(),
            subject_id: "subject-1".into(),
            result: Ok(crate::control_plane::client::LifecycleAnswer {
                request: serde_json::json!({"outcome": outcome, "message": message}),
            }),
            close_request_id: None,
            close_result: None,
        };
        app.apply_subject_outcome(finished("completed", None));
        assert_eq!(app.lifecycle_notices().len(), 1);
        assert!(app.lifecycle_notices()[0].text.contains("completed"));
        assert!(
            app.focus_message().is_none(),
            "kept apart from source/focus"
        );

        app.apply_subject_outcome(finished("refused", Some("process identity changed")));
        assert_eq!(
            app.lifecycle_notices().len(),
            1,
            "same request refines its notice"
        );
        assert!(app.lifecycle_notices()[0].failed);
        assert!(
            app.lifecycle_notices()[0]
                .text
                .contains("process identity changed")
        );

        press(&mut app, KeyCode::Char('c'));
        assert!(app.lifecycle_notices().is_empty(), "dismissed");
    }
}
