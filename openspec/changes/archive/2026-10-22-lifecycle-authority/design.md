# Design

## Context

The daemon already records effectful requests before dispatch, tracks managed launch subjects with fenced bindings, and keeps uncertain outcomes explicit. But publisher registration still carries process-asserted identity and private publisher storage, Herdsman currently hosts the baseline publication path, and the dashboard has enrollment logic. These boundaries leave a plain Pi session without baseline reporting and make recovery depend on copies outside Radar's durable records.

The architecture distinguishes three things:

- **Durable authority (`D`)**: Radar's admitted subjects, operations, execution generations, managed relations and committed outcomes.
- **Pi-session breadcrumb (`B`)**: a durable, versioned reflection of identity Radar issued for one launch; useful for correlation after a gap, never a second writer or authority.
- **Live observation (`O`)**: source-labelled channels such as location, process identity evidence, model/context facts and task state. These can change and become stale; they are not lifecycle decisions.

Reconciliation re-establishes an external observation baseline after continuity or confidence loss. It compares `D` with fresh `O`, using `B` as correlation evidence. It cannot reconstruct a missing authoritative decision from `B`, and is not normal polling or an effect retry.

## Goals / Non-Goals

**Goals:**
- Keep one authoritative Radar record and transition path for each Radar-managed logical subject and its successive executions.
- Make pi-radar the one baseline registrar/publisher for each Pi process, regardless of Herdsman presence.
- Let Herdsman submit orchestration intent and application evidence through Radar/pi-radar without duplicate registration or baseline publication.
- Persist a Radar-issued, non-secret identity breadcrumb in the corresponding Pi session to aid reconciliation.
- Preserve uncertainty when durable authority and external evidence cannot be safely joined.
- Keep independent lifecycle coverage alive without a display attached.

**Non-Goals:**
- Moving assignment/mailbox authority from Herdsman in this change.
- Making the Pi session or its breadcrumb authoritative for Radar-managed state.
- Moving live facts into durable lifecycle records or persisting every observation sample.
- Building a generic broker/workflow framework or a general downstream messaging service. A typed command-delivery lane is a later operation-specific change.
- Migrating journal storage to SQLite or requiring genesis replay.
- Replacing bounded polling before an alternative demonstrates equivalent coverage.

## Current Identity and Migration Audit

The current implementation has two identity layers and does not yet model an execution generation separately:

| Current record | Key/identity | Existing join evidence | Migration rule |
|---|---|---|---|
| Publisher registration | `source` + publisher `incarnation`; daemon assigns `agent_id` | Same exact source/incarnation retry; process claim and location are evidence, not merge keys | Keep the registration ID distinct from the managed subject ID. Never merge registrations because pane or Pi session UUID matches. |
| Managed lifecycle subject | Random `subject_id`; record also contains `spawn_request_id`, launch context, optional session, process claim, bound `agent_id`, state and optional `successor` spawn request | Registry projection joins on exact bound `agent_id` plus process claim; spawn binding names the exact agent registration and child source/incarnation | Preserve an existing subject ID as the logical subject only where the durable subject record proves it. An exact persisted successor link may establish an execution succession; otherwise do not infer one. |
| Enrollment | Creates a new random lifecycle subject for an already registered agent/process/session | Explicit enrollment request verifies lead, session and process before recording | Treat as managed from its enrollment record onward; do not backdate a launch or infer an earlier generation. |

The legacy `successor` field names a spawn request, not an explicit numeric execution generation. Proposed migration mapping:

1. Preserve each existing `subject_id` as an execution-record identifier and initially as its own logical-subject root.
2. For a subject with `successor = request_id`, follow only the exact durable spawn edge with that request ID and its exact `subject_id` attachment. If the edge attaches another lifecycle subject, treat that subject as the next execution in the chain; continue through its own exact successor link. Never join by pane, session UUID, process name or registration source.
3. For a uniquely linked chain with no cycle or conflicting predecessor, preserve the earliest subject ID as the logical subject ID and assign generations in chain order. Keep legacy execution-record IDs as aliases/references so existing operation/event records remain interpretable. Do not infer an unrecorded launch or generation.
4. A subject with no successor remains generation 1 under its own ID. An explicitly enrolled subject is a managed root from its enrollment record onward; do not backdate its lifecycle.
5. Keep each registry `agent_id` as the registration handle for its exact `(source, incarnation)`; map it to an execution only through the lifecycle subject's exact `bound_agent_id` plus the matching process claim and exact spawn edge where present. Registrations without a managed subject remain baseline-only.
6. If a successor request/edge/subject is missing, has no exact attachment, has conflicting links, or forms a cycle, do not merge the records. Preserve the old records and mark only that chain's continuity unresolved; no launch is replayed.

The daemon's `events::Log` is authoritative; old event files must not be rewritten to make this migration appear atomic. The `Event.subject_id` and the event-log facts key currently identify one launch execution. Migration reads those current facts plus exact spawn-edge attachments and appends a typed identity-mapping transition for each execution in a uniquely linked chain. Reopening the log must be idempotent: already mapped executions are not remapped or appended again. Existing event history remains addressable under its execution ID; new public semantic events expose the stable logical `subject_id` and the affected `execution_id`/generation. Pre-upgrade unresolved operation records may still target an execution ID, so cancel/stop/restart admission checks the stable logical target and every exact execution ID mapped to it; the migration does not rewrite operation or event history.

This migration mapping was reviewed and approved. The public contract choice is now explicit: `subject_id` becomes the stable logical identity; `execution_id` identifies one launch; `execution_generation` is a one-based `u32` and increases only along an exact durable successor chain, keeping its JSON number exactly representable without decimal-string encoding. Existing read/control calls that previously treated `subject_id` as one launch must resolve the current execution for the logical subject. Compatibility is handled by a protocol revision bump, not by silently changing meaning under the existing revision. Existing public `agent_id` remains a publisher registration identifier and must not be silently reinterpreted as the logical subject ID.

Accepted launch metadata is supplied in `spawn.create.request.managed` as optional `owner`, `run`, `label`, `role` and `parent_subject_id`. Text values are non-empty, control-free and bounded to 256 UTF-8 bytes. `parent_subject_id` is a canonical stable logical subject UUID and must resolve to the exact registered parent agent ID before admission. Radar stores accepted values in the execution event facts before dispatch and in lifecycle reads. Successor executions inherit metadata from the preceding Radar record; a live publication cannot overwrite it. Metadata changes require a future explicit Radar transition rather than publisher reconciliation.

### 1. Radar owns managed intent and resulting lifecycle state

An accepted managed launch intent creates or advances Radar's durable subject/execution record. Herdsman may provide requested parent, role, owner, run and label; Radar validates and records the accepted values as its own managed state when it admits and performs the operation. The source of a request does not become a second writer. Subsequent changes to managed metadata are explicit Radar transitions. Assignment and mailbox authority remain external until a separately agreed transfer.

An unmanaged Pi session has no Radar-executed launch intent. It can receive baseline registration and live observations, but Radar does not fabricate a managed parent, owner or launch history for it.

Post-launch association is daemon-owned work, not a TUI follow-up. Verified capture and durable recording fulfill the baseline managed launch; Pi registration/readiness is not part of that success guarantee. If the publisher registers later or a display closes, the daemon may bind the exact existing execution only after fresh registration, process-birth, session and breadcrumb evidence agree. It never launches again to complete an association, and an unavailable or contradictory source leaves the record unbound/unresolved.

### 2. One baseline registrar per Pi process

The pi-radar package provides a default Pi extension entry point. At `session_start`, it resolves the current Pi session and trusted location/process evidence, registers once, holds one lease, and publishes baseline channels. It starts no timers or sockets from the extension factory and shuts down idempotently at session shutdown. It retries daemon absence with bounded backoff without inventing a subject identity.

Herdsman depends on pi-radar as the host integration. It consumes the same publication/runtime handle through pi-radar's extension API and supplies launch intent or application-specific evidence through Radar. It does not independently call registration for that Pi process and does not publish a duplicate baseline. If the baseline extension/runtime is unavailable, Herdsman reports a diagnostic instead of silently creating a parallel registration.

Pi's separate package module roots mean Herdsman and pi-radar must not rely on sharing module singletons. The integration surface is a versioned, lazy `RadarRuntimeHandle` passed over Pi's session-wide `pi.events` bus. The handle exists during extension factory registration but starts no resources; `start()` memoizes creation of the publication and execution adapter, so the baseline extension and Herdsman can both request it without creating two publishers. Consumers issue a synchronous request with a reply callback. No response is a visible missing-provider condition; multiple responses are a duplicate-provider error, never a second publisher to adopt silently. The handle is a structural object of methods/data, not an imported class instance, so module identity and package roots do not matter. Event channel names and payload version are explicit; consumers use a compatible pi-radar SDK helper only for the stateless request/validation code. The provider registers the event listener in its factory but starts no timer/socket until `session_start`; the shared event bus exists for all loaded extensions before session handlers run. Host-provided Pi packages remain peer dependencies and must not be bundled into pi-radar's published runtime.

### 3. Radar-issued identity breadcrumb is a reflection

For a managed launch, Radar injects a version-1 marker containing only the stable logical subject ID, exact launch request ID and one-based execution generation. It contains no credentials, spawn token, private recipe, owner/role metadata or environment values. pi-radar persists it in a Pi-owned durable session entry and reports its version/identity with live registration evidence.

The supported Pi persistence seam is the non-context custom session entry: `pi.appendEntry(customType, data)` writes it to the Pi session file, and `ctx.sessionManager.getBranch()` reads entries on the active branch during `session_start`. The codec in `pi-radar/extension/radar-session-marker.ts` validates the exact versioned shape and chooses the newest marker on that branch; a malformed newest marker stays unresolved rather than falling back to older identity. Do not write markers to an arbitrary project file or treat the publisher's private `PublicationStore` as a substitute for the Pi session.

Radar's durable record remains authoritative. A marker can identify a candidate record, but current process birth, server/pane binding, generation and durable operation state still need fresh verification. A copied, stale or contradictory marker is unresolved evidence. A marker without a matching durable record cannot create one; recovery leaves the association unresolved and never replays the launch.

### 4. Live channels remain replaceable observations

pi-radar holds the baseline lease and writes session/location/process/facts channels with explicit source and freshness. These channels may be replaced as the session changes and are read models, not lifecycle state. Herdsman can contribute application-owned evidence through the same integration but cannot overwrite Radar's managed subject, parent, operation, owner, run or label. Channel attribution identifies the source of evidence; it does not grant authority over durable managed state.

The existing `agent_id` remains daemon-issued. This change does not merge registrations by pane reuse or session UUID alone. Existing registry identities need an explicit, fenced migration: managed subjects retain their logical ID while executions advance by generation; unmanaged legacy registrations age out under their current lease rules. If an old managed record cannot be joined unambiguously, it remains unresolved rather than being guessed into the new model.

### 5. Observation and commands share one fenced reducer

Observers produce evidence; the reducer compares it with current durable state and commits at most one edge-triggered transition with a short evidence summary (source, strength and observed time). Samples never enter the journal. Command results and observer evidence for the same subject serialize through the existing fence, so a cancellation racing an exit cannot apply two terminal transitions.

Definitive evidence (verified process exit or exact verified pane/server event) transitions immediately. Weak evidence (one missing inventory row) requires repeated confirmation within a bounded window or a fresh identity check. Failed, stale and incomplete reads are not absence evidence. A missing pane does not by itself prove the process exited.

### 6. Reconciliation is targeted and never retries effects

Reconciliation runs on daemon start or an explicit operator request, and what it exists to resolve are the conditions that leave durable state without an established external effect: a changed serving identity, an operation a crash left unknown, or a detected inconsistency. Those conditions do not schedule a pass of their own. It restores an observation baseline by checking current evidence against durable records. Steady-state samples continue as normal observation and drive ordinary transitions; they are not called reconciliation, and a lifecycle read reports them under `source.observation` rather than under any recovery name. The reducer and existing process/pane safety gates remain authoritative. Abandonment ends an operation's recovery obligation and administrative block only; it does not discharge physical safety checks, establish an outcome or authorize replay.

### 6a. Durable write order across the existing stores

There is no shared transaction across the operation store, the registry's spawn edges and the private lifecycle log, so the order is the safety property and recovery is defined per boundary:

1. The operation record is written (`Pending`), then claimed and started, before any dispatch. A crash before dispatch leaves a record whose own expiry releases its target; nothing was attempted.
2. The spawn edge is written with `created`/`launched` unknown *before* the pane effect, so a crash during the effect leaves an edge that claims no effect.
3. The create's outcome, location and verified serving instance are recorded immediately after the create and *before* the launch is attempted, so a confirmed pane is never lost to a later step's failure.
4. The launch's outcome is recorded after the launch. A launch that may have reached the pane is `unknown`, never `completed`.
5. For a tracked launch, the lifecycle subject exists before the bootstrap can be released, so a process that runs is always one Radar can name.
6. The operation is settled last, from evidence the earlier steps recorded.

A crash between any two steps leaves records that reconciliation can read: the intent is visible, a possibly-dispatched effect is never blindly repeated, and a partially written launch stays explicitly unresolved rather than being reported complete. Crash injection at each boundary is the acceptance test for this ordering (task 5.6); until it runs, this is a designed order rather than a verified one.

### 7. Independent observation sources and fair bounded progress

Collection sources report independent success/failure and freshness. A registry failure does not erase a good fleet sample; a failed or incomplete process scan cannot establish exit. Bounded sweeps must make fair progress across all subjects and containers rather than repeatedly inspecting the first sorted page: one shared paging rule (`control_plane::store::page_after`) resumes after the last record examined and wraps at the end of the listing, so every record is reached within a bounded number of passes whatever the listing's size. The cursors live in the daemon's observation loop, so a restarted daemon starts over — correct, because a cursor is only meaningful inside the run that built it and a listing can change underneath one. Collection deadlines are cooperative: blocking adapters need bounded calls, and the shutdown policy must account for an adapter that does not honor cancellation; a worker deadline cannot forcibly stop arbitrary Rust code.

### 8. Semantic events are public; the journal remains private

Public consumers receive semantic events with explicit cursor, replay and gap behavior. The private journal representation and retention can evolve. Observation snapshots and source freshness changes are not themselves durable events; only a managed transition they cause is. Coalescing is appropriate for replaceable snapshot notifications, not for committed events promised to a consumer.

## Risks / Trade-offs

- **Pi API compatibility must be checked at integration.** The current supported API documents `appendEntry` and active-branch `getBranch`; pin/typecheck the host version and test process restart plus extension reload before relying on marker recovery. Do not silently fall back to trusting a client cache.
- **Recovery can remain unresolved.** A breadcrumb can be missing or copied, and a daemon record can be unavailable or contradictory. This design prefers an explicit unknown over speculative identity reconstruction or a duplicate launch.
- **Single-host coupling.** Herdsman now requires the pi-radar extension runtime to be loaded. It must detect missing/incompatible baseline support and explain the problem; the install/update path must keep the dependency compatible.
- **Migration is identity-sensitive.** Existing managed lifecycle IDs, publisher registrations and spawn bindings cannot be merged by pane/session heuristics. A failed migration leaves a visible unresolved record; it does not re-run a launch.
- **Ownership fields become clearer but less independently editable.** Herdsman can request owner/run/label values, but changes after admission must be Radar transitions. This preserves one writer at the cost of an explicit command for later edits.
- **Same-UID processes are not authenticated by marker possession.** The daemon must bind the candidate to fresh process and location evidence and preserve the existing same-user trust limitations.
- **A live channel is not durable history.** Last-good channel state can be stale after a daemon/source gap; freshness and source attribution must remain visible rather than being promoted to fact.
- **Public event replay needs a stated retention/gap contract.** Snapshot notifications may coalesce; semantic events may not silently disappear under the same promise.
- **No database migration is required now.** Current persistence remains, but ordering and recovery boundaries across the operation store, registry and lifecycle event log still need explicit crash testing; do not claim cross-file atomicity or require replay from genesis.

## Alternatives Considered

- **Two independent reporters on one subject:** rejected. It adds reporter/subject identity, two-host arbitration, channel-fence coordination and duplicate registration failure modes where one baseline host plus explicit contributions is enough.
- **Herdsman remains the baseline host:** rejected as the long-term shape because a plain Pi session then has no baseline and the package named pi-radar is not the default registrar.
- **Put managed owner/run/label only on live channels:** rejected for Radar-executed launches. These values are accepted launch intent and durable managed state; channels are for replaceable observations.
- **Let the Pi breadcrumb recreate Radar records:** rejected. A session file is not proof that Radar admitted an operation, and replay/reconstruction from it could invent authority after a partial write or copy.
- **Move all facts into Radar's journal:** rejected. Application facts are external observations; polling every sample into durable history is unnecessary and can expose sensitive or transient content.
- **Add a generic broker or workflow DSL:** rejected for this change. Reuse typed operation-specific methods and the existing transport; downstream command delivery is separately designed per operation.

## Remaining Implementation Questions

1. Does Pi's documented `pi.appendEntry`/active-branch `getBranch()` API persist the marker across real daemon restart, Pi restart and extension reload on the supported host versions? Codec and adapter tests are not host acceptance.
2. What semantic-event retention and cursor-expiry contract do external consumers rely on?
3. Does the packaged Pi extension load as compiled ESM on the deployed Pi-Bolt version, and can Herdsman consume the shared runtime without a second publisher?

The write order (decision 6a) and the fairness cursors (decision 7) are now designed and implemented; what remains for them is crash injection at each durable boundary and beyond-one-page tests, which are acceptance work rather than open design questions. Keep questions 1–3 scoped to their owning tasks and validate them with concrete tests.

## Implementation Sequence

1. Audit existing operation, subject, registration and recovery paths; verify Pi session persistence and record the source-of-truth/migration table.
2. Establish stable Radar logical-subject and execution-generation records, including safe migration and fenced reducers.
3. Persist Radar-issued breadcrumbs in Pi sessions and use them only as reconciliation evidence; test absent, stale, copied and contradictory markers.
4. Make pi-radar the one default baseline extension/registrar; migrate Herdsman to submit intent and attach evidence through the same handle, with no duplicate registration.
5. Implement the transition evidence rules, targeted reconciliation, safe abandonment, fair progress and independent-source handling; add docs and integration acceptance per slice.
6. Validate semantic events, generated public DTOs, daemon restart/replacement and no-display coverage; run strict OpenSpec, Rust/package gates and checked Nix build.

## Deferred

A command-delivery lane for application operations (for example continue-with-assignment), assignment/mailbox authority transfer, database/checkpoint work, tmux control-mode replacement and generalized event-bus projection remain separate changes.
