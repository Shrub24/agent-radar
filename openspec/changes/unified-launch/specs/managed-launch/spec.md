## ADDED Requirements

### Requirement: Every managed launch runs through radar run
A launch the daemon creates SHALL start its pane with `radar run --launch`, which claims the recorded launch with its correlation secret and birth identity and receives the program from the daemon's record. No managed launch SHALL run through another chain.

#### Scenario: A spawned child claims its launch
- **WHEN** a pane created for a spawn runs `radar run --launch` with the launch's correlation
- **THEN** the daemon answers with the registration and program, and the program runs inside the registration's scope

#### Scenario: A process without the correlation
- **WHEN** `radar run --launch` presents a correlation or birth identity that does not match the recorded launch
- **THEN** it is refused and runs nothing

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
- **WHEN** the child still runs
- **THEN** it is stopped and its exit awaited before the relaunch, and a child that does not exit leaves the relaunch withdrawn

#### Scenario: Another parent's child
- **WHEN** the subject's recorded parent is not the caller's subject
- **THEN** the request is refused and nothing is created

### Requirement: A relaunch keeps the agent's pane when it exists
A relaunch SHALL place the process in the agent's own pane when that pane still exists, and otherwise split beside the parent.

#### Scenario: The child's pane was closed
- **WHEN** a child is continued after its pane was closed
- **THEN** it starts in a new pane beside its parent

### Requirement: Assignment facts belong to the launch
Owner, run, label and role SHALL be recorded per launch, and the subject's parent SHALL NOT change on a relaunch.

#### Scenario: Continuing under a new run
- **WHEN** a child is continued with a new run
- **THEN** reads show the new run, and the earlier launch keeps its own
