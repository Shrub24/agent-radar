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
- **WHEN** the child still runs or a successor is already reserved
- **THEN** the request is refused and the child is not signalled

#### Scenario: A caller without a granted subject
- **WHEN** a process whose claim is duplicate, pending or released asks to continue a child
- **THEN** the request is refused before any effect

#### Scenario: Another parent's child
- **WHEN** the subject's recorded parent is not the caller's subject
- **THEN** the request is refused and nothing is created

### Requirement: A parent can restart its own child
`lifecycle.restart` SHALL accept a request from the subject's recorded parent addressed by subject alone, with the program and args the parent sends; the daemon SHALL resolve the current process itself, stop it through its scope, await its exit and relaunch. An operator restart SHALL stay fenced by the claim it names.

#### Scenario: Restart under a new definition
- **WHEN** a parent restarts its live child with a new program
- **THEN** the current process is stopped and the child relaunches in its own pane and session with the new program

#### Scenario: Two restarts in a row
- **WHEN** an operator restart and a parent restart of the same child arrive one after the other
- **THEN** the second replaces the first's successor, and a restart arriving while a successor is reserved is refused

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
