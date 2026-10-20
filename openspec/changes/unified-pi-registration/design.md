## Context

Pi subjects follow canonical session UUIDs, while process registrations identify verified OS process incarnations. The partial implementation still sequences `agent.register`, `agent.context` and sweep adoption, freezes pane location in registration, and automatically promotes previously rejected duplicates. Replace that choreography rather than add another enrollment controller.

## Goals / Non-Goals

Deliver one presence report, durable claim decisions, explicit graceful release, automatic reconnect recovery, mutable location, and session-owned subject continuity. No transcript breadcrumbs, signing, workflow framework, storage replacement, pane movement, general incoming command transport or compatibility aliases. This is an alpha contract change; daemon, pi-radar and consumers update together.

## Decisions

### 1. One public report, three internal responsibilities

pi-radar sends a flat presence report: source/incarnation and process birth identity, session UUID, session-load claim ID, current location, optional launch correlation/managed metadata, and acknowledged registration/subject IDs when known. Radar resolves immutable process registration, mutable process context and session claim internally. Rename process-record `agent_id` to `registration_id` throughout the public contract and its consumers. The immutable identity is the process birth tuple (boot ID, PID, start ticks); changing location does not create another registration.

Return registration ID, claim ID and standing (`granted`, `duplicate`, `pending`), with subject ID only when granted. IDs returned earlier are correlation, not proof sufficient to overwrite durable grants. Validate supplied launch correlation; invalid correlation never falls back to adoption. Reuse the same binding implementation for manual sessions, launches and recovery.

### 2. Claims distinguish fresh session loads from retries

Create one claim ID at each Pi `session_start` (startup, reload, new, resume or fork). All renewals, transport retries and daemon reconnects for that loaded session reuse it. Persist its decision in Radar. A duplicate decision is sticky even after incumbent release or daemon restart; only a new session-load claim can be considered again. Transient observation/transport failures are pending, not duplicate decisions and not consumed opportunities.

pi-radar uses one report function and existing retry scheduling, retaining acknowledged protocol state privately, never in the transcript. A reconnect must not generate a new claim or resurrect a released one. The same claim ID cannot name different process/session identity. Periodic reassertion is contact, not fresh-load evidence.

### 3. Explicit release, persistent subjects

`session_shutdown` sends a bounded awaited release naming registration ID and exact claim ID (subject ID may be checked for consistency). Release is idempotent and only affects that grant. A delayed release cannot revoke a newer claim. Release removes session authority, not process registration or the durable subject, children, inbox or history. Retain unresolved release for retry where the process survives.

A new claim from the same verified registration may supersede its previous attachment, so loss of a shutdown response cannot strand a session switch. A released claim cannot be reacquired by reasserting it. A live process with no grant is unattached, not presumed exited.

### 4. Recovery ordering and crash backstop

Recover durable grants before admitting competing claims. Verify holder birth identity directly: absent or reused PID releases the attachment as verified exit; unreadable identity, expired heartbeat and missing pane do not. The sweep detects process exit, not new claim opportunities. When durable authority is missing or ambiguous, report pending rather than reconstructing history from receipts or session contents.

Persist claim decisions before acknowledgment. Use existing private atomic-file and transition facilities, with recoverable ordering: the claim record tracks pending work and the resolved subject, so retries can finish an interrupted binding without allocating another subject. Record binding changes, not every contact renewal, in lifecycle history. One RPC does not imply cross-file atomicity. No SQLite, genesis replay or cryptographic receipts.

### 5. Mutable placement and effect fences

Location belongs to process context; validate it before physical controls. Session selection changes the active subject while process registration remains stable. Fence control dispatch and handoff identity to the current granted claim and execution, not merely PID. Preserve logical parent/child relationships and inboxes through reattachment; execution-specific provenance remains immutable. Keep rows in physical tabs, with a minimal parent reference when needed; never move panes automatically.

### 6. Presentation and consumer boundary

Baseline-visible duplicate registrations have no subject authority and show a compact inline warning distinct from stale process/binary warnings. pi-radar's shared runtime exposes current standing and invalidates cached subject/communication identity at session boundaries. Herdsman consumes this SDK and owns assignment/settlement policy, not registration or socket transport.

No session launch markers or injected execution identity remain. Observed private restart recipes retain executable/argv/cwd/env and strip initial prompts and one-use credentials; restart resumes the selected session in its existing placement. Generated schemas describe consumed public DTOs.

### 7. Existing state and rollout

Preserve unambiguous existing subject/session associations, inboxes, parentage and history. Conflicting legacy mappings remain pending; do not merge. Process-record renaming may require a bounded storage adapter separate from the breaking public rename; document actual rollback limits. Do not preserve obsolete alpha wire aliases. Publish daemon/SDK together, hand off the changed contract to Herdsman, and leave live operator smokes explicitly pending.

## Validation

Use a few composed flows: acquire/switch/release/resume with stable process registration and subject continuity; duplicate/incumbent exit/restart/still rejected/fresh load accepted; lost replies and daemon restart with exact replay and delayed release unable to revoke a new grant. Exercise mutable pane context and one inline duplicate rendering assertion. Use existing gates; no speculative transport matrix.

## Deferred

Scoped incoming event feeds and targeted requests, signed receipts, history merging, assignment recovery, pane gathering/migration and richer cross-tab trees. The existing bounded event journal remains notification transport, not a per-process reliable command queue.
