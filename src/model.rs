//! Normalized fleet facts shared by the connector, the continuity reconciler
//! and (later) the tree/UI.
//!
//! These types deliberately contain no Herdr DTO shapes, metadata-token keys or
//! CLI details: raw `herdr` output is decoded inside [`crate::herdr`] and only
//! these facts cross the seam.

use std::time::Duration;

/// Connector-scoped runtime location: Herdr workspace, tab and pane identifiers.
///
/// Pane identity is a runtime location detail; it is never an agent identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
}

/// A validated canonical UUID (lowercase `8-4-4-4-12` hex).
///
/// Only explicitly supplied UUID metadata is ever turned into this type —
/// session file paths are never scanned for embedded UUIDs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionUuid(String);

impl SessionUuid {
    /// Parses a canonical UUID, normalizing hex digits to lowercase.
    ///
    /// Returns `None` for anything that is not UUID-shaped, including paths.
    pub fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        for (i, &b) in bytes.iter().enumerate() {
            let hex =
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || (b'A'..=b'F').contains(&b);
            match i {
                8 | 13 | 18 | 23 => {
                    if b != b'-' {
                        return None;
                    }
                }
                _ if !hex => return None,
                _ => {}
            }
        }
        Some(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reported session identity for an agent observation.
///
/// Identity is separate from [`Location`]: the same session may move between
/// panes, and a pane may host different sessions over time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionIdentity {
    /// An explicit UUID identity, reported as `agent_session.kind == "id"`.
    Uuid(SessionUuid),
    /// A source-qualified reference (typically a session file path).
    ///
    /// Kept exactly as reported: a path-like reference is never promoted to a
    /// UUID, however UUID-like the value looks.
    Reported {
        source: Option<String>,
        value: String,
    },
}

/// Explicit ownership lineage from `pi_herdsman_session` /
/// `pi_herdsman_parent_session` UUID metadata.
///
/// Used only for ownership joins (worker → owner). Lineage is never used as
/// session identity, and absent or malformed tokens yield no lineage at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lineage {
    pub session: SessionUuid,
    pub parent: Option<SessionUuid>,
}

/// Runtime lifecycle exactly as the source reports it.
///
/// This is not a semantic control state: `Idle` says nothing about
/// availability for new work, result delivery or safe lifecycle actions.
/// Unrecognized values are preserved verbatim in [`RuntimeStatus::Other`] so a
/// source change never fails an otherwise valid inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeStatus {
    Idle,
    Working,
    Done,
    Unknown,
    Other(String),
}

impl RuntimeStatus {
    /// Decodes a reported lifecycle label, preserving unknown labels.
    pub fn from_reported(label: &str) -> Self {
        match label {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "done" => Self::Done,
            "unknown" => Self::Unknown,
            other => Self::Other(other.to_string()),
        }
    }
}

impl std::fmt::Display for RuntimeStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => f.write_str("idle"),
            Self::Working => f.write_str("working"),
            Self::Done => f.write_str("done"),
            Self::Unknown => f.write_str("unknown"),
            Self::Other(other) => f.write_str(other),
        }
    }
}

/// A Herdr workspace: the first display grouping for the fleet tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub workspace_id: String,
    pub label: Option<String>,
    pub number: Option<u32>,
}

/// A Herdr tab within a workspace; shown as location detail, not a tree level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub tab_id: String,
    pub workspace_id: String,
    pub label: Option<String>,
    pub number: Option<u32>,
}

/// A runtime pane: location plus optional display text.
///
/// `label` is the pane's declared label, `title` its stripped terminal title;
/// either may be absent, and [`Pane::display_name`] defines the fallback used
/// by presentation rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub location: Location,
    pub label: Option<String>,
    pub title: Option<String>,
}

impl Pane {
    /// Best available human-readable name for a pane row.
    pub fn display_name(&self) -> Option<&str> {
        self.label
            .as_deref()
            .or(self.title.as_deref())
            .filter(|s| !s.is_empty())
    }
}

/// Herdsman's projection of an agent's state, as an owner publishes it.
///
/// This is a control projection, not a lifecycle: `Working` says a model turn
/// or tool call is in flight, `Blocked` says the assignment waits on its owner,
/// and `Waiting` says the turn has yielded while work it depends on is
/// unresolved. Values are additive, so an unrecognised one is preserved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SemanticState {
    Idle,
    Working,
    Waiting,
    Blocked,
    Settling,
    Unknown,
    Lost,
    Other(String),
}

impl SemanticState {
    /// Decodes a published projection value, preserving unknown values.
    pub fn from_published(value: &str) -> Self {
        match value {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "waiting" => Self::Waiting,
            "blocked" => Self::Blocked,
            "settling" => Self::Settling,
            "unknown" => Self::Unknown,
            "lost" => Self::Lost,
            other => Self::Other(other.to_string()),
        }
    }

    /// The word a row shows for this state.
    pub fn word(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Settling => "settling",
            Self::Unknown => "unknown",
            Self::Lost => "lost",
            Self::Other(other) => other,
        }
    }
}

/// The state a row presents: Herdsman's projection when it is published,
/// otherwise the runtime's own reported lifecycle.
///
/// Kept apart from [`RuntimeStatus`] so the details panel can say which source
/// a state came from, while presentation has one vocabulary to draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Waiting,
    Blocked,
    Settling,
    Done,
    Unknown,
    Lost,
    Other(String),
}

impl AgentState {
    /// The word a row shows for this state.
    pub fn word(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Settling => "settling",
            Self::Done => "done",
            Self::Unknown => "unknown",
            Self::Lost => "lost",
            Self::Other(other) => other,
        }
    }

    /// This state as Herdsman publishes it, for a details line that names the
    /// source. `None` for a state only the runtime reports.
    pub fn as_semantic(&self) -> Option<SemanticState> {
        Some(match self {
            Self::Idle => SemanticState::Idle,
            Self::Working => SemanticState::Working,
            Self::Waiting => SemanticState::Waiting,
            Self::Blocked => SemanticState::Blocked,
            Self::Settling => SemanticState::Settling,
            Self::Unknown => SemanticState::Unknown,
            Self::Lost => SemanticState::Lost,
            Self::Done | Self::Other(_) => return None,
        })
    }
}

impl From<&SemanticState> for AgentState {
    fn from(state: &SemanticState) -> Self {
        match state {
            SemanticState::Idle => Self::Idle,
            SemanticState::Working => Self::Working,
            SemanticState::Waiting => Self::Waiting,
            SemanticState::Blocked => Self::Blocked,
            SemanticState::Settling => Self::Settling,
            SemanticState::Unknown => Self::Unknown,
            SemanticState::Lost => Self::Lost,
            SemanticState::Other(other) => Self::Other(other.clone()),
        }
    }
}

impl From<&RuntimeStatus> for AgentState {
    fn from(status: &RuntimeStatus) -> Self {
        match status {
            RuntimeStatus::Idle => Self::Idle,
            RuntimeStatus::Working => Self::Working,
            RuntimeStatus::Done => Self::Done,
            RuntimeStatus::Unknown => Self::Unknown,
            RuntimeStatus::Other(other) => Self::Other(other.clone()),
        }
    }
}

/// What Herdsman publishes about an agent through Herdr's pane metadata.
///
/// Every field is `None` until the source publishes it: the contract is
/// additive, so an absent key means unavailable rather than a default.
/// [`Self::session_name`] is the source's human name and is deliberately not an
/// identity — identity lives in [`AgentObservation::session`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HerdsmanFacts {
    /// The role the agent runs as: a lead role, or a worker's agent definition.
    pub role: Option<String>,
    /// A lead's published session name.
    pub name: Option<String>,
    /// The agent definition a pane reports as its display agent.
    pub definition: Option<String>,
    /// A managed worker's runtime label: the name its owner and its siblings
    /// use for it, and the only faithful source for a worker's name. Herdr's
    /// own agent name truncates it and its title welds it to the task.
    pub label: Option<String>,
    /// A managed worker's run identity.
    pub run: Option<String>,
    /// Whether this pane's tokens carried any `pi_herdsman_*` key.
    ///
    /// Presence, not readability: the owner's metadata is what makes a Pi pane
    /// managed, so a key whose value this build cannot read still counts. The
    /// decoder sets this at the adapter boundary, where the raw token map is,
    /// because the named fields below discard keys this version does not know.
    pub managed_metadata: bool,
    /// The active assignment's request identity.
    pub request: Option<String>,
    /// A lead's pending owner question.
    pub ask: Option<String>,
    /// The active assignment's display text.
    pub assignment: Option<String>,
    /// The active assignment's start, Unix milliseconds as published.
    pub assigned_started_unix_ms: Option<u64>,
    /// How long the active assignment had been running when it was observed.
    /// Measured at observation time from Herdsman's own start, because a
    /// presentation that had to interpret a timestamp would need a clock.
    pub assigned_for: Option<Duration>,
    /// The owner's projection of this agent's state, while it is published.
    pub state: Option<SemanticState>,
    /// The session's model identity as published, provider prefix included.
    pub model: Option<String>,
    pub provider: Option<String>,
    pub thinking: Option<String>,
    /// Context usage as published, percent sign included.
    pub context_usage: Option<String>,
    /// The source's human session name.
    pub session_name: Option<String>,
    /// What this pane is waiting on: `agent:<label>` for each outstanding
    /// child, and `owner` while its own question to its owner is outstanding.
    /// Outstanding work, not state: it is published while working and while
    /// stopped, and cleared when the last item resolves.
    pub awaited: Vec<String>,
    /// How many of the pane's background tasks are running, as its own
    /// extension counts them. Another publisher owns this key, and the count
    /// speaks only for the running ones: it is not the set of unresolved work.
    pub background_running: Option<u32>,
    /// The pane's unresolved background tasks as its own extension publishes
    /// them, each an `<id>:<phase>` entry. The awaited facts are the union of
    /// the owner's awaited items and these ids, because neither publisher can
    /// express the whole pane alone.
    pub background_tasks: Vec<String>,
    /// The oldest outstanding background task's start, ISO 8601 as published.
    pub background_started: Option<String>,
}

impl HerdsmanFacts {
    /// Whether the source published nothing at all about this agent.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The name to label this agent with: a worker's runtime label, a lead's
    /// published name, or nothing when the source published neither.
    pub fn display_name(&self) -> Option<&str> {
        self.label.as_deref().or(self.name.as_deref())
    }

    /// Whether anything is outstanding: a child agent, an owner answer, or a
    /// background task this pane started.
    ///
    /// The published task list is the evidence, never the running count: a pane
    /// whose tasks have all exited into `review` has nothing running and
    /// everything unresolved.
    pub fn is_awaited(&self) -> bool {
        !self.awaited.is_empty() || !self.background_tasks.is_empty()
    }

    /// The ids of the background tasks this pane still reports, in published
    /// order, each id once: the unresolved set, not a count of it.
    ///
    /// An entry is published as `<id>:<phase>` and split at its last colon, so
    /// an id that itself contains one survives. An entry with no colon is all
    /// id, and a phase word Radar does not know keeps the task it belongs to
    /// rather than dropping it.
    pub fn background_task_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = Vec::with_capacity(self.background_tasks.len());
        for entry in &self.background_tasks {
            let id = entry.rsplit_once(':').map_or(entry.as_str(), |(id, _)| id);
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    /// What it is waiting for, in words, for a details line: the awaited items
    /// as published, then the unresolved background tasks.
    pub fn awaiting(&self) -> Option<String> {
        let mut parts = self.awaited.clone();
        let unresolved = self.background_task_ids().len();
        if unresolved > 0 {
            parts.push(format!(
                "{unresolved} background task{}",
                if unresolved == 1 { "" } else { "s" }
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }

    /// Measures the active assignment's age from its published start.
    pub fn stamp_assigned_age(&mut self, now_unix_ms: u64) {
        self.assigned_for = self
            .assigned_started_unix_ms
            .map(|started| Duration::from_millis(now_unix_ms.saturating_sub(started)));
    }

    /// The model without its provider prefix, for a row.
    ///
    /// Everything up to the first `/` goes: publishers disagree on whether the
    /// field is `provider/model` and the `provider` token cannot say which, so
    /// the prefix is dropped whatever provider it names. The details panel
    /// shows the model exactly as published. A model id that itself contains a
    /// `/` and is published bare would lose its first segment on the row.
    pub fn model_name(&self) -> Option<&str> {
        let model = self.model.as_deref()?;
        match model.split_once('/') {
            Some((_, rest)) if !rest.is_empty() => Some(rest),
            _ => Some(model),
        }
    }

    /// Model and thinking level as one glance, for a dense row.
    pub fn model_and_thinking(&self) -> Option<String> {
        match (self.model_name(), self.thinking.as_deref()) {
            (Some(model), Some(thinking)) => Some(format!("{model}:{thinking}")),
            (Some(model), None) => Some(model.to_string()),
            (None, Some(thinking)) => Some(thinking.to_string()),
            (None, None) => None,
        }
    }
}

/// A normalized observation of one agent, keyed in the inventory by its pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentObservation {
    pub location: Location,
    /// Reported agent name (for example `"pi"`), if any.
    pub name: Option<String>,
    /// Reported display label (the stripped terminal title), if any.
    pub label: Option<String>,
    /// Runtime lifecycle as reported; never a semantic control state.
    pub status: Option<RuntimeStatus>,
    /// Reported session identity; `None` when the source reports none.
    pub session: Option<SessionIdentity>,
    /// Explicit ownership lineage; `None` unless valid UUID tokens were supplied.
    pub lineage: Option<Lineage>,
    /// What Herdsman publishes about this agent. Empty unless the source
    /// published metadata; never inferred from runtime facts.
    pub facts: HerdsmanFacts,
}

impl AgentObservation {
    /// The state a row presents: the activity state derived from what is on
    /// the pane.
    ///
    /// Herdsman's contract specifies this derivation and says the owner's
    /// projection is not a second authority for it. The two describe different
    /// things and are written by different writers at different cadences: the
    /// projection is what the owner believes about the assignment (`settling`,
    /// `delivered`, `lost`), while this is what the pane is doing now, so a
    /// briefly stale projection must not contradict the row. The projection
    /// still feeds the derivation's first step, and the details panel shows it
    /// as its own labelled line.
    pub fn state(&self) -> AgentState {
        self.activity_state()
    }

    /// The pane's activity state, derived as Herdsman's contract specifies:
    /// a live `lost` from the owner wins, work in flight stays in flight, an
    /// unknown or unreported native state stays unknown, a non-empty union of
    /// the owner's awaited items and the pane's background task ids is waiting,
    /// and otherwise the native state stands. The derivation is in that order
    /// and the running count takes no part in it.
    pub fn activity_state(&self) -> AgentState {
        if matches!(self.facts.state, Some(SemanticState::Lost)) {
            return AgentState::Lost;
        }
        let native = match &self.status {
            Some(status) => AgentState::from(status),
            None => AgentState::Unknown,
        };
        match native {
            // A busy pane is working, not waiting on what it also awaits.
            AgentState::Working => AgentState::Working,
            // Nothing observed: an awaited item does not make an unreadable
            // pane readable.
            AgentState::Unknown | AgentState::Other(_) => native,
            // Nothing outstanding: the native state is the state.
            _ if !self.facts.is_awaited() => native,
            // Outstanding work with nothing in flight is what waiting means.
            _ => AgentState::Waiting,
        }
    }

    /// The role a row labels the agent with: a lead's role or a worker's
    /// definition, exactly as published.
    pub fn role(&self) -> Option<&str> {
        self.facts.role.as_deref()
    }
}

/// One successful inventory: all workspaces, tabs, panes and reported agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FleetObservation {
    pub workspaces: Vec<Workspace>,
    pub tabs: Vec<Tab>,
    pub panes: Vec<Pane>,
    pub agents: Vec<AgentObservation>,
}

impl FleetObservation {
    /// The pane with this connector-scoped id, if the inventory contains it.
    ///
    /// If the source ever reports duplicate pane ids, the first record wins.
    pub fn pane(&self, pane_id: &str) -> Option<&Pane> {
        self.panes.iter().find(|p| p.location.pane_id == pane_id)
    }

    /// The agent currently reported on this pane, if any.
    ///
    /// The `agents` inventory is authoritative for agent facts; pane records
    /// may mirror agent fields but are never consulted here.
    pub fn agent_on_pane(&self, pane_id: &str) -> Option<&AgentObservation> {
        self.agents.iter().find(|a| a.location.pane_id == pane_id)
    }
}

/// Evidence about a pane's foreground, decoded inside the connector.
///
/// The reconciler treats only [`ForegroundEvidence::NonShell`] as positive
/// supersession evidence; absence or ambiguity retains the last observation.
// `NonShell` is far larger than the other variants because it is the only one
// with a process to describe. Boxing its facts would add an allocation to every
// pane's evidence for a difference no held-in-memory set notices: there is one
// of these per pane, not one per process sampled.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForegroundEvidence {
    /// The pane shell owns the foreground process group (shell PID evidence).
    Shell,
    /// A non-shell process group leader is in the foreground.
    NonShell {
        pid: i32,
        name: Option<String>,
        /// The leader's command line as the operating system reports it.
        command: Option<String>,
        /// What this machine knows about that process. Filled by the collector
        /// from the operating system, never by the runtime's own report.
        local: LocalFacts,
    },
    /// PID fields are missing or inconsistent: foreground state is unknown.
    Inconclusive,
}

/// The last path component of a path or a bare name.
fn base_name(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

/// What this machine knows about the process holding a pane's foreground,
/// beyond what the runtime reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalFacts {
    /// How long the process has been alive, measured from its own start time
    /// rather than from when Radar first saw it.
    pub running_for: Option<Duration>,
    /// What it has done to the terminal.
    pub terminal: TerminalMode,
    /// How the foreground program's executable compares with the installed one.
    pub binary: BinaryIdentity,
    /// What the process is using, from the sample taken for it. `None` when no
    /// sample was taken: this machine has no reader for that process, the
    /// process could not be read, or the platform cannot sample at all. A
    /// reading from an earlier refresh is never carried here.
    pub resources: Option<ProcessResources>,
}

/// Which process incarnation a sample belongs to.
///
/// The kernel reuses process ids, so a pid does not name a process: one
/// incarnation is the boot it started in, its pid, and when it started. Those
/// are what make two readings comparable — counters may only be subtracted
/// from an earlier reading of the same identity, and a difference taken across
/// two of them measures neither process.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProcessIdentity {
    /// The boot this process started in, as the kernel reports it.
    pub boot_id: String,
    /// The process id, unique within one boot and one incarnation of it.
    pub pid: i32,
    /// Start time since boot, in clock ticks (`starttime` in
    /// `/proc/<pid>/stat`): the kernel's stamp for this incarnation.
    pub start_ticks: u64,
}

/// What one process is using, as the collector sampled it.
///
/// These are the resources of that process and of nothing larger: a build
/// running beneath a pane is a descendant and is summed separately in
/// [`Self::descendants`], and nothing here says who owns the process, what work
/// it serves, or whether it is progressing. Every field comes from one read of
/// the process, so a sample never mixes two incarnations; RSS is that process's
/// resident set, in which pages shared with another process are counted here as
/// well.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessResources {
    /// The incarnation these readings belong to.
    pub identity: ProcessIdentity,
    /// The kernel's scheduler state when the sample was taken.
    pub state: ProcessState,
    /// Resident set size in bytes. `None` when this machine cannot convert the
    /// kernel's page count.
    pub rss_bytes: Option<u64>,
    /// Interval CPU since the previous sample of this same identity. `None`
    /// until such a sample exists, and whenever the interval cannot be
    /// measured.
    pub cpu: Option<CpuPercent>,
    /// What the processes beneath this one are using, as the refresh's scan
    /// observed them. Never merged with the fields above: a build's work is not
    /// its launcher's.
    pub descendants: DescendantResources,
}

/// A total over processes, and what it actually covers.
///
/// A sum is not a measurement by itself: it can be small because its members
/// were idle, or because the scan could not read them. The variant keeps that
/// difference, so an incomplete total can never be read as a complete zero, and
/// the reason travels with it because the reader has to know which members are
/// missing to know what the number is worth.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Total<T> {
    /// Every member of the set contributed to it.
    Complete(T),
    /// A lower bound: some members could not contribute, for the reason given.
    Partial(T, String),
    /// Nothing could be totalled, for the reason given.
    Unknown(String),
}

/// What the processes beneath one root are using, as one scan observed them.
///
/// The members are the kernel's descendants — the ancestry its parent links
/// reported when the scan read them — and never a claim of ownership: a build
/// beneath a pane belongs to no agent, session, assignment or background task
/// that Radar knows of, and none of these totals is a cgroup or workload total.
/// A process beneath two roots contributes to each of their totals while being
/// read once, and RSS is summed per process, so pages shared between two
/// descendants, or between a descendant and its ancestor, are counted once for
/// every process that maps them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DescendantResources {
    /// How many processes the scan observed beneath the root. `None` when none
    /// could be enumerated at all.
    pub observed: Option<u32>,
    /// The resident set of those of them whose size could be read.
    pub rss_bytes: Total<u64>,
    /// Their interval CPU. Every one of them needs its own matching pair of
    /// readings, so a descendant seen for the first time contributes nothing
    /// and leaves this partial.
    pub cpu: Total<CpuPercent>,
}

/// Interval CPU use of one process, as a percentage of one CPU.
///
/// Held in hundredths of a percent, as an integer: the value is a ratio of two
/// kernel counter readings, and an exact integer is what a row can show and a
/// test can assert. `12.5%` of one CPU is `1_250`. It measures the process
/// rather than the machine, so a process that used two CPUs for a whole
/// interval is `20_000`, not `100%`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuPercent(u32);

impl CpuPercent {
    /// From hundredths of a percent of one CPU.
    pub fn from_hundredths(hundredths: u32) -> Self {
        Self(hundredths)
    }

    /// Hundredths of a percent of one CPU.
    pub fn hundredths(self) -> u32 {
        self.0
    }
}

/// The scheduler state the kernel reports for a process.
///
/// A state letter travels with every process and is observable, but it says
/// only what the scheduler is doing with it: a sleeping process may be waiting
/// on a socket or on nothing, and a zombie has already exited. It is not
/// activity, not progress and not a verdict on the work underneath.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessState {
    /// Running, or waiting its turn on a CPU.
    Running,
    /// Interruptible sleep: waiting, but wakeable.
    Sleeping,
    /// Uninterruptible sleep, usually blocked in the kernel on I/O.
    DiskSleep,
    /// Stopped by a signal.
    Stopped,
    /// Stopped because something is tracing it.
    TracingStop,
    /// Exited, not yet reaped by its parent.
    Zombie,
    /// Gone, or being torn down.
    Dead,
    /// Idle in the kernel, below the scheduler's oldest run queue.
    Idle,
    /// A state this kernel wrote that Radar does not name.
    Other(char),
}

impl ProcessState {
    /// What the kernel's state letter stands for, or [`Self::Other`] for a
    /// letter this kernel has and Radar does not name.
    pub fn from_letter(letter: char) -> Self {
        match letter {
            'R' => Self::Running,
            'S' => Self::Sleeping,
            'D' => Self::DiskSleep,
            'T' => Self::Stopped,
            't' => Self::TracingStop,
            'Z' => Self::Zombie,
            'X' | 'x' => Self::Dead,
            'I' => Self::Idle,
            other => Self::Other(other),
        }
    }
}

/// How a foreground process's running executable compares with the program
/// `PATH` resolves for the same name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BinaryFreshness {
    /// The running executable is the installed program, or another file inside
    /// the same installation.
    Current,
    /// The running executable's file was replaced or removed, or its
    /// installation differs from the one installed now.
    Stale,
    /// Nothing could be compared: the process is gone, its executable or the
    /// installed program is unreadable, the running file is not the named
    /// program, or this platform has no reader.
    #[default]
    Unknown,
}

/// What was compared, and how it came out.
///
/// The two identities are the installations the executables belong to — the
/// Nix store root, or the resolved path elsewhere — never parsed into versions
/// or package names: the freshness says whether they differ, and the identities
/// let a row name both without inventing anything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub freshness: BinaryFreshness,
    /// The installation the running executable belongs to. `None` when its
    /// executable could not be read, or was not the named program.
    pub running: Option<String>,
    /// The installation the `PATH` match for the program belongs to. `None`
    /// when no search directory has it.
    pub installed: Option<String>,
}

/// Whether a foreground program has taken the terminal over.
///
/// A terminal cannot see a program's intentions, but it does see the line
/// discipline: a full-screen program turns canonical mode and echo off, and a
/// command that prints and exits leaves them alone however long it runs. That
/// is the difference between an editor someone is working in and a build in
/// progress, and it is the only signal of the kind the kernel offers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalMode {
    /// Raw mode, no echo: the program is drawing the screen itself.
    FullScreen,
    /// The shell's line discipline is intact: output scrolls, input is echoed.
    Line,
    /// Unreadable: a process with no terminal, one that is already gone, or a
    /// platform whose terminal state Radar cannot read.
    #[default]
    Unknown,
}

impl ForegroundEvidence {
    /// A non-shell foreground as the runtime reports it, before this machine
    /// has been asked anything about the process.
    pub fn command(pid: i32, name: Option<String>, command: Option<String>) -> Self {
        Self::NonShell {
            pid,
            name,
            command,
            local: LocalFacts::default(),
        }
    }

    /// What this machine knows about the foreground process, if one is named.
    pub fn local(&self) -> LocalFacts {
        match self {
            Self::NonShell { local, .. } => local.clone(),
            _ => LocalFacts::default(),
        }
    }

    /// The foreground command as a row would show it: the program, then its
    /// arguments, with the executable's own path dropped — a Nix store path
    /// says where the program lives, not what it is doing.
    pub fn command_line(&self) -> Option<String> {
        let Self::NonShell { name, command, .. } = self else {
            return None;
        };
        let words: Vec<&str> = command
            .as_deref()
            .unwrap_or_default()
            .split_whitespace()
            .collect();
        let program = name
            .as_deref()
            .or_else(|| words.first().map(|word| base_name(word)))?;
        let mut arguments = words.iter().skip(1).copied();
        // A wrapper repeats the program as its first argument — a script under
        // an interpreter, a launcher under its own name. The reader wants the
        // program and what it was told, not the pipe it came through.
        let first = arguments.next().filter(|argument| {
            base_name(argument) != program
                && base_name(argument).strip_suffix(".py") != Some(program)
        });
        let arguments: Vec<&str> = first.into_iter().chain(arguments).collect();
        if arguments.is_empty() {
            Some(program.to_string())
        } else {
            Some(format!("{program} {}", arguments.join(" ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentObservation, HerdsmanFacts, Location, RuntimeStatus, SemanticState};

    fn facts(model: &str, provider: &str) -> HerdsmanFacts {
        HerdsmanFacts {
            model: Some(model.into()),
            provider: Some(provider.into()),
            ..HerdsmanFacts::default()
        }
    }

    /// Facts carrying what a pane's own publisher reports: the running count it
    /// publishes and the unresolved task list it publishes beside it.
    fn tasks(running: Option<u32>, entries: &[&str]) -> HerdsmanFacts {
        HerdsmanFacts {
            background_running: running,
            background_tasks: entries.iter().map(|entry| (*entry).to_string()).collect(),
            ..HerdsmanFacts::default()
        }
    }

    /// One observed agent with these facts, reported at this native status.
    fn observed(facts: HerdsmanFacts, status: Option<RuntimeStatus>) -> AgentObservation {
        AgentObservation {
            location: Location {
                workspace_id: "wA".into(),
                tab_id: "wA:t1".into(),
                pane_id: "wA:p1".into(),
            },
            name: Some("pi".into()),
            label: None,
            status,
            session: None,
            lineage: None,
            facts,
        }
    }

    #[test]
    fn a_row_shows_the_bare_model_whatever_provider_the_prefix_names() {
        // The live shapes: the `provider` token is sometimes the route and
        // sometimes the agent's name, so it cannot say what the prefix is.
        for (model, provider, bare) in [
            ("omniroute/coder-high", "omniroute", "coder-high"),
            ("coder-high", "omniroute", "coder-high"),
            ("openai-codex/gpt-6.1-sol", "codex", "gpt-6.1-sol"),
            ("anthropic/claude-sonnet-5-5", "claude", "claude-sonnet-5-5"),
            ("gpt-6-luna", "codex", "gpt-6-luna"),
        ] {
            assert_eq!(facts(model, provider).model_name(), Some(bare), "{model}");
        }
        assert_eq!(facts("vendor/", "x").model_name(), Some("vendor/"));
    }

    #[test]
    fn the_awaited_set_is_the_union_of_both_publishers_never_the_running_count() {
        // The live shape: the count reports nothing running while five tasks
        // have exited into review.
        let live = tasks(
            Some(0),
            &[
                "bg-2286:review",
                "bg-2284:review",
                "bg-2283:review",
                "bg-2282:review",
                "bg-2281:review",
            ],
        );
        assert!(live.is_awaited());
        assert_eq!(live.background_task_ids().len(), 5);
        assert_eq!(live.awaiting().as_deref(), Some("5 background tasks"));

        // The count is not the set: a running count with no task list is
        // nothing to wait on, and either publisher alone is enough.
        assert!(!tasks(Some(2), &[]).is_awaited());
        assert!(!HerdsmanFacts::default().is_awaited());
        assert!(
            HerdsmanFacts {
                awaited: vec!["owner".into()],
                ..HerdsmanFacts::default()
            }
            .is_awaited()
        );
    }

    #[test]
    fn task_ids_split_at_their_last_colon_whatever_the_phase_word() {
        let facts = tasks(
            Some(1),
            &[
                "bg-2286:review",
                // An id that itself carries a colon: only the last one
                // separates the phase.
                "wA:p1:flushing",
                // No phase published at all: the entry is all id.
                "bg-2281",
                // A phase word this version does not know keeps its task.
                "bg-2280:quiescing",
                // The same task listed twice is one unresolved task.
                "bg-2281",
            ],
        );
        assert_eq!(
            facts.background_task_ids(),
            vec!["bg-2286", "wA:p1", "bg-2281", "bg-2280"]
        );
        assert_eq!(facts.awaiting().as_deref(), Some("4 background tasks"));

        // A mixed set is the whole set, whatever each task is doing.
        assert_eq!(
            tasks(Some(1), &["bg-2:running", "bg-1:review"])
                .background_task_ids()
                .len(),
            2
        );
    }

    #[test]
    fn the_activity_derivation_follows_its_five_steps_in_order() {
        let awaiting = || tasks(Some(0), &["bg-1:review"]);
        let idle = || Some(RuntimeStatus::Idle);

        // 1. A fresh owner-published `lost` wins, whatever is outstanding.
        let lost = HerdsmanFacts {
            state: Some(SemanticState::Lost),
            ..awaiting()
        };
        assert_eq!(observed(lost, idle()).activity_state().word(), "lost");
        // 2. Work in flight stays in flight with work outstanding.
        assert_eq!(
            observed(awaiting(), Some(RuntimeStatus::Working))
                .activity_state()
                .word(),
            "working"
        );
        // 3. An unknown or unreported native state stays unknown.
        assert_eq!(
            observed(awaiting(), Some(RuntimeStatus::Unknown))
                .activity_state()
                .word(),
            "unknown"
        );
        assert_eq!(
            observed(awaiting(), None).activity_state().word(),
            "unknown"
        );
        // 4. Otherwise outstanding work with nothing in flight is waiting.
        assert_eq!(
            observed(awaiting(), idle()).activity_state().word(),
            "waiting"
        );
        // 5. With neither publisher reporting anything, the native state
        // stands — idle here, never inferred waiting.
        assert_eq!(
            observed(HerdsmanFacts::default(), idle())
                .activity_state()
                .word(),
            "idle"
        );
        assert_eq!(
            observed(HerdsmanFacts::default(), Some(RuntimeStatus::Done))
                .activity_state()
                .word(),
            "done"
        );
    }
}
