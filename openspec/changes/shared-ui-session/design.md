# Design

## Context

See `proposal.md` for motivation and capability scope. The current seams are concrete:

- `src/app.rs` combines tree/navigation state with geometry, details, private pickers and frontend controllers. `refresh()` projects fleet facts and reconciles selection; `reconcile_selection()` retains a surviving row, falls back from a task to its owner, then uses a bounded positional anchor.
- `src/app/commands.rs` dispatches palette commands through key identity. `src/main.rs` coordinates keyboard/mouse actions, controller polling and popup reconstruction.
- `src/picker.rs` and `src/ui/picker.rs` already supply one reusable picker model/renderer. Preserve that seam rather than replacing it during extraction.
- `src/project.rs::candidates()` runs the configured source with the caller's environment and then `select()` prepends the caller's cwd as an unfiltered first entry; `open_workspace_picker()` highlights it by default. `zoxide query -l` itself does not vary with cwd (verified on zoxide 0.10.0), so the process-dependent parts are that leading entry and the environment zoxide reads (data directory, exclusions, PATH). Moving discovery into the daemon unchanged would alter both.
- `src/control_plane/transport/mod.rs` already reserves admission lanes and waits asynchronously before synchronous handlers. `src/control_plane/observation.rs` is the demand-driven fleet authority.
- `src/control_plane/server/admission.rs` checks caller-supplied request IDs against method, target, requester and params, records admission before dispatch, and replays matching records. `server/places.rs` uses that path for creation. Preserve the exact path rather than inferring idempotency from record storage alone.

`sidebar-mode` is implemented but still has unchecked acceptance. Its `sidebar-view` delta is not yet in main specs. This change supplies a forward delta against that exact capability and must synchronize after its prerequisite, never overwrite or claim its acceptance. Other old main-spec statements about removed backends are not evidence that those backends should be reintroduced.

## Goals / Non-Goals

**Goals:** one owner per migrated field/workflow; deterministic shared transitions; no terminal types in the session contract; independent local panels; one valid blocking presenter; replay-safe domain handoff; a host-neutral UI protocol.

**One projection, one owner.** The session's tree projection is the canonical row set, order and nesting: every frontend draws those rows and hit-tests those rows. Local observation explains a row (its kind, marks, details and process facts) and never decides whether it exists. A projected row this view cannot explain is still drawn, from the label and marks the wire carries; a row the projection does not name is not drawn at all, however this view's own tree looks. The join is by row identity, never position. Rendering from a locally rebuilt tree would be a second projection that can disagree with the canonical one about order, membership and folds — the exact divergence the shared session exists to remove.

**Non-Goals:** another daemon/socket/database, lifecycle event reinterpretation, persisted UI workflow replay, multi-user editing, general actor frameworks, independent multi-host physical focus, a mux/terminal engine, full Vim operator grammar, or a wholesale widget-stack replacement.

## Decisions

Confirmed decision forks and rejected alternatives are recorded through Keep the Why in [UI session architecture](../../../context/ui-session.md). The sections below specify implementation; new settled forks belong in that topic rather than being lost in task or commit summaries.

### 1. Reducer plus one dedicated owner thread, not synchronized Apps

Introduce `src/ui_session/` for semantic model, reducer, projection and service. Keep frontend input/local state under `src/app/`, rendering under `src/ui/`, and public wire adapters in the control plane. A small in-process reducer runs first in Slice A; the same owner moves behind the daemon in B. No dual writes or bidirectional App synchronization.

The actor runs on one dedicated named OS thread, connected to the existing daemon runtime by bounded channels. This separates its scheduling/ownership from transport workers; it is not another daemon or worker pool. Start with a synchronous mailbox loop; an async/current-thread Tokio loop is optional only if later measurements or timer/wait integration justify it. Neither permits blocking domain or discovery work on the owner thread.

Inputs are semantic frontend actions, published facts, domain/host completions and attachment/presenter lifecycle messages. Effects are explicit typed requests executed outside the reducer. Do not introduce a separate generic workflow/action-coordinator framework. Local details, overlays and private editor handling stay in the normal frontend loop; a LocalAction enum is optional, not protocol.

### 2. Actor-owned projection, local rendering

The session derives ordered visible rows, ancestry, filters/folds, selection, freshness and command availability from existing normalized observations. Frontends render those rows without independently reproducing canonical navigation. Project semantic content, not rectangles, ANSI or clipped strings. Retain ordered visible rows, selected position and an ID-to-index lookup so ordinary next/previous navigation does not reconstruct the tree. Attention jumps can scan cached rows initially; specialized indexes need evidence. Keep existing sanitization and task provenance. Local details may read newer facts directly by stable identity; they must not join by position or silently transfer process identity.

Selection keeps current behavior: surviving identity, visible task owner, bounded old positional anchor, then none. Stale observation is not an empty authoritative sample. Do not introduce cross-location subject-following as incidental refactor behavior. Geometry-dependent page movement can supply a row distance, not rectangles, to semantic navigation.

### 3. Shared live filter with one editor

An attachment explicitly acquires/releases shared filter editing. Its caret is local; whole-query updates go to the actor live. Other views mirror rather than merge edits. Ownership loss retains the query; clearing is explicit. Starting a shared blocking interaction ends editor ownership and pending input. Filter query updates can only be coalesced consecutively, never across another action whose meaning depends on them.

Use nucleo-matcher as the initial candidate for case-insensitive fuzzy matching; preserve per-level ordering and ancestry, not flat score-ranked results. Do not spawn fzf or add a parallel matching engine without demonstrated scale needs. Start without perceptible debounce.

### 4. Content-addressed payloads and publication IDs

Keep a tree payload and lightweight session-state payload. Deterministically serialize each with serde_json, hash all its bytes with BLAKE3, and retain the immutable bytes by hash. Session state references the tree rather than embedding it. Publication IDs, hashes themselves and publication bookkeeping live outside the hashed objects. Published semantic freshness still counts; do not hide meaningful changes just to keep hashes stable.

Internally reuse Arc-backed payloads and serialized bytes. Selection-only actions cannot change the tree and do not rebuild it. An observation can require reprojection; hash equality then permits retaining the previous object. Reusing content A after A→B→A is valid and still has a new publication ID.

The actor has one increasing publication counter. An update contains its ID, tree/state hashes, optional payloads and addressed delivery information. No timestamp ordering, separate tree counter or additional actor epoch is needed. Clients report hashes they hold; changed payloads can be inlined and missing ones fetched. Pin current content and bound recent content by bytes/count. An evicted hash returns `content_expired`, leading to current-snapshot recovery. Store before publishing. A late fetch may fill a cache but cannot replace a newer intended publication.

Keep every encoded response within existing framing limits. A projection exceeding supported bounds must report an explicit diagnostic rather than silently truncate identities or produce an oversized line. This is a cache of current/recent payloads, not an event-sourcing or durable history store.

### 5. Attachment identity and action ordering

The frontend mints a fresh attachment UUID on each successful communication incarnation, including reconnect after daemon restart. Attachment returns the current publication and hashes/payloads. Client action order starts at zero; first submission is one. Reset the frontend's shared cache, not canonical session state or local geometry. Tag in-flight work with its attachment so old replies are ignored.

One ordered outbound worker submits `(attachment_id, action_no, gate_generation, action)`. The actor decides each next ID once. Semantic acceptance or refusal consumes the ID and has a retained disposition; transport busy/gap answers do not claim application. A duplicate cannot execute again; conflicting reuse is refused. A gap reports `expected_action_no` and permits retry or reattach. Do not buffer/reorder gaps initially or automatically replay uncertain actions with fresh IDs. Keep a bounded receipt cache per attachment. An identical retry returns the original receipt: action ID, disposition, associated server update ID and interaction/domain-request IDs established at acceptance. Retain the action payload or semantic fingerprint for conflict checks. Expiry returns `receipt_expired` requiring reconciliation, never a fresh execution. Original acceptance remains distinct from later workflow status; an operation ID not yet allocated at acceptance arrives through correlated updates. Process a receipt even when its associated publication is older than the currently displayed snapshot; only its old state must not overwrite newer state.

Stamp gate generation and displayed target when interpreting input, but assign an action number only when the ordered sender submits it. Flushing unsubmitted input at a gate change must not create sequence holes. A numbered in-flight action must be answered/reconciled under its existing ID, including a consumed semantic refusal; it cannot simply be renumbered or forgotten.

Action replies and streamed snapshots carrying state references use the same actor publication order. The frontend applies all inbound state through one ordering gate, so a slow reply cannot roll back a newer pushed publication. A target-dependent invocation must use a projection reflecting its preceding navigation or refuse safely; it cannot rebind to latest selection. Existing interaction and durable request IDs remain distinct from delivery counters.

### 6. Concrete RPC responsibilities

Use the existing socket and generated contracts. The server already reads repeated requests per connection, while current exchange_with_id opens a Unix stream for every call. Use a reusable ordered action/RPC connection and a dedicated push-subscription connection. The latter becomes a server-to-client stream after subscription acceptance; ordinary RPCs and content fetches remain on the independent action connection. Preserve authentication, bounded framing and reconnect rules without requiring arbitrary RPC multiplexing. Define the full A–D contract together through generated schemas and fixtures as a best-effort soft freeze: correct it in place as implementation reveals gaps, rather than publish successive partial contracts or preserve legacy paths. The methods are:

| Method | Responsibility |
| --- | --- |
| `ui.attach` | Register fresh attachment/role, optionally for an interaction; return authoritative baseline |
| `ui.detach` | Best-effort release of attachment/editor/presentation ownership |
| `ui.subscribe` | Accept an attachment-bound subscription, send an authoritative baseline, then push latest coherent publications and addressed pending deliveries until disconnect/detach |
| `ui.content.get` | Retrieve retained immutable content or explicit expiry |
| `ui.action` | Ordered typed command, editor acquisition/update/release, host acknowledgement, presenter claim, typed result or cancellation |

Group these under a proposed `ui.session.v1` advertised contract; do not add one capability per verb. Action results distinguish applied, semantically refused, replayed retained receipt, receipt expired/reconciliation required, busy, gap and detached. Specify exact names/fields/errors through public schema generation, not ad-hoc JSON; this is a soft contract, not a prohibition on necessary changes. Keep existing authentication/framing and no unsupported/legacy fallback. One namespace does not authorize agent registrations to acquire operator presentation authority; preserve existing caller-role checks.

The subscription protocol has an explicit acceptance/baseline frame followed by typed publication frames, correlated with its request and attachment identities; ordinary RPCs retain one-response semantics. There is no UI long-poll method, wait_ms renewal cycle or event-history replay cursor. The frontend retains last_update_id locally for ordering/deduplication. ui.attach still returns its agreed authoritative baseline; subscription registration captures the latest baseline and subsequent notifications without a gap, even if the actor changes between attach and subscribe. Register the watch receiver before reading its current value. Hash references and cached payloads avoid unnecessary retransmission.

A connection-owned async task waits for publications, peer disconnect and daemon shutdown, and writes with a bounded deadline. It must not use the blocking cloned-writer mutex, hold a control-handler permit or occupy a blocking-pool thread for the subscription lifetime. Admit subscriptions against an explicit bounded subscription limit, count them toward total connection capacity and keep lifecycle control capacity reserved. An idle healthy subscription is not ended by ordinary RPC idle expiry; detect peer closure independently of publications and enforce write deadlines when writing. Full subscription capacity returns typed busy before acceptance. Await actor answers asynchronously; use bounded mpsc for submissions, oneshot receipts and watch for latest projections.

A slow subscriber may skip intermediate publications and receive the latest coherent state, never impose backpressure on the actor or accumulate an unbounded per-client queue. Retain addressed effects until acknowledged or superseded outside watch; include still-pending deliveries in the latest frame and wake subscribers when those deliveries change even if tree/state hashes do not. Receipts stay on the ordered action path. Preserve gate invalidation independently of skipped modal frames. A write timeout ends the subscription; detach, EOF and shutdown also terminate it and release capacity.

Loss of the primary dashboard/sidebar subscription invalidates that attachment and releases its editor ownership while retaining the accepted query. The frontend reattaches with a fresh UUID/baseline; it never silently resumes an old action sequence. A separately claimed presenter has its own attachment and can outlive its origin. Subscription closure is loss of presentation authority, not proof that the process or admitted domain operation ended. Keep bounded expiry for attachments that never establish a subscription and existing presenter-abandonment semantics.

This is a UI latest-state subscription, not a migration of events.read, pi-radar domain APIs or the tmux backend observer. Keep a narrow reusable transport seam, not a generic subscription framework. Domain event subscriptions would need cursor/retention-gap semantics; tmux control-mode notifications are a separate upstream observation concern.

For throughput, the actor may drain already-queued messages in a bounded batch and coalesce publications only where intermediate views are unnecessary. Every command transition, effect and receipt keeps its ordered meaning. Do not wait to fill a batch or add default input debounce; bound each drain so cancellation, completions and shutdown are not starved. Navigation uses cached facts and never waits for a fresh sample. In B, then after C, measure input-to-render, queue wait and projection/serialization time on a representative fleet and a conservative larger case under normal daemon work. The goal is next-frame feedback and no accumulating backlog, not an unmeasured latency guarantee or a general benchmark suite. Reuse existing shutdown and demand mechanisms; do not sample forever merely because the actor exists.

### 7. Blocking interactions and detached presenters

The model has Idle, Preparing and AwaitingResult states with typed continuation data, not stored functions. Capture invocation target/evidence before another client can change selection. Opening/closing the gate advances a dedicated input generation; unrelated observation revisions do not. Frontends clear pending ordinary input and key sequences and show a blocking cue. This fences interpreted actions, not a claim about when unprocessed terminal bytes were physically typed.

A project workflow is:

1. Actor accepts invocation, captures context and enters Preparing.
2. Addressed discovery effect runs the configured source in the originating frontend's environment and returns fixed validated choices/diagnostics. Candidates do not depend on any working directory: the leading start-directory entry is removed and a configured base directory (default home) is the fallback when the source yields nothing, so the default highlight is the source's first entry. Walking a directory tree, merging several sources and ranking beyond the source's own order are deferred.
3. Actor requests presentation with stable effect/interaction IDs.
4. Dashboard mounts the existing component inline; sidebar host opens `radar ui present --interaction <id>` (illustrative command name).
5. Presenter attaches/claims, fetches its specification and privately handles query/highlight.
6. Its handler directly sends the typed ProjectChosen or CancelInteraction action to the socket and waits for acknowledgement.
7. Actor accepts one winner, releases the gate and continues through domain admission.

The generic Picker does not need a socket or a mandatory intermediate event bus. Its session-aware owner sends the result. Tmux only supplies the popup terminal; exit codes and parent callbacks carry no choice. Do not serialize closures or execute domain work inside the picker.

Retain addressed requests until acknowledgement/supersession. HostAccepted and PresenterClaimed are separate observations, allowed in either arrival order; do not wait for a popup command's eventual exit to establish readiness. A claim grants one valid presenter, not exactly-once OS-window creation under arbitrary failure. Duplicate presenters refuse/close. A valid presenter can outlive its origin. Explicit detach, subscription lifecycle and bounded attachment expiry provide abandonment handling; losing the origin's subscription does not invalidate a separately claimed presenter, and socket closure alone is not process-exit proof. Cancellation from another dashboard closes the presenter; a late conflicting result cannot dispatch.

Local help/error/outcome overlays and palette browsing do not create a shared gate. Selecting a command may do so. Existing host focus/zoom/hide remains local/addressed; shared selection never broadcasts physical focus. One physical workspace host remains the supported focus assumption.

### 8. Preserve the domain admission boundary

Reuse/extract a narrow typed entry to the existing service path rather than recursively calling the daemon socket or duplicating checks. Freeze method/target/requester/params and a stable domain request ID before dispatch. Preserve normalization, preflight, durable admission, replay and verification per operation.

UI intent accepted, effect queued, domain submission uncertain, domain admitted and authoritative outcome are distinct. An uncertain completion is read by original request ID; never generate a new ID automatically. Explicitly track effect prepared, dispatcher accepted, possibly submitted, durably admitted and established outcome. A full/stopped dispatcher returns typed scheduling refusal and resolves the corresponding pending UI state without blocking the actor. Refusal/completion delivery is not a best-effort try-send that can vanish under overload. If an accepted worker fails after possible submission, consult the frozen request and preserve uncertainty; queue acceptance or task spawn is never proof of domain admission. Domain results that leave the fleet unchanged still reach the actor through correlated completions; not all lifecycle events need translation or a `tui_relevant` tag. Normal facts arrive through existing observation publication.

UI interactions/receipts remain volatile. After daemon restart, old attachments/interactions cannot replay. Domain records survive independently. Do not promise restoration of the previous UI session's history without a persisted association; a local inspector can use established request reads/listing without claiming complete history.

### 9. Rust stack and frontend driver

Keep Ratatui 0.30, Crossterm 0.29, Serde/Schemars and existing Tokio. Add BLAKE3/nucleo-matcher for the defined roles after MSRV/Nix checks. Evaluate Strum and tui-actions in A, not after rewriting input. Strum can enumerate typed CommandId; stable wire names are explicit, not Display output. Map tui-actions strings at a local adapter. Its parser/prefix/completion APIs still need application timeout/ambiguity/cancellation handling; full Vim grammar is deferred. Do not combine overlapping keybinding engines.

Slice B adopts a Tokio frontend loop with Crossterm EventStream, futures-util StreamExt and select! over terminal input, frontend-local session/observation notifications, host completions and conditional animation ticks. One terminal-owning task applies local changes and draws synchronously with Ratatui; no other task writes to the terminal. Dedicated socket tasks bridge IPC into local channels; watch is an in-process notification, not the wire protocol. An ordered outbound worker handles submissions without awaiting slow RPCs in input handling. Blocking clients/subprocesses must use bounded off-task work or an async UI transport, never block the terminal owner.

Closed streams and connection failures trigger explicit reconnect/shutdown handling rather than a busy loop or unconditional frontend exit. Terminal cleanup covers quit, errors and cancellation; EventStream is the only terminal input reader. Use the necessary Crossterm event-stream, Tokio macros/time/sync/runtime and futures-util features, not an aggregation/application framework. Keep the actor's dedicated synchronous mailbox loop unchanged.

Drain available snapshots and render only the newest coherent one; preserve every action receipt, addressed effect and gate invalidation independently. Render coalescing adds no debounce/batch-fill delay. Conditional animation timers do not determine input/update responsiveness and must not accumulate catch-up ticks while disabled. Widget swaps and visual polish belong to `tui-interaction-ux`.

Keep projection/matching synchronous initially and measure the full cost, including fuzzy matching and visible-tree reconstruction. If a worker is eventually justified, tag each build with observation/query/fold/order/visibility generations and accept completion only for the current input set. Observation generation alone cannot fence a stale filter build. No worker pipeline, delta protocol or reconciliation engine is added pre-emptively.

## Risks / Trade-offs

- Multiple frontends amplify persistent connections → bounded subscription/total admission, async writes with deadlines, disconnect cleanup and transport saturation tests; no actor backpressure or UI polling.
- A retained payload expires before fetch → explicit expiry and current snapshot, not historical replay promises.
- Navigation plus Enter uses an outdated rendered target → ordered submission plus explicit target evidence; accept or refuse, never retarget.
- Popup launch acknowledgement is lost → retained effect IDs, host dedupe and a single valid claim; bounded abandonment, not a durable UI outbox.
- Service extraction changes admission → integration test lost completion after admission and preserve the exact typed path per migrated operation.
- Old specs still describe process-local sidebar state → prerequisite baseline sync and archive ordering; no edits that pretend unfinished acceptance passed.
- Pending local actions leak through a modal → generation barrier and local pending-input reset; no replay after unblock.

## Migration Plan

Use milestones A–D in tasks.md as ownership and verification boundaries, not a requirement for separately usable intermediate releases. A preserves current rendering while extracting semantics. Design the full B–D contract together, build the session/transport and interaction services, then migrate the frontend and shared commands in one coordinated pass. Do not build an interim frontend or temporary wire variants to demonstrate each milestone. B establishes canonical navigation and push subscriptions; C verifies both modal roots and domain handoff; D completes command migration and deletes obsolete paths. Each migrated field/workflow has one owner. `tui-interaction-ux` may begin after C interfaces and acceptance stabilize, even while D proceeds.

Synchronize the `sidebar-mode` baseline before merging this forward sidebar delta. Coordinate final archive against other pending domain changes without reviving superseded backends. Require matching daemon/frontend release contracts; no mixed-version compatibility paths. Reverting deployment means deploying matching artifacts, not preserving parallel decoders. New source must be Git-visible before Nix validation; use jj export in this workspace.

Validation extends existing model/handler/TestBackend tests with a few invariant-focused cases. Live tmux acceptance remains user-run. Schema-affecting completion runs format, strict Clippy, Rust/SDK suites, OpenSpec validation, generated-contract guards and Nix. Do not rewrite unrelated dirty plan/node_modules changes.

## Open Questions

Only bounded implementation details remain: exact cache/receipt byte limits, renew/expiry intervals, and leader timeout duration. Choose and document conservative constants within existing framing/resource bounds. Specify the complete public method/field set with generated fixtures as a best-effort baseline for B–D; necessary in-place corrections are allowed. Subscription/frame limits and write deadlines must be chosen within existing framing/resource bounds. No ownership or recovery semantics are deferred.
