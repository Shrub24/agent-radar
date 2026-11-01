# Spec Delta

## MODIFIED Requirements

### Requirement: Lifecycle actions require separate confirmation
Radar SHALL offer one semantic Close command on `D` and restart on `r`. Destructive confirmation SHALL name the selected resource and known running work, default to Cancel and send nothing until accepted. Keyboard, mouse and palette SHALL reach the same daemon-owned target resolution; unsupported rows SHALL NOT redirect to parents.

#### Scenario: Close confirmation
- **WHEN** the operator confirms `D` on an eligible agent
- **THEN** one operation terminates its managed execution and removes its pane without a second `D`

#### Scenario: Mouse confirmation
- **WHEN** the operator accepts the drawn close confirmation with the mouse
- **THEN** it uses the same target and checks as keyboard acceptance, never a focus second-click

#### Scenario: Filter entry
- **WHEN** a close key is typed while editing a filter
- **THEN** it changes filter text rather than dispatching close

#### Scenario: Cancel or accidental Enter
- **WHEN** the operator cancels a close dialog
- **THEN** no termination or removal is submitted

### Requirement: Direct runtime close requires positive unmanaged evidence
Close SHALL terminate known managed executions through the common lifecycle implementation before terminal removal. Ordinary unmanaged workloads SHALL use best-effort terminal teardown. Uncertain agent identity SHALL NOT authorize signalling a guessed process; a confirmed eligible enclosing-container close MAY still remove its verified resource with unresolved execution outcomes retained.

#### Scenario: Ordinary unmanaged pane
- **WHEN** the operator confirms Close on an ordinary shell
- **THEN** its pane is removed without inventing a managed execution

#### Scenario: Unverified Pi pane
- **WHEN** a narrow agent Close cannot establish its exact execution association
- **THEN** it refuses to signal a guessed process; separately confirmed enclosing-container closure retains its broader removal intent

#### Scenario: Retained managed association
- **WHEN** a known managed association remains on an eligible retained pane
- **THEN** the common close resolves its execution state rather than using an independent mux-only bypass

#### Scenario: Incomplete managed discovery
- **WHEN** a confirmed container close cannot identify every hosted agent
- **THEN** it may remove the verified root without signalling guessed identities or claiming unverified process exits

### Requirement: Container close is all or nothing at admission
A confirmed container Close SHALL recursively handle its hosted managed executions and remove its panes through the common close implementation. Managed children SHALL NOT alone cause refusal. Graceful shutdown SHALL share a deadline and incomplete child handling SHALL NOT alone block bounded enclosing-resource removal; verified identity changes SHALL still fence destruction.

#### Scenario: Mixed tab
- **WHEN** a tab contains an ordinary shell and a managed agent
- **THEN** Close requests managed termination and removes the selected tab without requiring separate operator actions for each child

### Requirement: Managed operations are owner-routed
Radar SHALL route managed Close and restart to the daemon's common lifecycle implementation using exact subject/claim and placement identities. A spawn-parent relationship SHALL authorize parent requests, not select a separate termination implementation. Assignment and message-consumption authority SHALL remain with their existing owners.

#### Scenario: Idle managed worker restart
- **WHEN** an eligible managed worker is restarted
- **THEN** the daemon replaces its execution through the common lifecycle implementation

#### Scenario: Busy or parentless target
- **WHEN** a managed agent has no spawn parent or is busy but has eligible verified execution and resume context
- **THEN** those facts alone do not force an owner-file route or a second lifecycle implementation; normal restart admission and confirmation apply

#### Scenario: Close abandoned assignment
- **WHEN** Close will interrupt active work
- **THEN** confirmation names that loss and closure does not settle the assignment

#### Scenario: Different entrypoints
- **WHEN** an authorized parent and the operator each request equivalent managed closure through their respective interfaces
- **THEN** both reach the same managed termination and pane-removal semantics after authorization

### Requirement: Lifecycle results do not replace observation
Lifecycle work SHALL NOT block input or collection. Confirmed resource removal SHALL be presented normally and rows SHALL reconcile from observation. Confirmed survivors or failed removal SHALL be actionable; unavailable exit code alone SHALL remain diagnostic. Uncertain outcomes SHALL remain readable without being presented as proof that no effect occurred.

#### Scenario: Owner control unavailable
- **WHEN** retired owner-file transport is absent but daemon lifecycle authority is available
- **THEN** managed close uses the daemon and does not require that retired transport

#### Scenario: Applied operation
- **WHEN** Close or restart returns a valid outcome
- **THEN** the TUI reconciles rows through subsequent observation, without fabricating topology

#### Scenario: Control-plane outcome unknown
- **WHEN** a request reply is lost after possible dispatch
- **THEN** the view reconciles the existing operation rather than submitting a fresh destructive request

#### Scenario: Missing code only
- **WHEN** a close verifies the requested removal but cannot capture an exit code
- **THEN** the TUI does not leave the branch pending or require another confirmation solely for that missing code

#### Scenario: Confirmed surviving agent
- **WHEN** a managed execution is verified to survive after its terminal was removed
- **THEN** the outcome exposes that actionable failure without changing it into a verified exit

### Requirement: L launches the configured managed-agent command
`L` SHALL create dedicated placement in the selected eligible workspace and run `[agents].launch_command` through common managed admission and supervision. It SHALL NOT leave the program as an untracked container recipe. Existing target-shell startup, directory and direnv behaviour SHALL remain; adding a launch SHALL require no destructive confirmation.

#### Scenario: Launch the default agent
- **WHEN** the operator presses `L` on an eligible workspace with the default launch command
- **THEN** a new tab starts the default agent with verified placement, scope, claim and completion reporting

#### Scenario: Launch from a pane
- **WHEN** the operator presses `L` on an eligible pane
- **THEN** its new tab receives managed supervision and target-shell context while the source pane is left untouched

## REMOVED Requirements

### Requirement: Owner file transport preserves trust and identity
**Reason:** Retired owner-file transport is not the managed execution authority; retaining it would preserve a divergent close implementation.
**Migration:** Use daemon domain admission and existing durable operation records through the common lifecycle implementation. No owner-file compatibility adapter is retained.

### Requirement: Outcomes come from files, never local timeout
**Reason:** The effective implementation uses daemon operation records, not owner-file precedence. A timeout still does not establish that an effect never happened.
**Migration:** Read the existing durable operation outcome; active close may perform its bounded authorized escalation. Settled same-ID replay returns the recorded result without redispatch.
