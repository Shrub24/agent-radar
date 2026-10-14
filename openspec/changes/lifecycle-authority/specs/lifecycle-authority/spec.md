## ADDED Requirements

### Requirement: Radar owns the lifecycle of what it executes
Radar SHALL be the single authority for the lifecycle and the resulting durable state of operations it executes. Every transition of that state SHALL go through Radar. External agents SHALL supply intent and application-specific evidence and SHALL NOT maintain independent copies of Radar-managed lifecycle state. State Radar did not execute and cannot verify SHALL remain with its owner and SHALL be held only as source-labelled, freshness-bounded evidence.

#### Scenario: Stop of a launched process
- **WHEN** an agent's process is stopped through Radar
- **THEN** the durable outcome is Radar's recorded result of that operation, and no agent publication can overwrite it

#### Scenario: Agent-published facts
- **WHEN** an agent publishes its model or context usage
- **THEN** Radar keeps it as that publisher's evidence with its freshness and does not treat it as Radar-managed lifecycle state

### Requirement: Observation drives transitions
An observation of a Radar-owned subject that establishes a lifecycle change SHALL commit at most one durable transition, with a short evidence summary naming the source, its strength and the observed time. The observation itself SHALL NOT be written to the event log. An observation that leaves the durable state unchanged SHALL append nothing. Observers and command paths SHALL commit transitions through one reducer per subject, so a record cannot take two conflicting transitions.

#### Scenario: Unchanged condition
- **WHEN** the same exit is observed on consecutive samples
- **THEN** one transition and one event exist for it

#### Scenario: Exit races a stop
- **WHEN** an observer sees a process exit while a stop of the same subject is in flight
- **THEN** the subject takes one terminal transition and the stop's record names the exit it observed

### Requirement: Transitions require evidence of sufficient strength
Definitive evidence, such as a verified process exit or a notification naming an exact verified pane and server identity, SHALL transition the subject immediately. Weak evidence, such as one inventory lacking a pane or a failed or stale read, SHALL change durable state only after it is confirmed by repetition within a bounded window or by an identity check, and any contradicting observation SHALL clear the pending confirmation.

#### Scenario: One inventory misses a pane
- **WHEN** a single tmux listing omits a bound pane and the next listing contains it
- **THEN** no transition is committed

#### Scenario: Pane stays absent
- **WHEN** a bound pane is absent from every observation across the confirmation window
- **THEN** the subject transitions to an ended-externally outcome with that evidence

### Requirement: Reconciliation recovers unresolved lifecycle facts
Reconciliation SHALL recompute Radar-owned lifecycle facts that are unresolved from current evidence and SHALL commit its conclusions through the same transition path. It SHALL run on daemon start, on a changed serving identity, for an operation left unknown, on a detected inconsistency and on an explicit operator request, and SHALL NOT run periodically. It SHALL NOT retry an effectful operation.

#### Scenario: Restart with an unknown record
- **WHEN** the daemon starts with a record left unknown by a crash
- **THEN** it verifies the record's effect from evidence and commits the result, or leaves it unresolved with its reason, and dispatches nothing again

#### Scenario: Steady state
- **WHEN** every Radar-owned subject is resolved and observations keep arriving
- **THEN** no reconciliation runs

### Requirement: Unresolved effects stay explicit and can be released
An effect that cannot be verified SHALL remain an explicit unresolved outcome with its reason and SHALL NOT be reported as completed or failed. An operator SHALL be able to abandon a record that can never resolve, which SHALL commit a terminal abandoned outcome and an event and SHALL release what the record blocked.

#### Scenario: Unobservable effect
- **WHEN** an input send was dispatched and its result cannot be established
- **THEN** the record stays unresolved with a reason and is not replayed

#### Scenario: Abandoned record releases the stop gate
- **WHEN** an operator abandons an unresolved record that blocked stopping the tmux server
- **THEN** the record is terminal, an event names it, and the stop gate no longer counts it

### Requirement: Consumers read semantic events
Public consumers SHALL read Radar's semantic events with opaque cursors and SHALL NOT depend on the internal journal's record shape, retention or snapshots.

#### Scenario: Journal format changes
- **WHEN** the internal journal representation changes without changing a semantic event
- **THEN** consumers that read events through their cursors are unaffected
