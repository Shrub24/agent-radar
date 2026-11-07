# managed-execution Specification

## Purpose
Run a managed agent in place, inside a physical resource group established before it executes, while Radar keeps logical identity and the mechanisms that actually observe execution keep its outcome.

## Requirements

### Requirement: Managed execution runs in place
`radar run -- COMMAND [ARG...]` SHALL execute the command in the caller's verified Radar tmux pane, preserving the pane's terminal, cwd, environment and argv exactly. It SHALL NOT create a pane, window or workspace, and SHALL NOT start a shell to interpret the command. It SHALL refuse before executing when the pane is unverified or the command is empty.

#### Scenario: Run in the current pane
- **WHEN** the operator runs `radar run -- pi` in a verified Radar pane
- **THEN** Pi starts in that pane with the pane's terminal, cwd and environment, and no new pane is created

#### Scenario: Literal arguments survive
- **WHEN** an argument contains a variable reference, a glob or a newline
- **THEN** the command receives those bytes unchanged

#### Scenario: Unsupported placement
- **WHEN** the pane cannot be verified as a Radar-managed pane under this daemon
- **THEN** the launch is refused before anything executes and the reason is reported

### Requirement: Agents and commands are separate admissions
`radar run --agent -- PROGRAM` SHALL admit an agent: the launch's registration is exported to the program so its session can present it and be attached to a subject. `radar run -- COMMAND` SHALL admit a managed command: a registration and a scope and nothing else. A command SHALL NOT receive its registration or any identity in its environment, SHALL NOT be attached to a subject or claim, and a registration admitted as a command SHALL be refused when presented as an agent's.

#### Scenario: A shell's children inherit no identity
- **WHEN** a command is admitted and starts a Pi process from within it
- **THEN** that Pi sees no launch registration, reports as an ordinary session, and is not confused with the command

#### Scenario: A command cannot be attached as an agent
- **WHEN** a process presents the registration of a managed command
- **THEN** the daemon refuses it and the command gains no subject, claim or lifecycle authority

### Requirement: The physical group exists before the command executes
Every managed launch SHALL create a transient systemd user scope in a Radar slice and admit the application into it before the command runs. A required setup failure SHALL refuse the launch before execution, and SHALL NOT launch unscoped. The scope SHALL be named from the process's registration id, never from argv, cwd, environment, prompt text or session identity.

#### Scenario: Scope established first
- **WHEN** a managed launch starts
- **THEN** the application's first process is already a member of the recorded scope

#### Scenario: Scope creation fails
- **WHEN** the scope cannot be created or the application cannot be admitted
- **THEN** the command does not execute, the launch records a refusal, and no unscoped fallback occurs

#### Scenario: Unit names stay public-safe
- **WHEN** a launch carries environment values or a prompt
- **THEN** no unit name, description or slice name contains them

### Requirement: Scope state is not an execution outcome
The scope's state SHALL NOT be reported as command success, failure, readiness or assignment outcome. Application exit SHALL come only from the existing typed physical exit observation. A scope that empties or reports failure SHALL NOT by itself settle a managed operation.

#### Scenario: Nonzero exit through a scope
- **WHEN** the managed command exits with a nonzero status
- **THEN** the recorded exit carries that status or signal and the scope's own result is not used as the outcome

#### Scenario: Lingering member
- **WHEN** the launched command exits while a descendant keeps the group non-empty
- **THEN** the execution reports its exit and the group remains, without claiming completion of the workload

#### Scenario: Missing transient unit after the fact
- **WHEN** the transient unit is no longer readable
- **THEN** no clean exit is inferred from its absence

### Requirement: Managed stop stays graceful
Managed scope stop SHALL send the configured termination signal without an automatic SIGKILL escalation. A stop that leaves processes alive SHALL be reported as unresolved with the survivors named, not as success. Group stop SHALL target the execution's main scope only.

#### Scenario: Program ignores the signal
- **WHEN** a managed stop leaves the program running
- **THEN** Radar reports an unresolved stop with survivors and does not escalate to SIGKILL

#### Scenario: Main process gone while a member lives
- **WHEN** a stop is issued for a scoped process whose main process has already exited while a descendant keeps the group non-empty
- **THEN** Radar goes through the scope stop and the survivor check and reports the stop as unresolved with the survivors named, rather than reporting a clean exit from the main process alone

#### Scenario: Failed unit state after a graceful stop
- **WHEN** the scope enters a failed or timeout state because processes survived
- **THEN** Radar reports that state as expected and continues to track the workload

#### Scenario: Independent tasks are untouched
- **WHEN** the main scope stops
- **THEN** independently scoped tasks and subagents are not signalled

### Requirement: Recovered scope identity is verified
Recovery SHALL bind a scope to an execution only when the recorded unit identity and a verified process birth identity inside its group both match. A reused name or a stale group path alone SHALL NOT authorise a signal, a termination or an outcome.

#### Scenario: Daemon restarts while the workload runs
- **WHEN** the daemon restarts with a recorded execution still running
- **THEN** it reports the workload running and keeps its scope association without signalling it

#### Scenario: Name reused by an unrelated workload
- **WHEN** a process in a group with a recorded name does not match the recorded birth identity
- **THEN** the association is unresolved and no effect targets it

### Requirement: Group measurements are presentations, not attribution
Radar MAY expose a managed execution's group memory, CPU and task counts. It SHALL present them as charged measurements and SHALL NOT add them to process-tree totals or claim per-agent causal cost.

#### Scenario: Group values exposed
- **WHEN** a managed execution's resource measurement is read
- **THEN** it is labelled as a group measurement and not combined with a summed process metric

### Requirement: Process facts belong to the registration
Radar SHALL record a managed process's unit and slice identity, captured recipe, expected and bound birth identity, signal and exit evidence against its registration. Changing a session attachment SHALL NOT rename its scope or create a second owner of those process facts. The subject SHALL NOT own process-lifetime facts.

#### Scenario: Scope association survives a daemon restart
- **WHEN** the daemon restarts with a recorded process still running
- **THEN** its registration still names its unit and slice, and recovery verifies the member identity before any effect

#### Scenario: Session changes in the same process
- **WHEN** the process reloads or opens a different session
- **THEN** its claim changes but its registration and physical group remain unchanged

#### Scenario: A command has no Pi session
- **WHEN** Radar launches a command with no subject attachment
- **THEN** it records the process registration and scope without fabricating a subject or claim

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
