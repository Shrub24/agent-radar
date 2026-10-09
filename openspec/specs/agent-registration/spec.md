# agent-registration Specification

## Purpose
Keep agent lifecycle identities and direct publisher facts in the coordinator independently of the multiplexer, providing a durable foundation for later launch and recovery controls.

## Requirements

### Requirement: Session context and live incarnation are separate
The daemon SHALL register an agent incarnation independently of its session UUID and mux location. Registration SHALL carry a publisher incarnation, optional owner/run links and optional backend-qualified location. Duplicate attaches to one session SHALL remain distinct records. Process identity SHALL be verified by birth identity, never PID alone.

#### Scenario: Two attaches share a session
- **WHEN** two live publishers register the same session with different process incarnations
- **THEN** both remain distinct and neither replaces the other by session UUID

#### Scenario: Recycled process id
- **WHEN** a registered PID has a different boot or start identity
- **THEN** it does not validate the registered incarnation

### Requirement: Direct reports preserve provenance
The daemon SHALL accept execution and assignment snapshots directly without consulting or writing Herdr. Execution evidence, owner assignment projection, waiting reason, last outcome and advertised actions SHALL stay separate and source-labelled. Unknown vocabulary SHALL be preserved. Registration and advertising SHALL NOT by themselves authorize restart or certify assignment completion.

#### Scenario: Failed turn on an idle process
- **WHEN** execution reports idle and a failed last outcome while its owner reports an unsettled assignment
- **THEN** all facts remain available separately without being collapsed into successful idle

#### Scenario: No mux backend
- **WHEN** a registered publisher reports while the physical backend is unavailable
- **THEN** the report is accepted independently and the location observation remains unavailable

### Requirement: Writer generations and sequences fence updates
The daemon SHALL issue a registration handle bound to the publisher incarnation and require it for updates. Updates SHALL carry a monotonically increasing sequence; equal identical content SHALL be idempotent, and conflicting or older updates SHALL be refused. A replacement writer SHALL NOT silently supersede a fresh writer; an explicitly retired or expired writer SHALL not update its replacement.

#### Scenario: Delayed reconnect report
- **WHEN** an old handle publishes after its writer was replaced
- **THEN** the update is refused and the replacement's facts remain unchanged

#### Scenario: Repeated snapshot
- **WHEN** the same handle repeats an equal sequence and identical snapshot
- **THEN** the answer is idempotent and no new physical operation is performed

### Requirement: Registration durability does not imply freshness
The daemon SHALL persist accepted registrations and snapshots before acknowledging them. Reads SHALL expose freshness and process verification independently. Expiry or disconnect SHALL mark reports stale, not prove process exit. After daemon restart, restored reports SHALL remain stale until their publisher reconnects and refreshes them. Positive absence SHALL require independent physical evidence.

#### Scenario: Daemon restarts
- **WHEN** durable records are restored
- **THEN** identities survive but their snapshots are not presented as freshly published

#### Scenario: Publisher stops reporting
- **WHEN** its freshness lease expires
- **THEN** the last report is marked stale without claiming the agent exited or became idle

### Requirement: Launch specifications are explicit private records
The daemon SHALL accept a versioned launch/resume specification tied to a registered agent: executable, argv, cwd and explicit session reference, with a provenance and revision. It SHALL store this privately and SHALL NOT reconstruct it from titles, scrollback or process arguments. Public snapshot reads SHALL omit the executable arguments and session paths. Registration SHALL execute nothing.

#### Scenario: Register saved-session launch
- **WHEN** a trusted publisher supplies a launch specification
- **THEN** it is durably stored without spawning, typing into or closing a pane

#### Scenario: Missing launch specification
- **WHEN** an agent has no explicit launch specification
- **THEN** its record states unavailable and no resume command is inferred

### Requirement: Registry endpoints are bounded and independent
Registry reads and writes SHALL use the daemon's trusted socket and private state directory, enforce bounded fields, records and pages, and refuse unknown request fields. Invalid records SHALL surface an error rather than disappear. Existing mux report methods SHALL retain their forwarding behavior; registry operations SHALL not enter mux mutation lanes or dispatch physical controls.

#### Scenario: Corrupt persisted registration
- **WHEN** a registry read encounters a malformed record
- **THEN** it reports invalid stored evidence instead of returning a successful list that silently omits it

#### Scenario: Existing report client
- **WHEN** a client sends the existing mux report method
- **THEN** its established Herdr-forwarding semantics remain unchanged

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
