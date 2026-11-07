# Spec Delta

## ADDED Requirements

### Requirement: Messages express producer-addressed intent
The daemon SHALL provide one typed message lane independent of raw events and observations. Each accepted message SHALL have immutable identity, producer provenance, tagged body and a nonempty authorised frozen set of subject recipients. Raw journal movement alone SHALL NOT create agent notifications.

#### Scenario: Unrelated daemon event
- **WHEN** a container observation or operation transition is recorded without an explicit message producer
- **THEN** no message or recipient attention change is created

### Requirement: Publication is immutable and fanout is atomic
Retrying the same message ID and identical immutable content SHALL return the accepted message. Different content or recipients SHALL be refused. Publication SHALL persist one canonical recipient set before exposing success; interrupted publication SHALL NOT expose a partial accepted recipient set.

#### Scenario: Lost publish response
- **WHEN** a producer retries an accepted publication after losing the reply
- **THEN** it receives the existing message and no duplicate deliveries

#### Scenario: Crash during multicast publication
- **WHEN** the daemon restarts after publication was interrupted
- **THEN** either the message is absent or its full canonical recipient set is recoverable

### Requirement: Subject continuity governs access
Inbox, content, receipt and subscription access SHALL derive from current verified execution context. A verified successor SHALL recover the same subject's mailbox. A registration, claim, session, pane or label SHALL NOT replace stable subject addressing.

#### Scenario: Successor and stale execution
- **WHEN** a verified successor takes over a subject with pending work
- **THEN** it can retrieve that work and a stale execution cannot use its old grant

### Requirement: Inbox and receipt survive notification loss
The lane SHALL expose bounded pending descriptor pages and separate content retrieval. Each recipient's durable receipt SHALL be idempotent and independent of other recipients. Missed attention SHALL NOT remove pending work. Receipt SHALL mean only durable recipient-side capture, not domain settlement.

#### Scenario: Independent multicast receipts
- **WHEN** P receives and acknowledges a message also addressed to Q
- **THEN** Q's delivery remains pending and both subjects retain authorised retrieval during retention

#### Scenario: Capture before lost ack
- **WHEN** local capture succeeds but the receipt response is lost
- **THEN** redelivery is safe through message-ID deduplication and idempotent receipt

### Requirement: Pending content is bounded without silent eviction
The lane SHALL enforce payload, recipient, pending-delivery, record and aggregate-byte bounds before acceptance. Capacity failure SHALL be definite non-acceptance. Pending content SHALL NOT expire or be evicted to create room; fully received content MAY be pruned after the documented retention window.

#### Scenario: One recipient is full
- **WHEN** a recipient set includes a mailbox already at capacity
- **THEN** the entire new publication is refused without partial fanout or eviction

### Requirement: Recipient attention is derived delivery state
Heartbeat and subscription attention SHALL use the same recipient mailbox state. Pending work SHALL remain discoverable after daemon restart or failed consumption even if no new message arrives. Attention is not a raw-event replay cursor and SHALL NOT acknowledge messages.

#### Scenario: Restart before notification
- **WHEN** a message persists and the daemon stops before announcing attention
- **THEN** rebuilding the mailbox exposes the pending delivery on the next baseline or successful heartbeat
