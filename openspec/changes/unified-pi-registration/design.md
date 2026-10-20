## Context

The agreed product scope is in `scope.md`. Existing seams include `agent.register`, the sequenced `agent.context` publisher, `execute_enroll`/`record_enrollment`, `enroll_subject`, launch `fence_registration`, daemon post-launch binding and reconciliation, and pi-radar's `publication.subject()` shared runtime.

Today these seams distinguish launch and adoption too strongly, carry Pi-session markers, and retain execution-centric assumptions in restart and parent projection. Handoff recipients already use subject IDs; changing attachment semantics must preserve that durable communication identity rather than copy messages into a new inbox.

## Goals / Non-Goals

**Goals:** one verified binding implementation; first-class manual adoption; session-bound child/inbox/history continuity; removal of active breadcrumb machinery.

**Non-goals:** duplicate-history merging or takeover, fork investigation, general recovery controllers, assignment recovery, new resume commands, pane movement, richer cross-tab nesting, and a database or execution-ID migration.

## Decisions

### 1. Resolve subjects by session, attach processes separately

Retain daemon-issued subject IDs and associate them durably with canonical session UUIDs. A process registration retains its incarnation/birth identity and reports its current session. Its active managed association resolves through that session, not through an old session entry or pane.

On session switch, detach the previous subject and resolve/bind the selected one. Preserve the old subject's child relationships, inbox and history. Controls and publication must be fenced to the active subject attachment as well as process birth identity: unchanged PID does not mean unchanged agent authority.

Reuse existing execution records, generations and writer fencing where consumed. Record actual binding changes, not every heartbeat. No Pi-persisted subject/execution markers or new independent identity store.

### 2. Converge at the current report and binding seams

pi-radar requests management by default through its existing registration/context driver. Add only consumed typed report/result fields and additive capability negotiation. Generic baseline publishers remain baseline-only unless requesting management. Old daemons continue baseline service without simulated management success.

A shared daemon routine resolves an existing durable binding or validates supplied launch correlation before considering new adoption. It verifies process birth identity, pane occupant and fresh canonical session context, captures private execution context when binding, and commits under existing transition serialization. External reads occur outside the transition lock; revalidate the relevant association before commit.

Existing launch binding, explicit enrollment and reconciliation call this same implementation. Invalid correlation refuses rather than selecting the adoption branch. A new untracked session becomes an enrolled lead with null parent; a tracked session retains its recorded parent. Correlation is consumed for the admitted launch, not reapplied on every later session selection.

Repeated reports, lost replies and daemon restart resolve existing records before creating another subject or attachment. Accepted baseline context and unresolved management remain distinguishable. Reuse existing retry/heartbeat scheduling; do not add an enrollment pump. Context replay must not renew its lease merely to retry a pending binding.

### 3. Treat duplicates as unsupported, not as standby executions

The accepted attachment remains the incumbent. A second process reporting that session is baseline-visible, warns as duplicate and gets no managed identity or inbox access. Persist enough conflict state that daemon restart, heartbeat or extension reload cannot accidentally promote the stale duplicate after the incumbent disappears.

Reconsider only after a real session reload/resume from persisted state or a new process, and only when the incumbent is released or verified no longer attached. Failed scans and expired leases do not prove release. Use the Pi host's session-load signal if it distinguishes a fresh load from reconnect/reload; otherwise require process restart. No history comparison, merge or forced takeover mechanism.

The behavioral contract is settled. Verifying the host signal selects between these defined implementation paths; it is not permission to invent a weaker admission rule.

### 4. Resolve parents and inbox rights through current session subjects

Logical parentage remains durable Radar state; execution-specific spawn edges remain immutable history. Restore basic parent display/selection from logical subject relationships rather than requiring the original parent registration to survive. Keep physical rows in their own tabs; an out-of-tab parent reference is sufficient.

Communication reads/acknowledgments resolve the caller's current accepted subject. Reattachment to the same session preserves inbox access; switching to another session revokes the prior association. Old sender/recipient fields and acknowledgments are not rewritten. Duplicates have no subject authority.

Herdsman must invalidate cached subject/parent/communication handles on session change. The SDK supplies current identity; it does not restore assignments, declare handoffs valid or authorize worker closure.

### 5. Restart the subject's session using observed execution context

Capture the verified executable/argv/cwd/environment once when establishing the execution binding for either origin. Keep original launch intent as operation history, not as a separate privilege requirement. Freeze fresh current subject/session/attachment before destructive restart and revalidate at the effect boundary.

Reuse `resume_launch` argument reconstruction. Remove stale session selectors and one-use credentials; refuse ambiguous initial prompts before termination. Restart stays anchored to the old lead pane/tab where available. It does not refresh direnv, credentials or the original shell launcher. A manually resumed lead elsewhere retains identity and relationships with no automatic movement.

### 6. Delete the competing marker path

Stop appending/reading session launch markers and deriving routine matching/stale/contradictory marker projections. Remove marker-only environment plumbing, host API requirements, tests and documentation. Ignore historical custom entries. Retain released optional fields/schema exports where needed for compatibility, but no active branch exists solely to populate them.

Retain initial private launch correlation, process birth verification, writer leases and daemon-side effect fencing. These protect real boundaries; removing breadcrumbs does not remove them.

### 7. Adapt existing records without merging authority

Reuse an unambiguous existing session-to-subject association; preserve history, subject IDs, inboxes and parentage. Subjects missing session evidence remain unresolved until verified context supplies it. Multiple existing subjects naming the same session are an explicit legacy conflict, not an automatic merge or a reason to delete history. Duplicate records are not the supported concurrent-session case.

Use existing records and storage facilities for the mapping/attachment changes. No genesis replay, new database or broad backfill/discovery service. Record the rollback limit: an old daemon cannot be assumed to understand new attachment semantics or persisted fields. Verify format readability and document supported rollback before deployment rather than promise automatic downgrade safety.

## Risks / Trade-offs

- Session changes can invalidate pending controls with the same PID: current attachment is a required fence.
- Mutable application caches can publish for the wrong session: update/invalidate at the real pi-radar and Herdsman boundaries, not only the daemon DTO.
- Duplicate admission requires evidence of fresh loading: process restart is the conservative fallback if the host cannot supply it.
- Existing ambiguous subjects cannot safely be merged: surface them without introducing repair tooling until real data warrants it.
- Observed context can contain secrets or expired values: keep recipes private and document the reproduction promise.
- Cross-tab children remain an edge case: basic links survive, topology stays untouched, gathering remains in `ideas.md`.

## Migration Plan

Deliver in small vertical slices with their own behavior checks and docs: manual adoption through the real reporter; session switching/reattachment and effect fencing; duplicate admission; relationship/inbox continuity and minimal projection; observed restart and marker deletion. Extend existing tests rather than build a separate conformance framework.

Ship daemon capability support before publishing the SDK that requests it. Coordinate Herdsman's current-subject/cache handling, update consumers, and run focused real Pi acceptance. Do not call that cutover complete on Rust tests alone. Rollback is subject to the actual persisted-format checks above.

## Open Questions

No unresolved product choices block the task list. The Pi fresh-load event and legacy stored mappings require bounded implementation checks with defined conservative outcomes; fork internals, legacy repair tooling and pane migration remain deferred.
