## ADDED Requirements

### Requirement: A launch result names the child by its subject id
pi-radar SHALL return the launched child's subject id from a ready launch, and SHALL treat a bound edge that carries no subject as unresolved rather than substituting its registration.

#### Scenario: A launch result names the subject
- **WHEN** a launch completes
- **THEN** the result carries the child's subject id, taken from the spawn edge

#### Scenario: An edge bound without a subject
- **WHEN** the daemon binds the edge but records no subject
- **THEN** the launch is unresolved and names the missing subject

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
