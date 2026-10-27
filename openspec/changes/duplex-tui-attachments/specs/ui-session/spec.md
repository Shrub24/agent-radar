# Spec Delta

## MODIFIED Requirements

### Requirement: Attachment establishes a fresh ordering baseline
Open and reconnect SHALL establish a fresh attachment UUID and action counter starting at zero, with the first action one. Attachment registration and its primary publication receiver SHALL be established together in one actor transition. Readiness SHALL require coherent baseline adoption, not merely upgrade acknowledgement or attachment acceptance. Old-attachment responses SHALL be discarded. Attaching SHALL NOT reset canonical state or the actor's publication counter.

#### Scenario: Reconnect
- **WHEN** a frontend loses its connection and reconnects
- **THEN** it uses a fresh attachment and complete baseline, rejects old responses and does not compare publication counters across daemon restarts

#### Scenario: Another dashboard attaches
- **WHEN** a dashboard attaches to an existing session
- **THEN** its baseline reflects the current canonical state without resetting selection or filter

### Requirement: Subscription starts from a gap-free current baseline
The combined attachment opener SHALL establish exactly one connection-owned primary publication receiver and an authoritative binary baseline, with no separate attach-to-subscribe operation. Transitions after establishment SHALL be available through the baseline or a subsequent newer head without needing unrelated input. Failed upgrade or baseline delivery SHALL release the attachment and its resources. Publication IDs SHALL be used for local stale-delivery rejection, not history replay.

#### Scenario: Transition during establishment
- **WHEN** the actor changes after establishment and before baseline transmission
- **THEN** the baseline or a subsequent head conveys that transition or a newer coherent state

#### Scenario: Baseline write fails
- **WHEN** actor establishment succeeds but acknowledgement or baseline delivery fails
- **THEN** the connection releases its attachment, presenter claim and observation demand without awaiting a second client operation

## ADDED Requirements

### Requirement: Attachment liveness gates actor admission
Connection loss SHALL release an attachment lease without blocking a transport executor or requiring free space in the ordinary actor mailbox. The actor SHALL check lease validity before admitting attachment actions and SHALL reap released attachment resources during normal processing. Cleanup SHALL be scoped to the released attachment identity. Release SHALL NOT cancel a domain operation already admitted.

#### Scenario: Queued action outlives its connection
- **WHEN** an action is queued, its lease is released, and the actor then considers it
- **THEN** it is not admitted as new work from a live attachment

#### Scenario: Mailbox full during disconnect
- **WHEN** the ordinary actor mailbox is full as a connection disappears
- **THEN** release is nonblocking and cleanup remains discoverable without a successful ordinary detach enqueue

#### Scenario: Old cleanup after reconnect
- **WHEN** cleanup for an old UUID is observed after a fresh attachment exists
- **THEN** it cannot release the new attachment or its resources

### Requirement: Optimistic navigation preserves ordered intent
Frontends MAY navigate optimistically but SHALL submit semantic actions in a bounded FIFO with their displayed gate, without latest-wins input coalescing. Commands SHALL capture displayed row IDs. New gates SHALL fence unsent stale input and key sequences; coherent gate adoption MAY restore authoritative selection over unaccepted speculation. Older heads SHALL NOT replace newer adopted state; ordinary accepted-action barriers SHALL remain except for this gate reconciliation.

#### Scenario: Rapid navigation and command
- **WHEN** a frontend moves optimistically from A to B and immediately invokes a row-dependent command before receiving a publication
- **THEN** navigation is submitted first and the command identifies B, rather than silently capturing A or whichever global selection exists later

#### Scenario: Modal overtakes speculation
- **WHEN** a new gate arrives while old-gate navigation is unsent or awaiting an answer
- **THEN** stale input is fenced immediately and coherent authoritative reconciliation may discard the unaccepted speculative selection without mixing payloads from different heads

### Requirement: Reply loss is distinct from admission refusal
The transport SHALL distinguish definite enqueue rejection from unobserved results after enqueue. Timeouts SHALL NOT prove non-execution. Identical retries under a valid attachment MAY use retained receipts. Reconnect SHALL NOT automatically repeat uncertain actions under fresh IDs. Recovery SHALL use known durable request IDs; exact recovery is not guaranteed when the ID was never learned.

#### Scenario: Mailbox rejects an action
- **WHEN** the actor mailbox refuses enqueue
- **THEN** the transport reports definite non-admission without claiming an action transition occurred

#### Scenario: Answer lost after enqueue
- **WHEN** the actor accepted a message but its reply becomes unavailable
- **THEN** the client treats the outcome as potentially applied, not safe to repeat as a new command

#### Scenario: Fresh attachment lacks durable correlation
- **WHEN** a connection is lost before the frontend learns the resulting domain request identity
- **THEN** reconnect adopts current state without claiming that the old action counter guarantees exact durable result recovery
