## Purpose

Expose agents through their durable subjects while keeping process attachment, replacement and delivery inside Radar.

## ADDED Requirements

### Requirement: Subject reads distinguish live and retained agents
`subject.list` SHALL page subjects by subject id with a `live`, `retained` or `all` selection, defaulting to `live`. `subject.get` SHALL read the same record by subject id, including retained subjects. A live subject SHALL have a verified current granted attachment; reserved launches SHALL read as starting, not live. Uncertain liveness SHALL remain explicit rather than imply exit.

#### Scenario: Process exits
- **WHEN** Radar verifies the current process exited
- **THEN** the subject leaves the live listing, remains in retained and all listings, and remains readable under the same subject id

#### Scenario: Launch is starting
- **WHEN** the subject has a reserved launch without granted presence
- **THEN** it appears in all subjects as starting and does not appear as live

#### Scenario: Process cannot be verified
- **WHEN** process evidence becomes unreadable
- **THEN** reads report unknown attachment state rather than declaring the subject exited

### Requirement: Child access derives from the caller
`subject.children` SHALL derive the parent from the caller's granted self attachment and apply the same live, retained and all selection, defaulting to all. Child reads and mutations SHALL verify the recorded parent relationship. A refused or detached caller SHALL receive no child authority. Requests SHALL NOT select a parent using a caller-supplied parent subject.

#### Scenario: Retained worker can be selected
- **WHEN** a parent lists its children after a worker exits
- **THEN** it receives the retained worker's subject and recorded session for a later continue

#### Scenario: Refused caller
- **WHEN** a duplicate caller requests a child operation
- **THEN** Radar refuses without reading private child detail or changing assignment state

### Requirement: Agent consumers address subjects rather than processes
Public agent reads and child operations SHALL accept subject ids and report subject state, session, label, current location, channel freshness and management status without child registration or claim ids. Radar SHALL resolve attachment and process internally through current subject authority, not historical edge ordering. Process loss SHALL update subject state without deleting its identity or relationships.

#### Scenario: Replacement process
- **WHEN** a replacement attaches to a subject
- **THEN** the same subject address reaches the replacement without consumer-side rebinding

#### Scenario: Late report from an old process
- **WHEN** a superseded process reports status or exit
- **THEN** it cannot overwrite the subject's current attachment state

### Requirement: Assignments belong to subjects
Radar SHALL accept assignment update and retirement addressed to a child subject, preserve owner provenance and existing writer fencing and lease semantics, and retain the accepted assignment independently of the child's process. A replacement granted attachment SHALL receive the still-valid assignment without any parent read or SDK replay. Retirement SHALL withdraw the assignment, not delete the subject.

#### Scenario: Replacement without parent polling
- **WHEN** a child restarts while an assignment remains valid and its parent performs no read
- **THEN** Radar makes the assignment available to the replacement only after it receives a granted attachment

#### Scenario: Expired assignment
- **WHEN** a child returns after its assignment lease expires
- **THEN** Radar does not represent that assignment as fresh or renew it merely because the child returned

#### Scenario: Duplicate process
- **WHEN** another process is refused the subject
- **THEN** it receives no subject assignment authority

### Requirement: Refused processes remain diagnostic evidence
Fleet observation SHALL preserve duplicate and unresolved process diagnostics separately from subject inventory. A refused process SHALL NOT become another subject or gain agent controls solely to remain visible.

#### Scenario: Duplicate is visible but not an agent identity
- **WHEN** a second process opens an already-held session
- **THEN** the fleet can show its refused standing while subject discovery still returns one agent
