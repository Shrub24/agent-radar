# durable-handoffs Specification

## Purpose

The durable path by which a managed worker hands its result to the parent Radar records for it: one immutable publication per handoff, a recipient inbox that survives restarts, and a separate receipt. Radar owns the record and its transport; it does not thereby complete an assignment, authorize a worker's closure or speak for Herdsman's settlement policy.

## Requirements

### Requirement: Idempotent immutable handoff publication
The daemon SHALL persist bounded handoff content under a stable UUID. Repeating the same publication ID with the same immutable sender, recipient, correlation, summary and payload SHALL return the existing record; reusing the ID with different immutable content SHALL be refused.

#### Scenario: Retry after a lost publication response
- **WHEN** a sender retries publication with the same ID and identical immutable content after losing the response
- **THEN** the daemon returns the already-published handoff without creating a duplicate
- **AND** a retry with different immutable content under that ID is refused

### Requirement: Recipient inbox and identity continuity
A managed recipient SHALL recover pending handoffs by inbox scan without relying on a notification. The first contract SHALL address only stable Radar subject IDs, and a verified successor SHALL access that same subject inbox. Unmanaged/session addressing is out of scope; stale or unresolved managed identity SHALL be refused.

#### Scenario: Recover a pending handoff after restart
- **WHEN** a recipient reconnects after a daemon restart
- **THEN** its inbox query returns every unacknowledged handoff addressed to its stable subject
- **AND** recovery does not depend on replaying a pointer event

#### Scenario: Verified successor reads the subject inbox
- **WHEN** a registration is attached to the managed subject of its current Pi session, with a verified process and a resolved identity
- **THEN** the daemon allows that execution to recover the same subject inbox
- **AND** a detached registration, a session it no longer publishes, or an unresolved identity is refused

#### Scenario: Managed sender targets its recorded parent only
- **WHEN** a managed sender publishes a handoff to its recorded `parent_subject_id`
- **THEN** the daemon accepts the target if the parent subject is resolved
- **AND** a root sender or any different target is refused

### Requirement: Separate durable receipt and retained retrieval
The daemon SHALL store the recipient receipt separately from immutable content. A receipt SHALL mean only that the recipient-side consumer durably recorded the payload. A recipient SHALL be able to retrieve the full content after acknowledgement, and repeated acknowledgement SHALL return the existing receipt without replacing it.

#### Scenario: Acknowledge after durable local capture
- **WHEN** the recipient has durably recorded the payload locally and acknowledges it
- **THEN** the daemon records an idempotent receipt separately from immutable content
- **AND** the recipient can still retrieve the full payload afterward

### Requirement: Bounded storage and recoverable notification
The daemon SHALL enforce per-payload and aggregate storage bounds without silently evicting pending handoffs. Pointer events SHALL reference durable communication records without embedding their payload; inbox scans SHALL remain the recovery source when a notification is missed. Acknowledged content MAY expire under the documented retention policy.

#### Scenario: Capacity is exhausted
- **WHEN** a publication would exceed the configured payload, pending-recipient, record-count, or byte bound
- **THEN** the daemon returns a definite non-accepted capacity response
- **AND** it does not silently evict a pending handoff

#### Scenario: Publication event is missed
- **WHEN** a recipient misses or coalesces a pointer event
- **THEN** a later inbox scan still returns the durable pending handoff
- **AND** the event contains no payload body

### Requirement: Handoff transport remains separate from lifecycle settlement
Radar SHALL own handoff publication, durable payload, recipient accounting and receipt state. Herdsman SHALL retain authority over assignment validity, worker eligibility and lifecycle disposition. Neither publication nor receipt SHALL authorize worker closure or settle the Herdsman completion obligation.

#### Scenario: Handoff is published but not consumed
- **WHEN** a worker's completion handoff is published and the parent has not durably recorded it
- **THEN** the handoff remains pending and recoverable
- **AND** publication alone does not authorize the worker's closure

#### Scenario: Handoff is received but not settled
- **WHEN** the parent records the handoff and acknowledges receipt
- **THEN** Herdsman may continue its separate wrapup and lifecycle policy
- **AND** the receipt is not treated as settlement success
