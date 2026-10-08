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

Also landed and archived on 2026-10-06: `stale-binary-marks` (warning glyph and
configurable colour; conservative live executable comparison), `lifecycle-controls`
(`x` pane/agent close, `X` tab close, `r` idle managed-worker restart, separate
default-Cancel confirmation) and `nix-distribution` (checked locked package, default
app and a Home Manager module; native `x86_64-linux` build verified, `aarch64-linux`
evaluated only). The lifecycle `verification.md` separates stub-owner verification
from the unperformed live destructive smoke, and existing Herdsman owners must reload
for their control inboxes.

## Next

The `foldable-process-descendants` change is landed: verified per-member
samples, the collapsed Processes table and its local selection/folding mode are
implemented, verified and archived. Its gates are recorded in that change's
`verification.md`. Keep the next work separate from lifecycle and state
contracts.

The active `mux-control-plane` change starts with Herdr wrapping: a Radar-owned
daemon exposes inventory, focus, guarded close, creation/splitting, input, output
reads and metadata reporting behind a backend-neutral interface. Durable requests
avoid replay after ambiguous effects; current owner controls stay intact. Radar
keeps its direct adapter as an explicit mode/startup fallback, never as a retry
of an uncertain operation. pi-extensions can port mux calls onto this interface.

Agent/session awareness, lead recovery, stale restart plans, a publisher registry
and parent/child topology remain future work over this seam. tmux is a later backend,
not a prerequisite for the Herdr wrapper. The daemon's existence does not settle
assignment authority or prove that a relaunched process recovered its children.

In priority order:

1. **Lead restart and recovery.** Establish safe lead/standalone restart and
   recovery of existing children, pending requests and task results. Keeping a
   session UUID is necessary but does not prove recovery. This is the foundation
   for stale restarts, not a direct Herdr relaunch fallback.
2. **Structured waiting and failure reporting.** Publish activity, waiting
   reason, last outcome and available action separately. Distinguish a question
   awaiting an answer, a permission request, a scheduled rate-limit retry and a
   failed turn with no retry. Coordinate this contract with lead recovery.
3. **Stale-lead restart plans and batches.** Build on the recovery contract:
   detect eligible stale leads, show a confirmed plan, execute through the owner
   and report each result. Prefer leads over indiscriminately restarting all
   panes. Startup detection and a restart offer come before opt-in automation.
4. **Owner lifecycle/state daemon.** Complement Herdr first with direct owner
   reporting, durable registration, reconnect/recovery and control routing.
   Replacing Herdr's state/lifecycle role is a later migration, not a requirement
   to replace its pane inventory and focus at the same time. Design this seam
   alongside priorities 1–2; migrate transport incrementally.
5. **Herdr-agnosticity and tmux-parity audit.** Cover Radar, pi-herdsman,
   publishers and launchers. Record actual parity gaps before starting another
   mux backend; use the daemon separation rather than assuming it fixes parity.
6. **Further TUI and information surfaces.** Review the completed Processes
   table and fleet density with the operator, then add non-consuming preview,
   session statistics/search and fleet failure summaries in that order.

Background-task resource enrichment still needs publisher-captured birth
identity; it is not a prerequisite for lead recovery. Command-specific actions,
new tabs, a project picker, separate runtime/agent modes and arbitrary process
browsing remain later work. Cooperative whole-fleet restart follows safe
lead-only restart; it must not become a bulk kill.

### Lead recovery and stale restarts

Basic confirmed pane/tab close and idle managed-worker restart are already
implemented. The missing owner operation is lead/standalone restart, including
its recovery guarantees:

- Identify the exact session and live process incarnation. Confirm that the old
  process has exited before resuming its session; never create a second attach
  as a side effect of restart.
- Preserve or explicitly reconcile child runs, ownership/lineage, pending asks
  and requests, unresolved task results and control outcomes. A new process
  retaining the same session UUID is not sufficient evidence of success.
- Have the owner declare restart eligibility and supported recovery. A lead
  may look idle while still owning active children or unconsumed results; row
  activity alone must not authorize its restart.
- Publish request acceptance, execution and the resulting session/process
  identity. Unknown execution stays unknown and suppressed, not automatically
  retried or inferred successful because a pane disappeared.
- For stale batches, use the existing exact binary-counterpart comparison;
  unknown freshness is not stale. Freeze and revalidate each target, report
  refused/skipped/unknown results, and leave the initiating session until last.
- On Radar launch, detect stale leads and offer a restart plan first. Silent
  startup restart is not the default. Later opt-in automation needs positive
  lead identity, owner-certified eligibility and the same recovery guarantees.

Evaluate moving Herdr's restart-on-launch responsibility into this owner-routed
path. Do not copy its all-pane restart behavior: preserve ordinary panes and
managed children unless their owner explicitly includes them in the plan.

### Structured activity, reasons and outcomes

**Ownership split:** Pi supplies execution evidence; Herdsman supplies assignment
authority; the coordinator supplies backend-independent physical observations
and controls. Keep those channels distinct even when one daemon transports all
three. Execution status does not certify assignment delivery or restart eligibility.

A process being idle does not mean its last turn succeeded, and one word such
as `blocked` cannot describe every reason an agent needs attention. Prefer Pi's
upstream evidence and owner reports over inferring state from terminal text:

| Activity | Reason or outcome | Action |
| --- | --- | --- |
| waiting | user answer required | open the pending question |
| blocked | permission required | open the permission request |
| waiting | rate-limited, retry scheduled | show retry timing |
| idle | last turn failed with 429, no retry scheduled | show failure and supported retry action |
| unknown | publisher disconnected | show stale evidence, not a successful idle session |

Define reason/outcome lifetimes, observation timestamps and pending-request
identities so old failures do not overwrite current activity and a dismissed
notice does not resolve a pending question. Available actions must be advertised
capabilities, not promises that Radar can issue them yet. Preserve coarse token
fallback where richer reporting is absent; missing detail stays unknown.

### Upstream Pi Program status (OSC 7501)

Plan execution reporting around Pi 1.1.0's Program status rather than duplicating
its state inference in extensions. Verified upstream at commit
`1cedd32724abfcb0915f76cc61b6827e2c16dbad`:

- Wire states: `idle`, `working`, `blocked`, `done`, `error`, plus `clear` to remove
  a report. Preserve `error` on the wire even if the UI calls it failed.
- `blocked` can carry `permission`, `question` or `auth`; optional messages are
  session names, dialog titles or the first line of an error, not prompts or model
  output. Treat those messages as potentially sensitive diagnostics nonetheless.
- Runs and compaction report working; unretried run errors report error. Successful
  retries supersede earlier errors, and cancellation settles to idle. These are
  execution outcomes, not Herdsman assignment completion or recovery evidence.
- Pi emits after a supporting terminal answers the OSC query; `PI_PROGRAM_STATUS=1`
  forces emission and `=0` disables it. Upstream explicitly says tmux/screen do not
  forward reports. Forcing emission is not proof that a backend can observe them.
- Current local Pi builds do not yet include this addition (user report). Keep
  existing evidence/fallback until a deployed build and adapter prove support.

The future coordinator should expose structured program status as an optional,
source-labelled observation with freshness and pane/process-incarnation binding.
Test support through the backend's structured terminal parser/event API; do not
scrape scrollback or seize the pane's PTY. A tmux route needs an explicit capture
mechanism or a Pi publisher bridge. Missing support is unknown, not idle; clear,
pane reuse, process exit and reconnect must not carry an old failure to a new process.
The current Herdr-wrapping slice does not implement an OSC parser or a new publisher.

References (pinned originals):
- [Terminal setup: Program status](https://github.com/badlogic/pi-mono/blob/1cedd32724abfcb0915f76cc61b6827e2c16dbad/packages/coding-agent/docs/terminal-setup.md#program-status)
- [Reporter semantics](https://github.com/badlogic/pi-mono/blob/1cedd32724abfcb0915f76cc61b6827e2c16dbad/packages/coding-agent/src/modes/interactive/program-status-reporter.ts)
- [OSC codec and fields](https://github.com/badlogic/pi-mono/blob/1cedd32724abfcb0915f76cc61b6827e2c16dbad/packages/tui/src/program-status.ts)

### Direct owner reporting and a lifecycle/state daemon

The initial migration complements Herdr:

- The coordinator wraps Herdr as the first backend for physical pane inventory,
  locations, focus and controls, with a backend-neutral port for consumers.
- Pi/Herdsman and background-task owners publish lifecycle facts directly;
  Radar joins them by exact session/run/process identities, with pane location
  kept separate from agent ownership.
- A daemon provides durable registration, fresh snapshots, reconnect/recovery
  and request routing. Registration must distinguish session context from a
  live publisher incarnation; reconnect must not revive an old writer's state.
- Each operation has a declared owner. The daemon transports facts and requests
  unless an explicit supervision contract makes it the lifecycle owner;
  transport alone does not authorize it to kill or relaunch processes.
- Controls retain exact-target preflight, confirmation, exclusive execution,
  expiry and acknowledged outcomes. They must preserve the existing meaning of
  started/unknown requests across Radar or daemon restarts.
- Background-task observation remains non-consuming. Neither state display nor
  a new transport may acknowledge a result or compete with the agent's `get`.

Start with the physical mux wrappers in `mux-control-plane`; develop direct
execution reporting and assignment-aware recovery with pi-extensions afterwards.
Migrate one surface at a time with an explicit fallback; do not silently turn the
current metadata-only task bus into a lifecycle command channel.

### Verification and shipping gates

- Complete the descendant-table source, PTY and checked Nix gates.
- Run an operator-approved live lifecycle smoke against owners that loaded the
  supported contract; stub-owner tests do not prove live recovery.
- Confirm real background-task publisher integration separately from the stub
  publisher PTY tests.
- Resolve inherited requirement-length warnings exposed by OpenSpec 1.14.1
  before the next all-spec strict shipping gate, without changing behavior.

### Nix distribution (done)

Implemented and archived as `nix-distribution`; its `verification.md` records the
native build, the pinned Home Manager consumer cases and the remaining limits.
Darwin packaging, a NixOS module and an overlay are not provided.

### Herdr-agnosticity / tmux parity audit

Audit the whole integration chain:

- Radar adapter and assembly: inventory, locations, foreground evidence, focus,
  pane/tab close, identifiers and containment. Identify anything still leaking
  Herdr meaning through supposedly normalized values.
- pi-herdsman and the other publishers: discovery, metadata/title publication,
  pane creation/focus/close/relaunch and reconnect behavior. Radar's trait alone
  cannot make worker launching or owner lifecycle work under tmux.
- Identity and transport: stable session/run/worker joins, mux-scoped pane ids,
  workspace/tab equivalents, bus fallback and owner-control cross-checks. Audit
  the provider-specific `paneId` contract, not just command spelling.
- Build a parity matrix: inventory; live agent facts; foreground/process roots;
  focus; unmanaged pane/tab close; managed close/restart; task bus; retention and
  source failure. Mark each as supported, equivalent with a documented mapping,
  unsupported or needing an upstream contract change.
- Deliver a grounded gap list, smallest necessary seam changes and a tmux
  implementation/validation plan. Do not start a plugin framework or a second
  backend during the audit. tmux panes/windows/sessions are not automatically
  interchangeable with Herdr panes/tabs/workspaces.

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
(`herdsman-control.fixture.json`, contract b86b4133, clarification 8f57974d).
Owner implementation 92e52f0e43a1 is accepted locally; docs correction 6fc793ed
clarifies that lost close leaves a surviving shell pane untouched. These were
reported as local, unpushed commits. An active assignment close is learned through
normal terminal-result delivery; idle close/restart creates no model prompt.
Existing owner processes must reload before directories/watchers are available.

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

The runtime seam now includes pane and tab close. Managed close and restart
use a separate owner-control client. Results use `outcome` `closed | restarted | refused |
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
- **Further agent controls.** Steer, interrupt and extend remain separate future
  decisions. Close and idle-worker restart now use Herdsman's owner preflight;
  this does not authorize additional controls or batch actions.

### Lifecycle controls: implemented baseline and restart roadmap

Three operations, in increasing order of what they can destroy. The shape that
matters is that Radar *triggers* and the owner *executes*: pane operations belong
to the runtime provider, agent operations to Herdsman's separate owner-control
interface. The task bus stays metadata-only; its `ops` list is not this lifecycle
transport. Radar must not kill or relaunch agent processes itself.

- **Close a pane (`x`) or tab (`X`), confirmed — implemented.** `d` is taken by the details toggle.
  A tab-close confirmation names the tab and its affected panes, using the
  selected row's observed tab without introducing a second view mode. Refuse
  the whole tab if managed-agent containment is known or uncertain; do not close
  some panes and then discover an owner refusal. What
  dies differs by what the pane holds, and the confirmation should say which: an
  idle shell loses its terminal state and scrollback; a pane running a command
  loses that process and its pane output; a pane hosting a managed worker orphans the owner's assignment, whose
  child will never resolve unless Herdsman retires it. So closing a managed pane
  either goes through Herdsman or is refused with a reason.
- **Restart a managed worker (`r`, confirmed, idle only) — implemented.** Ask its
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
- **Restart the fleet (after a `pi` update), later.** Lead recovery and confirmed
  stale-lead batches above come first. Never a bulk kill: an agent in
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
- **Background-task lifecycle controls are a future maybe, not scoped.** The hello advertises the
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

## Display/TUI refinement

The objective is useful information at a glance, not more fields on every row.
Do this against representative busy fleets, narrow terminals and deep branches:

- Keep fleet overview compact; show the relevant process or task and its age,
  not every shell/interpreter/helper. Full ancestry remains available through
  folding/details. OS liveness must not masquerade as an agent activity state.
- Review repeated titles, model/thinking text, state words, task badges and
  command wrappers. A datum should earn its inline position; do not duplicate
  the same command as an agent title, task child and independent process row.
- Make row kinds and ownership/ancestry distinguishable, with consistent mark
  columns and connectors. Preserve a selected row across refresh/sort, and
  handle missing/reused process identities without silently selecting another.
- Prioritize useful details: exact identity, command/cwd, age, parent/owner link,
  task phase and actionable diagnostic. Unavailable optional fields should not
  create walls of placeholders.
- Check truncation, footer wrapping, confirmation/outcome space, details width
  and scrolling. Necessary actions and warnings must remain visible at small
  sizes; persistent outcomes must not swallow the fleet view.
- Compare concrete render/mockup alternatives before changing defaults. Retain
  user-controlled colours, glyphs, motion and view choices rather than adding
  another arbitrary theme or many new toggles.

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

## Known-process enrichment

The first consumer is an existing background-task row, not a new OS hierarchy.
Enrich known tasks and currently observed foreground/agent processes with local
facts. Child processes matter because a shell or launcher often does little work
while its payload consumes resources. Preserve the existing agent/task ownership
joins; OS parentage must never invent an assignment or owner.

### Mechanisms and first-change scope

- Linux `/proc/<pid>/stat`: process birth ticks, PPID, process group/session,
  kernel state, user/system CPU time and RSS. `/proc/<pid>/status` can provide
  thread counts; cwd/executable symlinks are existing observation patterns.
- CPU percent comes from two samples of the same birth identity and monotonic
  elapsed time, not a single counter. Use one-core accounting (100% = one core;
  multithreaded work may exceed it) and keep the first sample unavailable.
- Report root CPU/RSS and an explicitly labelled live-descendant aggregate plus
  child count. Sum RSS is not unique memory: shared pages may be counted more
  than once. Do not add cumulative waited-child CPU to live-child counters.
- Kernel state and an optional readable wait-channel hint describe observations,
  not a verdict that work is hung. Sleeping, quiet output and low CPU do not
  establish a stalled task. Blocking permission/process races yield unavailable
  metrics, not failed fleet collection.
- Start with CPU, RSS, state and descendant count in existing task/command
  details. Avoid extra rows, inline counters or a broad TUI redesign. Disk I/O
  rates (`/proc/<pid>/io`), historical peaks, alerts and cgroup accounting are
  separate follow-ons unless required to cover the chosen workload.
- No generic process browser, system-wide monitoring view, persistent analytics
  database, session-token analytics or new control operations in this change.

### Descendant tree (landed)

Active change: none. The change is implemented, verified and archived as
`openspec/changes/archive/2026-10-08-foldable-process-descendants/`. The sampler
now carries confirmed per-member name, birth identity, parent identity, state,
CPU and RSS beside the qualified sums, and the Processes page draws them as an
initially collapsed table with a root anchor, deterministic preorder, aligned
name/PID/CPU/RSS columns and a labelled `t` mode for selection and branch
folding. Rows are ordered deterministically and their observed names sanitized;
selection and folds follow birth identity, never PID alone.

- Rejected: mapping an external `pstree` into the panel. Its output carries no
  usable identity for selection and arranges the same text with no metrics.
- Rejected for now: adopting `tui-tree-widget`. Radar already rolls its own tree
  (`TreeNode`, stable row identities, its own row rendering); the widget would
  supply selection and folding but neither the sampled data nor the column
  layout, so it earns a dependency only if that folding work proves costly.
- The tree stays beneath the selected row's own process. It is not a system-wide
  process browser, and root and descendant figures remain drawn apart rather
  than added together.

### Identity/coverage blockers (publisher report, 2026-10-06)

`pi-bash-processes` records optional `ManagedTask.procIdent = {pid,startToken,comm}`
asynchronously immediately after spawn. Linux `startToken` is decimal stat field
22, not wall-clock task `startedAt`; fallback is `ps lstart`. `comm` changes on
exec and is diagnostic only. The current task bus omits this identity.

- Extend the publisher/bus contract to advertise the actually captured optional
  birth identity, including a format discriminator and Linux boot scope. Ensure
  identity-capture completion publishes an update. Do not retrofit a current
  `/proc` token as if it had been captured at spawn.
- Publisher confirmation: `outputBytes` and `lastOutputAt` update on captured
  stdout/stderr chunks and publish through a coalesced 200 ms output refresh;
  state changes also publish. This is not a heartbeat or delivery guarantee.
  Captured output bytes are neither disk nor network I/O. CPU/RSS sampling stays
  independent and bus disconnection/freshness remains explicit.
- Legacy/missing identity and pid reuse must not attach metrics to an unrelated
  process. Show task metadata, with process enrichment unavailable, rather than
  guessing from PID/command/cwd. `startedAt` is not proof of process identity.
- `task.pid` is the directly spawned detached child (initial group/session
  leader), often a shell that may exec without changing PID/start ticks. For
  systemd resource controls it can be a launcher, not the service payload. A
  process-tree aggregate must state its coverage; complete payload accounting
  needs an explicit verified service/cgroup anchor, not guessed daemon ancestry.
- Finalized `review`/`flushing` task records must not follow a recycled PID.
  Preserve publisher phase/exit data even when live metrics are unavailable.
- No output-silence diagnosis from an old bus snapshot or mere quietness,
  even though continuously producing tasks refresh their captured-output counters.

### Architecture and validation

- Add a small off-thread sampler above the runtime seam. Its inputs are already
  joined, typed task/foreground anchors; outputs are measurements keyed by
  stable identity and sample time. Keep Linux reading in `src/procfs.rs` and
  reuse the existing process-age/binary facts rather than another platform layer.
- Feed task anchors from the connected task projection; do not make mux
  inventory depend on extension task records. One shared sampled parent map can
  support descendant attribution without exposing an arbitrary OS tree.
- Bound the snapshot and previous-counter cache to current anchors/observed
  descendants. Revalidate birth identities around collection; `/proc` is not an
  atomic snapshot, so distinguish partial coverage from zero resources.
- Task disconnect/replacement, process exit, permission denial and non-Linux
  hosts degrade enrichment only. Leave state authority and task joins untouched.
- Boundary tests should cover known root plus busy child, first/second CPU
  samples, PID reuse, counter reset, missing identity, process exit during reads,
  partial descendants, publisher disconnect and launcher-only coverage.

Expected work: one upstream identity-contract/publisher slice, then two or
three thin Radar slices (identity/decode plus sampled facts; task/foreground
integration and details; final regression gate). Complexity is moderate; full
systemd/cgroup payload coverage adds a separate scope. Implementation waits for
agreement on the identity contract, not a general Herdr/tmux audit.

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

1. **Further controls.** Focus, confirmed unmanaged close and owner-routed
   managed close/idle-worker restart are settled. Lead recovery, structured
   reasons/outcomes and stale-lead batches are prioritized above but still need
   owner contracts. New tabs, steer, interrupt and extend remain later decisions;
   task lifecycle remains outside the metadata bus.
2. ~~**Mouse capture** trades the terminal's text selection for clicks.~~
   Taken, and written down in the README. The open half is whether a bypass key
   is enough, or whether capture should be a setting.
3. **Configuration growth.** Every new role, animation, mark and eventually sort
   key adds to `config.toml` and to `--print-config`. Worth watching that it
   stays a file a person can read.
