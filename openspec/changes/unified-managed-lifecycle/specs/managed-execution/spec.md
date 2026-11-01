# Spec Delta

## Purpose

Define the common execution lifetime, verified terminal placement and completion evidence for managed agents regardless of how their launch was admitted.

## ADDED Requirements

### Requirement: Managed executions have claim-scoped placement
Every managed execution SHALL identify its exact claim and verified hosting pane independently of spawn parentage. Current placement SHALL reflect verified pane moves without changing subject identity or historical spawn provenance. A pane handle without its backend generation SHALL NOT authorize removal.

#### Scenario: Admitted agent has no spawn edge
- **WHEN** `radar run --agent` admits an agent in an existing pane
- **THEN** container discovery can find its exact execution through verified placement without inventing a spawn edge

#### Scenario: Pane moves between tabs
- **WHEN** fresh topology verifies a managed pane moved
- **THEN** current containment follows its new location while subject identity and recorded producer parentage remain unchanged

### Requirement: All admitted agents share managed execution capabilities
Configured, spawned and directly admitted agents SHALL use the same execution supervision, registration, termination and completion behaviour after placement/admission. An existing-shell launch SHALL NOT lose lifecycle capabilities because it has no spawn edge. Ordinary shells SHALL NOT require managed admission.

#### Scenario: Two admission paths
- **WHEN** equivalent agents start through `L` and `radar run --agent`
- **THEN** both have verified execution identities, managed scopes, completion reporting and the same eligible lifecycle controls

### Requirement: Program completion is independent of pane death
Radar SHALL observe the managed program's completion even when its invoking shell remains alive. A report SHALL be authenticated to its execution attempt and distinguish the actual program from launch wrappers. Pane-command status SHALL NOT be substituted for a hosted program's status.

#### Scenario: Agent returns to shell
- **WHEN** an admitted agent exits with code 7 and its interactive shell resumes
- **THEN** its claim records that completion without waiting for the pane to die or removing the shell

#### Scenario: Unrelated reporter
- **WHEN** a completion report cannot establish authority for the named claim
- **THEN** it is refused without recording an exit or code for that execution

### Requirement: Completion is persisted at the event rather than on reads
Radar SHALL attempt to persist observed completion at exit and capture available status before intentional pane removal. Reads SHALL return recorded facts without backend status capture. Missing code, signal or capture acknowledgement SHALL NOT block otherwise justified lifecycle transitions or requested resource cleanup, and SHALL NOT be represented as successful code zero.

#### Scenario: Code outlives its pane
- **WHEN** a managed program exits with a captured code and its pane is then removed
- **THEN** later reads return the recorded code without querying the removed pane

#### Scenario: Status cannot be captured
- **WHEN** exit is verified but code capture fails during a confirmed close
- **THEN** cleanup proceeds and the result distinguishes verified exit from unavailable status

### Requirement: Termination is bounded and addresses verified execution scopes
A managed termination SHALL request graceful shutdown, wait within a shared operation deadline and escalate remaining verified scope members when the requested action requires closure. It SHALL NOT target a scope by name pattern alone. A removed terminal SHALL NOT itself be proof that all execution processes exited.

#### Scenario: TERM is ignored
- **WHEN** a verified managed execution ignores graceful termination during Close
- **THEN** Radar escalates within the bounded shutdown operation and records the verified or unverified result without waiting indefinitely

#### Scenario: Scope association is unavailable
- **WHEN** a recorded scope cannot be verified for the target execution
- **THEN** no unrelated scope is signalled and confirmed resource removal may proceed with that execution's exit unverified

### Requirement: Restart creates a successor claim on the same subject
Restart SHALL preserve logical subject identity and allocate a fresh execution claim. Termination and completion of the predecessor SHALL address that exact claim, not a later current claim. A replacement SHALL NOT launch while predecessor termination remains unverified.

#### Scenario: Restart preserves identity
- **WHEN** a managed agent restarts successfully
- **THEN** its subject remains the same, its successor has a distinct claim and the predecessor retains its own completion facts

#### Scenario: Old claim was replaced before action
- **WHEN** a request names a predecessor claim that is no longer the eligible attempt
- **THEN** the replacement is not terminated by resolving the subject again
