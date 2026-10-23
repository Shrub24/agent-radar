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
Radar SHALL retain a private executable, argv, cwd and environment restart recipe captured from the verified managed execution. Either launched or adopted sessions SHALL be eligible for the same lifecycle controls when their evidence requirements are met. Restart SHALL resume the selected subject's fresh current session without replaying an initial prompt or one-use launch credentials. Titles and scrollback SHALL NOT supply launch recipes. Public reads SHALL omit private launch material. Ordinary baseline reporting SHALL execute nothing.

#### Scenario: Register saved-session launch
- **WHEN** a trusted publisher supplies a launch specification
- **THEN** it is durably stored without spawning, typing into or closing a pane

#### Scenario: Missing launch specification
- **WHEN** an agent has no explicit launch specification
- **THEN** its record states unavailable and no resume command is inferred

#### Scenario: Adopt a running lead
- **WHEN** Radar verifies an eligible Pi session's process and pane and captures its launch context
- **THEN** it records a managed session subject with no parent if the session was untracked, without requiring prior launch intent

#### Scenario: Unsafe restart recipe
- **WHEN** the captured recipe cannot safely resume the selected session without replaying a possible initial prompt
- **THEN** restart refuses before terminating the process

### Requirement: Registry endpoints are bounded and independent
Registry reads and writes SHALL use the daemon's trusted socket and private state directory, enforce bounded fields, records and pages, and refuse unknown request fields. Invalid records SHALL surface an error rather than disappear. Existing mux report methods SHALL retain their forwarding behavior; registry operations SHALL not enter mux mutation lanes or dispatch physical controls.

#### Scenario: Corrupt persisted registration
- **WHEN** a registry read encounters a malformed record
- **THEN** it reports invalid stored evidence instead of returning a successful list that silently omits it

#### Scenario: Existing report client
- **WHEN** a client sends the existing mux report method
- **THEN** its established Herdr-forwarding semantics remain unchanged

### Requirement: Current session context is mutable and fenced
pi-radar SHALL report process birth identity, current location, session UUID and a session-load claim ID through one presence operation, with acknowledged `registration_id` and `subject_id` when known. Radar SHALL resolve internal process registration, mutable context and durable session subject separately. It SHALL return `registration_id`, claim ID and `granted`, `duplicate`, `pending` or `released` standing; only a grant includes `subject_id`. Retries SHALL reuse the claim ID. The public process identifier SHALL be `registration_id`, without an `agent_id` compatibility alias. Generic publisher registration/context operations SHALL NOT be pi-radar's session-management path.

#### Scenario: Reconnect reasserts a claim
- **WHEN** pi-radar renews or reconnects for the same loaded session
- **THEN** Radar returns the stored decision without allocating another subject or creating a fresh admission opportunity

#### Scenario: A process switches or forks its session
- **WHEN** the same process reports a new session-load claim
- **THEN** its process registration remains unchanged, the new session is resolved to its own subject, and the previous subject retains its children, inbox and history

#### Scenario: An equal sequence repeats identical content
- **WHEN** a publisher replays the same sequence and content after an uncertain acknowledgment on a separately fenced context channel
- **THEN** the daemon returns the stored context without extending its lease

#### Scenario: Another handle tries to take the record
- **WHEN** a handle that is not the current context writer reports before retirement or lease expiry
- **THEN** the daemon refuses it and leaves the incumbent record unchanged

#### Scenario: Process location changes
- **WHEN** the same verified process reports a new pane
- **THEN** Radar updates mutable location without changing process registration or subject identity solely because of placement

#### Scenario: Another process presents an acknowledged registration
- **WHEN** the report's registration ID belongs to another process birth identity
- **THEN** Radar refuses the report and preserves the current attachment

#### Scenario: Verification is temporarily unavailable
- **WHEN** process or pane evidence cannot be established
- **THEN** Radar retains `pending` standing, publishes no subject authority, and permits retry with the same claim

#### Scenario: Session changes while a control is pending
- **WHEN** a process switches away from the subject named by a pending effect before dispatch
- **THEN** the effect does not execute against that process on the strength of PID/birth identity alone

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

### Requirement: Managed Pi subjects identify tracked sessions
A Pi subject SHALL be Radar's durable identity for one canonical Pi session UUID, independent of process, session-file path and pane. One subject SHALL have at most one accepted active process attachment; one process SHALL have at most one current session-subject attachment. A new session SHALL NOT inherit another session's parent or inbox solely because it occupies the same process. Missing durable authority SHALL NOT be recreated from Pi custom entries.

#### Scenario: A new process resumes a known session
- **WHEN** a fresh process reports a tracked session and the previous attachment is released or verified no longer active for that session
- **THEN** Radar binds it to the existing subject and retains that subject's relationships and inbox identity

#### Scenario: An untracked session is opened
- **WHEN** eligible Pi reports a session absent from Radar's durable records
- **THEN** Radar creates a new subject rather than recovering authority from conversation contents

### Requirement: Pi enrollment uses one verified binding path
pi-radar SHALL request automatic management for each eligible Pi session through `agent.presence`. Radar SHALL require verified process birth identity, pane occupancy and fresh session evidence. Manual adoption, managed launch and reconnect SHALL use this same report path. Herdsman SHALL consume the returned association and SHALL NOT run enrollment or register with the daemon.

#### Scenario: Manual Pi becomes managed
- **WHEN** pi-radar reports an eligible manually started Pi session
- **THEN** it receives a managed subject association without operator enrollment or Herdsman

#### Scenario: Retried report or daemon restart
- **WHEN** an unchanged attachment is reported after a lost reply or daemon restart
- **THEN** Radar restores or returns the recorded binding without creating another subject or repeating a launch

#### Scenario: Verification is unavailable
- **WHEN** required process/location evidence is unavailable
- **THEN** baseline publication remains possible but management is visibly unresolved

### Requirement: Supplied launch correlation constrains binding
Radar SHALL validate supplied launch correlation against its admitted launch and process evidence before binding. Invalid correlation SHALL NOT fall back to adoption. A validated launch and an adopted session SHALL converge on the same binding representation. Dispatch, managed registration and application readiness SHALL remain distinct outcomes.

#### Scenario: Child registers after launch
- **WHEN** a child registers with valid launch correlation and matching evidence
- **THEN** Radar binds its session subject with the recorded parent relationship rather than creating an unrelated lead

#### Scenario: Correlation is invalid
- **WHEN** correlation is invalid or identifies a launch inconsistent with the reported process
- **THEN** Radar refuses binding without inferring launch success or creating an alternative subject

### Requirement: Duplicate live sessions remain preliminary
Radar SHALL durably record duplicate claim decisions, keep those registrations baseline-visible with a distinct inline warning, and grant no subject or inbox authority. Renewal, incumbent release and daemon restart SHALL NOT promote the rejected claim. Only a fresh Pi session-load claim SHALL permit reconsideration.

#### Scenario: Incumbent exits after duplicate opens
- **WHEN** a duplicate remains running after the incumbent exits
- **THEN** it remains unmanaged until a fresh session load or restart makes it eligible for verification again

#### Scenario: Verification is temporarily unavailable
- **WHEN** process verification or daemon recovery cannot establish whether a claim can be granted
- **THEN** its standing is pending and the same claim may retry without a fresh session load

### Requirement: Relationships and receipt authority follow the session subject
Logical parent relationships SHALL remain Radar-owned durable subject relationships rather than depend on the original parent registration. Execution-specific spawn and communication provenance SHALL remain historical records. Handoff access SHALL resolve through the caller's current accepted subject binding. Receipt SHALL NOT imply application settlement or permission to close a worker.

#### Scenario: Lead resumes in a different tab
- **WHEN** an eligible fresh lead attaches to its session subject in another tab
- **THEN** its child links and inbox identity remain and the UI retains a basic parent association while rows stay in their physical tabs
- **AND** registration moves no pane and requires no reunion operation

#### Scenario: Session switch changes inbox access
- **WHEN** a process switches from session A to session B
- **THEN** its active association no longer grants A's inbox rights, and A's pending messages remain addressed to A's subject

#### Scenario: Child process is replaced
- **WHEN** a fresh process validly attaches to an existing child session subject
- **THEN** it retains the recorded parent rather than inheriting parentage from its new pane or caller-supplied claims

### Requirement: Session breadcrumbs are not a management protocol
pi-radar SHALL NOT write or require Pi-session launch identity markers. Historical entries SHALL NOT gate controls, restore authority or produce routine marker-mismatch state. Initial launch correlation, process verification, writer leases and effect fencing SHALL remain independent of session entries.

#### Scenario: Old marker exists or host lacks custom entries
- **WHEN** a session contains historical launch markers or the host cannot write custom session entries
- **THEN** ordinary binding behavior is unchanged and no old execution identity is inherited

### Requirement: Legacy subject adaptation preserves authority
Radar SHALL preserve existing subject IDs, operation history, inboxes and parentage when establishing an unambiguous session association. It SHALL NOT silently merge conflicting legacy subjects or infer missing session evidence. Rollback support SHALL be documented according to persisted-format compatibility rather than presumed.

#### Scenario: Multiple old subjects name one session
- **WHEN** legacy records give conflicting subject identities for the same session
- **THEN** Radar exposes an unresolved association without merging or deleting their authority
