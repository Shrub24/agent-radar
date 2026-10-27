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
**Verification:** corroborated — milestone C uses the same generic Picker in inline and detached roots; real handler tests cover direct results, origin loss, remote cancellation, receipt replay with one backend create, and presenter-stream loss
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

Frontend input, updates and relevant completions wake processing directly. Available snapshots can collapse to one latest coherent render, but action receipts, effects and input-gate invalidation remain independently processed. The reusable action/RPC connection and dedicated push subscription are separate, so waiting for publications cannot hold up ordered input; the subscription choice is recorded below. No default input debounce or batch-fill delay is selected.

**Reason:** fast actor processing is not enough if incoming results wait behind an idle terminal poll or pending serial read. Rendering every superseded frame adds work without improving feedback. Cached row/index navigation and separate payloads remove avoidable repeated computation.

**Rejected alternative:** retain periodic state-application polling and draw every intermediate snapshot. Those introduce unnecessary waiting/render work. Async actor execution, projection workers and deltas are not assumed fixes; they need measured benefit first.

**Consequence:** event-driven behavior alone does not require async execution. The frontend mechanism was subsequently selected separately below; the actor stays synchronous. A later projection worker, if justified, must fence all relevant observation/query/fold/order/visibility inputs rather than accept stale builds.

## Push latest UI state through a dedicated subscription

**Id:** 582f431c-95d9-4da0-ad27-1f2f89a1c2ba
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer approval of push subscriptions and Slice B scope, 2026-10-26
**Verification:** corroborated — core proposal, design and session/transport deltas specify push delivery; implementation is pending
**Revisit when:** consumers need durable transition history rather than latest coherent UI state
**See:** ui-session.md#wake-from-events-and-render-the-latest-state — 8c31790a-7bf0-4af0-ad07-12245cccbd26 — as of 2026-10-26

Slice B uses ui.subscribe on a dedicated connection: an authoritative baseline followed by pushed latest-state publications. The action/RPC connection remains independent. Async bounded writers isolate slow clients from the actor; retained addressed effects and action receipts are not disposable snapshot notifications.

**Reason:** a continuously attached UI is naturally represented by a subscription with explicit lifetime, backpressure and teardown. Choosing it before the frontend migration avoids a later transport rewrite. Subscription registration must cover changes after attach without a baseline gap; reconnect adopts current state, not a historical replay stream.

**Rejected alternative:** ui.read with bounded long-poll renewal. It can wake immediately and support an event-driven frontend; it was not rejected as inherently slow or incorrect. Push removes the idle renewal cycle and makes stream lifetime explicit.

**Rejected alternative:** stream through the existing blocking cloned writer or queue every publication per client. Neither gives the required nonblocking actor boundary and bounded slow-client behavior.

**Consequence:** this is not a generic event-subscription framework. Existing domain reads and tmux observation remain separate; durable domain subscriptions would need cursor and retention-gap semantics, not UI snapshot coalescing.

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
**Verification:** corroborated — `zoxide query -l` output was identical across working directories on zoxide 0.10.0; milestone C removes the leading entry in `src/project.rs`, adds `tui.project_base`, and tests source ordering, fallback and unusable-base diagnostics through frontend discovery
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

## Navigation is the shared model; geometry, details and process reading stay local

**Id:** 3c9a7e41-6d0b-4f58-9b1e-2a5c8f7d4e10
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer approval of the field list during Slice A3, 2026-10-26
**Verification:** corroborated — `src/ui_session/navigation.rs` holds the tree, folds, filter, pane view, task and finished toggles, order, selection and anchor with no terminal, geometry or details types; App delegates through one `navigate` step; the bus-task test fails when the owner fallback is removed
**Revisit when:** the session needs to own filter editing, or a frontend needs a reading position that must survive selection changes
**See:** ui-session.md#canonical-interaction-authority-not-synchronized-apps — 3abf8b19-768b-495f-8097-3566d4228855 — as of 2026-10-26

Selection, folds, filter, pane view, task and finished toggles, order and the selection anchor are shared. Terminal geometry, details page and scroll, opened blocks, the process cursor, filter editing mode, confirmations, notices and focus targets stay local to the frontend.

Moving the selection resets the local reading state (scroll, opened blocks, process cursor). Those belong to the row they were made in, so the reset is part of the selection step, not a separate call.

**Reason:** the shared fields are what two presentations must agree on, and the local ones depend on a particular frame's size or on what one reader has opened. Keeping geometry out makes the model testable without a terminal.

**Rejected alternative:** share the details page and scroll as well, so every frontend shows the same page. The sidebar and dashboard lay the panel out differently, so a shared scroll position would be wrong in one of them.

**Rejected alternative:** keep navigation inside App and extract only the projection. The projection is the easy half; selection, folds and the filter change together and would still need App's geometry to be correct.

**Borderline, kept local for now:** `focus_targets` and `source_current` are derived from observation and could move with the session in Slice B. `filter_editing` becomes the editor-owner state in Slice B, so it moves then rather than now.

## Chord resolution: what tui-actions provides and what the application owns

**Id:** 7f2e0b93-1d4a-4c6e-8a57-b0d9e3f61c24
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer question about pending-chord capabilities during Slice A, 2026-10-26
**Verification:** corroborated — read in tui-actions 0.1.1 `keymap.rs`: `is_prefix`, `complete` and `is_unbound` are stateless queries over a sequence of events; no pending buffer, timeout or Escape handling exists in the crate. crossterm-keybind 0.4.4's public `match_any` takes one `KeyEvent` and has no sequence model.
**Revisit when:** the first multi-key binding (gg, leader) is added, or tui-actions gains pending-state support
**See:** ui-session.md#dashboard-keys-resolve-through-a-typed-command-table-on-tui-actions — e956f22c-f814-4061-a4d0-cc58aeef9897 — as of 2026-10-26

tui-actions gives the chord grammar, conflict detection at bind time, and stateless queries: whether an event starts a multi-step binding, whether a full sequence completes one, and whether a sequence is unbound. The application owns the pending buffer, the sequence timeout, Escape cancellation, and clearing pending input when a modal gate changes.

**Reason:** this is why tui-actions was chosen over crossterm-keybind, which resolves one event at a time. The stateless queries are the part the crate can prove; the state machine is application behaviour that has no consumer until a multi-key binding exists.

**Consequence:** Slice A task 1.2 stays open. Its gg, leader, timeout, Escape and gate-reset checks move to the first sequence binding, where they can be tested against real behaviour.

## Command targets are derived and frozen in the session, not in the frontend

**Id:** abd5d0fc-46f8-42f9-bc42-26aa36898ee0
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer instruction to complete Milestone D with one writer per command, 2026-10-27
**Verification:** corroborated — `src/ui_session/targets.rs` is the only implementation of selection, containment, losses and subject routing; `App::begin_action`, `confirm`, `Confirmed` and the frontend `Closer`/`ContainerControl`/`LifecycleControl` controllers were deleted; `a_destructive_command_acts_on_the_row_its_view_displayed` fails if the frozen request follows the shared selection instead of the displayed row
**Revisit when:** a command appears whose target genuinely cannot be derived from the observation the daemon already publishes
**See:** ui-session.md#canonical-interaction-authority-not-synchronized-apps — 3abf8b19-768b-495f-8097-3566d4228855 — as of 2026-10-27

A destructive command carries the row identity its view displayed. The session looks that row up by identity, decides what the command acts on from the observation it already holds, and freezes the request — method, parameters, request identity, prompt and losses — before anything is asked or sent. Keys, the palette and the pointer all submit the same semantic command; none of them derives a target.

A confirmation is therefore a question whose answer cannot retarget it: the answer names the interaction, and the effect the session already froze is what the daemon dispatches. A deletion that loses nothing skips the question but is still one frozen effect.

**Reason:** a target derived from `App`'s cursor is only correct in the view that pressed the key. Once several views share one session, the same key in a second view would act on that view's selection while a dialog opened elsewhere still named the first. Freezing identity at invocation is what makes the shared session safe for commands that cannot be undone.

**Rejected alternative:** have the frontend build the request and submit it for dispatch. It keeps one code path per command in the frontend, which is exactly the duplication being removed, and it would leave eligibility — stale inventory, managed routing, unresolved containers — split across two implementations that can disagree.

**Rejected alternative:** keep `App`'s eligibility helpers and let the session call them. They read `App` state (cursor, geometry, local flags); the parts that are pure over the observation were moved into the session instead, and `App` now calls the same functions so its remaining paths cannot drift.

**Consequence:** a command that needs what only the invoking frontend knows is prepared, not dispatched: a configured launch submits its shell, `PATH` and command, and the session derives the place, the directory and the recipe. That is the only use of `Preparing` left besides project discovery.

**Consequence:** the effect runs through the same admission path a client request reaches, under the frozen request identity, so a retried acknowledgement cannot become a second effect.

**Open:** the forced repeat of a deletion the daemon held back now lives in the session, keyed by the container the refusal named; it is carried on the plan rather than in frontend state.

## Focus is decided by the session and performed by the daemon

**Id:** 5e31bdb0-5694-4f48-884d-d9a1c1182cb3
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer instruction that focus be abstracted away from the frontend, 2026-10-27
**Verification:** corroborated — `ui_session::targets::focus` derives the target from the session's own tree and observation; `SharedCommand::Focus` produces one frozen effect with `confirms: false`; `server::ui` executes a focus plan through `core.backend.focus_outcome`; the tmux adapter names the client with `switch-client -c` instead of relying on the caller's environment; `App::focus_targets` and `focus_selected` no longer exist
**Revisit when:** a workspace can be focused with no client attached, or a second client per server becomes supported
**See:** ui-session.md#command-targets-are-derived-and-frozen-in-the-session-not-in-the-frontend — abd5d0fc-46f8-42f9-bc42-26aa36898ee0 — as of 2026-10-27

`Enter` submits the row it is on. The session derives where that row focuses from the tree and observation it already publishes, freezes one effect, and the daemon performs it: it resolves the client attached to the target's session and switches that client. The frontend holds no focus targets and names no client.

**Reason:** focusability is a fact about the observation — is the row placed, is the location still there — and deriving it per frontend meant one view could allow what another refused, with the derivation written twice. Moving the effect too removes the last dependency on the calling process's environment: a daemon has no `$TMUX`, so `switch-client` without a named client would refuse or move the wrong terminal.

**Rejected alternative:** keep the effect in the frontend and share only the decision. It preserves instant feedback and the local "open a terminal when nothing is attached" recovery, but it keeps two implementations of the same tmux call and leaves the client implicit.

**Rejected alternative:** keep the derivation in the frontend and let the session dispatch. The frontend would still need the whole map to say which row it wants, which is the duplication being removed.

**Consequence:** staleness stays local. Whether the snapshot a view drew is current is its own reading, so a view refuses before it asks; the daemon does not re-derive it.

**Consequence:** a focus with no client attached is refused with `no_client` and the target it could not show, and the view that asked opens a terminal for that place and re-asks the same session command for the same row. The daemon cannot spawn a terminal, so the process that can is the one that asked, and it stays one command: the retry is a re-submission, not a second implementation of the decision.

**Rejected alternative:** accept the refusal and require a client to exist beforehand. It removes the recovery but loses the behaviour operators relied on, where focusing a workspace with no terminal opened one.

**Rejected alternative:** address a presentation effect to the attachment that asked, so the session drives the recovery. It is the general answer and it will be needed if a second client ever exists; today it buys nothing over the requester reacting to a refusal it already understands, and it needs addressing machinery the one-view model does not yet justify.

**Consequence:** one client per tmux server is now an explicit assumption in the adapter rather than an accident of environment inheritance.

## The session owns which rows exist; the view only explains them

**Id:** 1716cef7-0e2d-4868-94a5-9a31a31ac0a5
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** external implementation review, 2026-10-27, accepted as a slice of its own
**Verification:** corroborated — `Navigation::projected` joins the publication's rows to the local tree by identity and the tests pin both directions of authority: a projected row the tree lacks is drawn as structure, and a tree row the projection omits is not drawn at all; the renderer, selection index, pointer hit test and scroll extent all read `App::draw_rows`
**Revisit when:** the frontend needs row data the projection does not carry, or two views must render different row sets
**See:** ui-session.md#navigation-is-the-shared-model-geometry-details-and-process-reading-stay-local — 3c9a7e41-6d0b-4f58-9b1e-2a5c8f7d4e10 — as of 2026-10-27

The session's tree projection is the canonical row set, order and nesting. Every frontend draws those rows and hit-tests those rows. The local observation explains a row — its kind, marks, details, process facts — and never decides whether it exists. A row this view cannot explain is still drawn, from the label and marks the wire carries; a row the projection does not name is not drawn.

**Reason:** a locally rebuilt tree is a second projection, and two projections disagree about order, membership and folds exactly when it matters — a filter change, a fold, a task appearing, a row disappearing. The session exists to remove that class of divergence; leaving the draw path on a local tree left it intact.

**Rejected alternative:** send the rich row detail over the wire so the frontend needs nothing local. That makes the publication large and the session responsible for presentation facts it has no business owning, when a join by identity is already exact.

**Rejected alternative:** keep drawing from the local tree and reconcile the two with a third cache. The reconciliation would itself need a rule for which side wins, which is the decision this entry makes once.

**Consequence:** a projected row with no local detail is drawn plainly rather than richly. That is the honest rendering of "the session says this row is here and this process does not know it yet", and it is visible rather than silently dropped.

**Consequence:** details, fold markers and process panels still read the local tree by identity, so a row that exists only in the projection selects but shows no detail until the next observation.

## A subscription has no read timeout until its baseline arrives

**Id:** a629a11e-79fb-4378-b825-000905f3f4bc
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** a frontend that stopped re-attaching after a daemon restart, reproduced only under a loaded suite
**Verification:** corroborated — with the timeout in place, ten loaded control-plane runs were clean where three in ten had stalled; the attach trace showed the attempt parked in `read_line` on a subscription the daemon had accepted
**Revisit when:** the daemon can prove a subscription is being served before it writes, or the push loop needs a keepalive

The subscription connection keeps the ordinary reply read timeout until the first frame — the baseline — is in hand, and only then waits without one. A quiet fleet is not a failure, but an accepted-and-never-served subscription is, and it must not be mistaken for a quiet one.

**Reason:** without the distinction the attach thread blocks indefinitely on a subscription the daemon accepted and never answers: no error, no backoff, no retry, and the frontend never sees state again. That is a liveness defect that presents as a hung UI, not a failed connection.

**Rejected alternative:** a health check or keepalive on the push stream. It needs a protocol addition and a second way to say the same thing; the first frame is already the strongest possible evidence that the subscription is being served.

**Consequence:** a subscription that would have delivered its baseline after the reply timeout is abandoned and retried, which costs one attachment. The reply timeout is short enough that this is invisible unless the daemon is overloaded past it.

## Host acceptance is not tracked separately from the presenter claim

**Id:** 21eaeff8-c82e-4274-a401-8a58eeafc2a4
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** a follow-up review asking whether the published field drives any transition
**Verification:** corroborated — `host_accepted` had one writer (the acknowledgement action) and no reader anywhere in the tree; the acknowledgement action's only other effect was an ownership check that contradicted the rule that any view may cancel
**Revisit when:** something needs to distinguish "the host opened a presenter" from "a presenter attached" — for example an inspector that must explain a popup that never appeared

The protocol no longer carries an acknowledgement action or a field recording that the invoking host accepted a presentation. A presenter claims the interaction when it attaches, and a host that could not present reports `PresentationFailed`, which closes the interaction exactly as any other cancellation does.

**Reason:** state that nothing reads is not a guarantee, it is a claim about a guarantee. Keeping it invites a reader to assume some behaviour depends on it, and it costs a protocol action, a field and match arms in three places.

**Rejected alternative:** keep it as diagnostic state for the planned operation inspector. When the inspector exists it will want facts with an audience, designed against what it shows; carrying the field until then is speculative state, and a popup that never opened is already visible as an interaction that expired or was cancelled.

**Consequence:** the distinction between host acceptance and presenter claim survives in the design as reasoning but not on the wire. If a future consumer needs it, the honest move is to add an observation with a reader, not to reintroduce an unused field.

## Dissents recorded from the follow-up review

**Id:** e9458aaf-eb60-4cc8-a909-b3cd9072b403
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** an external review proposing one authoritative row representation and treating the client's synchronisation flags as accidental state
**Verification:** corroborated — the renderer draws from `draw_rows()` only (every `visible_rows()` call in `src/ui.rs` is inside its test module), and each client flag has a distinct reader
**Revisit when:** `Structure` rows become routinely visible, or two flags are found with the same reader and the same lifetime

Two review findings were heard and not adopted, and the reasons belong here rather than only in a conversation.

**Rows have two authorities on purpose.** The session's projection owns structure: which rows exist, their order, their nesting and their folds. The local observation owns facts about a row: its contextual label, its state marks, its processes and its details. `DrawRow::Structure` is how a projected row that local observation cannot yet explain is drawn honestly, and `selected_row()` resolving through local visibility means a selected row this process cannot explain selects but shows no detail. The alternative — one representation, fed from the session — makes the session own presentation facts and turns the UI contract into a rendering contract.

**The client's synchronisation flags are not accidental state.** `floor` is the minimum publication reflecting this view's acknowledged action; `delivered` is delivery dedupe; `resync` forces one re-delivery after a refusal; `incarnation` fences actions queued across attachments; `connected` and `pending` feed readiness. They answer different questions and have different lifetimes, so grouping them behind one value is welcome and collapsing them into fewer states is not.

