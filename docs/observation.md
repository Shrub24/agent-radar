# How Radar observes a fleet

Where each fact on a row comes from, how an agent is joined to a pane, how stale
and missing data is shown, and how background-task detail reaches a row.

## Observation and continuity

Radar observes in one of two modes, chosen once before anything is drawn, and
neither reads agent facts from the backend's own agent records:

- **Through the daemon** (the default), every agent fact comes
  from the daemon's registry. One refresh reads the daemon's pane inventory,
  every agent (`subject.list`, paged) and the spawn topology
  (`spawn.list`), and joins them in one pass, so a frame comes from one moment.
  A backend's own agent records, display metadata and native agent status are not
  read for anything the dashboard shows. A daemon that does not declare
  `registry.v1` and `spawn.v1` fails the refresh with that diagnostic rather
  than showing a fleet sourced from somewhere else.
- **No direct mode.** If the daemon is unreachable the dashboard shows a blocking
  diagnostic and retries. It never falls back to a backend's agent state or to a
  pane-only tree.

### Where an agent row's facts come from

Each authority is its own channel with its own writer, provenance and freshness,
so one can expire, be replaced or stay silent without disturbing the others:

- **execution** — what the session reports it is doing (`pi`).
- **assignment** — what the owner reports it assigned: activity, role,
  definition, run, the assignment text, what it awaits and the ask it waits on
  (`herdsman`).
- **facts** — the session's own model, provider, thinking level, context usage,
  session name and background tasks (`pi`).

A whole snapshot replaces the previous one, so an omitted field is withdrawn
rather than remembered, and a channel nobody has published reads `unknown`,
never `idle`. Freshness is the writer's lease: an expired snapshot reads `stale`
beside the value it last described, and never silently turns into a current
state. Waiting, lost and settling are **derived** by the dashboard from
published facts and freshness, and a derived word never rewrites what a writer
published: a row can read `waiting` while the execution line still reads `done`,
and a finished execution never completes the owner's assignment.

### Joining a registration to a pane

A registration joins a pane on its backend-qualified location — `backend`,
`instance`, workspace, tab and pane — together with the exact birth identity it
claimed: `boot_id`, `pid` and `start_ticks`, served as `process.claim`. Radar
samples the birth identity of the pane's own foreground process for itself, and a
PID alone, a title, a label, an alias and a session UUID never join an agent to a
pane.

A registration that cannot be joined exactly is shown **unresolved**, with the
location it last reported: no claim, a mismatched or unreadable process, a pane
the inventory does not hold, or two registrations on one identity, which is
ambiguity rather than evidence. An unresolved row owns no pane — it is not
focused, its controls refuse, and it borrows nothing from the pane it names. A
registered pane that vanishes from a successful inventory is unresolved, not a
claimed process exit.

### Protection is not close eligibility

A registration at a pane keeps that pane, and the tab containing it, out of a
generic unmanaged close, including where its join is unresolved or ambiguous: the
pane may be someone's agent even where Radar cannot prove whose. Close
eligibility is a separate and stronger fact, established by a daemon-authored
spawn edge to that exact child; a registration, a label, a session UUID or a
metadata flag never makes a target eligible. `D` on a registration is routed to
the worker's owner where Radar holds a subject for it, and otherwise closes the
pane the row shows; it never claims an owner it cannot name.

### What is still read from pane metadata

A backend's own agent records and the `pi_herdsman_*` pane tokens are read as
**physical and containment evidence only**: they keep a pane that has an occupant protected
while Radar cannot name that occupant. They are never display facts for an agent
row — a registered row is named by its registration, its label else its agent id,
and its values come from the channels above. Ordinary panes keep the backend's
pane title and label, which describe the pane rather than an agent.

### Refresh and continuity

- A refresh runs off the UI thread with at most one in flight. It waits one
  second between completed refreshes, each command has a five-second timeout, and
  the registry read of one refresh has a single bounded deadline across its
  pages. Input and quitting do not wait for a stalled read, and a refresh that
  finishes past its deadline is never applied.
- Failed collection preserves the last-good inventory with a **stale** diagnostic.
  Initial failure is **unavailable**, not an empty fleet. A successful empty
  snapshot clears the inventory; successful recovery clears the diagnostic. The
  fleet heading carries the state (`Fleet · current`, `Fleet · STALE`,
  `Fleet · UNAVAILABLE`), and the selected row's details write the failure out —
  there is no status line of its own.
- The **row's state word is the session's activity state**, derived in the
  contract's order: a live `lost` from the owner wins, work in flight stays in
  flight, an unknown or unreported execution stays unknown, the owner's
  `settling` stands over an idle execution, a non-empty union of the owner's
  awaited items (assignment `awaited`) and the ids of the session's own
  background tasks (facts `background_tasks`) is `waiting`, and otherwise the
  reported state stands. The running count takes no part in it, so a session
  whose tasks have all exited with work unresolved is `waiting` with nothing
  running, and the details keep the published running count distinguishable from
  the unresolved set.
- The owner's **assignment state** is a different fact and is shown as its own
  labelled detail line (`assignment: settling (owner projection)`) rather than
  choosing the row's word. It is what a lead's own fleet view names for that
  worker, and it is written by a different writer at a different cadence: a
  `settling` or `delivered` projection can be briefly stale, and the row must
  not contradict the pane because of it. `settling`, `delivered` and `unknown`
  projections stay reachable in the details.
- A registration is placed at the workspace and tab its location names, or under
  an `unplaced` root when it reports none. A registration for another backend or
  server is never shown, and one that names no pane, or a pane the inventory
  lacks, is shown only while a publisher holds a live lease or the daemon
  recorded a spawn edge for it, so a dead agent's record ages out of the tree.
  `A` creates a workspace named for a chosen project, whatever row is selected.
  One private picker blocks shared tree input across all views: inline in the
  dashboard or detached over a compact workspace sidebar. Candidates come from
  `[projects] source` (default `zoxide query -l`). The picker lists git projects and `Ctrl+g`
  reveals every directory the source prints (it opens on all when none is a git
  project). An empty or failed source falls back to `[projects] base`
  (default home), with a diagnostic. The invoking start directory is not prepended.
  `L` immediately launches the configured `[agents] launch_command` argv (default `["pi"]`) as a managed agent in a new tab of the selected place's workspace — from a pane it opens its own window rather than splitting that pane — and it uses the selected pane's directory (or the workspace's) and starts the resolved command through the user's login/interactive shell, applying the target directory's direnv environment without copying the TUI's environment. Use `a` to add an empty shell for arbitrary commands. A lead started with `L` is enrolled as a lifecycle subject when it registers, so it can be stopped and restarted; `E` enrolls any other registered lead after a confirmation.
  Enter on a row focuses it; with no tmux client attached it opens a terminal
  attached to the server (`[tui] terminal`, else `$TERMINAL -e`) and asks again. Children are not nested under their
  parent: the exact spawn edge is shown in the details. An ordinary pane never
  duplicates a current or retained agent row.
- Radar does not read Herdsman mailboxes, does not treat a backend's agent name or
  title as a Herdsman fact, and never infers ownership from tab position or from
  the human session name — ownership is exact session identity.
- When an agent disappears but its pane remains, Radar keeps its last observation
  **in memory**, visibly retained rather than current. Foreground evidence for
  the pane distinguishes a shell from a positively identified replacement
  command using the process's own PID and birth identity; inconclusive evidence
  leaves the association unverified.
- A new reported agent replaces the old association. Positive non-shell evidence
  or a successful snapshot proving pane loss clears it. Retained facts are
  last-observed, not current status or historical outcomes. Restarting Radar clears
  them; it has no durable history.

Polling is best effort: inventory and process reads are not atomic, short
commands between polls can be missed, and foreground confirmation can lag by two
refresh cycles. Continuity across backend restarts or session moves is not promised.

## Background-task detail (the bus)

The `pi_bg_*` pane tokens summarize background-task state but cannot carry
the task detail, so `pi-bash-processes` publishes full snapshots. The daemon
owns that ingest and exposes volatile detail through `tasks.list.v1`; the TUI
never listens to publisher connections. Until the publisher migrates, the daemon
accepts its existing line protocol on a temporary compatibility socket path
(`$RADAR_SOCKET`, else `$XDG_RUNTIME_DIR/agent-radar/radar.sock`, else
`/tmp/agent-radar-<uid>/radar.sock`). This entry shares transport and task state
with the versioned control socket. Reading tasks does not acknowledge, settle
or consume anything; `pi-bg get` remains the retrieval path. See
[`radar-bus.md`](radar-bus.md) and its
[`radar-bus.fixture.json`](radar-bus.fixture.json) for the protocol.

A publisher task list joins an agent row only by the exact Pi session UUID
reported by that row. The optional `pane` in a legacy publisher `hello` is not a
join fallback. Nothing else joins task detail — not pane, cwd, title or human
session name. A connected session with no matching agent row remains in the
daemon snapshot but is not attached to another row.

One projection decides what a row's tasks are, and the task rows, the `N bg`
badge and the details all read that same projection, so they cannot disagree. A
matched publisher's list is authoritative, an **empty list included** — the pane's
own `pi_bg_tasks` ids are never unioned into it. They are the fallback for a row with no connected publisher for its exact
session, and each of those rows is marked `tokens`, or `last-observed` when the
agent itself is retained. A row a publisher speaks for
shows the publisher's phase word, and when `pi_bg_running` disagrees the panel
says so instead of choosing. Since the token counts only `running` tasks, only
those are compared with it. A phase word Radar does not know is shown as
published.

Each unresolved task is a selectable row beneath its agent, labelled with its
published command or, without one, its id, then the phase as published, the age
its published start time implies, and the program's mark from the configured
`[processes]` table where the command names one. Whether those rows are listed
is a per-view choice: `agents` starts hidden, `running` and `all` start shown,
and `b` toggles the current view alone — the pane view, the finished-session
toggle, branch folds and the `N bg` badge are untouched. `running` and `all`
list every unresolved task whatever phase it published, including `flushing`,
`review`, a word Radar does not know and none at all: the processes view is a
view of outstanding work, not a claim that a task's process is still alive. The
rows read the current projection whenever they are shown, so a list that arrived
while they were hidden is what appears. A task hidden under the selection falls
back to its owner row, which is still there to be read from.

A task's identity is its owner row, the session its facts were reported in,
and its task id — never the command, and never the id alone, because ids are
reused between panes and between successive sessions in one pane. The same id
under a new session is therefore a different task, and a selection does not
follow it: it falls back to the agent row above it. A task row is a leaf: it
cannot be folded, and it has no disclosure control. `Enter` or a second click on
it focuses its owner's pane through the same focus action, with the same refusal
when that location is stale or no longer observed, because a task is read
through the pane its owner row names and is never consumed or controlled.

Selecting a task shows its own panel: its id, phase and source basis, and —
where the publisher sent them — its command, working directory, process id,
start, last output, output size and exit code. The owner's panel states the same
facts on its per-task line, drawn from the same projection, so the rows, the
parent's `N bg` count and both panels cannot disagree. A fact the publisher did
not send draws nothing: absent is not a zero, and a `tokens` row shows only the
id and phase its pane published. A state word this Radar does not know is shown
as written. `command` and `cwd` are the sensitive fields: both are sanitized and
bounded to 256 characters where they reach the screen, and are never written to
a file, the configuration or a log. Task rows move only while their source
reports the process alive now: a `running` word on a `last-observed` row is
stated and stays still.

Task data lives in daemon memory only while its publisher connection is active. A disconnect removes that session's entry immediately and the row falls back to its token facts, because a gone publisher is not evidence that its tasks ended. An explicit empty list is different: the publisher is connected and reports nothing unresolved, which the details state in words.

Publisher ingest is a daemon service, separate from registry and inventory
freshness. If it cannot bind its compatibility socket or is otherwise
unavailable, `tasks.list` reports that diagnostic; the TUI keeps the rest of the
daemon-backed fleet visible and labels task detail `· bus off`.

