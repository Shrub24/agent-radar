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
navigation. The nested fleet tree is now synced and archived at
`openspec/changes/archive/2026-10-06-nested-fleet-tree/`: connected agent/task
branches, per-branch folding, selectable/filterable task details and parent-pane
focus. Its owner gate passed 236 tests, fmt, Clippy, build and both PTY smokes;
see its `verification.md`.

Also landed and archived on 2026-10-06: `background-task-visibility` (agents view
hides task children, running/all show every unresolved phase, `b` toggles per
view; 244 tests) and `runtime-provider-seam` (inventory, foreground evidence and
focus behind `src/runtime.rs`, Herdr in `HerdrRuntime`; 255 tests). Each has its
owner-gate results in its archived `verification.md`.

## Next

1. **Lifecycle controls.** Confirmed pane/tab close and idle-only single-agent
   restart in the existing tree. Confirmation never overrides an owner's refusal
   for outstanding work or results. Radar requests the action; the runtime
   provider or Herdsman executes it. The owner contract is fixed (below) but the owner
   side is not implemented, so Radar may build and test its client against the
   fixture and must not enable controls until the owner confirms it landed. Fleet restart, new-tab creation, a project
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

Owner contract (2026-10-06): `herdsman-control/v1` is specified in
`pi-extensions/pi-herdsman/docs/reference/herdsman-control.md` with a fixture
(`herdsman-control.fixture.json`, commit b86b4133, local and unpushed). The
owner side — directory, watcher, claim, execution — is not implemented; code the
client against the fixture and keep the UI controls off until it lands. One
product question is still with the user: whether the owner's model is told a
request happened (the design says no prompt).

- **Transport:** owner-created request/result directories under
  `~/.pi/agent/pi-herdsman/control/<ownerSessionId>/`, mode 0700, not symlinks;
  not the metadata bus or a new socket.
- **Target:** agent + run id, with pane id, Pi session UUID and session path as
  cross-checks. The owner repeats its full preflight at execution; a mismatch
  refuses rather than redirecting the action.
- **Outcome:** owner-enforced absolute expiry and exclusive claim before effect.
  A result reports completion; a claim without a result means execution started
  with unknown outcome and must never be auto-retried. No claim after expiry
  means not executed. This is not a client timeout that implies cancellation.
- **Wake:** an owner-pane control token is a short-lived completion hint, not a
  replacement for the request's result and fresh inventory.
- **Close:** require confirmation naming what is lost. Direct mux close needs
  positive unmanaged evidence. A Pi pane without Herdsman metadata is unknown;
  metadata-bearing and retained managed panes stay owner-routed. Refuse an entire
  tab if containment is managed or uncertain, never partially close it.
- **Restart:** idle managed workers only, continuing the same Pi session and
  preserving label, run id and lineage. Working/waiting/blocked are busy; lost
  is close-only; unknown refuses. Lead and standalone restart are unsupported,
  not a prompt to fall back to Herdr process launch.

The runtime seam has landed. Pane and tab close extend its trait when the
lifecycle change has a consumer; managed close and restart are a separate
owner-control client. Results use `outcome` `closed | restarted | refused |
unknown`; refusals carry `invalid_request`, `target_not_found`,
`target_ambiguous`, `agent_busy` or `unsupported_target`. Confirmation must echo
operation, label and run id. A tab or workspace holding a managed pane is
refused whole: send one close per managed agent, then close the container.
Metadata absence remains unknown, never proof of unmanaged state.

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
to the runtime provider, agent operations to Herdsman's separate owner-control
interface. The task bus stays metadata-only; its `ops` list is not this lifecycle
transport. Radar must not kill or relaunch agent processes itself.

- **Close a pane or tab (`x`, confirmed).** `d` is taken by the details toggle.
  A tab-close confirmation names the tab and its affected panes, using the
  selected row's observed tab without introducing a second view mode. Refuse
  the whole tab if managed-agent containment is known or uncertain; do not close
  some panes and then discover an owner refusal. What
  dies differs by what the pane holds, and the confirmation should say which: an
  idle shell loses its terminal state and scrollback; a pane running a command
  loses that process and its pane output; a pane hosting a managed worker orphans the owner's assignment, whose
  child will never resolve unless Herdsman retires it. So closing a managed pane
  either goes through Herdsman or is refused with a reason.
- **Restart a managed worker (`r`, confirmed, idle only initially).** Ask its
  owner to relaunch into its retained pane while preserving the Pi session,
  label, run id, lineage and owner accounting. Lead/standalone sessions remain
  unsupported; do not add a direct mux-launch fallback. The confirmation popup never permits an active
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
- **Order.** Radar's side has landed: the listener (`src/bus.rs`), event-driven
  updates (`src/app.rs`), the shared task projection (`src/tree.rs`), and its
  row/detail rendering (`src/ui.rs`). Listener, projection, presentation and PTY
  checks use a stub publisher. End-to-end publisher validation is separate;
  its status should be confirmed with `pi-bash-processes`. Per-agent stats from
  the session file (cost, tokens, last activity, errors) need no bus.

## Tree presentation

- **Nested tree — done.** Subagents and background tasks have connected,
  foldable branches (`├─`, `└─`, `│`). Each task is selectable, filterable and
  has its own details; Enter or a second click focuses its parent's observed
  pane. Agent disclosure-marker clicks fold without focusing. Connectors follow
  the visible sorted/filtered tree, including single-child branches.
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
process age and terminal mode. Attention jumps already help navigate individual
rows; fleet summaries and diagnostic reports do not exist yet.

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
