## MODIFIED Requirements

### Requirement: Reads and events report the new channels
`subject.get`, `subject.list` and `subject.children` SHALL expose the subject's channels with source, incarnation, generation, sequence and freshness, never fencing handles or child registration and claim ids. A publication SHALL append an identifier-only pointer event. Internal process provenance SHALL remain recorded; consumer subject-state notifications SHALL name the subject.

#### Scenario: Pointer event
- **WHEN** a facts snapshot is accepted
- **THEN** the event journal records an identifier-only entry for that agent and channel

## ADDED Requirements

### Requirement: Registration identity is for self publication
pi-radar SHALL retain its own registration and claim for presence, publication and caller authority. Public `agent.get` and `agent.list` process discovery SHALL be replaced by subject reads. Process-addressed storage and sampler joins SHALL remain daemon-internal; self-publication SHALL NOT confer authority over another registration.

#### Scenario: Child inspection
- **WHEN** a parent inspects its child
- **THEN** it supplies the child subject and its own caller authority, not the child's process identity
