## ADDED Requirements

### Requirement: Managed subjects compose independent identities
The daemon SHALL identify a managed subject separately from its process incarnation, Pi session and pane. Replacement SHALL preserve the subject and verified child relationships while retaining prior bindings as history. Registration `agent_id` SHALL remain incarnation-scoped; session UUID equality SHALL NOT select or authorize a replacement.

#### Scenario: Multiple attaches
- **WHEN** two registrations share a Pi session UUID
- **THEN** their process bindings remain distinct and neither takes over the other's managed subject

#### Scenario: Session switch
- **WHEN** a process publishes a new session context
- **THEN** its process and managed-subject identities remain unchanged and its resume data is not silently retargeted

### Requirement: Lifecycle ownership resides in the daemon
The daemon SHALL own managed-operation admission, effects, monitoring, recovery and lifecycle projection. Disconnecting a TUI SHALL NOT cancel an accepted operation or lose its outcome. Clients SHALL only request controls and display daemon facts; assignment, readiness and results SHALL remain with their existing publishers/owners.

#### Scenario: Operator quits during stop
- **WHEN** the TUI disconnects after an accepted stop
- **THEN** daemon monitoring continues and another client can read the same durable outcome

### Requirement: Launch tracking precedes agent registration
A lifecycle-capable daemon launch SHALL establish a verified process birth binding before releasing the requested program to execute. Process control SHALL NOT depend on child registration. A launch whose process binding cannot be established SHALL remain unresolved rather than adopt the pane's current occupant.

#### Scenario: Program exits immediately
- **WHEN** the launched program exits before agent registration
- **THEN** the daemon can observe the captured process's exit independently of the missing registration

#### Scenario: Uncertain launch dispatch
- **WHEN** launch may have dispatched but no process correlation arrives
- **THEN** the record reports unresolved process identity and no stop or retry targets a guessed occupant

### Requirement: Bootstrap cancellation is durable and race-fenced
Cancellation SHALL durably revoke unused launch and registration correlations. Binding and cancellation SHALL have one serialized durable winner; a token revoked before binding SHALL never bind later, including after daemon restart. Cancellation of a bound or running launch SHALL use the same exact-process control as stop. Token revocation, verified process exit and pane cleanup SHALL be reported separately.

#### Scenario: Late registration after cancellation
- **WHEN** a child presents a correlation revoked by an accepted cancellation
- **THEN** binding refuses and the subject remains cancelled or unresolved, not bound

#### Scenario: Binding wins the race
- **WHEN** registration commits before cancellation
- **THEN** cancellation targets that exact bound process generation rather than reporting an unbound launch with no process

#### Scenario: Unbound launch left a pane behind
- **WHEN** a cancel revokes a launch that never bound a registration and that launch was dispatched into a pane this daemon created
- **THEN** that pane is removed only in the verified backend instance that created it, and the outcome is recorded on the launch as its own fact

#### Scenario: Launch may still be alive in its pane
- **WHEN** a cancel's signal is not verified gone, or no verified instance can be established for the recorded pane
- **THEN** the pane is kept and the reason is reported, because closing it would end a process without a signal or an exit claim

#### Scenario: Cleanup is asked for twice
- **WHEN** a launch already records a pane cleanup outcome
- **THEN** a later cancel reports that outcome and closes nothing again

#### Scenario: Cancel of a bound launch
- **WHEN** a cancel targets a launch already bound to a registration
- **THEN** its pane is left to the managed child close path and the cancel records no pane cleanup

### Requirement: Stop targets one verified process incarnation
The daemon SHALL signal only the frozen managed process birth identity, use SIGTERM without PID-only fallback or SIGKILL escalation, and wait boundedly for independently verified exit. Pane disappearance, report expiry and retained pane-command status SHALL NOT prove that exit. Stop SHALL NOT imply descendant termination or assignment completion.

#### Scenario: PID reused
- **WHEN** the recorded PID now has a different birth identity
- **THEN** no signal reaches its replacement

#### Scenario: Process ignores termination
- **WHEN** SIGTERM was dispatched but exit is not verified before the deadline
- **THEN** the operation reports uncertainty and no replacement launches

### Requirement: Restart replaces a binding only after verified exit
Operator restart SHALL require explicit confirmation, a restartable managed subject and a private session-specific resume specification. It SHALL verify old-process exit before launching once into a fresh pane. Completion SHALL require replacement registration matching the captured new process and fresh expected Pi-session context through one-use operation correlation. Live children SHALL remain untouched.

#### Scenario: Replacement binds successfully
- **WHEN** the old process exited and the correlated replacement verifies its process and expected session
- **THEN** the same subject names the new generation and retains its children and prior history

#### Scenario: Replacement fails before binding
- **WHEN** a replacement may have launched but does not complete verified binding
- **THEN** partial effects remain durable and no automatic second replacement is launched

### Requirement: Restartability is explicitly enrolled and private
A daemon-spawned subject or explicitly operator-enrolled registered Pi process SHALL become restartable only with verified process/location provenance and an explicit resume recipe for the selected session. Publisher launch data alone SHALL NOT grant restartability. Capture and storage SHALL be bounded, private and secret-safe; public reads and diagnostics SHALL omit argv, session paths, environment and correlations.

#### Scenario: Registered lead has no recipe
- **WHEN** an operator selects a registered lead without an explicit current-session resume specification
- **THEN** restart refuses without signaling or creating a pane

#### Scenario: Old environment contains correlations
- **WHEN** a captured specification is used for replacement
- **THEN** old one-use Radar correlations are excluded and fresh operation correlations are supplied

### Requirement: Effects are durable and replay-safe
Each accepted control SHALL record an exact target generation and immutable request before effects. Identical request-ID replay SHALL read or reconcile that operation without redispatch. Conflicting replay and conflicting controls on an unresolved generation SHALL refuse. Reads SHALL distinguish revocation, signal dispatch, verified exit, pane cleanup, replacement launch and binding certainty.

#### Scenario: Lost answer
- **WHEN** a mutation may have dispatched and its answer is lost
- **THEN** the caller can read the same operation and cannot trigger another effect by replaying its ID

### Requirement: Recovery reconciles evidence without relaunch
After daemon restart, runtime handles SHALL be reacquired only against verified durable birth identities. Missing, changed or unavailable physical evidence SHALL remain independently classified. Recovery SHALL preserve subjects, relationships and operation history and SHALL NOT automatically launch replacements or repeat uncertain effects.

#### Scenario: tmux server replaced
- **WHEN** a new server reuses a recorded pane ID
- **THEN** it is not the recorded pane binding and no cleanup targets it

### Requirement: Projection preserves source authority
Lifecycle reads SHALL expose process, session, pane and operation evidence with source and freshness rather than one independently authored agent status. Notifications SHALL trigger reconciliation; missed notifications SHALL be recoverable through bounded reads. Pi execution and Herdsman assignment facts SHALL remain independent of physical controls.

#### Scenario: Process stopped during active assignment
- **WHEN** a stop verifies process exit while the assignment remains unsettled
- **THEN** the projection shows both facts and does not fabricate successful assignment completion

### Requirement: Lifecycle events are durable transition authority
Managed lifecycle transitions SHALL commit typed versioned events to one daemon-scoped FIFO log before effects. Envelopes SHALL identify subject, source, sequence and event identity, with known operation/causation references. Reducers SHALL reconstruct projections without I/O or dispatch. Payloads SHALL omit private recipes and credentials. Existing records SHALL enter through explicit baseline imports.

#### Scenario: Replaying recorded lifecycle
- **WHEN** a daemon rebuilds its lifecycle projection from committed events
- **THEN** it reproduces the recorded state without signaling, launching or closing anything

#### Scenario: Event storage unavailable
- **WHEN** an intent cannot be durably committed
- **THEN** the associated undispatched effect refuses and no signal or replacement launch occurs

### Requirement: Delivery and reconciliation preserve ordering and authority
Conflicting transitions SHALL serialize by logical subject, while independent effects MAY execute concurrently within bounded daemon lanes. Subscribers SHALL observe committed sequence order through independent cursors and SHALL NOT block effect completion. Reconciliation MAY coalesce pending hints but SHALL NOT discard committed events. Observed discrepancies, unavailable evidence and conflicts SHALL remain source-labelled; pane signals SHALL NOT infer process exit or assignment completion.

#### Scenario: Missed external signal
- **WHEN** periodic reconciliation discovers a physical discrepancy after a missed signal
- **THEN** it commits the resulting observation and updates only the facets authorized by that evidence

#### Scenario: Crash after effect dispatch
- **WHEN** an intent exists but its effect result was not committed
- **THEN** replay preserves uncertainty and recovery reconciles evidence rather than repeating the effect

### Requirement: New controls are explicitly negotiated
Lifecycle reads, cancellation and stop SHALL be capability-gated additions to protocol 2. Restart and enrollment SHALL be operator-facing daemon requests used by the confirmed TUI, not Herdsman restart or CLI features. Unsupported backends SHALL refuse before effects, and no direct or owner-file fallback SHALL execute a managed control. TUI confirmation SHALL NOT be presented as authentication against same-UID callers.

#### Scenario: Unsupported backend
- **WHEN** the selected backend cannot establish managed-process tracking
- **THEN** new lifecycle controls are unavailable while existing supported observation and mux operations remain usable
