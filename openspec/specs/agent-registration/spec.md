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

### Requirement: Registration may carry a single-use spawn token

A registration MAY carry the spawn correlation token issued to the launch that
started it. The token SHALL never appear in a public read, SHALL bind at most
one pending edge, and SHALL be refused when malformed or already spent.

#### Scenario: A registration binds its spawn
- **WHEN** a child registers with the token minted for its spawn
- **THEN** the registration succeeds and the pending edge binds to that agent

#### Scenario: A public read hides the token
- **WHEN** any agent registration, get or list read is served
- **THEN** the spawn token does not appear in the response

#### Scenario: A spent or malformed token
- **WHEN** a registration presents a token that already bound a child or is not a valid token
- **THEN** the registration is refused and nothing binds

### Requirement: Spawn bootstrap reuses exact registration
A spawned publisher SHALL present the delivered token privately in agent.register with its own source, incarnation and process claim. Successful registration SHALL consume the token for that exact registration, with identical retry remaining idempotent. The publisher SHALL remove the token from its launch environment after capture and SHALL NOT persist it in owner sidecars, facts, labels or logs.

#### Scenario: Child bootstrap
- **WHEN** a spawned child registers with its own incarnation and the delivered token
- **THEN** the returned agent_id identifies that registration and the spawn edge binds to it

#### Scenario: Registration answer is lost
- **WHEN** the child repeats the identical registration after an uncertain acknowledgement
- **THEN** it receives the existing registration and no second binding or registration is created

#### Scenario: Token reused by another incarnation
- **WHEN** another incarnation attempts to consume an already-bound spawn token
- **THEN** registration refuses and the original binding remains unchanged

### Requirement: Binding does not imply application readiness
Token binding SHALL establish only the exact registration associated with a daemon spawn. It SHALL NOT certify assignment readiness, process exit or assignment completion. Consumers SHALL read execution and assignment reports separately with their own freshness and provenance; never treat launch dispatch, pane presence or a Herdr alias as application readiness.

#### Scenario: Bound but no execution publication
- **WHEN** a child is bound but has not published fresh execution facts
- **THEN** topology reports the binding while execution readiness remains unknown

#### Scenario: Bound with fresh execution publication
- **WHEN** a bound child publishes execution facts
- **THEN** a consumer can read those facts using its exact agent_id without looking up a Herdr agent alias

### Requirement: Agent facts are a separate self-authored channel
The registry SHALL accept a separate self-authored `facts` channel for model, provider, thinking, context window usage, session display name and unresolved background tasks. Context window usage SHALL be `context_window` carrying integer `tokens` in context and integer `window` size, where `window` is greater than zero and `tokens` does not exceed it. It SHALL use independent writer fencing, sequence and freshness and SHALL NOT merge facts with execution or assignment.

#### Scenario: Facts without an owner
- **WHEN** an ordinary session registers and publishes facts
- **THEN** the registry exposes them with that publisher's provenance and requires no owner or assignment

#### Scenario: Unknown field
- **WHEN** a facts snapshot contains a field the contract does not define
- **THEN** it is refused and the previous snapshot is unchanged

#### Scenario: Context usage is typed
- **WHEN** a publisher reports 86000 tokens in a 200000-token window
- **THEN** reads expose both integers and a display derives 43% from them

#### Scenario: Invalid context window
- **WHEN** a snapshot has a zero window, tokens above the window, a negative count or text in place of an integer
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
