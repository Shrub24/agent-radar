# Spec Delta

## MODIFIED Requirements

### Requirement: Idempotent immutable handoff publication
The daemon SHALL publish handoffs as the typed handoff body in the shared agent-message lane, not a separate handoff mailbox. Repeating a message ID with identical sender, recipients, correlation, summary and content SHALL return the accepted message; conflicting immutable content SHALL be refused.

#### Scenario: Retry after a lost publication response
- **WHEN** a worker retries a handoff with the same ID and body after losing the reply
- **THEN** the common message lane returns the existing publication without a duplicate

### Requirement: Recipient inbox and identity continuity
Handoff publication SHALL retain its current parent-only policy: a verified managed worker may target only its resolved daemon-recorded parent subject. Delivery SHALL use the common subject mailbox, including verified successor recovery. Root, unrelated, unmanaged or unresolved targets SHALL be refused.

#### Scenario: Worker addresses another subject
- **WHEN** a worker tries to send a handoff to someone other than its recorded parent
- **THEN** publication is refused despite the common envelope supporting recipient sets

#### Scenario: Recover a pending handoff after restart
- **WHEN** the parent reconnects after a daemon restart
- **THEN** the common inbox returns every pending handoff without needing a notification replay

#### Scenario: Verified successor reads the subject inbox
- **WHEN** a verified successor takes over the parent subject
- **THEN** the same pending handoff is available through the message inbox
- **AND** a stale or unresolved execution is refused

#### Scenario: Managed sender targets its recorded parent only
- **WHEN** a verified worker publishes to its resolved daemon-recorded parent
- **THEN** publication is accepted, while root senders and different targets are refused

### Requirement: Separate durable receipt and retained retrieval
Handoff receipts SHALL use the common per-recipient message receipt and retained content retrieval. Receipt SHALL attest only to durable recipient capture, remain idempotent, and SHALL NOT replace immutable handoff content.

#### Scenario: Acknowledge after durable local capture
- **WHEN** the parent durably records the handoff then acknowledges its delivery
- **THEN** the message receipt persists and content remains retrievable during retention

### Requirement: Bounded storage and recoverable notification
Handoffs SHALL share the message lane's storage bounds, pending recovery and attention notifications. There SHALL NOT be a parallel handoff store or reliance on a raw-event pointer. Missing or coalescing attention SHALL NOT lose a pending handoff.

#### Scenario: Capacity is exhausted
- **WHEN** publication would exceed a message-lane storage or pending-recipient bound
- **THEN** it is definitely not accepted and no pending handoff is evicted

#### Scenario: Publication event is missed
- **WHEN** no handoff attention update reaches the parent, including after a missed historical pointer event
- **THEN** a later common inbox read still returns its pending delivery without a raw-event dependency

### Requirement: Handoff transport remains separate from lifecycle settlement
Radar SHALL own the common message delivery record and receipts. Herdsman SHALL retain assignment validity, worker eligibility and lifecycle disposition. Neither publication nor receipt SHALL authorize worker closure or settle the completion obligation.

#### Scenario: Handoff is published but not consumed
- **WHEN** a worker publishes and its parent has not durably captured the result
- **THEN** delivery remains pending and publication does not authorise worker closure

#### Scenario: Handoff is received but not settled
- **WHEN** the parent has durably captured and acknowledged a handoff
- **THEN** Herdsman's separate settlement and worker-closure decisions remain outstanding
