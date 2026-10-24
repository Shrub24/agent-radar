## MODIFIED Requirements

### Requirement: A consumer lists its own children
pi-radar SHALL list the calling process's children through Radar's subject-derived child read, each with its subject id, Pi session id, attachment state, current location when known and label. The default SHALL include retained children; live-only selection SHALL be explicit. A caller without a granted subject SHALL be refused.

#### Scenario: Selecting a child by session
- **WHEN** a consumer has a Pi session id for one of its children
- **THEN** the listing names that child's subject id

#### Scenario: A duplicate caller
- **WHEN** the caller's claim is not granted
- **THEN** the listing is refused

### Requirement: Child control is addressed by subject id
pi-radar SHALL address assign, retire, inspect and observe of a child by subject id through daemon subject operations. It SHALL NOT receive, resolve or cache child registration or claim ids, choose a current process from spawn history, or require caller-supplied session identity to validate the target. Its own registration and claim SHALL remain self-publication authority.

#### Scenario: A restarted child keeps its key
- **WHEN** a child's process is replaced and the same subject attaches under a new registration
- **THEN** each operation by the recorded subject id reaches the new process

#### Scenario: A resumed subject has several edges
- **WHEN** a subject has more than one bound edge
- **THEN** Radar resolves the current attachment from subject authority and pi-radar does not rank edges

#### Scenario: A child with no subject on its edge
- **WHEN** an unbound launch cannot yet name a granted child
- **THEN** the SDK reports launch state without substituting a process identifier as the child key

### Requirement: An assignment follows the agent across a restart
pi-radar SHALL publish child assignments by subject to Radar, which SHALL own assignment retention and delivery across process replacement. SDK reads SHALL have no assignment publication side effects and SHALL NOT keep process-specific child writers or replay assignments when a process changes.

#### Scenario: A child restarts with a retained assignment
- **WHEN** a child with a still-valid published assignment attaches under a new registration
- **THEN** Radar delivers that assignment to the granted attachment without an SDK read or replay

#### Scenario: A duplicate holder
- **WHEN** another process opens the same session and is refused the subject
- **THEN** it receives no assignment
