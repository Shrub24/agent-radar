# managed-topology

## ADDED Requirements

### Requirement: Containers are durable logical subjects

The daemon SHALL record every container it creates as a durable logical subject with its own identity, its kind (workspace, tab or pane slot), its parent link, a label and its provenance. A container record SHALL outlive the backend handle it was created at and SHALL survive daemon restart. Container identity SHALL NOT be derived from a backend handle, a session name or a pane index.

#### Scenario: Restart preserves identity

- **WHEN** the daemon restarts while a created container still exists
- **THEN** the container is read with the same identity and parent as before, and its binding is reacquired from verified evidence

#### Scenario: A reused handle does not resurrect a deleted container

- **WHEN** a backend later reports a handle belonging to a container already deleted
- **THEN** no recorded container returns for that handle

### Requirement: A container's binding is evidence, not identity

A container SHALL carry its backend binding separately from its identity: the backend, the serving instance, and the raw handle. The daemon SHALL verify the binding against fresh evidence before any effect. Missing, replaced or ambiguous bindings SHALL leave the container visible and unresolved rather than dispatch an effect. A handle that reappears under a different serving instance SHALL NOT be treated as the recorded container.

#### Scenario: The serving instance was replaced

- **WHEN** a container's handle now exists under a different serving instance than the one recorded
- **THEN** the container is shown with an unresolved binding and no deletion or reload is dispatched for it

#### Scenario: The binding cannot be established

- **WHEN** a backend answers neither the container's handle nor its serving instance
- **THEN** the container remains recorded and visible, and no effect is dispatched on a guess

### Requirement: Provenance distinguishes created containers from observed ones

Radar SHALL record a container as its own only when it created it. A container Radar only observed SHALL be shown as unmanaged, SHALL keep the existing generic close safeguards, and SHALL NOT be recorded as owned, reloaded from an invented recipe, or adopted implicitly.

#### Scenario: An externally created session appears

- **WHEN** a workspace exists that Radar did not create
- **THEN** it is listed as unmanaged with its own identity and location, and closing it follows the existing generic safeguards

#### Scenario: Reload of an unmanaged container

- **WHEN** an operator asks to reload an unmanaged container
- **THEN** the request refuses without a recipe, and offers to reload only after an explicit recipe is supplied

### Requirement: Creation names its container

The daemon SHALL create a workspace, tab or pane container only from an explicit request. A container that has a parent SHALL name it, and a workspace is a root that names none. A request missing a parent it needs SHALL refuse without an effect.

#### Scenario: A request with no parent

- **WHEN** a creation of a container that has a parent names no parent container
- **THEN** it refuses and creates nothing

#### Scenario: A workspace is a root

- **WHEN** a workspace is created with no parent
- **THEN** its record carries no parent and nothing is refused

### Requirement: A creation that runs a command carries a recipe

A creation that runs a command SHALL carry a recipe of label, working directory, command, environment and backend options. A creation that carries no recipe SHALL run nothing.

#### Scenario: No command without a recipe

- **WHEN** a creation carries no recipe
- **THEN** the container is created and nothing runs in it

### Requirement: A creation is recorded before it is reported

The daemon SHALL record the container and its verified binding before reporting a creation. A creation whose effect could not be confirmed SHALL remain unresolved rather than reported as created or retried under a new request ID. An agent container SHALL bind its child through the existing spawn contract and record the pane slot the launch was dispatched into.

#### Scenario: A created workspace is owned

- **WHEN** a workspace is created through the daemon
- **THEN** a readable container record with its binding exists before the creation is reported

#### Scenario: Creation could not be confirmed

- **WHEN** creation may have been dispatched but no location was confirmed
- **THEN** the result reports an unresolved creation, no container is recorded as created, and a retry needs a new request

#### Scenario: An agent container binds its child

- **WHEN** an agent container is created
- **THEN** the child binds through the existing spawn contract to its exact registered identity, and the container records the pane slot the launch was dispatched into

### Requirement: Recipe values are private launch data

Recipe working directory, command, environment and backend options SHALL be stored bounded and owner-only, and SHALL NOT appear in any container read, operation readback, log, diagnostic or event.

#### Scenario: A recipe value is never readback

- **WHEN** any container read, operation readback, log or diagnostic is produced
- **THEN** no working directory, command, environment value or backend option from a recipe appears in it

### Requirement: Deletion is confirmed, fenced and durable

Deleting a container SHALL require explicit confirmation naming the exact container and the possible losses, SHALL fail closed when the binding cannot be verified, and SHALL record a durable outcome before the caller is answered. Replay of the same request ID SHALL report the recorded outcome without closing anything again. Deletion SHALL NOT claim that any process exited, that any child was unregistered, or that any assignment completed.

#### Scenario: Replay after a lost answer

- **WHEN** the caller repeats the request ID of a deletion whose answer was lost
- **THEN** it reads the recorded outcome and no second close is dispatched

#### Scenario: Unverified binding

- **WHEN** a deletion is requested for a container whose binding cannot be verified
- **THEN** it refuses with no record and no effect

### Requirement: Deletion does not silently end a live managed process

When a container holds a live managed process, the daemon SHALL refuse deletion that would end it, SHALL name that process in the refusal, and SHALL offer the recorded stop-then-delete sequence. Pane removal and verified process exit SHALL be reported as separate facts. A container holding an unmanaged process SHALL keep the existing generic close behaviour with its containment and identity safeguards.

#### Scenario: A live managed child occupies the pane

- **WHEN** deletion targets a container whose pane holds a live managed child
- **THEN** it refuses naming that child, and no pane close is dispatched

#### Scenario: Stop then delete

- **WHEN** the operator runs the stop operation first and the process exit is verified
- **THEN** the deletion proceeds and reports the pane removal, with the verified exit reported by the stop and not by the deletion

### Requirement: Per-pane reload relaunches one pane slot

Reloading a pane container SHALL re-run its recorded recipe in the same pane slot. Only a pane container this daemon created with a recipe SHALL reload; anything else SHALL refuse and say what is missing. Reload SHALL require the previous command to be provably gone, and SHALL refuse rather than replace one still running or whose exit cannot be established. Reload SHALL record its own outcome and SHALL NOT report a Pi session, a new container, an assignment outcome, or any process exit.

#### Scenario: Reload of a managed pane

- **WHEN** a managed pane's process exit is verified and its recipe is recorded
- **THEN** the recipe is run again in that same pane slot and the reload outcome is recorded on the container

#### Scenario: The process may still be running

- **WHEN** a reload is requested for a pane whose command is still running, or whose exit the backend cannot establish
- **THEN** it refuses without replacing anything in that pane, and says to stop it first

#### Scenario: A container Radar did not create

- **WHEN** a reload targets a container recorded as observed rather than created
- **THEN** it refuses, because Radar holds no recipe for a place it did not make

#### Scenario: No recipe

- **WHEN** a reload is requested for a container with no recorded recipe
- **THEN** it refuses and states that no recipe is recorded

### Requirement: Reconciliation reports drift instead of guessing

Container reads SHALL carry per-source freshness, and reconciliation SHALL report observed drift — a missing container, a foreign occupant, a changed handle, a replaced serving instance — as source-labelled facts. Reconciliation SHALL NOT rewrite a container's identity, adopt a foreign handle, delete a record or dispatch an effect, and its reads SHALL remain bounded and paginated.

#### Scenario: A container disappeared

- **WHEN** a recorded container's handle is reported by no backend
- **THEN** the read reports it as missing with the time it was last observed, and its record is kept

#### Scenario: Reconciling dispatches nothing

- **WHEN** a reconciliation finds drift
- **THEN** it records the observation and dispatches no close, reload or creation

### Requirement: Confirmed container controls are daemon operations

A confirmed container control SHALL freeze the exact target — container identity plus expected binding — SHALL submit one request ID, and SHALL be read back by that ID. The daemon SHALL revalidate identity and eligibility before any effect. A transport that cannot reach the daemon SHALL refuse with no fallback to a second transport and no local retry, and a lost answer SHALL leave an operation that can be read again rather than an accepted effect that is forgotten.

#### Scenario: The answer is lost

- **WHEN** a confirmed control may have been accepted but its answer is lost
- **THEN** the client reads the same request ID again, and dispatches nothing else for that target

#### Scenario: Unsupported operation

- **WHEN** the selected container needs an operation the backend or daemon does not support
- **THEN** it refuses and explains, without a fallback transport or a fabricated outcome
