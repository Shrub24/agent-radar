## ADDED Requirements

### Requirement: A consumer lists its own children
pi-radar SHALL list the calling process's children, derived from its own granted subject, each with its subject id, Pi session id, state (live, released or gone), pane and label, and a caller without a granted subject SHALL get no children.

#### Scenario: Selecting a child by session
- **WHEN** a consumer has a Pi session id for one of its children
- **THEN** the listing names that child's subject id

#### Scenario: A duplicate caller
- **WHEN** the caller's claim is not granted
- **THEN** the listing is refused

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

