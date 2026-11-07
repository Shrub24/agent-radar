# managed-child-stop Specification

## Purpose
Define safe, durable pane closure for managed children created by the daemon, without transferring assignment authority or claiming process termination from a mux close.

## Requirements

### Requirement: Close targets an exact daemon-created child pane
A spawn-close request SHALL identify the spawn relationship and bound child, never a caller-chosen pane. The daemon SHALL authorize that relationship, resolve the exact eligible execution and its current verified placement, and invoke common managed close. Historical create location SHALL NOT override verified current placement or authorize a replacement execution.

#### Scenario: Caller names a pane
- **WHEN** a caller supplies a pane independently of its authorized child relationship
- **THEN** the malformed request is refused without effects

#### Scenario: Moved child
- **WHEN** a bound child's pane has a verified new tab location
- **THEN** close targets its current execution and pane without rewriting historical spawn provenance

#### Scenario: Unbound edge
- **WHEN** a caller targets an edge with no eligible bound child
- **THEN** it is refused without signalling or removing a guessed pane

### Requirement: Close requires fresh occupant verification
Before destructive effects, close SHALL validate the exact claim, pane association and backend generation. A replacement execution or unrelated current occupant SHALL NOT be terminated using an old child identity. A live eligible execution SHALL use common managed termination before pane removal; an already-ended eligible execution SHALL require no live foreground process or recorded exit code.

#### Scenario: A different process occupies the pane
- **WHEN** an old close target now refers to a different execution occupying the pane
- **THEN** close refuses rather than terminating the replacement

#### Scenario: Occupant evidence unavailable
- **WHEN** neither live-claim association nor an eligible ended-execution association can be established for the pane
- **THEN** this narrow child-close request refuses without signalling a guessed process or removing an unrelated pane

#### Scenario: Verified child
- **WHEN** the bound execution and its placement remain eligible
- **THEN** common graceful termination and bounded escalation precede pane removal

### Requirement: Close is durable and idempotent

The daemon SHALL persist the operation identity and target before dispatch. Replaying the same request id with identical content SHALL return the recorded result without dispatching again; the same id with different content SHALL be refused. A possibly dispatched close without confirmation SHALL remain unknown and SHALL NOT be retried automatically or routed through another backend.

#### Scenario: Identical replay
- **WHEN** the caller repeats a close request with its original request id and content
- **THEN** the daemon returns the stored outcome without a second close dispatch

#### Scenario: Lost result after dispatch
- **WHEN** close may have been dispatched but its outcome cannot be confirmed
- **THEN** the durable result remains unknown and no retry occurs

### Requirement: Pane closure does not assert process termination
Managed close SHALL report execution termination and pane removal separately. Verified process or scope evidence MAY establish execution exit; confirmed pane removal alone SHALL NOT do so. The operation SHALL NOT synthesize assignment completion or agent-advertised work outcomes.

#### Scenario: Confirmed pane close
- **WHEN** the managed close verifies execution termination and removes its pane
- **THEN** both outcomes are reported without changing assignment facts

#### Scenario: Pane disappears without a confirmation
- **WHEN** pane removal may have occurred but no confirmation or fresh same-generation absence is available
- **THEN** the removal result remains unknown and process exit is not inferred

#### Scenario: Pane removed with unverified process exit
- **WHEN** pane removal is confirmed but execution termination remains unverified
- **THEN** the result reports removal without claiming every process exited

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
The spawn-close entrypoint SHALL remain restricted to the caller's authorized bound spawn relationship. Directly admitted agents SHALL instead be eligible through the common subject/operator close entrypoints without a fabricated edge. All entrypoints SHALL share execution termination and pane-removal semantics after authorization.

#### Scenario: Foreign child
- **WHEN** a caller targets a child outside its authorized spawn relationship
- **THEN** the request is refused without effects

#### Scenario: Not yet implemented
- **WHEN** the common managed-close implementation is unavailable
- **THEN** the capability is not advertised as working and the operation refuses explicitly

#### Scenario: Admitted agent through operator close
- **WHEN** the operator closes an eligible directly admitted agent
- **THEN** the same managed-close behaviour runs without requiring a spawn edge

### Requirement: Dead-pane cleanup is fenced to the creating instance
Pane removal SHALL be fenced to the verified backend generation associated with the execution's placement. A created pane SHALL retain its creating-generation evidence; direct admission SHALL establish placement through verified admission. A missing or changed generation SHALL NOT be inferred from a pane handle or authorize removal of a reused handle.

#### Scenario: Another server generation
- **WHEN** a new backend generation exposes the recorded physical handle
- **THEN** the new pane is not removed using the old execution's placement

#### Scenario: The serving process changed
- **WHEN** the current serving process differs from the placement's verified generation
- **THEN** the old pane reference authorizes no removal

#### Scenario: No verified instance
- **WHEN** the serving generation cannot be verified for the selected pane
- **THEN** removal is not dispatched against that unverified pane

### Requirement: Closing an exited child is a defined result
An already-ended eligible child SHALL remain closable through the common workflow without a recorded numeric exit status. Fresh evidence SHALL establish that its pane has not been reassigned to another execution. The result SHALL distinguish an already-ended execution, optional recorded status and confirmed pane removal; same-ID replay SHALL not dispatch again.

#### Scenario: Exited child, retained pane
- **WHEN** an eligible child's execution ended and its pane remains safely attributable
- **THEN** common close removes it and returns the recorded optional completion facts

#### Scenario: No recorded exit
- **WHEN** the child's exit is verified but no code was captured and its retained pane is eligible
- **THEN** close removes the pane without refusing for the missing code

#### Scenario: Pane absent
- **WHEN** the selected pane is freshly confirmed absent under its verified generation
- **THEN** the result reports resource absence without asserting unverified execution exit

#### Scenario: Pane respawned after the exit
- **WHEN** the retained pane was reused by a different execution
- **THEN** the old child's close does not terminate that execution
