# Spec Delta

## Purpose

Define safe, durable pane closure for managed children created by the daemon, without transferring assignment authority or claiming process termination from a mux close.

## ADDED Requirements

### Requirement: Close targets an exact daemon-created child pane

A close request SHALL identify the daemon's spawn edge and its bound child `(source, incarnation)`; it SHALL NOT carry a caller-chosen pane id. The daemon SHALL refuse before dispatch if the edge is absent, unbound, already closed, or names a different child, and SHALL refuse if the edge's recorded location carries no pane. Children named by labels, titles, session UUIDs or pane position are never eligible.

#### Scenario: Unbound edge
- **WHEN** a caller requests close for an edge with no bound child
- **THEN** the request is refused and no backend effect occurs

#### Scenario: Caller names a pane
- **WHEN** a request supplies a pane id independently of the recorded edge
- **THEN** it is rejected as malformed and nothing is dispatched

### Requirement: Close requires fresh occupant verification

Immediately before dispatch the daemon SHALL verify that the recorded pane still exists within its recorded containment and that the pane's current foreground process carries the birth identity held for the bound child's registration. It SHALL refuse, with no close effect and no accepted record, when that evidence is missing, inconclusive or different, and SHALL NOT accept pane presence or session lineage as occupant proof.

#### Scenario: A different process occupies the pane
- **WHEN** the foreground process birth identity differs from the bound child's registered identity
- **THEN** close refuses without dispatching and leaves that occupant untouched

#### Scenario: Occupant evidence unavailable
- **WHEN** foreground evidence is inconclusive or the registration holds no process claim
- **THEN** close refuses without dispatching and records no accepted close

#### Scenario: Verified child
- **WHEN** the recorded pane holds the exact bound child's birth identity within its recorded containment
- **THEN** close may be dispatched for that pane

### Requirement: Close is durable and idempotent

The daemon SHALL persist the operation identity and target before dispatch. Replaying the same request id with identical content SHALL return the recorded result without dispatching again; the same id with different content SHALL be refused. A possibly dispatched close without confirmation SHALL remain unknown and SHALL NOT be retried automatically or routed through another backend.

#### Scenario: Identical replay
- **WHEN** the caller repeats a close request with its original request id and content
- **THEN** the daemon returns the stored outcome without a second close dispatch

#### Scenario: Lost result after dispatch
- **WHEN** close may have been dispatched but its outcome cannot be confirmed
- **THEN** the durable result remains unknown and no retry occurs

### Requirement: Pane closure does not assert process termination

The operation SHALL report only the mux pane-close outcome. It SHALL NOT claim that the foreground process or process group exited, and SHALL NOT synthesize an execution-status or assignment-state transition. Missing pane evidence after an unknown result remains unresolved, not confirmed closure.

#### Scenario: Confirmed pane close
- **WHEN** the backend confirms it closed the target pane
- **THEN** the result records pane closure and makes no claim about process exit or assignment completion

#### Scenario: Pane disappears without a confirmation
- **WHEN** the pane is absent after a possibly dispatched close but no valid backend confirmation exists
- **THEN** the outcome remains unknown or unresolved and is not reported as a confirmed close

### Requirement: Close intent does not change assignment state

A close request MAY carry `complete` or `cancel` intent for audit and display. The daemon SHALL record that intent but SHALL NOT write an assignment fact or alter agent-advertised execution status. The assignment owner remains responsible for deciding and publishing assignment completion.

#### Scenario: Complete intent
- **WHEN** a caller requests pane close with `complete` intent
- **THEN** the operation is labelled complete-intent, but assignment state is unchanged by the daemon

#### Scenario: Cancel intent
- **WHEN** a caller requests pane close with `cancel` intent
- **THEN** the operation is labelled cancel-intent, but assignment state is unchanged by the daemon

### Requirement: Spawn topology remains truthful after close

A close SHALL preserve the original spawn edge and child binding. The edge SHALL not be deleted or unbound. Later missing pane evidence SHALL read unresolved unless the close operation itself has a confirmed result; it SHALL not be interpreted as process exit.

#### Scenario: Read topology after close
- **WHEN** the caller reads an edge after a confirmed pane close
- **THEN** it still identifies the same parent and child, alongside the separately readable close result

#### Scenario: Missing pane without confirmed close
- **WHEN** the recorded pane is absent and no close result confirms the effect
- **THEN** topology reads unresolved and no process-exit claim is made

### Requirement: Close is restricted to daemon-managed children

The daemon SHALL close only a pane named by its own spawn edge with a bound child. It SHALL not adopt or control foreign panes, leads, standalone agents, or children spawned outside the daemon, and SHALL not resume or restart the child. Managed close SHALL not be advertised as a capability while any eligible target necessarily refuses.

#### Scenario: Foreign child
- **WHEN** a caller targets a registered child with no daemon-authored spawn edge
- **THEN** the operation is refused without backend effects

#### Scenario: Not yet implemented
- **WHEN** the verification path is not implemented or not advertised
- **THEN** `child.close` refuses explicitly and no capability is claimed
