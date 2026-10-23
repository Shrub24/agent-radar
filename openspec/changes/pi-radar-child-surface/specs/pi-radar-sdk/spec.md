## ADDED Requirements

### Requirement: Consumers address a child by its subject id
pi-radar SHALL address a launched child, in the runtime surface a consumer imports, by the child's subject id and by no registration id, claim id, source or incarnation. It SHALL resolve the child's current registration from the daemon's spawn edge for each operation, so a child that restarted or resumed under the same subject is the same child.

#### Scenario: A restarted child keeps its key
- **WHEN** a child's process is replaced and the same subject attaches under a new registration
- **THEN** assign, retire, inspect and observe by the recorded subject id reach the new process

#### Scenario: A launch result names the subject
- **WHEN** a launch completes
- **THEN** the result carries the child's subject id and no registration id or bound process

#### Scenario: A child with no subject yet
- **WHEN** the child's edge is unbound
- **THEN** the surface reports the child as not yet addressable and does not substitute a registration

### Requirement: A launch takes its parent from the caller's own publication
`launch` SHALL derive the parent from the process's own publication and SHALL NOT accept registration or subject identifiers for the parent from the caller.

#### Scenario: A launch without a parent argument
- **WHEN** a consumer launches a worker
- **THEN** the managed parent is the calling process's own subject and the consumer passes no identity for it

#### Scenario: The caller has no managed subject
- **WHEN** the publication has no granted subject
- **THEN** the launch is refused before any request and says so

### Requirement: The handoff sender is a subject id
A received handoff SHALL expose its sender as the sender's subject id, and a consumer matching a handoff to a child SHALL compare subject ids.

#### Scenario: A handoff from a restarted child
- **WHEN** a child publishes a handoff after its process was replaced
- **THEN** the sender equals the subject id the consumer recorded at launch

### Requirement: The spawn edge records the bound child's subject
The daemon SHALL record the bound child's subject id on the spawn edge's `bound` record, and the public spawn contract and generated schemas SHALL carry it as an optional field.

#### Scenario: A bound edge
- **WHEN** a managed child presents its registration and the edge binds
- **THEN** `bound` carries that registration's granted subject id

#### Scenario: A legacy edge
- **WHEN** an edge was recorded before this field existed
- **THEN** it reads with no subject and no migration is run
