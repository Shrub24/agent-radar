# Radar plan

The queue behind the OpenSpec changes. An item lives here with the decision it
still needs, and leaves when it becomes a change worth specifying. What is
already settled is in `README.md` and `openspec/`; this file is the future.

## Accepted baseline

The operator accepted the live smoke on 2026-10-06. The initial fleet overview,
extension bus, pane focus and tree controls are synced into `openspec/specs/`
and archived under `openspec/changes/archive/2026-10-06-<change>/`.

Built: Herdsman facts and derived activity, owner projection in details,
assignment age, model/thinking/context, unresolved background-task counts and
bus-backed task details; pane/process views; configurable colours, marks and
motion; title/model prefix and workspace-suffix stripping; finished sessions
in the agents-only view through `e`; sorting, attention/working jumps and mouse
navigation.

## Next

1. **Proper nested tree.** Planned in
   `openspec/changes/nested-fleet-tree/` (proposal, design, delta spec and six
   tasks; strict validation passes). Git-style connectors, selectable
   background-task rows beneath their agent, and folding at each branch. Keep
   exact-identity ownership, per-level sorting, filtering ancestry and stable
   selection. Implementation has not started.
2. **Lifecycle controls.** Confirmed pane/tab close and idle-only single-agent
   restart in the existing tree. Confirmation never overrides an owner's refusal
   for outstanding work or results. Radar requests the action; the runtime
   provider or Herdsman executes it. Settle the supported owner contracts before
   enabling destructive actions. Fleet restart, new-tab creation, a project
   picker and separate runtime/agent modes are deferred.

### Runtime provider interface

Herdr is the first mux adapter, not the application's interface. Observation,
foreground evidence, focus and runtime actions cross a small Radar-owned
provider interface using normalized locations and outcomes. Herdr CLI commands,
socket operations, wire types and transport failures stay inside its adapter.
The app, tree and UI must not branch on Herdr operation names. Unsupported
provider capabilities are explicit; a future mux need not emulate Herdr's whole
interface. No plugin loader or second production adapter is needed now.

Managed-agent lifecycle is a separate owner-control interface. A mux closing a
pane cannot substitute for Herdsman validating and retiring an assignment.

Owner-side discovery (2026-10-06): Herdsman has close preflight but no external
control endpoint or general restart operation. Its relaunch primitive currently
covers idle, directly owned workers only, not lead/standalone sessions. The owner
has proposed a separate versioned request/result file interface with atomic
admission, owner-enforced expiry and an explicit unknown-outcome state; this is
not implemented or a finalized consumer contract. The task bus remains
metadata-only. Before specifying lifecycle implementation, agree target support,
capability discovery and refusal semantics with that owner. Missing metadata is
not evidence that a pane is unmanaged; retained managed associations still
block direct pane/tab closure.

## Interactivity

Radar already asks Herdr to focus a pane or workspace. Lifecycle controls expand
that action surface and need stronger target validation and confirmation.

- **Focus and mouse support are built.** `Enter` and a second click on a
  selected agent or pane row use the same focus action. A click on a workspace
  heading selects and folds it; the wheel scrolls the panel under the pointer.
  Mouse capture takes the terminal's text selection, which the README states
  along with the bypass key.
- **Controls beyond focus.** Anything that acts on an agent — steer, interrupt,
  extend, close — makes Radar a second control surface for semantics Herdsman
  guards with its own preflight. That is a different proposition from focus and
  should be decided separately. Analytics over what Radar already observes is
  the cheaper half of this and comes first.

### Lifecycle controls: next, pending owner contracts

Three operations, in increasing order of what they can destroy. The shape that
matters is that Radar *triggers* and the owner *executes*: pane operations belong
to Herdr, agent operations to Herdsman, advertised over the bus's `ops` list. A
first version must not do any of this itself.

- **Close a pane or tab (`x`, confirmed).** `d` is taken by the details toggle.
  A tab-close confirmation names the tab and its affected panes, using the
  selected row's observed tab without introducing a second view mode. Refuse
  the whole tab if managed-agent containment is known or uncertain; do not close
  some panes and then discover an owner refusal. What
  dies differs by what the pane holds, and the confirmation should say which: an
  idle shell loses nothing; a pane running a command kills that process and its
  output; a pane hosting a managed worker orphans the owner's assignment, whose
  child will never resolve unless Herdsman retires it. So closing a managed pane
  either goes through Herdsman or is refused with a reason.
- **Restart an agent (`r`, confirmed, idle only initially).** Ask its owner to
  replace the process while preserving the session and owner accounting; a
  replacement pane may differ. The confirmation popup never permits an active
  or unretrieved assignment to bypass preflight. Three consequences to design for: Herdsman owns the assignment
  and the parent link, so a restart it does not know about leaves a stale
  assignment behind; a reconnect must replace the old bus connection even if
  the resumed Pi session keeps its UUID; and `pi_bg` results that the agent has
  not retrieved must be accounted for rather than silently discarded. It goes through Herdsman for those reasons, and
  never through `pi-bg`, whose `get` and `stop` are the agent's own consumption
  path.
- **Restart the fleet (after a `pi` update).** Never a bulk kill: an agent in
  `review` still has a completion its `pi-bg get` would consume, and a hard kill
  throws those away. It has to be cooperative — ask each agent to stop, wait for
  it, relaunch, report the ones that did not stop — and it needs an ordering rule
  for the session that issues it, which is itself in the fleet. Restarting every
  session except the one asking is the sane default, with that one named and left
  until last.
- **Confirmation.** A two-step confirm that names the target and what is lost
  (unretrieved results, a running command), not a y/n prompt that a stray
  keystroke answers. The mouse equivalent is clicking the confirmation, not a
  second click on a row.

## The bus: detail from the extensions that own it

The pane tokens are a thin pointer — Herdr caps a pane at 32 keys and a worker
already uses about 30. Detail belongs to the extension that owns it, pushed to
Radar rather than pulled from it.

- **Shape.** Radar listens on one well-known socket
  (`$XDG_RUNTIME_DIR/agent-radar/radar.sock`). An extension dials it, says hello
  (protocol version, session UUID, optional pane id, the operations it supports)
  and sends the complete list of unresolved tasks whenever it changes — a full
  replace, never deltas. Radar not running costs the extension nothing, and a
  Radar that starts later gets the snapshot on the next hello.
- **Why push.** The agent has no endpoint to call, and the protocol has no
  receipts or acknowledgement, so nothing about it competes with `pi-bg get`,
  which stays the only way an agent consumes a completion. Metadata only: no
  output text travels over it.
- **Ownership.** `pi-bash-processes` owns the background-task detail — id,
  state (`running`, `flushing`, `review`), command, cwd, pid, start, last output,
  output size, exit code. No log path in v1. Specified in
  `openspec/specs/extension-bus/` and `openspec/specs/background-task-detail/`.
  Herdsman supplies the agent, its role and the top-level state, and the task ids
  in `pi_bg_tasks`. Radar joins them by exact identity: the session UUID in the
  hello is the pane's `pi_herdsman_session`, and task ids match.
- **Baseline stays the tokens.** Without the bus a row shows what it shows now,
  including the count of unresolved tasks; the bus only adds detail. When the
  tokens and the bus disagree, the bus's list is shown and the count is not
  trusted over it.
- **Contract.** A versioned `radar-bus.md` with a fixture, beside Herdsman's
  `pane-metadata.md`; each extension implements its half.
- **Lifecycle controls are a future maybe, not scoped.** The hello advertises the
  operations a client supports and v1 advertises none, so a later `stop-task` or
  similar can be added without a protocol break. It would need its own sync story
  with the extension's state and is decided separately (open question 1).
- **Order.** Radar's side has landed: the listener (`src/bus.rs`), the state it
  feeds beside the observation (`src/app.rs`), and the details join
  (`src/ui.rs`), verified against a stub publisher by the listener, detail-join
  and PTY checks. End-to-end publisher validation is separate from those stub
  checks; its status should be confirmed with `pi-bash-processes`. Per-agent
  stats from the session file (cost, tokens, last activity, errors) need no bus.

## Tree presentation

- **Nested tree, git style.** Subagents under their owner and a worker's
  background tasks under the worker, drawn with connectors (`├─`, `└─`, `│`)
  rather than indentation alone, each level foldable. Ownership is already
  exact-identity and the tasks are already facts; what is missing is the shape.
  Next slice: make each task a row of its own, using the same connectors for a
  branch with one child as for one with several.
- ~~**Sorting.**~~ Done as `s`: source → state → name, per level, with a group
  ranked by its most urgent row and the selection following the row by identity.
  Age order and a configured default order are not built and are not needed yet:
  the state ladder already puts what needs a human first.
  Jump-to-next-row-of-a-kind landed with it as `n`/`N` and `w`/`W`.
- **Title and finished-session loose ends are closed.** A separated trailing
  workspace suffix is stripped, and `e` lists finished sessions even with
  ordinary panes hidden.

## Peek and preview

Radar sees Herdr's metadata, local process facts and bus task metadata, not their
output or conversations. Beyond those: the pane's own recent output, the
Pi session behind an agent (its file path is already reported), and the artifact
a worker is on, which `pi_herdsman_task` often names as a file. Needs a decision
on whether Radar reads files it did not observe, and what it shows for a path
that has gone.

## Brand and command awareness

Radar already distinguishes a program that has taken the terminal over from a
command still on the shell's line discipline, and it now recognises *which*
program: `[processes]` maps a program name to a mark, drawn in the pane row's
second column wherever a Nerd Font can draw it, with the mode mark as the
fallback for anything unlisted. What is left of this direction is behaviour
rather than appearance — the row following the program (its own description, and
eventually keys it responds to) and a wider selection of marks. The table lives
in configuration, as the vendor marks and colours already do.

## Processes and links

The process view lists panes with a live command, its age and its terminal mode.
Next: the tree behind those processes — what spawned what — and the links between
running work and the things that asked for it: an agent, an assignment, a
background task.

## Sessions: interaction and search

Radar knows every agent's exact session UUID, its session file and its human
name, and is careful to treat them as different things. Next: find an agent by
any of them, and get to the session from its row. Needs a decision on what
"get to" means for a file-backed session — open it, copy it, or hand it to
another tool.

## Failure analysis

A fleet view is also an incident view: what failed, what stalled, what was
proven lost, what is only retained, and for how long. Radar already carries the
inputs — state, retention basis, source freshness, the collection diagnostic,
process age and terminal mode. The reporting side does not exist: nothing yet
summarises the fleet, or says which rows deserve attention.

## Open questions

1. **What may Radar do to Herdr?** Focus is settled: it is the one action, and
   it changes no agent. Lifecycle control is a
   larger one. This decides the rest of the interactivity section.
2. ~~**Mouse capture** trades the terminal's text selection for clicks.~~
   Taken, and written down in the README. The open half is whether a bypass key
   is enough, or whether capture should be a setting.
3. **Configuration growth.** Every new role, animation, mark and eventually sort
   key adds to `config.toml` and to `--print-config`. Worth watching that it
   stays a file a person can read.
