# mux-control-plane Specification

## Purpose
Abstract physical multiplexer operations behind a backend-neutral daemon interface, with Herdr as the first backend, so Radar and future consumers stay independent of any one mux grammar or transport while mux-specific decoding remains adapter-owned.

## Requirements

### Requirement: The control plane exposes normalized mux primitives
The daemon SHALL expose inventory, foreground evidence, focus, guarded close, creation/splitting, input, bounded output reads and metadata reporting through a backend-neutral interface. Herdr SHALL be the first backend; its grammar, wire shapes and discovery SHALL remain adapter-owned. Unsupported operations SHALL refuse explicitly through declared capabilities.

#### Scenario: Herdr inventory parity
- **WHEN** direct and daemon adapters decode the same snapshot
- **THEN** their normalized fleet observations agree

#### Scenario: Another backend lacks reporting
- **WHEN** a fake backend without reporting receives a report operation
- **THEN** the daemon refuses unsupported and does not substitute another operation

### Requirement: Local transport is private and versioned
The daemon SHALL use bounded newline-delimited JSON requests/responses carrying version and ID over a Unix socket in a user-owned 0700 non-symlink directory. Path order SHALL be RADAR_CONTROL_SOCKET, XDG_RUNTIME_DIR/agent-radar/control.sock, then /tmp/agent-radar-<uid>/control.sock. Foreign versions and unknown methods SHALL refuse explicitly. Shutdown SHALL remove only the daemon's own socket.

#### Scenario: Untrusted path
- **WHEN** the socket directory is symlinked, foreign-owned or insufficiently private
- **THEN** binding refuses without changing that directory

#### Scenario: Invalid request
- **WHEN** a client sends a malformed or oversized line, unknown method or foreign version
- **THEN** no backend operation is dispatched and the error is bounded and explicit

#### Scenario: Already serving
- **WHEN** another daemon is listening on the configured socket
- **THEN** a second daemon refuses rather than replacing its endpoint

### Requirement: Reads do not create operation records
Ping, inventory, foreground evidence, output reads and record lookup SHALL be reads without durable operation records. Effectful operations SHALL persist private atomic request records before dispatch, including ID, operation, target, requested time and expiry. A client disconnect SHALL not cancel accepted work.

#### Scenario: Polling inventory
- **WHEN** a client repeatedly reads inventory
- **THEN** no lifecycle records are created

#### Scenario: Lost connection
- **WHEN** a client disconnects after an operation is accepted
- **THEN** the operation's outcome remains independently readable

### Requirement: Effectful request IDs execute at most once
A repeated request ID SHALL return its existing record without replay; the same ID with different contents SHALL refuse. Conflicting unresolved mutations SHALL serialize or refuse across operations and overlapping targets. Started records SHALL survive restart and SHALL never be retried automatically. Expired unstarted requests SHALL not execute.

#### Scenario: Duplicate create
- **WHEN** the same create request is submitted twice
- **THEN** the backend creates at most one location and both responses refer to the same record

#### Scenario: Conflicting methods
- **WHEN** close and input conflict on the same target while one is unresolved
- **THEN** they do not execute concurrently merely because the methods differ

#### Scenario: Restart after dispatch
- **WHEN** the daemon restarts with a started record but no completed outcome
- **THEN** the record remains unknown and is not replayed

### Requirement: Outcomes describe actual and uncertain effects
Operation records SHALL distinguish pending, completed, refused and unknown. Backend errors before dispatch may refuse; errors after possible dispatch SHALL remain unknown with known partial effects. A timeout SHALL not imply cancellation or non-execution. Neither socket success nor process disappearance SHALL fabricate effects the backend did not establish.

#### Scenario: Reply lost after close dispatch
- **WHEN** the backend may have accepted close but no valid answer is available
- **THEN** the outcome is unknown, not retried and not shown as definitely closed

#### Scenario: Preflight refused
- **WHEN** an operation fails its preflight before dispatch
- **THEN** the record names the refusal and no backend mutation occurs

### Requirement: Close preserves current identity and containment safeguards

Close SHALL require a frozen expected target identity and fresh observation.
Direct mux close SHALL retain the positive-unmanaged containment checks and
SHALL NOT close managed or uncertain panes. A distinct managed-child close MAY
close a daemon-spawned pane only when its durable edge names a bound child and
fresh evidence verifies that location, containment and occupant. It SHALL
report the pane outcome only. Owner-routed managed close/restart is unchanged,
with no fallback between the routes.

#### Scenario: Replacement occupant
- **WHEN** a different session or run occupies the requested pane at execution
- **THEN** close refuses and leaves that occupant untouched

#### Scenario: Mixed tab
- **WHEN** any tab member is managed or uncertain
- **THEN** the whole direct close refuses without partially closing members

#### Scenario: Unmanaged safeguards unchanged
- **WHEN** a direct close targets a managed, uncertain or mixed-container target
- **THEN** it refuses before dispatch

#### Scenario: Verified daemon-managed child
- **WHEN** managed close targets a bound child whose recorded pane and containment still match current observation
- **THEN** it may request pane closure and records that effect separately, without claiming the process exited

### Requirement: Reporting is a backend bridge
The control plane SHALL wrap source-provided state, session and display metadata without claiming a new registry or lifecycle authority. Caller source, sequence, TTL and reported values SHALL be preserved where the backend supports them; unsupported reporting SHALL refuse. No report SHALL imply recovered assignments, children or pending work.

#### Scenario: Extension reports metadata
- **WHEN** a client supplies display tokens and a source sequence
- **THEN** the Herdr adapter forwards the report without deriving additional agent facts

### Requirement: Radar chooses transport without effectful fallback retries
Radar SHALL support the daemon client and the existing direct adapter. Startup failure to handshake may select the direct adapter with a diagnostic. Once an operation may have been accepted by the daemon, Radar SHALL not retry it directly. Managed lifecycle eligibility and confirmation SHALL remain unchanged. Radar SHALL not auto-start the daemon.

#### Scenario: Daemon absent at startup
- **WHEN** a configured daemon cannot handshake
- **THEN** Radar reports its direct fallback choice and continues observation

#### Scenario: Daemon disconnects during close
- **WHEN** the close response is lost after submission
- **THEN** Radar does not call Herdr directly to repeat close

### Requirement: Wrapper scope does not claim session recovery
This change SHALL not derive resume commands, restart leads, register agent topology or claim recovery from a new PID or a shared session UUID. Creation and input SHALL be named mux primitives, not a promise to restore agent assignments or results. Later lifecycle policy SHALL be added independently of the mux backend.

#### Scenario: Lead restart requested
- **WHEN** a client asks this version for lead recovery or inferred resumption
- **THEN** the operation refuses unsupported rather than inventing a launch command

### Requirement: Launch runs a resolved command in a pane this daemon created

A backend MAY declare a launch capability, which SHALL execute the exact
command resolved for a spawn in a pane the daemon created. A backend without
it SHALL refuse a spawn before dispatch, and launch SHALL carry no recovery
claim. An adapter that reaches the pane by typing SHALL refuse, before any
pane input, a resolved argv holding a newline: it SHALL NOT split the command,
and it SHALL NOT write a launch script.

#### Scenario: A backend that declares launch
- **WHEN** the daemon launches a child in a pane it created
- **THEN** the declared capability covers the operation and the resolved command is what runs

#### Scenario: A backend that does not
- **WHEN** a spawn needs launch from a backend that does not declare it
- **THEN** the operation is refused before dispatch and no effect is recorded

#### Scenario: A command a typing adapter cannot carry
- **WHEN** the command resolved for a spawn holds a newline in its argv
- **THEN** the launch is refused before any pane input and no launch script is written

#### Scenario: Launch is not recovery
- **WHEN** a launch completes
- **THEN** nothing about a stopped, resumed or restarted process is claimed
