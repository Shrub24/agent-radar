# agent-registration Specification (delta)

## ADDED Requirements

### Requirement: Current session context is mutable and fenced
The daemon SHALL accept a current session association for a registered agent as a separate record, fenced like other published facts: one writer at a time, a strictly forward sequence, a bounded lease and daemon-time freshness. Publishing it SHALL NOT change the agent id, the registration content or the writer generation.

#### Scenario: A process switches or forks its session
- **WHEN** a registered publisher republishes its current session with a newer sequence
- **THEN** the read reports the new session while the agent id and registration are unchanged

#### Scenario: An equal sequence repeats identical content
- **WHEN** a publisher replays the same sequence and content after an uncertain acknowledgment
- **THEN** the daemon answers with the stored record and does not extend its lease

#### Scenario: Another handle tries to take the record
- **WHEN** a handle that is not the current writer reports context before retirement or lease expiry
- **THEN** the daemon refuses it and the incumbent's record is unchanged

### Requirement: Session context carries no private material
A context record SHALL carry only a canonical session UUID or an explicit null meaning no current session. Public reads SHALL NOT expose session-file paths, launch arguments, environment, writer handles or raw provider text. Absence of a record SHALL mean never published, distinct from an explicit null.

#### Scenario: A publisher reports a session
- **WHEN** a context record is written and then read publicly
- **THEN** the read names the session UUID, its source and its freshness, and nothing private

#### Scenario: Never reported and explicitly cleared
- **WHEN** one agent has published no context and another published an explicit null
- **THEN** the first reads as absent and the second reads as no current session

### Requirement: Registry reads report context freshness honestly
Reads SHALL report each context record's freshness and age it out on the daemon's clock. A record received before this daemon began serving SHALL read stale until republished. Stale or absent context SHALL NOT be reported as process exit, idleness or completed work.

#### Scenario: The daemon restarts
- **WHEN** the daemon starts and reads a context record written by an earlier process
- **THEN** the record reads stale until a publisher reports again

#### Scenario: A publisher stops reporting
- **WHEN** a context lease expires with no newer report
- **THEN** the read reports stale facts and no conclusion about the process

#### Scenario: A list page is read
- **WHEN** any list page is returned
- **THEN** every entry carries its own context key, whether fresh, stale or absent
