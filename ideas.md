# Radar ideas and deferred work

Parked work, with its options, a rough priority and how confident we are in the
direction. Nothing here is committed scope. Active work lives in
[plan.md](plan.md); an item moves there when its trigger is met.

Priority: **P1** needed soon after the current cutover, **P2** when a trigger
fires, **P3** nice to have. Confidence is how sure we are of the direction, not
of the detail.

## Lifecycle and daemon

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Unfinished-operation set | (a) keep the full scan in `unfinished()`; (b) reducer-maintained set of unfinished request IDs, rebuilt on replay, exposed as a named view | P2 | high on (b) | Record volume makes the scan visible, or a second consumer needs it |
| Clear stuck unfinished operations | (a) operator command that marks an operation abandoned with a durable reason; (b) reaper after a deadline with visible outcome. Scoped as (a) in the `lifecycle-authority` change | P1 | medium | First real stuck record blocks a server stop |
| Two-phase request feedback and composed client operations | Today one call returns the durable record once the operation settles, so there is no ack and no callback. What the two phases need already exists: admission is durable before dispatch and readable (`mux.request.get`), completion is replay-safe by id, and `events.read`/`observation.get` with `wait_ms` is the change-wait primitive on the waiting lane. Options: (a) keep the shape and let the client compose the phases (current); (b) a *composed* operation — a durable parent record over ordinary step requests (same ids, replayable), returning an id immediately and completing asynchronously, so one client-facing request aggregates several internal actions without changing any handler's own scope; (c) a push callback, which needs subscription machinery the journal could back but the transport does not have. For (b): the step list and policy are fixed per operation type (no composition DSL — arbitrary sequence/all/any graphs are the workflow engine we avoid); conditions must be durable evidence-backed predicates, never snapshots, since a revision is volatile and epoch-qualified; the composition aggregates and never becomes a second account of a step's effect; waiting must not occupy a worker or a connection, and a pending composition must **not** count as in-flight for a stop drain. A completion signal needs adding either way: `write_settled` journals nothing, so only `lifecycle.*` transitions are events today — prefer a bounded `wait_ms` on the request read over a new event kind. Cautions: an ack is never authority (the record is), and a received ack must never be readable as dispatched | P2 | high on (a) | A consumer needs a composed operation (Herdsman's "launch a usable lead": spawn plus binding and registration verified) |
| Radar-owned assignment and execution relations | The assignment relation (who, since when, outcome) and executed topology become Radar lifecycle state, expressed as daemon commands (assign, release, complete) that Herdsman calls; assignment details and agent-observed execution facts stay external evidence with freshness. Principle recorded in `lifecycle-authority` | P2 | medium | Herdsman's assign/release flow is ready to be expressed as daemon commands |
| Restart rollback gap | (a) accept and reconcile on next start (current); (b) compensating successor record | P3 | medium | A failed successor write is observed in live use |
| Restart and recovery live acceptance | Disposable tmux/Pi run: daemon kill, TUI quit, server replacement, lost response | P1 | high | Live run (user) |
| Tracing | `tracing` with journal output for the daemon and file output for the TUI (never the terminal); spans keyed by request, subject and operation IDs; the typed event log stays authoritative. Folded into the `shared-observation` change | P1 | high | Shared observation lands (sampler timing needs diagnosing) |
| Native systemd ordering and switch behaviour | `radar-tmux@` carries `X-SwitchMethod = "keep-old"` (verified with `sd-switch --dry-run`: the default plans Stop/Start of the template instance, which kills the server and its panes; `keep-old` keeps it). The daemon unit deliberately keeps the default, so a switch that changes its `ExecStart` restarts it, and the socket unit is untouched, so the listening socket and its queue survive the gap. A stop drains first (3 s), refusing requests that arrive while stopping. Remaining: verify on a live switch, and add `Wants=`/`After=` for daemon edges | P1 | high | Live switch on a real machine |
| Checked Home Manager evaluation (daemon-service 6.3) | (a) add a Home Manager input used only by a check; (b) rely on live acceptance (current choice) | P3 | medium | Module option wiring breaks unnoticed |
| Non-Nix unit generation | (a) keep Nix as the only module (current); (b) generate FHS unit files from the flake in CI | P3 | medium | A non-Nix install is actually needed, or packaging diverges from the module |
| tmux control mode observer | Read-only, output-free control-mode client per session as wake source for the sampler; verified on tmux 3.7c, no 3.8 needed. Change `tmux-control-mode` | P1 | high | After `shared-observation` |
| Tmux 3.8 shell-boundary events | OSC 133 pane-command events and `set-hook -B` monitors as more wakes plus separate typed shell-boundary evidence; keep reconciliation as the recovery path; 3.8 required, no capability gate | P2 | medium (not testable locally, tmux 3.8 not installed) | tmux 3.8 available for isolated verification |
| Process evidence row | pidfd/procfs as a separate evidence lane beside tmux events | P2 | medium | Tmux 3.8 observation lands |
| Shared daemon-owned sampled observation | Daemon sampler producing an immutable snapshot (revision, collection time, per-source freshness); `observation.get` with `since_revision` and "not modified"; sampling only while subscribed; tree, fold, selection and filter stay in the client; full snapshots first, deltas deferred. Slices: (1) sampler plus read, TUI switched with no fallback; (2) demand-driven, event-woken scheduling; (3) tmux 3.8 monitors feed the sampler. Open decisions: per-client foreground/focus evidence, wire size; measure registry-read cost at N agents first. Publisher ingest is daemon-owned through `tasks.list.v1`; the old publisher socket remains only as a temporary compatibility entry. | P1 | medium | Sidebar needs it; registry read re-verifies processes on every call |
| Agent status surface for a lead | A bounded `status` read per agent for a lead to replace the pane-text `inspect`: state, context, run time, last N tool names and intents (no output), optional recent messages or recap. Needs a new producer-published channel, since no tool, intent or recap fact exists today; daemon composes it with lifecycle, execution, facts and assignment as a named view of the shared observation. Footguns: producer claims are not verified, privacy of intents and messages, bounded retention, who may inspect whom (same-UID trust today) | P2 | low | Shared observation lands and a lead needs to supervise children without reading panes |
| Bus merged into the control socket | A `tasks` lane on the control socket with the bus's volatile, connection-scoped semantics; `pi-bash-processes` migrates; the compatibility bus socket and `extension-bus` spec are removed. `consolidate-daemon-ingest` leaves the lane, state and read in final shape | P2 | high | pi-extensions is out of the Herdsman cutover |
| Log retention and snapshots | Compaction and periodic snapshots for the durable event log, per-domain schema versioning | P1 | high | Prerequisite for any new event domain (messaging, tasks); log is unbounded today |

## Query and filtering

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Generic query layer for daemon reads | (a) typed named predicates per read with one shared scan helper (current direction); (b) embed the `cel` crate (cel-rust, 0.15.0 per docs.rs) | P3 | medium | A second consumer needs structured predicates |
| Structured TUI filters | (a) keep text-only substring match with ancestors kept (current); (b) typed fields such as state or assignment; (c) fuzzy matching (nucleo or fuzzy-matcher, not verified) | P2 | medium | Operators need to filter by state or assignment, not just text |

## Explicit child-pane gathering

Deferred: `/radar gather-children` could move live child panes into the lead's
current tab using tmux `join-pane`/`move-pane`, without respawning them. Radar
would own selection and movement; pi-radar would expose the operator command.
These commands preserved pane ID, PID, process birth identity and PTY in an
isolated tmux 3.7c experiment.

Do not implement this in `unified-pi-registration`. Default behavior is no
movement: a lead manually resumed elsewhere retains its session subject,
child links and inbox identity. Keep cross-tab presentation minimal; no new
layout manager, cross-tab nesting redesign or command stub. Radar-controlled
restart stays anchored to its existing pane/tab where available; a future
resume action should preserve placement too.

Priority: **P2**. Trigger: operators actually need to gather a resumed lead's
children. A future implementation must report partial moves honestly rather
than imply that gathering several panes is atomic.

## TUI and navigation

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Tab row for container actions | Carry the tab's container identity on its row so tab-level delete and reload have a durable target | P1 | high | Section 4 of `daemon-managed-lifecycle` or managed-topology tab actions |
| Primary pane for non-agent windows | Rule for choosing the tab identity when a window has no agent | P2 | low | A window without an agent is confusing in the tree |
| Recipe input in the TUI | (a) keep recipes out of the TUI (current, recipe is external); (b) a typed command line with explicit argv entry; never shell-parsed | P3 | medium | Operators need to create panes without a recipe file |
| Container rename and move | Labels only first; no layout move | P3 | medium | Daily use shows the label is wrong |
| Layout templates per workspace | Reuse tmux's layout engine rather than building one | P3 | low | Repeated manual layout recreation |
| Client-local navigation | Per-client selected window and view state, given that tmux window selection is shared by clients on one session | P1 (part of the sidebar prototype) | medium | Sidebar prototype, plan.md Priority 1B |
| Sidebar host | Kitty outer split or nested tmux shell, compared on disposable resources | P1 (prototype) | low until measured | Sidebar prototype |
| Structured activity, reasons and outcomes | Typed reasons on execution and assignment rows | P2 | medium | Operators cannot tell why a row is idle or blocked |
| Known-process enrichment and descendant tree | Mechanisms recorded in plan.md | P3 | medium | Resource questions in daily use |
| Optional systemd workload accounting and controls | [Exploration](docs/systemd-resource-management.md): shallow per-agent-execution slices containing a main scope and independently managed background-task scopes; subagents have separate slices. Logical ownership remains in Radar. Accounting first; no automatic freezing or ownership-driven migration. No implementation scope yet | P2 | medium; launch compatibility unverified | A concrete agent/task accounting or control need warrants the isolated feasibility experiment |
| Preview and search | Peek into pane output | P3 | low | Repeated need to see output without focusing |

## Herdr removal and housekeeping

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| `remove-herdr-backend` change | Remove backend code first; display labels and `pi_herdsman_*` tokens later, tokens only after Herdsman cutover acceptance | P1 | high | Daemon-service committed (done) and cutover accepted |
| Remove typed backend-primitive gates | Once Herdr is removed, delete `Primitives` and its preflight gates if tmux is the sole backend; keep public `Contract` names and revision negotiation | P2 | high | `remove-herdr-backend` lands
| Archive finished changes | `contract-v2`, `window-tabs`, `managed-topology` still unarchived | P1 | high | Next housekeeping pass |
| Supersede `managed-lead-restart` | Mark superseded by `daemon-managed-lifecycle` section 3; its Herdr readiness contract no longer applies | P1 | high | Next housekeeping pass |
| Socket-level enrollment and restart refusal tests | Lifecycle 3.1 to 3.3 socket tests, including malformed enrollment requests | P1 | high | Before section 4 wires restart |

## Rust crates

| Crate | Verdict | Priority | Confidence | Trigger / caveat |
| --- | --- | --- | --- | --- |
| `rustix` | Folded into `consolidate-daemon-ingest`. Replace the 51 `libc::` references in 10 `src` files (pidfd open/signal, poll, termios, sysconf, uid, `O_NOFOLLOW` opens) in `process_control.rs`, `procfs.rs`, `store.rs`, `registry.rs`, `server.rs`; `sigaction` stays or moves to `signal-hook`; API coverage not fully checked | P1 | medium | Part of `consolidate-daemon-ingest` |
| `tracing` | See Tracing above | P1 | high | With shared observation |
| `specta` 2 / `schemars` | Generate contract types for Herdsman (TypeScript) and validate Python fixtures; unchecked: how many wire shapes are typed serde structs, specta 2 release status | P2 | low | Contract drift between Rust and consumers causes a bug |
| `tokio` | Adopted in the daemon transport only (`consolidate-daemon-ingest`): handlers stay synchronous on a bounded blocking pool; control mode and waits build on it | P1 | high | Done in that change; do not convert domain code |
| `nucleo` | Fuzzy TUI filtering | P3 | high | See Structured TUI filters |

## Long-term

| Item | Options | Priority | Confidence | Trigger |
| --- | --- | --- | --- | --- |
| Messaging as an event-bus projection | New message domain (sent, delivered, acknowledged) on the typed log, mailboxes as projections, subscriber cursors already exist. Hard part is authority: Herdsman owns mailbox and assignment today, so moving it is a contract migration. Needs log retention, schema versioning, backpressure and per-subject access rules. Delivery, acknowledgment and process exit stay separate facts | P3 | low | Log retention done and a concrete need to move mailbox authority |
| Daemon-managed background tasks | A task is a subject kind on tracked launch plus verified stop (both exist) with output capture. Hard parts: output retention and descendants (daemon refuses to claim descendant termination). Completion notification depends on messaging; pull-only until then. Today rows come from the pi-bash-processes bus | P3 | low | Messaging exists, or a task needs supervision beyond display |

Principle: the TUI is an optional display with no ingest and no authority; anything it still owns (bus listener, reconciler, `/proc` sampling) moves to the daemon.

Order: consolidate daemon ingest, shared observation, tmux control mode, sidebar, log retention and snapshots, messaging, then tasks.

## Deferred by decision (see plan.md)

- Generic asynchronous work, job supervisor, notification broker, durable inbox and
  universal wait coordination. Deferred until a separate migration proves a gain.
- Lead recovery and stale restarts beyond the operator-confirmed TUI path.
- Pi program status and OSC 7501 for Pi state. No native tmux support even in 3.8.
- Upstream Herdsman and pi-bash-processes contract changes, which are owned by
  the pi-extensions side.

## Publishers must publish `context_window`

`facts.context_usage` text is no longer accepted: the facts channel takes `context_window { tokens, window }` integers and refuses anything else. The Pi extension (pi-extensions) and Herdsman must publish it before their next release or their whole facts snapshot is refused. The Herdr `context_usage` token stays a percentage-only legacy input.

## Lifecycle-authority acceptance still owed

The `lifecycle-authority` code is in place; these are the verification gaps that
remain, deliberately deferred rather than claimed:

- **Crash injection at each durable boundary.** There is no shared transaction
  across the operation store, spawn edges and the lifecycle log, so the designed
  write order (`openspec/changes/lifecycle-authority/design.md`, decision 6a) is
  unverified. Inject a crash after each durable step, reopen the daemon, and prove
  accepted intent stays visible, a possibly-dispatched launch is never repeated,
  and a partial record stays unresolved.
- **Beyond-one-page sweeps on a real fleet.** Fair paging is unit-tested; a real
  registry or container set larger than one page is not.
- **Event coalescing versus expiry.** `events.read` already answers
  `cursor_expired` with no partial sequence; a test that committed semantic events
  are either delivered or explicitly expired is still owed.
- **Live acceptance.** PTY/tmux managed actions, live Pi marker persistence across
  restart and reload, two-display and no-display coverage, and the Herdsman
  cutover to the shared runtime handle.
- **`observation.get` still serializes internal types.** `server::read_observation`
  returns the observation snapshot directly instead of through dedicated
  Schemars DTOs, so the public observation contract is not yet separated the way
  the rest of the protocol is. Small follow-up change.
