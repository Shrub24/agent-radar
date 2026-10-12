## ADDED Requirements

### Requirement: Agent facts are a separate self-authored channel
The registry SHALL accept a separate self-authored `facts` channel for model, provider, thinking, context usage, session display name and unresolved background tasks. It SHALL use independent writer fencing, sequence and freshness and SHALL NOT merge facts with execution or assignment.

#### Scenario: Facts without an owner
- **WHEN** an ordinary session registers and publishes facts
- **THEN** the registry exposes them with that publisher's provenance and requires no owner or assignment

#### Scenario: Unknown field
- **WHEN** a facts snapshot contains a field the contract does not define
- **THEN** it is refused and the previous snapshot is unchanged

### Requirement: Facts contract is negotiated and bounded
The daemon SHALL advertise `registry.facts.v1` on protocol v2. Text SHALL use the existing 1 KiB bound; task and awaited lists SHALL contain at most 64 entries. Nested unknown fields and invalid timestamps SHALL be refused without changing the previous snapshot. Snapshots SHALL replace the complete channel value; omission withdraws an optional field.

#### Scenario: Oversized list
- **WHEN** a publisher submits more than 64 background tasks or awaited items
- **THEN** the daemon refuses it and preserves the previous snapshot

#### Scenario: Withdraw a fact
- **WHEN** a successor facts snapshot omits the previously published model
- **THEN** reads no longer expose that model

### Requirement: Assignment snapshots carry owner-only facts
The assignment snapshot SHALL optionally carry the owner's role or definition, run and request identity, assignment display text with its start time, outstanding awaited items, and a lead's pending ask. They SHALL retain publisher-reported owner provenance, distinct from the facts channel, and SHALL NOT be inferred from execution activity. Owner authorship is a publisher convention under the existing same-UID trust boundary, not authenticated daemon authority.

#### Scenario: Awaited items
- **WHEN** an owner publishes awaited items for a lead
- **THEN** reads expose them with the owner's provenance and without changing the lead's execution activity

### Requirement: One writer owns each fact
Each published fact SHALL have exactly one owning channel. A read SHALL NOT combine two channels' values for one field, and an agent whose channel is absent or stale SHALL expose that fact as unavailable or stale rather than substituting another source.

#### Scenario: Stale facts
- **WHEN** the facts channel's lease has expired
- **THEN** reads report its snapshot as stale and still keep it separate from fresh execution

### Requirement: Reads and events report the new channels
`agent.get` and `agent.list` SHALL expose each channel's snapshot with source, incarnation, generation, sequence and freshness and never the fencing handle. A publication SHALL append a pointer event like every other channel, carrying identifiers only.

#### Scenario: Pointer event
- **WHEN** a facts snapshot is accepted
- **THEN** the event journal records an identifier-only entry for that agent and channel
