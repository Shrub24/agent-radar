# Spec Delta

## MODIFIED Requirements

### Requirement: Work is admitted by lane with reserved capacity
The transport SHALL admit control requests, publication ingest and waiting reads against separate limits, and SHALL keep capacity for control requests that ingest and waiting reads cannot use. UI action submissions SHALL have bounded admission. UI subscriptions SHALL have an explicit separate subscription limit and count toward total connection capacity; their lifetime SHALL NOT consume control or waiting-read permits. A request over its limit SHALL receive a typed busy answer rather than consume unbounded resources or delay another lane.

#### Scenario: Ingest saturated
- **WHEN** publication ingest has reached its limit
- **THEN** a lifecycle control request is still admitted

#### Scenario: Waiters saturated
- **WHEN** waiting reads have reached their limit
- **THEN** further waits are refused as busy and control requests are unaffected

#### Scenario: UI backlog saturated
- **WHEN** the UI actor cannot accept another action within its bound
- **THEN** submission receives an explicit retryable admission answer without being represented as an applied action

#### Scenario: Subscription capacity saturated
- **WHEN** the UI subscription limit is reached while reserved control connection capacity remains
- **THEN** a further subscription is refused as busy before acceptance and lifecycle controls remain admissible

### Requirement: Waiting reads do not occupy handler capacity
A read waiting for journal events SHALL wait asynchronously in the transport and SHALL NOT hold synchronous handler capacity. Journal results and cursors SHALL retain their existing semantics. Awaiting a UI actor reply SHALL NOT block a transport worker thread. UI publication delivery SHALL use the dedicated subscription requirements below rather than a UI long-poll method.

#### Scenario: Event during a wait
- **WHEN** a fact commits while a journal reader waits
- **THEN** the reader receives that event before the wait ends and no handler capacity was held during the wait

#### Scenario: Wait ends empty
- **WHEN** a journal wait ends with no new event
- **THEN** the reader receives an empty list and the same cursor

## ADDED Requirements

### Requirement: UI subscriptions push typed frames independently of action RPCs
The existing authenticated socket SHALL support ui.subscribe: an acceptance/baseline frame followed by typed publications correlated with request and attachment. Actions, receipts and content fetches SHALL use independent reusable one-response RPCs. UI updates SHALL require no read renewal or wait_ms. Existing domain reads SHALL remain unchanged; this change SHALL NOT introduce a generic subscription framework.

#### Scenario: Action during an idle subscription
- **WHEN** a frontend's subscription is open with no new publication and the user navigates
- **THEN** the action uses the independent reusable RPC path without waiting for a subscription frame

#### Scenario: Publication without input
- **WHEN** the session publishes while the frontend is idle
- **THEN** the server pushes a publication frame without another client read request or a periodic renewal

### Requirement: Subscription writers are bounded and asynchronous
Subscriptions SHALL use connection-owned async writers with bounded write deadlines and buffering, not a blocking cloned-writer mutex. Their lifetime SHALL NOT hold handler/control capacity or block the actor/other clients. They MAY skip intermediate publications. Ordinary RPC idle expiry SHALL NOT end a healthy subscription, and peer closure SHALL be detected independently of publications.

#### Scenario: Slow subscriber
- **WHEN** a frontend stops consuming publication frames
- **THEN** retained publication buffering stays bounded, the actor and other subscribers continue, and a blocked write exceeding its deadline ends that subscription

#### Scenario: Quiet live subscriber
- **WHEN** a subscription remains connected without publications beyond the ordinary RPC idle timeout
- **THEN** it remains subscribed without requiring idle read renewals

### Requirement: Subscription termination releases presentation resources
Peer closure, detach, write failure/deadline and shutdown SHALL release subscription capacity. Primary subscription loss SHALL invalidate its dashboard/sidebar attachment and release editor ownership, retaining the query. Recovery SHALL use a fresh attachment/baseline, without automatically replaying uncertain actions under new IDs. Separately claimed presenters SHALL remain independent. Closure SHALL NOT imply process exit or domain cancellation.

#### Scenario: Peer closes while idle
- **WHEN** a frontend closes its subscription while no publication is pending
- **THEN** the transport detects closure and releases subscription capacity and attachment ownership without waiting for the next publication

#### Scenario: Origin disconnects with a live presenter
- **WHEN** a sidebar loses its primary subscription after a separate presenter has claimed its interaction
- **THEN** the sidebar attachment is invalidated but the presenter can still resolve the interaction

#### Scenario: Shutdown with idle subscriptions
- **WHEN** the daemon shuts down while subscription tasks are waiting
- **THEN** those tasks terminate and release capacity without waiting for a publication or a long-poll timeout
