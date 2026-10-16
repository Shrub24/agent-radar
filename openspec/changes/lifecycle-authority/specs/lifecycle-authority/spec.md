## ADDED Requirements

### Requirement: Managed execution identity is stable and versioned
The public `subject_id` SHALL identify the stable logical subject. Each execution SHALL have a distinct `execution_id` and a one-based `execution_generation` represented as an unsigned 32-bit integer.

#### Scenario: A logical subject starts a new execution
- **WHEN** Radar starts a successor execution for an existing logical subject
- **THEN** the public `subject_id` remains unchanged, the `execution_id` is new, and the generation increments by one

#### Scenario: A client uses the lifecycle contract
- **WHEN** a client reads or controls a managed subject
- **THEN** it negotiates the protocol revision that defines stable `subject_id` semantics before using that identity

### Requirement: Public managed-intent DTOs remain generated from Rust
The managed launch metadata and Pi-session identity-marker DTOs consumed by pi-radar SHALL be defined in the Rust public-contract module, exported as directional JSON Schemas, and used through generated TypeScript declarations. A contract change SHALL fail generation-drift or fixture validation until the artifacts and consumer types are updated.

#### Scenario: Managed intent or marker DTO changes
- **WHEN** a Rust managed-intent or launch-marker field changes
- **THEN** the checked JSON Schemas and pi-radar TypeScript types are regenerated from the Rust DTOs and the package drift test detects stale artifacts

### Requirement: Radar records accepted managed launch intent
Radar SHALL be authoritative for lifecycle state it admits and executes. External orchestrators SHALL NOT maintain an independently writable copy of Radar-managed lifecycle state.

#### Scenario: Herdsman requests a managed launch
- **WHEN** Herdsman submits a launch intent with a parent, role, owner, run or label
- **THEN** Radar records the accepted subject, operation, execution generation and managed metadata as Radar state, and Herdsman does not create a competing registration for that process

#### Scenario: Parent subject does not match the spawn parent
- **WHEN** a launch intent names a parent `subject_id` that is absent or bound to another registered agent
- **THEN** Radar refuses before recording a spawn edge or dispatching an effect

#### Scenario: Caller supplies a Radar-issued marker variable
- **WHEN** a spawn request attempts to set any `PI_RADAR_MANAGED_*` identity environment variable
- **THEN** Radar refuses before admission so the child cannot forge its own breadcrumb

#### Scenario: Publisher reports conflicting metadata
- **WHEN** a live registration or facts channel reports owner, role, run or label data that conflicts with the accepted launch intent
- **THEN** the managed subject retains Radar's accepted metadata and the channel cannot overwrite it

#### Scenario: Application-owned facts
- **WHEN** a Pi publisher reports model, provider, context usage or other application facts
- **THEN** Radar retains them as source-labelled, freshness-bounded channel evidence and does not treat them as managed lifecycle state

### Requirement: pi-radar is the single baseline registrar for a Pi process
A Pi process with the pi-radar baseline extension loaded SHALL have one baseline registration and one lease, independent of whether Herdsman is installed. Herdsman SHALL attach its orchestration intent and application-specific evidence through the Radar integration and SHALL NOT independently register the same Pi process or duplicate baseline channels. A plain Pi process SHALL be able to publish baseline session, location, process and facts observations without Herdsman.

#### Scenario: Plain Pi session starts
- **WHEN** a Pi session starts with pi-radar loaded and no Herdsman integration
- **THEN** pi-radar registers the process with Radar and publishes the baseline observations available to that session

#### Scenario: Herdsman is also installed
- **WHEN** a Pi session starts with both pi-radar and Herdsman integrations
- **THEN** there is one baseline registration and Herdsman's additions refer to that same Radar subject

#### Scenario: Baseline publisher cannot reach the daemon
- **WHEN** the daemon is unavailable during session startup
- **THEN** pi-radar retries registration according to its bounded reconnect policy and does not invent a Radar subject identity or report a successful registration

#### Scenario: Display closes during post-launch registration
- **WHEN** the TUI closes after a managed launch is durably captured but before pi-radar's registration is joined
- **THEN** the daemon retains the launch record and performs any later binding from fresh exact identity evidence without requiring a display or dispatching another launch

### Requirement: Pi-session launch breadcrumbs reflect Radar-issued identity
For a Radar-managed launch, Radar SHALL issue a versioned, non-secret identity breadcrumb for the launched Pi session. The breadcrumb SHALL identify the Radar subject and execution/operation generation it reflects. It SHALL NOT contain one-use credentials or authorize a process to override Radar state. A stale, absent or contradictory breadcrumb SHALL remain evidence of uncertain correlation rather than being silently repaired into authoritative state.

#### Scenario: Managed launch persists its breadcrumb
- **WHEN** a Pi process starts from a Radar-managed launch and the Pi session supports durable extension metadata
- **THEN** the session stores the versioned Radar-issued subject and execution identity for later correlation

#### Scenario: Copied or stale breadcrumb
- **WHEN** a process presents a breadcrumb whose subject, generation, process identity or current Radar record does not match
- **THEN** Radar preserves the mismatch as unresolved evidence and does not bind the process or change managed state solely from the breadcrumb

#### Scenario: Breadcrumb is absent during recovery
- **WHEN** reconciliation cannot find a breadcrumb for a running process
- **THEN** it uses other fresh identity-bound evidence if available, otherwise leaves the association unresolved and does not recreate a missing managed record or replay the launch

### Requirement: Observation drives edge-triggered lifecycle transitions
An observation of a Radar-owned subject that establishes a lifecycle change SHALL commit at most one durable transition, with a short evidence summary naming its source, strength and observed time. The observation itself SHALL NOT be written to the event log. An observation that leaves durable state unchanged SHALL append nothing. Observers and command paths SHALL commit through one fenced transition path so a record cannot take conflicting transitions.

#### Scenario: Unchanged condition
- **WHEN** the same exit is observed on consecutive samples
- **THEN** one transition and one semantic event exist for it

#### Scenario: Exit races a stop
- **WHEN** an observer sees a process exit while a stop of the same subject is in flight
- **THEN** the subject takes one terminal transition and the stop record names the exit it observed

#### Scenario: Live channel changes
- **WHEN** a publisher updates a model or context fact without changing managed lifecycle state
- **THEN** the channel projection changes with its source and freshness, but no lifecycle event is appended

### Requirement: Transitions require evidence of sufficient strength
Definitive evidence, such as a verified process exit or a notification naming an exact verified pane and server identity, SHALL transition the subject immediately. Weak evidence, such as one inventory lacking a pane or a failed or stale read, SHALL change durable state only after confirmation within a bounded window or an identity check. A contradicting observation SHALL clear pending confirmation. Failed, stale or incomplete observations SHALL NOT establish absence.

#### Scenario: One inventory misses a pane
- **WHEN** a single tmux listing omits a bound pane and the next listing contains it
- **THEN** no transition is committed

#### Scenario: Pane stays absent
- **WHEN** a bound pane remains absent across the confirmation window with sufficient fresh evidence
- **THEN** the subject transitions to an ended-externally outcome with the evidence summary

#### Scenario: Observation source is unavailable
- **WHEN** a source read fails or is stale during the confirmation window
- **THEN** that read does not count as confirmation that the process or pane is absent

### Requirement: Reconciliation restores observation continuity
Reconciliation SHALL restore an external observation baseline for unresolved Radar state after continuity or confidence loss. It SHALL run on daemon start, changed serving identity, an unknown operation, a detected inconsistency or an explicit operator request; it SHALL NOT run periodically. It SHALL NOT retry an effectful operation.

#### Scenario: Restart with an unknown operation
- **WHEN** the daemon starts with an operation left unknown by a crash
- **THEN** it verifies the effect from current evidence and the durable record, commits a justified transition or leaves the operation unresolved with its reason, and dispatches nothing again

#### Scenario: Breadcrumb matches durable state
- **WHEN** a breadcrumb names the same subject and generation as a durable Radar record and fresh process evidence confirms the binding
- **THEN** reconciliation may restore the observation association without creating a new subject or launch

#### Scenario: Breadcrumb has no authoritative record
- **WHEN** a breadcrumb names a subject or generation absent from Radar's durable state
- **THEN** reconciliation does not recreate that state from the breadcrumb alone and reports unresolved correlation

#### Scenario: Steady state
- **WHEN** all managed subjects are resolved and observations keep arriving
- **THEN** no reconciliation runs

### Requirement: Unresolved effects stay explicit and can be safely released
An effect that cannot be verified SHALL remain an explicit unresolved outcome with its reason and SHALL NOT be reported as completed or failed. An operator MAY abandon a record that can never resolve. Abandonment SHALL end that record's recovery obligation and release only its administrative block; it SHALL preserve uncertainty, SHALL NOT establish the external outcome, SHALL NOT authorize replay, and SHALL NOT bypass independent process or pane safety checks.

#### Scenario: Unobservable effect
- **WHEN** an input send was dispatched and its result cannot be established
- **THEN** the record stays unresolved with a reason and is not replayed

#### Scenario: Daemon stops between durable launch steps
- **WHEN** the daemon stops after accepting launch intent but before all operation, spawn-edge and lifecycle records are durable
- **THEN** recovery exposes the accepted or uncertain state and never reports fulfillment or blindly dispatches the launch again

#### Scenario: Abandoned record releases only its administrative block
- **WHEN** an operator abandons an unresolved record that blocked stopping the tmux server
- **THEN** the record is terminal and an event names it, the administrative stop gate no longer counts that record, and any independent physical safety refusal remains in force

### Requirement: Consumers read semantic events
Public consumers SHALL read Radar's semantic events with opaque cursors and SHALL NOT depend on the internal journal's record shape, retention or snapshots.

#### Scenario: Journal format changes
- **WHEN** the internal journal representation changes without changing a semantic event
- **THEN** consumers that read events through their cursors are unaffected

#### Scenario: Semantic event names an execution
- **WHEN** a lifecycle transition affects one execution of a logical subject
- **THEN** its semantic event identifies the stable `subject_id` and the affected `execution_id` and generation

#### Scenario: Legacy execution chain is ambiguous
- **WHEN** a legacy successor link is missing, forked, cyclic or contradictory during migration
- **THEN** Radar keeps the affected execution records separate, marks their identity continuity unresolved, appends no speculative cross-record mapping, and dispatches no launch
