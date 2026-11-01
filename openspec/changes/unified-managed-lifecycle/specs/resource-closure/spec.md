# Spec Delta

## Purpose

Define one domain close operation for agents and terminal containers, composing managed termination with resource removal while keeping partial outcomes truthful.

## ADDED Requirements

### Requirement: Close resolves a domain target rather than a display hierarchy
The daemon SHALL resolve Close from canonical row identity to an exact execution/pane or container. Explicit invalid targets SHALL NOT fall back to focused or selected rows. Filtered, collapsed or historical display membership SHALL NOT determine a container's lifecycle children.

#### Scenario: Agent identity is pane-keyed
- **WHEN** Close names an agent row whose displayed ID contains a pane handle
- **THEN** the daemon resolves and validates its claim association rather than treating that handle as process identity

#### Scenario: Hidden child
- **WHEN** a container holds an agent hidden by the TUI filter
- **THEN** closing the container still includes that execution in graceful shutdown discovery

### Requirement: Close ends the execution and removes its pane
One confirmed agent Close SHALL invoke managed termination and attempt completion capture before removing its hosting pane. It SHALL NOT require a second Close merely because the agent was admitted in an existing shell. Spontaneous agent completion SHALL NOT itself authorize removing that shell.

#### Scenario: Close admitted agent
- **WHEN** the operator confirms Close on an eligible `radar run --agent` row
- **THEN** Radar handles its managed execution and removes the named pane in the same operation

#### Scenario: Spontaneous admitted exit
- **WHEN** the admitted program finishes without a Close request
- **THEN** the invoking shell remains available and completion is recorded independently

### Requirement: Container closure composes child closure before removal
Container Close SHALL recursively discover hosted executions, initiate their graceful shutdown and remove descendant panes before confirming the root's removal. Graceful waiting SHALL use one operation-wide deadline rather than a full timeout per child. If the root remains, a bounded enclosing-resource removal SHALL complete the requested close when possible.

#### Scenario: Several stubborn agents
- **WHEN** a workspace contains several agents that ignore graceful termination
- **THEN** their graceful waits share a deadline, escalation is bounded and workspace closure does not wait one full grace period per agent

#### Scenario: Last leaf removes its ancestors
- **WHEN** removing the last pane causes its window and session to disappear
- **THEN** fresh same-generation absence satisfies resource removal without requiring duplicate ancestor destruction

### Requirement: Close favours requested removal without fabricating process exit
Discovery errors, unavailable status and uncertain child shutdown SHALL NOT alone veto a confirmed close of its verified root resource. Root removal and execution outcomes SHALL remain distinct. Confirmed surviving managed executions or failed removal SHALL be actionable; missing status alone SHALL remain diagnostic.

#### Scenario: Child discovery fails
- **WHEN** child discovery fails after the root was safely resolved and its Close confirmed
- **THEN** Radar attempts bounded root removal and records incomplete graceful handling instead of reporting an empty subtree

#### Scenario: Place closes over unverified exit
- **WHEN** the backend confirms root removal but a managed process exit is unverified
- **THEN** the place is reported removed without recording that process as exited

### Requirement: Closing branches reject conflicting Radar mutations
While Close is active, Radar SHALL reject overlapping launch, move and restart requests into its branch. Native tmux changes MAY occur without a global topology transaction. Destruction SHALL remain fenced to the selected backend generation and exact execution identities.

#### Scenario: Launch into closing workspace
- **WHEN** another Radar request tries to launch into a workspace currently closing
- **THEN** it is refused promptly instead of creating another execution there

#### Scenario: Backend restarts during closure
- **WHEN** a new server generation reuses the target handle during Close
- **THEN** the new resource is not removed using the old reference

### Requirement: Close preserves durable replay and bounded escalation
A close operation SHALL persist its identity and target before destructive effects. Identical settled replay SHALL return its recorded outcome without new effects; conflicting content SHALL refuse. Escalation within the active operation MAY follow an uncertain child attempt, but SHALL NOT become unlimited redispatch or claim an uncertain attempt never happened.

#### Scenario: Same request repeated
- **WHEN** a settled container Close is repeated with identical identity and content
- **THEN** its recorded outcome is returned without another termination or removal

#### Scenario: Leaf removal answer is lost
- **WHEN** leaf removal is uncertain during a confirmed active container Close
- **THEN** the operation may use its bounded root-removal fallback while retaining the uncertainty of the earlier effect
