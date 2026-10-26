# Spec Delta

## MODIFIED Requirements

### Requirement: Work is admitted by lane with reserved capacity
The transport SHALL admit control requests, publication ingest and waiting reads against separate limits, and SHALL keep capacity for control requests that ingest and waiting reads cannot use. UI update waits SHALL use waiting admission, and UI submissions SHALL have bounded admission. A request over its limit SHALL receive a typed busy answer rather than consume unbounded resources or delay another lane.

#### Scenario: Ingest saturated
- **WHEN** publication ingest has reached its limit
- **THEN** a lifecycle control request is still admitted

#### Scenario: Waiters saturated
- **WHEN** waiting reads have reached their limit
- **THEN** further waits are refused as busy and control requests are unaffected

#### Scenario: UI backlog saturated
- **WHEN** the UI actor cannot accept another action within its bound
- **THEN** submission receives an explicit retryable admission answer without being represented as an applied action

### Requirement: Waiting reads do not occupy handler capacity
A read waiting for journal events or a UI publication SHALL wait asynchronously in the transport and SHALL NOT hold synchronous handler capacity. Journal results and cursors SHALL retain their existing semantics. UI waits SHALL return a current publication or an explicit timeout, detached or shutdown answer. Awaiting a UI actor reply SHALL NOT block a transport worker thread.

#### Scenario: Event during a wait
- **WHEN** a fact commits while a journal reader waits
- **THEN** the reader receives that event before the wait ends and no handler capacity was held during the wait

#### Scenario: Wait ends empty
- **WHEN** a journal wait ends with no new event
- **THEN** the reader receives an empty list and the same cursor

#### Scenario: UI clients waiting
- **WHEN** several presentations wait for a new UI publication
- **THEN** those waits do not occupy the synchronous handler pool and lifecycle controls remain admissible within their own bounds

## ADDED Requirements

### Requirement: UI connections are reusable without read-side head-of-line blocking
The transport SHALL support repeated UI exchanges on a connection within existing framing, authentication, timeout and admission rules. Frontends SHALL separate ordered action submission from pending update reads so a long read cannot hold up an action. Connection loss SHALL preserve established attachment/reconciliation and uncertain-submission semantics rather than silently replay effectful commands.

#### Scenario: Action while waiting for a publication
- **WHEN** a frontend has an update read outstanding and the user navigates
- **THEN** its action uses the independent reusable action path without waiting for the read to finish

#### Scenario: Reuse ends
- **WHEN** an idle timeout or transport failure closes a reused connection
- **THEN** the frontend recovers through defined connection/attachment handling without treating an uncertain action as new
