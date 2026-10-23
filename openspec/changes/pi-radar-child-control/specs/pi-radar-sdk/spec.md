## ADDED Requirements

### Requirement: Child control is addressed by subject id
pi-radar SHALL address assign, retire, inspect and observe of a launched child by its subject id, resolving the child's current registration from the daemon's spawn edges on each operation and caching nothing across a restart.

#### Scenario: A restarted child keeps its key
- **WHEN** a child's process is replaced and the same subject attaches under a new registration
- **THEN** each operation by the recorded subject id reaches the new process

#### Scenario: A resumed subject has several edges
- **WHEN** a subject has more than one bound edge
- **THEN** the edge with the newest `bound_at` decides the current registration

#### Scenario: A child with no subject on its edge
- **WHEN** the edge records no subject
- **THEN** the operation reports the child as not addressable and does not fall back to a registration

### Requirement: An assignment follows the agent across a restart
pi-radar SHALL keep the last assignment it published for a child, keyed by subject, and when the child's registration changes SHALL retire the writer on the old registration and publish that assignment on the new one.

#### Scenario: A child restarts with a retained assignment
- **WHEN** a child with a published assignment attaches under a new registration
- **THEN** the new registration carries the same assignment and the old channel is retired

#### Scenario: A duplicate holder
- **WHEN** another process opens the same session and is refused the subject
- **THEN** it receives no assignment

### Requirement: A launch can continue an existing child agent
`spawn.create` SHALL accept an optional `continue_subject_id`, refused before any effect unless the subject is recorded, is the requesting parent's own child, has a recorded session, and has no live claim and no reserved successor. The daemon SHALL start the child in the subject's recorded session, replacing any session selector in argv, and reserve the launch on that subject.

#### Scenario: Continuing a released child
- **WHEN** a parent continues its own child whose process ended
- **THEN** the launch reserves on that child's subject and the child is started in the subject's recorded session

#### Scenario: Continuing a child that is still attached
- **WHEN** a live claim still holds the subject
- **THEN** the request is refused and nothing is created

#### Scenario: Continuing another parent's child
- **WHEN** the subject's recorded parent is not the requesting parent's subject
- **THEN** the request is refused and nothing is created

#### Scenario: The caller names a session in argv
- **WHEN** the requested argv contains `--session` or another session selector
- **THEN** the daemon replaces it with the subject's recorded session

#### Scenario: The continued child reports another session
- **WHEN** the child's first presence report names a session other than the subject's recorded one
- **THEN** the report is refused and the subject is not attached

### Requirement: A fresh launch never takes over another subject's session
At first presence, the daemon SHALL refuse to grant a launch's placeholder subject a session that another subject already holds a record for, and the launch SHALL stay unattached. A process that was not launched SHALL still reattach to the session's existing subject.

#### Scenario: A fresh launch opens an owned session
- **WHEN** a launch without `continue_subject_id` reports a session recorded for another, released subject
- **THEN** the report is refused, and the session's subject keeps its children and inbox

#### Scenario: A manually started process resumes a session
- **WHEN** an unlaunched process reports a session recorded for a released subject
- **THEN** it is granted that subject, as before
