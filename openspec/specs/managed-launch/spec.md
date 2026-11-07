# managed-launch Specification

## Purpose
Every daemon-created agent launch starts through `radar run --launch`, continues or restarts an existing subject on its recorded session, and succeeds only when its first verified presence is granted within a daemon-owned deadline.

## Requirements

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

### Requirement: A relaunch resumes the agent's own session
A relaunch SHALL start the program with the given args, session selectors removed and `--session` with the agent's recorded session appended, and SHALL refuse a bare argument in args.

#### Scenario: Restart
- **WHEN** the operator restarts an agent
- **THEN** it runs with its captured program and argv in its recorded session

#### Scenario: The session is gone
- **WHEN** the recorded session no longer exists
- **THEN** the program exits without creating a session and the launch is recorded as failed

### Requirement: A parent can continue its own child
`spawn.create` SHALL accept an optional `continue_subject_id` and relaunch that subject with the program and args the parent sent, refused before any effect unless the subject is the caller's own child, has a recorded session, and has no reserved successor.

#### Scenario: Continuing a released child
- **WHEN** a parent continues its child whose process ended
- **THEN** the launch is reserved on that subject and the child runs in its recorded session

#### Scenario: Continuing a live child
- **WHEN** the child still runs or a successor is already reserved
- **THEN** the request is refused and the child is not signalled

#### Scenario: A caller without a granted subject
- **WHEN** a process whose claim is duplicate, pending or released asks to continue a child
- **THEN** the request is refused before any effect

#### Scenario: Another parent's child
- **WHEN** the subject's recorded parent is not the caller's subject
- **THEN** the request is refused and nothing is created

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

### Requirement: Assignment facts belong to the launch
Owner, run, label and role SHALL be recorded per launch, and the subject's parent SHALL NOT change on a relaunch.

#### Scenario: Continuing under a new run
- **WHEN** a child is continued with a new run
- **THEN** reads show the new run, and the earlier launch keeps its own

### Requirement: The daemon withdraws a launch that never presents
Radar SHALL withdraw a managed launch whose reservation has not been replaced by a granted claim within a fixed ceiling counted from its spawn edge's recorded time: revoke the claim, stop the captured process through its scope, close the launch's own pane, and record the withdrawal and its reason on the edge. The ceiling SHALL NOT be a request parameter. A launch that was withdrawn SHALL leave its subject free to be continued.

#### Scenario: A child never presents
- **WHEN** a launched process has not presented within the ceiling
- **THEN** its claim is revoked, its process is stopped, its pane is closed and its edge reads as withdrawn with a reason

#### Scenario: A child presents as the ceiling passes
- **WHEN** a child's first report and the withdrawal arrive together
- **THEN** exactly one wins: a bound child is stopped as a bound child, and a withdrawn launch refuses the late report

#### Scenario: The daemon restarts during a launch
- **WHEN** the daemon restarts before a launch presents
- **THEN** the ceiling still counts from the edge's recorded time and the launch is withdrawn on schedule

#### Scenario: A withdrawn launch frees its subject
- **WHEN** a launch is withdrawn
- **THEN** the subject has no live claim and a later continue is accepted

### Requirement: A launch outcome is read from the edge
pi-radar SHALL report a launch, continue or restart as ready once the edge shows a granted subject, and as failed with the daemon's reason once the edge shows it withdrawn. It SHALL NOT cancel a launch and SHALL NOT impose a timeout of its own.

#### Scenario: A withdrawn launch
- **WHEN** the edge reads as withdrawn
- **THEN** the caller receives a failure naming the daemon's reason and may retry

#### Scenario: The caller aborts its wait
- **WHEN** the caller aborts while the launch is starting
- **THEN** the wait ends and the daemon's ceiling still cleans the launch up

### Requirement: A subject has one live claim, reserved or granted
A reservation SHALL be a claim minted before its process exists. Reserving and granting SHALL pass through one transition that refuses while the subject has any other live claim, reserved or granted; a grant SHALL replace only the reservation naming the presenting process.

#### Scenario: A second reservation
- **WHEN** a subject holds a reserved or granted claim and another launch tries to reserve it
- **THEN** the second reservation is refused

#### Scenario: A grant against another's reservation
- **WHEN** a process presents a session whose subject is reserved for a different process
- **THEN** the report is refused as a duplicate and the reservation is unchanged
