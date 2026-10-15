# Design

## Context

See `proposal.md` for the motivation. This design refines the change against Radar's durable execution model; it does not introduce a new persistence engine or general workflow framework.

Today `Collector::collect` reads inventory, registry data and process evidence for a display, and returns a whole-pass failure when inventory or registry collection fails. `ObservationState` retains presentation associations in the display's memory. Separately, `spawn_sweeper` polls lifecycle subjects and adopts/observes topology without a display. Moving the collector unchanged would duplicate that work and would not supply independent source freshness.

The durable lifecycle/container journal, operation records and registry/publication storage already have different persistence responsibilities. They must not be presented as one transactionally consistent store merely because a snapshot combines their projections. Tracing and the typed `Contract` machinery are already implemented; this change reuses them.

## Goals / Non-Goals

**Goals:**

- One daemon-owned observation projection shared by displays and socket clients.
- Observation coverage required by managed subjects and admitted operations independent of display lifetime.
- Explicit freshness, continuity and identity for independently collected evidence.
- A bounded public snapshot interface that does not expose private implementation types.
- A migration that preserves existing lifecycle coverage and eliminates display-owned collection.

**Non-goals:**

- A database migration, checkpoints, journal retention or a requirement to replay from genesis.
- Assignment/mailbox authority transfer, application command acknowledgment or generic durable workflows.
- Implementing stable logical identity across executions or repairing display-owned launch enrollment in this change. Those are explicit follow-on lifecycle work, not guarantees supplied by moving observation.
- tmux control mode, snapshot deltas, sidebar UX or per-client focus/navigation state.
- Treating a source observation, a snapshot revision or a notification as a committed domain transition.

## Decisions

### 1. Keep authority, evidence and presentation separate

Durable managed state records Radar's accepted decisions and established outcomes. Sources provide qualified external evidence. The shared snapshot is a derived read model composed from both; its retained associations are presentation continuity, not authoritative parentage or lifecycle state.

A snapshot identifies the durable journal position used for lifecycle/container projection and the versions/times of the other source reads. Until storage is consolidated there is no invented global transaction revision across registry files, operation records and the journal. An immutable snapshot is internally fixed, but it is not a claim that tmux, procfs and application publications were observed simultaneously.

Lifecycle changes continue through the existing fenced transition machinery. The snapshot layer neither appends lifecycle conclusions nor rewrites managed relationships. `lifecycle-authority` defines additional observation-driven transitions on this seam.

**Alternative:** exporting raw inventory and having every display retain/join it repeats ownership and collection work. Exporting a serialized `ObservationState` exposes implementation and clock state. Use explicit public projection DTOs instead.

### 2. Demand has internal and display owners

Managed subjects, active operations and configured-server topology adoption retain the source coverage their guarantees require. Adoption coverage must remain active for externally created places even before they contain a managed subject. Display leases add optional detail, such as resource measurements and process tables. Expiry of all display leases may stop that optional work, but cannot stop lifecycle observation, deadlines or baseline recovery required by active managed state.

A read renews display demand for its requested classes. Required dependencies between classes are resolved by the daemon; a reader cannot request a shape that lacks the evidence needed to qualify it. No durable subscription is introduced for a display.

The same rule applies before dedicated pidfd/control-mode observers exist: retain existing bounded polling coverage until a replacement is proven. A daemon with neither internal demand nor display demand may idle.

**Alternative:** sampling everything continuously is simpler but needlessly scans procfs. Sampling only while a display asks violates the optional-display principle. Internal demand plus optional display detail preserves both guarantees.

### 3. One collection coordinator, with fair bounded work

A synchronous daemon-owned coordinator owns collection history, presentation continuity and the published snapshot. Runtime work runs outside the `Core` lock and outside async transport tasks. At most one collection pass is in flight; timer, committed-change and task-source wakes coalesce into bounded pending work. The existing synchronous domain/async transport seam remains.

Migrate the physical inventory work currently duplicated by the display collector and topology sweep toward one source read feeding their respective consumers. Preserve subject observation and topology adoption throughout the migration; do not remove the sweeper first and assume the display sampler replaces it.

Batch limits require continuation: rotate or page through every tracked record rather than repeatedly selecting the same sorted prefix. The current subject sweep's first-256 selection and container pass's first-128 selection are not sufficient fairness policies. Pending work must survive coalescing until all affected records have been examined.

Cancellation/deadlines are cooperative unless an adapter explicitly enforces a subprocess deadline. A timeout must not create an unbounded succession of detached workers. Keep control admission separate, bound worker resources and shutdown, and test a stalled source without claiming arbitrary blocking code can be forcibly cancelled.

**Alternative:** a new actor framework or async rewrite is unnecessary. Reuse the existing sampler/runtime interfaces, adding only the ownership and scheduling needed by these consumers.

### 4. Separate baseline reconciliation from normal observation

Durable state recovery restores recorded authority locally. Reconciliation establishes an external observation baseline after startup, source/backend generation replacement, an unreplayable subscription gap or evidence whose continuity/confidence is lost. Normal polling, callbacks, timers and subscriptions then maintain that baseline and advance operation-specific processing.

Track continuity by source/scope, not as one global fleet flag. One unavailable procfs scope does not invalidate a healthy tmux source. A successful check establishes only the baseline it actually verified. Repeated attempts to restore a failed scope are bounded/backed off recovery attempts; they are not permission to reconstruct missing authoritative decisions.

The existing periodic functions may temporarily retain their names during migration, but the design and public diagnostics distinguish their ordinary observation work from baseline reconciliation. Unknown operation outcomes may require targeted outcome verification without implying that every source baseline was lost.

**Alternative:** calling all background work reconciliation obscures normal progress and encourages broad rescans. Deleting polling because reconciliation is exceptional removes required coverage.

### 5. Fence source evidence before durable use

Qualify evidence with its source, observation time, source continuity/generation and the relevant execution/binding identity. When command paths or later lifecycle controllers use it, they revalidate against current durable state through the same serialized transition path. A sample captured before a restart or rebinding cannot terminate the replacement execution.

Snapshot freshness is not mutation authorization. Existing guarded commands continue to verify their targets; controllers cannot substitute a display snapshot for the evidence their effect requires. Source notifications are wake hints unless they establish the exact condition for the fenced execution.

Loss of source continuity gates only the affected scope. Unrelated operations remain available, and a command may proceed if it independently obtains the fresh, identity-fenced evidence its guarantees require. Refuse operations whose required evidence cannot be established; neither a global mutation freeze nor an unconditional pause of every affected subject is required.

The later authority change owns confirmation policies. Repeated failed/stale reads cannot prove death; confirmed pane absence establishes pane loss, not necessarily process exit. Definitive process evidence remains distinct.

**Alternative:** comparing only a pane ID or session UUID is insufficient across execution/backend replacement. A snapshot-wide timestamp alone also cannot establish identity or ordering.

### 6. Failures and freshness are source-local

Refactor collection to retain independently successful source results and last-good values for failed sources. Each source records its last successful observation, latest attempt/failure and continuity state. Never turn an unavailable source into a successful empty inventory.

Retained joins record the source versions that support them. A fresh registry read joined to stale inventory does not become a fresh verified binding. Dependent derived fields inherit the limitations of their inputs.

Resource/process measurement incompleteness is diagnostic evidence, not lifecycle evidence of death. Preserve available measurements with their scope/completeness rather than translating an unrelated procfs failure into a dead agent. Do not silently claim a complete total from a partial scan.

Use monotonic time for scheduling, demand expiry and intervals. Export wall-clock times for readers without treating clock movement as execution ordering. A source becoming stale is an observable snapshot change even if its last-good values are unchanged.

**Alternative:** whole-pass failure is easy to preserve from the collector but hides successful sources and couples unrelated failures. Keeping values without age/failure hides uncertainty.

### 7. Restart-safe revisions and bounded paging

The public snapshot identity is an opaque epoch-qualified revision. A new daemon process has a new epoch; a client holding an old identity cannot receive a false `not_modified` because a local counter restarted. Keep counters internal rather than exposing a new unconstrained JavaScript number.

Publish an immutable snapshot when its payload or exposed freshness/continuity metadata changes. Merely reading a snapshot does not create a revision. A successful sample whose exported collection time changes may legitimately publish a new revision; do not promise both updated timestamps and an unchanged payload revision.

`observation.get` supports requested classes, bounded waiting and revision-consistent paging. Cursors bind the snapshot identity, requested classes and page position; they are not fresh sampling requests with a mutable class set. Retain snapshots under explicit byte/time limits rather than assuming that retaining exactly two revisions lets every reader finish. Expired cursors return typed `snapshot_expired`; readers restart without combining pages. Bound response size including the envelope, and define an explicit outcome for an individual item that cannot fit rather than silently dropping it.

Transport waits use the existing waiting lane: a read that asks to wait is admitted there rather than to the control lane, so a display long-polling for a revision cannot occupy the slots effectful requests need. The wait rechecks the revision under the same lock that records demand, so a publish between looking and waiting cannot be missed. A waiter holds no domain lock; it does hold one bounded waiting-lane worker for the duration of its wait, as a bounded long poll does. It returns the latest available state, not a guarantee of observing every intermediate snapshot.

**Alternative:** deltas and lossless observation streams need retention/gap machinery absent from this slice. Start with full snapshots and change waits. Snapshot revisions are separate from durable event cursors.

### 8. Explicit public DTOs and one client projection

Define observation request/result DTOs independently of private sampler, registry and store structs. Generate schemas and SDK declarations through the checked Schemars workflow for the public shapes clients consume. Preserve source labels and uncertainty; omit private recipes, environment overlays, correlation secrets and internal lock/clock state.

The TUI fetcher assembles a complete snapshot before replacing its view. A failed/expired partial read leaves the previous view visibly stale; keyboard input and quit remain responsive. The TUI retains only view-local tree construction, folds, selection, filters and presentation. It has no raw collection fallback.

Advertise the typed observation contract and update the protocol revision according to the current contract policy. Reuse existing tracing initialization; add identifier-only collection/request spans without payload or launch values.

**Alternative:** serializing internal structs is quick but freezes incidental implementation into the wire contract. Generating every private Rust type or replacing RPC/Serde is unnecessary.

## Risks / Trade-offs

- **Moving collection does not remove lock contention** → Keep collection outside `Core`; take bounded source views, publish under short locks and validate stalled-source/control concurrency. Existing synchronous durable I/O still has its own latency; do not promise hard real-time control.
- **External sources are not atomically observed** → Publish provenance/version vectors, qualify joins and revalidate before effects.
- **No display hides missing internal demand** → Test managed-source progress after disconnecting all readers, not just demand lease expiry.
- **Record limits cause starvation** → Test batches larger than one pass and a changing inventory with durable continuation until the batch is drained.
- **Epoch resets or paging mix histories** → Test restart with an old revision, class/cursor mismatch and expiry during assembly.
- **A wake can be lost or coalesced too aggressively** → Publish state before notifying, recheck after subscription and retain dirty work while a pass runs. Polling remains a bounded safety source.
- **Public snapshots leak private launch state** → Allowlisted DTOs plus schema/fixture tests; neither debug formatting nor logs may bypass that seam.
- **Existing L enrollment still depends on display memory** → Keep this gap explicit. Daemon-owned tracked launch/enrollment is a separate lifecycle prerequisite for claiming display-independent managed-agent launch, not a side effect of this change.

## Migration Plan

1. Define the public snapshot/source identities and DTOs; validate directional schemas and the SDK contract on representative fixtures.
2. Add the daemon-owned coordinator and independent source outcomes. Preserve existing managed observation coverage while sharing inventory reads and adding fair continuation.
3. Serve bounded snapshot reads/waits and internal/display demand. Test two readers, no readers with active managed state, source failures and source generation changes.
4. Switch the TUI to the complete-snapshot fetcher. Exercise existing process, task, registry and retained-row behavior through that seam, then delete the old display collection path rather than keeping two authorities.
5. Align protocol docs and task evidence; run the non-E2E gate and checked Nix build. PTY/tmux end-to-end and two-display live acceptance remain explicit operator checks under the current testing instruction.

Deploy daemon and bundled client together; an old daemon produces the existing blocking diagnostic rather than a fallback. Roll back the binaries together. This slice adds no durable state schema migration, so rollback must not require reconstructing authority from observations.

## Follow-on Lifecycle Work

Implement shared observation first, followed by stable logical identity versus execution records and daemon-owned launch progress/enrollment. Keep the existing display-owned L enrollment gap explicit until that follow-on work lands.

The baseline managed launch operation is fulfilled by verified capture and recording of the intended process execution, not by Pi registration, application readiness or assignment acceptance. Those are distinct conditions; future operations may require stronger acknowledgments without redefining successful process launch.

`lifecycle-authority` must separately define safe abandonment, operation outcome verification and evidence-driven transitions. Abandonment ends the recovery obligation and releases only the operation's administrative block. It preserves uncertain-effect history and independent physical safety checks, does not establish an external outcome and never implicitly authorizes repeating the effect. A force-forget safety waiver is not part of this decision.

Use operation-specific recovery rules and keep speculative reconstruction of missing authority out of scope. Assignment acknowledgment, recipient inboxes, persistence consolidation and a generic workflow engine are not prerequisites for this observation slice.
