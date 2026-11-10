# Design

## Context

See proposal.md for motivation. The initial implementation at `5482f789` records
fresh launches by registration and observes, stops and withdraws them without a
subject. Restart and existing-child continuation reserve a successor claim.
Disposable live acceptance and the checked package build passed at that checkpoint.

The checkpoint is not the final design: reserving a successor overwrites
`current_claim`; `Claim` embeds `ProcessRecord` and `sync_claim` copies changes;
adoption and launch retain separate construction entry points; and the tree still
creates unmatched-subject rows. A green checkpoint does not verify this rework.

## Goals / Non-Goals

**Goals:** separate process ownership, attachment authority and presentation; keep
one execution lifecycle; make reservation and grant changes atomic; give the
projection authoritative joins before presence.

**Non-goals:** fresh-subject allocation commands, new subject/claim-oriented views,
workspace recovery and compatibility machinery. Do not broaden this into a generic
workflow framework or add interfaces for hypothetical providers.

## Decisions

### 1. Execution creation is shared; identification and attachment compose

One creator records the registration with optional placement and optional launch
intent. Identification records the verified process incarnation, immediately for
adoption or later for a launch. Optional claim reservation and spawn provenance
compose around that execution rather than creating another execution path.

Fresh `L`, recipe and child launches have no preassigned subject. Presence resolves
or creates the session's subject and grants its attachment. Restart and continuing
an existing child already know their subject, so they reserve its successor claim.
A reserved or granted claim names both its subject and registration; an execution
can exist without either attachment. A pending or denied presence attempt may not
yet resolve a subject.

Always inventing subjects is rejected because failed and resumed launches need not
introduce a new durable agent identity. Forbidding early claims is also rejected:
restart and continuation require a reserved attachment before dispatch. A new
fresh-subject preallocation API is unnecessary for this change.

### 2. Reservation excludes competitors without replacing a grant

Claim records are the authority. The current grant, exclusive reservation and
most recently granted history are derived queries/indexes over those records,
not independently writable attachment pointers in SubjectRecord. Update the
indexes in the serialised transition and rebuild them from durable claim history
at startup. Reject incompatible grants or reservations rather than choosing by
file order or inventing authority. Historical ordering comes from recorded
transitions, not an independently maintained last-claim field.

A reserved claim blocks competing reservations and grants, not the incumbent's
existing authority. Only the registration it names may fulfil it, and only after
the incumbent grant releases. Grant records the claim's new standing and updates
the derived indexes under the same serialised transition; the same claim is no
longer a reservation.
An ordinary presence grant also checks for a conflicting reservation.

Use the owning transition lock for the full check-and-write transition. Replay
reconstructs granted and reserved state without dispatching any effect. A stale
release names its claim and may release only that exact grant, never a successor.

Replacing current authority at reservation is rejected: it revokes the incumbent
before its process exits and makes the current pointer mean two different things.
Inferring reservations from history is rejected for the same reason.

### 3. Restart preserves incumbent authority until release

Restart validates the incumbent and reserves one successor execution and claim.
It stops the exact predecessor registration and launches the reserved execution
only after verified exit and release. The successor must identify and present
before its reservation becomes a grant; reservation is not liveness evidence.

If stopping fails, cancel only the exact successor reservation; the incumbent's
grant was never displaced. After verified predecessor exit, successor failure
leaves a detached subject once withdrawal settles. Never restore a grant to an
exited predecessor. Reservation cancellation and withdrawal must not clear a later
reservation or grant.

### 4. A claim owns attachment facts, not process snapshots

A claim is a proposed or established attachment, not a synonym for granted
authority. Reserved, pending, denied/duplicate and released standings belong to
the attachment lifecycle as well as granted. A denied presence attempt need not
have a resolved subject; a reservation or grant must name one. A stale claim is
one that no longer matches the subject's applicable attachment reference; staleness
need not be another persisted standing.

Use one authoritative attachment record for identity and standing. Reconcile the
existing presence decision and lifecycle claim rather than retaining independently
written identities and statuses. Derived indexes and composed reads are allowed,
but they do not own another claim state machine.

Claims identify their registration, intended or resolved subject, standing and
attachment-specific session facts. Execution records own birth identity, placement,
scope, signalling, completion and cleanup. Subject records own durable session
association, parentage and owner/run/label, not attachment authority.

Remove the embedded process snapshot from claim storage and remove `sync_claim`.
A composed read may return all three records, but must not write copies back into
attachment storage. This replaces synchronisation obligations with explicit joins.

Accepted execution intent carries metadata before attachment. At grant, nonempty
accepted launch metadata takes precedence over the continued subject's metadata;
when no metadata is supplied, the continuing subject retains its metadata. Apply
this once at attachment, not on every process update. Empty metadata does not
express a clear operation; adding such an operation is out of scope. Parentage in
the spawn edge remains historical provenance rather than a second mutable subject.

### 5. Default rows are joined views rooted in registrations

Project one execution row per registration included by the default view's existing
lifecycle/history filters. Enrich it with its reserved or granted claim, subject,
reported facts and verified placement. Join it into topology through that placement;
a pane with a represented registration does not also render as an unrelated pane.
Unregistered panes remain ordinary topology rows. Unplaced registrations remain
visible as unplaced; detached subjects alone do not create default-view rows.
Revoked executions that never presented are hidden from the default view for now.

A derived row record composes the registration, optional claim, optional subject
and placement. It is a view, not a durable row store or another owner of placement.
Topology links may name registrations; joins must still validate placement's
backend generation. Choose the concrete public DTO layout during implementation
without publishing competing authoritative placement copies.

First presence enriches the same registration row instead of merging a subject row
and a pane row. A claim contributes state and authority, not another display unit.
Restart can have predecessor and successor rows because these are two executions,
subject to the view's existing history filtering. Do not collapse executions just
because their claims name the same subject. Frozen actions preserve the exact
registration and, where applicable, claim selected by the operator.

Hiding pending subjects alone is rejected as a general solution: reserved claims
must join correctly too. Subject-first and claim-first views may be introduced
separately, without changing the default row unit.

### 6. Observation publishes the join, not private launch data

Publish public execution summaries sufficient to render registrations before any
presence: registration identity, lifecycle state, verified placement and optional
attachment/subject references. Include recipe and container launches without spawn
edges. Publish reservation and grant distinctly so consumers do not infer authority
from a pending join. Existing subject-oriented reads remain subject-oriented;
execution observation must not require a subject or a spawn edge.

Do not infer joins from title, timing, pane ordering or foreground process alone.
Do not expose recipes, environment values, launch correlation or completion keys.
Regenerate producer and client schemas and advance the wire revision beyond the
wire-18 checkpoint when the observation document changes; both peers require an
exact match. Keep one authoritative publication, not a client-maintained mirror.

### 7. Lifecycle effects address the execution

Claiming a launch, capture, completion, exit wake, sweep, withdrawal, termination
and pane cleanup address registrations. An operation that resolves a subject also
carries the exact claim it saw, so a later grant cannot redirect that operation.
The spawn edge composes parent provenance onto the child's registration and binds
the child's subject at grant. No fallback lookup resolves one identity kind as
another, and no execution effect depends on having a spawn edge.

### 8. Ordered session boundaries establish and release claims

Radar mints claim IDs. For a verified registration with a matching reservation,
presence resolves that reserved claim and returns its ID; grant fulfils the same
claim rather than replacing it with a client-minted one. No claim-ID environment
variable is needed. For an ordinary attachment Radar creates and returns the claim.
The client persists the returned identity for subsequent reports and exact release.
A returned reserved or pending claim ID does not imply granted authority.

Session shutdown releases the exact claim; session start sends registration and
session, then persists Radar's returned claim ID for heartbeats and exact release.
A same-session reload releases the old claim and grants a new one without changing
registration or subject identity. Carry session-start reason as intent/context:
startup, reload, new, resume or fork, not as an identity or retry token.

Rely on Pi's ordered, awaited shutdown/start hooks. Preserve that promise chain
through Radar and sequence lifecycle work per registration. Source verification
found Radar's event wrapper discards the handler promise, despite an async
shutdown callback, and start presence is fire-and-forget. Fix those owning seams
instead of designing around an upstream reload race. A quit reason alone does not
prove process exit or end the registration.

Retries of an unanswered start recover the applicable recorded claim; heartbeats
and releases naming a terminal claim cannot resurrect it or affect its successor.
Use existing request/decision recovery rather than introduce a mandatory separate
attachment-attempt subsystem. Do not infer a new session boundary from a new
client claim ID. Remove generic previous-claim replacement and its Reserved skip:
Radar resolves the reservation directly, and exact predecessor release cannot
cancel another claim or another registration's reservation.

A separate reservation hold was considered and rejected: exclusion and grant
eligibility would still need distinct checks, while adding another domain concept.
Reservation fits the same proposed-attachment lifecycle as pending and denied
claims. Injecting a reserved ID into the child was also rejected: Radar can resolve
it from the verified registration and return it directly.

### 9. Standing exposes semantics; transitions own relational checks

Represent standing as an enum with derived semantic predicates where callers
share a question, such as grants_authority, reserves_attachment and is_terminal.
Do not persist independent authority/exclusivity flags or build a configurable
state-policy matrix. Keep explicit state matches inside lifecycle transitions
where the distinction matters.

A grants_authority predicate expresses only standing eligibility. A central
granted-attachment resolver must also use the maintained subject attachment index
and registration/process evidence appropriate to the operation. Ordinary callers use that resolver rather than
assembling partial authorisation from predicates.

Do not define a generic is_exclusive boolean: a reservation blocks competing new
attachments while preserving an incumbent grant; a grant prevents a successor
from granting before release while permitting successor reservation. Reserve and
grant enforce these relational rules once under the transition owner.

### 10. Claim release and registration finalisation are shared transitions

Session shutdown is the normal exact-claim release path. Verified process exit is
the non-graceful fallback: record execution end and settle every outstanding claim
on that registration through the same claim transition. A failed launch enters
shared explicit registration finalisation, including reserved/pending claim
cleanup and verified launch-owned resource cleanup. Keep execution and attachment
history. Do not use Rust Drop for fallible durable work or external effects.

Remove admission-time release_if_gone and opportunistic process observation to
repair claim ownership. Observation updates execution/claim state through its
own lifecycle transitions; queries read maintained indexes. If an update is
missing or delayed, diagnose the owning lifecycle path rather than reconstruct
state for each query. No new heartbeat-expiry or missed-hook reconciliation
subsystem is required.

Use existing durable transition and request recovery seams for idempotent
finalisation, including interrupted cleanup. Do not add per-function journals,
completion flags or a new execution engine here. The cross-daemon durable work
and terminal-response policy is a subsequent refactor recorded in plan.md.

### 11. Reservation mismatch fails the intent, not process registration

If this registration presents a subject/session inconsistent with its own reserved
claim, settle that reservation and fail the intended restart/continuation with a
retained diagnostic visible in the TUI. Then use ordinary attachment rules for the
actual session if it is otherwise eligible. Never report the intended action as
successful merely because the actual session attached. Do not automatically retry
the failed intent. A reservation belonging to a different registration remains an
exclusion conflict and must not be cancelled by this operation.

## Implementation Checks and Scope Boundary

- Verify the hook wrapper returns async handler promises, session boundaries await
  their owned work, and existing transport-failure handling does not silently
  abandon a queued release or claim success. No infinite wait or new automatic
  intent-retry mechanism.
- Verify same-session reload, lost first reply, terminal-claim heartbeat and stale
  release through the client/control boundary without weakening the hook-ordering
  contract or introducing speculative attempt identities.
- Verify no successor dispatch before verified predecessor exit/release. A grant
  check still refuses an incumbent conflict, but no early-successor retry workflow
  is required by the normal restart sequence.
- Measure execution-backed publication before performance optimisations. The
  maintained attachment index is required by ownership semantics, not an optional
  speculative performance cache.
- The wider execution-policy audit found shared durable request settlement only
  on some paths, procedural observation effects and notification-only cursors.
  plan.md records that subsequent scope; do not require universal transactional
  event consumption as a prerequisite or claim it is already implemented here.

Settled operational choices: hide revoked never-presented executions; accept the
one-time row-key selection/fold reset; keep the unreleased Radar 0.12.0 and
pi-radar 0.16.0 versions while advancing the changed wire to 19. Unsupported alpha
state is reset, not migrated. Restart already passes its prepared execution to
spawn dispatch in the checkpoint; verify that invariant through rework rather
than introducing another execution reservation.

## Risks / Trade-offs

- Separate grant and reservation add attachment-index state → serialize transitions and
  cover incumbent authority, competing grants, cancellation and replay through
  behavioural tests; do not add another authority store.
- Registration-rooted projection changes row identity and action targeting → test
  launch-to-presence row continuity, unplaced executions, pane moves and frozen
  actions, including launches without spawn edges.
- Removing snapshots changes persistence and reads → current versions only;
  exercise replay and composed reads rather than preserving old decoders.
- Metadata applies at grant rather than reservation → execution intent remains
  readable beforehand; intentional fresh-subject allocation remains deferred.

## Implementation and Validation Plan

Reopen the constructor, restart and row criteria the first checkpoint did not meet.
Land shared execution/attachment ownership, independent reservation transitions and
registration-rooted observation/projection in coherent slices, each with its own
behavioural coverage and documentation. Retain the initial gate as historical
checkpoint evidence, not a pass for revised semantics.

Validate stale release, competing grant and restart failure paths at the real
control seam. Prove the key guards non-vacuous with focused negative controls.
Exercise live `L`, child launch, continuation, restart and branch close, then run
the full Rust/SDK/OpenSpec gate and checked Nix build on the revised tree.

## State and Version Policy

Only current record versions and the exact current wire revision are supported.
Advance changed record versions, refuse unsupported state at startup, and use
`radar reset --hard` for alpha state reset. No migrations, aliases, dual readers,
compatibility fallbacks or cutover work are part of this change.
