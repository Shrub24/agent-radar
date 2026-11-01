# Spec Delta

## MODIFIED Requirements

### Requirement: Every managed launch runs through radar run
Daemon-created and directly admitted managed launches SHALL enter one supervised `radar run` execution chain after their placement and admission checks. Daemon-created launches SHALL claim their recorded reservation; direct admission SHALL verify the caller's existing placement. Both SHALL run the actual program inside its execution scope and expose the same lifecycle capabilities.

#### Scenario: A spawned child claims its launch
- **WHEN** a spawned child presents its valid launch credential and identity
- **THEN** the common supervised chain runs its recorded program and binds the actual managed execution

#### Scenario: Directly admitted agent
- **WHEN** `radar run --agent` verifies admission in an existing shell pane
- **THEN** it enters the same supervised chain without creating a pane or fabricating a spawn parent

#### Scenario: A process without the correlation
- **WHEN** a daemon-created launch cannot establish its recorded admission authority
- **THEN** no managed program is released

### Requirement: A parent can restart its own child
`lifecycle.restart` SHALL authorize a recorded parent against its child subject, resolve and freeze the current claim, and use the common termination and start behaviour. Operator restart SHALL remain fenced by its expected claim. The successor SHALL retain subject identity, use a fresh claim and resume the recorded session; an existing successor reservation SHALL refuse another restart.

#### Scenario: Restart under a new definition
- **WHEN** an authorized parent restarts a live child with an eligible program
- **THEN** the predecessor is terminated through the common workflow and a fresh claim resumes that child's session

#### Scenario: Two restarts in a row
- **WHEN** a restart arrives while a successor is reserved
- **THEN** it is refused without terminating a different execution; a later authorized restart resolves the current successor rather than the original claim

### Requirement: A relaunch keeps the agent's pane when it exists
Restart SHALL reuse verified dedicated placement when safe. It SHALL NOT inject a replacement into an operator's resumed interactive shell; that case SHALL create replacement placement alongside the recorded location. If placement is gone, an eligible recorded-parent fallback MAY be used; otherwise relaunch SHALL refuse rather than guess from current focus.

#### Scenario: Dedicated dead pane remains
- **WHEN** restart has verified reusable dedicated placement
- **THEN** the replacement starts there through the common supervised launch chain

#### Scenario: Agent was admitted from a shell
- **WHEN** an agent admitted from an interactive shell is restarted
- **THEN** replacement placement is created beside its verified location and the original shell is not commandeered

#### Scenario: The child's pane was closed
- **WHEN** the old pane is absent but a verified recorded-parent fallback exists
- **THEN** the successor starts beside that parent through common managed supervision

#### Scenario: Placement cannot be resolved
- **WHEN** neither safe placement nor an eligible recorded-parent fallback exists
- **THEN** relaunch refuses without using whichever pane currently has focus
