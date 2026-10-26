# UI session architecture

These are confirmed design decisions for the planned shared-session architecture, not claims that it is already implemented. Protocol and migration detail live in the [core design](../openspec/changes/shared-ui-session/design.md) and [UX design](../openspec/changes/tui-interaction-ux/design.md).

## Canonical interaction authority, not synchronized Apps

**Id:** 3abf8b19-768b-495f-8097-3566d4228855
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer architecture discussion leading to the OpenSpec proposals, 2026-10-26
**Verification:** corroborated — recorded in the shared-ui-session proposal and design; implementation is pending
**Revisit when:** presentations no longer need shared navigation or command continuation

One daemon-hosted reducer owns main-tree navigation and command continuation. Existing domain services retain lifecycle authority. Geometry, viewport and detail-panel interaction stay local; local input does not need to enter the actor protocol. Bounded Tokio channels connect the owner to existing services; dedicated-thread scheduling is recorded separately below.

**Reason:** sharing fleet facts alone does not preserve an interactive command across sidebar, dashboard and popup processes. Shared navigation needs one interpretation, but differently sized presentations do not have equivalent layouts or detail controls.

**Rejected alternative:** synchronize fields between independently executing Apps. That retains competing navigation/workflow logic and leaves continuation dependent on presentation lifetime.

**Rejected alternative:** move every App field and terminal event into the daemon. That couples canonical state to presentation-specific geometry and controls without solving an additional requirement.

## One modal presenter submits results directly

**Id:** 4fe50d45-6c7c-4b0a-a244-657ad3f833c5
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer architecture discussion leading to the OpenSpec proposals, 2026-10-26
**Verification:** corroborated — recorded in the ui-interactions delta and core design; implementation is pending
**Revisit when:** a command needs multiple simultaneous presenters or a new presentation host
**See:** ui-session.md#canonical-interaction-authority-not-synchronized-apps — 3abf8b19-768b-495f-8097-3566d4228855 — as of 2026-10-26

A shared command interaction blocks the session and has one presenter, whose query/highlight/editor mechanics remain private. Inline owners and detached presenters send typed, interaction-addressed results directly to the session. Help, error details and ordinary command browsing remain local rather than creating a shared gate.

**Reason:** the shared concern is the command awaiting a choice, not every step of choosing. A tmux popup supplies a terminal for a process that can use Radar IPC; its exit status need not carry a selected value. Logical waiting must leave the actor responsive to cancellation and facts.

**Rejected alternative:** reconstruct an independent popup App or return the choice through the parent process. Those make continuation depend on the parent's state/lifetime and risk target substitution.

**Rejected alternative:** mirror picker mechanics or globally block on every informational overlay. Neither is needed to coordinate one shared command.

## Live filtering has one editor

**Id:** f8bdecb0-385f-4e39-8efe-8d26d8f9bf8c
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer architecture discussion leading to the OpenSpec proposals, 2026-10-26
**Verification:** corroborated — recorded in the ui-session delta and core design; implementation is pending
**Revisit when:** independent concurrent editing becomes an actual product requirement
**See:** ui-session.md#canonical-interaction-authority-not-synchronized-apps — 3abf8b19-768b-495f-8097-3566d4228855 — as of 2026-10-26

The filter is shared and live as typed, with at most one editor owner. Caret mechanics stay local and other views mirror the query and canonical results. Releasing ownership does not implicitly erase the query.

**Reason:** live narrowing is the desired interaction; concurrent text merging is not a requirement. A single editor preserves a coherent tree without suppressing live feedback.

**Rejected alternative:** publish only committed queries or keep a local preview until commit. That adds a second temporary navigation/selection context and is not justified merely to avoid per-edit local IPC.

**Rejected alternative:** allow concurrent shared editing and reconcile text. That solves a collaboration problem outside the current scope.

## Content identity is separate from publication order

**Id:** 2d9f29d7-a939-4f95-96b9-c877530eb4a5
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer architecture discussion leading to the OpenSpec proposals, 2026-10-26
**Verification:** corroborated — recorded in the ui-session delta and core design; implementation is pending
**Revisit when:** payload retention must become durable or content-addressing cost is demonstrated to be material

Tree and session payloads have full-content hashes, separate from the actor's ordered publication identity. Current/recent immutable payloads are cached by hash with bounded retention and current content pinned. Expired references recover through the current snapshot.

**Reason:** selection-only updates need not resend the tree, and A→B→A can reuse A rather than allocate a new content identity. Every actual payload field counts; publication bookkeeping is outside those payloads.

**Rejected alternative:** separate changing tree revision numbers as the sole content identity. They identify versions but do not naturally recognize reused content. Full snapshots on every navigation action also repeat unchanged data unnecessarily.

**Consequence:** hashes do not avoid initial projection work by themselves. Known-unchanged inputs can skip work; hashing detects identical output afterward. The cache is not an unbounded event history.

## Attachment identity fences reconnect; counters order each direction

**Id:** 94272329-1ea5-4bf4-a01c-f138f9df8118
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer architecture discussion leading to the OpenSpec proposals, 2026-10-26
**Verification:** corroborated — recorded in the ui-session delta and core design; implementation is pending
**Revisit when:** ordered submission or snapshot delivery is replaced by a different transport model
**See:** ui-session.md#content-identity-is-separate-from-publication-order — 2d9f29d7-a939-4f95-96b9-c877530eb4a5 — as of 2026-10-26

A fresh attachment UUID plus a client counter identifies submissions. The actor has an independent increasing publication counter. Reattachment establishes the current snapshot and a new client sequencing baseline; it does not reset canonical state. Gaps are explicit and recoverable, not silently skipped. Bounded gap buffering is deferred.

**Reason:** both directions need ordering, but client counters need not be comparable across clients. Fresh attachment identity already fences old responses across reconnect, including a frontend surviving daemon restart.

**Rejected alternative:** an additional actor epoch solely for reconnect fencing. The attachment boundary already supplies that identity.

**Rejected alternative:** use precise wall-clock timestamps as the ordering guarantee. Actor-enforced monotonic timestamps could work, but counters express order directly without adding clock behavior. Unique per-publication UUIDs also add no needed function beyond the chosen counters.

## Retain Ratatui and improve the frontend selectively

**Id:** 2ebd5e0e-f862-4441-9edf-e41dfb772e15
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer framework comparison and accepted sliced plan, 2026-10-26
**Verification:** corroborated — both OpenSpec proposals retain Ratatui; dependency evaluations are still pending
**Revisit when:** a required interaction cannot be delivered cleanly by the selected Rust frontend stack

Ratatui remains the frontend baseline. The shared model stays independent of terminal key types, while action/keybinding tools are evaluated early and richer widgets are adopted for concrete UX needs.

**Reason:** Bubble Tea fits the architecture, but did not establish a decisive UI-capability advantage. Better state communication and selected Rust components can improve the current UI without changing the frontend language/runtime.

**Rejected alternative:** pivot to Bubble Tea v2 during session extraction. Architectural compatibility alone did not distinguish it sufficiently from Ratatui plus an explicit event/input boundary.

**Consequence:** Strum/tui-actions evaluation is planned, not completed adoption. Candidate widget crates and animation libraries are not an approved dependency bundle.

## Settle shared interaction before choosing the sidebar host

**Id:** 920405ce-9453-4066-ab6c-176f98826390
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer hosting discussion and accepted implementation order, 2026-10-26
**Verification:** corroborated — core ownership work precedes hosting comparisons in the proposal
**Revisit when:** shared-session and detached-presenter milestones establish the evidence for a hosting decision
**See:** ui-session.md#one-modal-presenter-submits-results-directly — 4fe50d45-6c7c-4b0a-a244-657ad3f833c5 — as of 2026-10-26

Radar needs host control capabilities, including full-window presentation, not necessarily ownership of terminal rendering. The shared-session boundary is settled before selecting between the current nested host, native per-window clients, a movable sidebar or a future embedded frontend.

**Reason:** host-managed popups are acceptable. Several observed popup problems come from independent App/continuation ownership, which changing terminal engines would not itself fix. A shared session makes hosting a presentation choice.

**Rejected alternative:** build a custom terminal frontend, replace tmux with RMUX/Zellij, or fork an integrated mux as a prerequisite to coherent interactions. Those remain possible future directions but are not necessary to solve the current ownership problem.

## Isolate actor scheduling on a dedicated thread

**Id:** b60d09ec-26d3-4266-bafd-e7baebe087ca
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer scheduling clarification, 2026-10-26
**Verification:** corroborated — recorded in the core design and milestone B tasks; implementation and latency measurements are pending
**Revisit when:** measured queue/render latency or thread lifecycle cost contradicts this separation
**See:** ui-session.md#canonical-interaction-authority-not-synchronized-apps — 3abf8b19-768b-495f-8097-3566d4228855 — as of 2026-10-26

The shared-session actor has one dedicated thread with a synchronous mailbox loop, bounded channels to the existing daemon runtime and domain effects executed elsewhere. Async actor execution remains optional and measurement-driven, rather than a prerequisite for event-driven frontends.

**Reason:** single ownership is a natural scheduling boundary, and UI transitions need not compete with unrelated transport tasks on the same Tokio workers. This is isolation, not a measured claim of native-equivalent latency.

**Rejected alternative:** initially run the actor as another task on the shared transport runtime and introduce thread isolation only after contention appears. The explicit owner thread is preferred as the baseline rather than a later scheduler refactor; no separate daemon or actor framework is needed.

## Replay acceptance without claiming domain execution

**Id:** dbd0cb78-2dec-4154-9e6f-bbbddf7793c8
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** accepted receipt and dispatch-failure review, 2026-10-26
**Verification:** corroborated — core specs/design/tasks carry the refined semantics; implementation is pending
**Revisit when:** receipt retention or the domain admission/replay contract changes
**See:** ui-session.md#attachment-identity-fences-reconnect-counters-order-each-direction — 94272329-1ea5-4bf4-a01c-f138f9df8118 — as of 2026-10-26

Identical retained action retries return the original acceptance and its established interaction/request identities. Expired receipts require reconciliation. Effect preparation, dispatcher acceptance, possible submission and domain admission remain distinct; a full/stopped dispatcher resolves pending UI work with a typed failure, not a fabricated operation outcome.

**Reason:** an action can be deduplicated yet leave the caller unable to recover the interaction created by its lost reply. Queue acceptance can likewise succeed without the domain operation being admitted. Both require truthful correlation, not a second execution path.

**Rejected alternative:** remember only the last action number and answer already-processed, or treat a queue send/task spawn as domain success. The former loses the original continuation reference; the latter hides scheduling failure and submission uncertainty.

**Consequence:** receipts remain bounded/volatile, later workflow status is separate from original acceptance, and uncertain effects still use existing durable request IDs rather than a UI exactly-once journal.

## Wake from events and render the latest state

**Id:** 8c31790a-7bf0-4af0-ad07-12245cccbd26
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** accepted latency/IPC review, 2026-10-26
**Verification:** corroborated — core specs/design/tasks require wakeups, reuse and coalescing; latency remains unmeasured
**Revisit when:** latency measurements identify a different bottleneck or a transport requirement changes
**See:** ui-session.md#isolate-actor-scheduling-on-a-dedicated-thread — b60d09ec-26d3-4266-bafd-e7baebe087ca — as of 2026-10-26

Frontend input, updates and relevant completions wake processing directly. Available snapshots can collapse to one latest coherent render, but action receipts, effects and input-gate invalidation remain independently processed. Reusable action and update-read connections are separate, so a long read cannot hold up ordered input. No default input debounce or batch-fill delay is selected.

**Reason:** fast actor processing is not enough if incoming results wait behind an idle terminal poll or pending serial read. Rendering every superseded frame adds work without improving feedback. Cached row/index navigation and separate payloads remove avoidable repeated computation.

**Rejected alternative:** retain periodic state-application polling and draw every intermediate snapshot. Those introduce unnecessary waiting/render work. Async actor execution, projection workers and deltas are not assumed fixes; they need measured benefit first.

**Consequence:** event-driven behavior alone does not require async execution. The frontend mechanism was subsequently selected separately below; the actor stays synchronous. A later projection worker, if justified, must fence all relevant observation/query/fold/order/visibility inputs rather than accept stale builds.

## Inspect events without flattening their authority

**Id:** 2f1d8c62-8350-4b92-b487-a77972dbc8e7
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** accepted operation/event inspection review, 2026-10-26
**Verification:** corroborated — UX proposal/spec/design/tasks include bounded event browsing; implementation is pending
**Revisit when:** exhaustive subject history or new diagnostic sources become required

The local inspector includes relevant retained lifecycle events alongside operation records, linked by established subject/execution/request identities. Recorded events, operation outcomes, observation-derived status and available diagnostics remain visibly different sources. Retention and cursor gaps remain explicit.

**Reason:** activity investigation needs more than UI command outcomes, but timestamps and diagnostic text do not establish lifecycle transitions or complete history. Existing event/request reads supply a bounded useful view without another canonical workflow or journal.

**Rejected alternative:** limit the inspector to pending/completed UI outcomes. That omits recorded lifecycle activity relevant to the selected subject. Expanding inspection does not justify implying complete history or correlating unrelated sources by timestamp proximity.

## Project candidates do not depend on a working directory

**Id:** 2b012940-14cf-4d4d-b694-7d1a3a77d032
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer decision after checking zoxide behavior, 2026-10-26
**Verification:** corroborated — `zoxide query -l` output was identical from `/tmp`, the project directory and inside its own top entry (zoxide 0.10.0, this machine); the current leading start-directory entry is in `src/project.rs::select`
**Revisit when:** discovery gains a directory walker, several merged sources or ranking beyond the source's own order
**See:** ui-session.md#canonical-interaction-authority-not-synchronized-apps — 3abf8b19-768b-495f-8097-3566d4228855 — as of 2026-10-26

The picker offers the configured source's accepted directories in source order, with no leading entry for the requesting process's start directory. A configured base directory, defaulting to home, is offered only when the source yields nothing. The source still runs in the invoking frontend's environment.

**Reason:** the daemon moving into the path made "the directory Radar was started in" ambiguous between daemon and frontend, and the only cwd-dependent behavior was that leading entry. Zoxide itself does not read the working directory, but it does read environment such as its data directory, exclusions and PATH, which can differ in a service.

**Rejected alternative:** keep the start directory as an explicit input sent by the frontend. That is deterministic but lets different frontends lead with different directories and keeps a selection-like dependency in a flow that otherwise has none.

**Rejected alternative:** add a Radar directory walker and a `[tui.projects]` schema now. The source command is already the extension point; a walker needs its own timeouts, cancellation and exclusions. Pinned/recent/zoxide merging and frecency are deferred, not rejected.

**Consequence:** the default highlight is the source's first entry rather than the start directory, a deliberate change from the current bare-Enter behavior.

## Adopt the async frontend driver during session integration

**Id:** 219b7571-b656-4dc1-962d-08345252e783
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer approval of frontend runtime selection, 2026-10-26
**Verification:** corroborated — core proposal/design and milestone B task specify the driver; implementation is pending
**Revisit when:** integration exposes a concrete terminal ownership or runtime constraint
**See:** ui-session.md#wake-from-events-and-render-the-latest-state — 8c31790a-7bf0-4af0-ad07-12245cccbd26 — as of 2026-10-26

Use Tokio select! with Crossterm EventStream in Slice B. One task owns synchronous Ratatui rendering; socket workers deliver local notifications and an ordered outbound worker submits actions. The daemon actor remains on its dedicated synchronous thread.

**Reason:** terminal events, session publications, completions and conditional timers are independently changing sources. The async frontend provides a direct wakeup/ownership model while those sources are being integrated.

**Rejected alternative:** build a synchronous input-reader/common-inbox frontend in B and defer possible EventStream conversion to D. That can satisfy the behavior, but would leave a likely second event-loop rewrite immediately after the ownership migration.

**Consequence:** no slow RPC or subprocess may block the terminal owner, no concurrent task renders, and reconnect/closed-channel and terminal-cleanup paths are explicit. This selection does not justify async actor execution or background projection workers.

## Palette choices are tree commands regardless of panel focus

**Id:** cf522961-3555-48e4-833e-03659ebebb10
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer answer during Slice A implementation, 2026-10-26
**Verification:** corroborated — `app::tests::a_palette_choice_is_a_tree_command_even_while_details_hold_the_keyboard` passes; key presses keep the details-panel precedence
**Revisit when:** a palette command needs to act on the details panel, or panel focus becomes part of command availability

A palette choice runs its command by identity on the selected tree row, whichever panel holds the keyboard. Key presses keep the existing panel precedence: the details panel still answers its own keys first.

**Reason:** the palette replayed a key through the details handler, so Expand, Enter and the arrows were absorbed by page navigation, and Restart or Delete did nothing. A chosen label should mean what it says.

**Rejected alternative:** keep the replay so the palette inherits the key's precedence. That preserves the accidental behaviour and keeps key identity in the command path.

**Rejected alternative:** return focus to the tree before running the command. It works, but adds a focus transition the user did not ask for and would hide the difference from key presses.

**Consequence:** the same key and the same palette row can differ while details hold the keyboard. That is deliberate: a key is a local gesture, a palette row is an explicit choice.

## Dashboard keys resolve through a typed command table on tui-actions

**Id:** e956f22c-f814-4061-a4d0-cc58aeef9897
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer request to spike Strum and tui-actions, followed by the Slice A1 implementation, 2026-10-26
**Verification:** corroborated — `app::commands` tests cover case-sensitive resolution, the BackTab alias and table completeness; the throwaway spike bound all 29 chords without conflict
**Revisit when:** the keymap becomes user-configurable, or a sequence feature needs pending-chord state that the app must own

`CommandId` is a plain enum whose variant names are the stable ids; Strum derives iteration and string parsing. `COMMANDS` binds each id to its chords, and `tui-actions` (pinned at `=0.1.1`) resolves a key to its id. Non-command keys (movement, Esc, `?`, `c`) stay in the `handle_key` match. Modifiers are ignored, as they were before.

**Reason:** the chord that runs a command and the chord shown in the palette must be the same value, and later sequence support should add bindings rather than rewrite dispatch. Spike findings: printable characters arrive already shifted, so SHIFT must be dropped for characters; BackTab must be mapped to its own key; `g` and `g g` coexist, and resolving `g` alone while `g g` is a prefix is an application decision; conflicts are reported at bind time.

**Rejected alternative:** keep a `match` on `KeyCode` and add a separate palette table. That is the duplication the table removes, and the next key-driven feature would repeat it.

**Rejected alternative:** Strum alone, or crossterm-keybind. Strum does not resolve keys; crossterm-keybind resolves single key events and has no multi-step chord model for the leader-style bindings planned later.

**Consequence:** pending-chord timeout, Escape cancellation, and clearing pending sequences at gate transitions remain application work and are not yet proven. Slice A's tui-actions task stays open until they are.
