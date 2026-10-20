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

### Requirement: Committed daemon facts append ordered events
The daemon SHALL append one event with a monotonic sequence immediately after each registry, publication, spawn or close fact it records is durable, and SHALL start a new journal epoch after an unclean stop so a possibly missing event is never silent. Events SHALL carry kind, subject identifiers and time only, never environment values, tokens or request bodies.

#### Scenario: Publication committed
- **WHEN** an execution publication is committed
- **THEN** exactly one event with the next sequence names that agent

#### Scenario: Unclean stop
- **WHEN** the daemon restarts after a stop that did not complete cleanly
- **THEN** the journal epoch changes and consumers resynchronise

#### Scenario: Private launch data
- **WHEN** a spawn with environment values is recorded
- **THEN** its event contains no environment value or token

### Requirement: Events are read by resumable long poll
The daemon SHALL provide `events.read` returning ordered events after `after`, or an empty list when a bounded wait ends. Following the returned `cursor` SHALL never skip an undelivered event. It SHALL keep JSON-lines framing and worker bounds and advertise `events.v1`.

#### Scenario: Read result shape
- **WHEN** a read answers
- **THEN** the result carries the epoch, the events, a `cursor` `{epoch, sequence}` for the position after the last delivered event (the request's `after` when none were delivered), the journal `tail`, `more` for whether events exist beyond `cursor`, and the earliest retained sequence

#### Scenario: Bounded read parameters
- **WHEN** a client reads events
- **THEN** it supplies `after`, an optional last-seen `epoch`, a bounded `limit` (1..=500, default 100) and a bounded `wait_ms` (0..=1000, default 0)

#### Scenario: Waiting reader
- **WHEN** a reader waits and a fact commits within the wait
- **THEN** the reader receives that event before the wait ends

#### Scenario: Bounded prefix
- **WHEN** more events exist than `limit` or the response byte bound allows
- **THEN** the result delivers a prefix, `cursor` names the last delivered event, `tail` names the journal head and `more` is true

#### Scenario: Shutdown during wait
- **WHEN** the daemon stops while a reader waits
- **THEN** the wait ends and workers are joined

### Requirement: Lost history is explicit
When retention has dropped events after the caller's sequence, or the supplied epoch differs from the journal epoch, the daemon SHALL return successful typed `cursor_expired` carrying the current epoch, cursor and earliest retained sequence, and SHALL NOT return a partial sequence as complete.

#### Scenario: Expired cursor
- **WHEN** a consumer resumes from a dropped sequence
- **THEN** it receives `cursor_expired` and resynchronises from full reads (`agent.list`, `spawn.list` and request listings), then resumes from the returned cursor as `after` and `epoch`

#### Scenario: First read after retention dropped history
- **WHEN** a consumer reads with `after` zero after older events were dropped
- **THEN** it may receive `cursor_expired` rather than a partial sequence

#### Scenario: Stale epoch after an unclean restart
- **WHEN** a consumer resumes with an epoch the journal no longer has
- **THEN** it receives `cursor_expired` with the current epoch and cursor

### Requirement: Events report facts, not inferences
Events SHALL NOT report process exit, readiness or assignment outcome that the daemon has not recorded, and absence of an event SHALL NOT imply freshness or liveness.

#### Scenario: Pane vanished
- **WHEN** a launched pane disappears
- **THEN** no exit event is appended

### Requirement: Launch context support is negotiated
The daemon SHALL advertise the `spawn.launch_context.v1` contract only when its spawn handler and selected backend support the cwd/environment contract. Consumers SHALL require a compatible protocol and that contract before using extended spawn fields. A backend without support SHALL refuse supplied launch context before effects; no fallback is allowed after uncertain dispatch.

#### Scenario: Older daemon
- **WHEN** a consumer observes a compatible daemon without `spawn.launch_context.v1`
- **THEN** it does not attempt the extended launch request and treats launch migration as unsupported

#### Scenario: Backend lacks launch context
- **WHEN** a backend cannot transport cwd or environment supplied by a spawn
- **THEN** the daemon refuses before accepting or dispatching that spawn

### Requirement: Launch values stay private across operation readback
Environment values and spawn tokens SHALL be kept only in bounded private launch/request state. Public operation responses, request listings, topology, diagnostics and logs SHALL NOT expose them. Private exact request content SHALL still distinguish identical same-ID replay from changed cwd or environment; redacted public projections SHALL NOT be used to compare requests.

#### Scenario: Secret launch value
- **WHEN** a spawn supplies a secret environment value and its operation or topology is read
- **THEN** no public response or diagnostic contains that value or the daemon token

#### Scenario: Same ID changed environment
- **WHEN** a request ID is repeated with a different environment value
- **THEN** the conflict refuses without redispatch even though both public projections omit that value

#### Scenario: Backend failure includes command data
- **WHEN** a backend failure echoes launch input
- **THEN** the daemon reports a bounded diagnostic without launch environment values or the token

### Requirement: Daemon physical backend is explicitly selected
The daemon SHALL use exactly one physical backend per process, chosen by explicit configuration, with `herdr` as the default. A tmux backend SHALL target a named tmux server and never the default one. An unknown backend or unreachable server SHALL fail startup or refuse operations, with no fallback to another backend.

#### Scenario: Unknown backend
- **WHEN** the daemon is configured with an unknown backend name
- **THEN** it refuses to start and names the supported backends

#### Scenario: Named tmux server
- **WHEN** the tmux backend is selected with a server name
- **THEN** every effect targets that server and none targets the default tmux server

### Requirement: tmux adapter declares only implemented capabilities
The tmux adapter SHALL advertise observation, focus, create, input, output, close and launch only as implemented, using stable tmux ids qualified by backend and server as location handles. Operations it cannot express SHALL be refused by absent capability before dispatch.

#### Scenario: Stable handles
- **WHEN** a pane is observed, then windows are reordered
- **THEN** the pane keeps its handle

### Requirement: tmux launch uses direct argv
The tmux adapter SHALL launch children from resolved executable and argv without typing into a shell, carrying cwd and the environment overlay exactly and keeping environment values and the token out of diagnostics. It SHALL NOT infer readiness from timing or output.

#### Scenario: Shell-sensitive argv
- **WHEN** argv contains spaces, quotes and newlines
- **THEN** the child receives them unchanged

### Requirement: Physical exit is a typed observation
For panes it launched with a supporting backend, the daemon SHALL record at most one exit observation of `exited` with a code or a signal and an observed time. A missing pane, missing report or unreachable server SHALL remain unknown or unresolved and SHALL NOT be reported as an exit.

#### Scenario: Nonzero exit
- **WHEN** a launched child exits with code 3 and its pane remains
- **THEN** the exit observation reports code 3

#### Scenario: Pane vanished
- **WHEN** a launched pane disappears without a recorded exit
- **THEN** the location is unresolved and no exit is reported

#### Scenario: Out-of-band pane respawn
- **WHEN** a same-UID client respawns the managed pane before Radar reads its retained exit status
- **THEN** the tmux status may be replaced, and a later observation is not proof that the originally bound agent process exited

### Requirement: Exit does not imply assignment outcome
An exit observation SHALL NOT change execution or assignment publications and SHALL NOT trigger close, resume or restart. Closing a dead pane SHALL remain the separate guarded `child.close` operation.

#### Scenario: Exited child pane
- **WHEN** an exit is observed
- **THEN** the pane stays until `child.close` is requested and publications are unchanged

#### Scenario: Dead pane does not bypass close verification
- **WHEN** `child.close` is requested for a dead pane with an exit observation but no current foreground-process identity
- **THEN** the daemon refuses before durable close admission or dispatch, and leaves execution and assignment publications unchanged

### Requirement: The wire contract is one namespaced version-2 scheme
The daemon SHALL serve protocol version 2 through one namespaced method scheme. A request naming protocol version 1 or a removed method name SHALL be refused with a typed error naming the supported version, and no alias SHALL be served.

#### Scenario: Version 2 method names
- **WHEN** a client addresses protocol version 2
- **THEN** it uses `daemon.ping`; `mux.observe`, `mux.process_info`, `mux.output`, `mux.focus`, `mux.close`, `mux.create`, `mux.input` and `mux.report`; `mux.request.get` and `mux.request.list`; the `agent.*` methods; `spawn.create`, `spawn.get`, `spawn.list` and `spawn.close`; and `events.read`

#### Scenario: Old method name
- **WHEN** a client calls `child.close` or the flat `observe`
- **THEN** the daemon answers `unknown_method` and performs no effect

#### Scenario: Old protocol version
- **WHEN** a client sends version 1
- **THEN** the daemon refuses with the version it serves

### Requirement: Capabilities share one naming scheme
The handshake SHALL list capabilities as `<namespace>.<name>`, with a `.v<N>` suffix on every versioned contract. Backend capabilities SHALL be `mux.*` and SHALL be the backend's own declaration. Daemon contracts SHALL be `registry.v1`, `events.v1`, `spawn.v1`, `spawn.close.v1`, `spawn.launch_context.v1` and `spawn.exit_observation.v1`, each advertised only when every capability it needs is present. A capability SHALL NOT be advertised that the daemon would refuse to serve.

#### Scenario: Close needs the backend's close
- **WHEN** the wired backend does not declare `mux.close`
- **THEN** `spawn.close.v1` is not advertised and `spawn.close` is refused with no record

### Requirement: The handshake names the real backend
`daemon.ping` SHALL report the protocol version, the wired backend identity (`herdr`, `tmux` or `none`) and, for tmux, the server instance. It SHALL NOT report a generic placeholder.

#### Scenario: tmux daemon
- **WHEN** the daemon runs against tmux server `radar`
- **THEN** the handshake reports backend `tmux` and instance `radar`

### Requirement: The handshake advertises a revision and contracts
The daemon's handshake SHALL report a protocol revision, an integer increased whenever a method or field is added, and the daemon contracts it serves, each only when every backend need of its methods is met. It SHALL NOT advertise backend primitives. Clients bundled with the daemon SHALL require a minimum revision and SHALL NOT probe individual contracts. External consumers SHALL check contract names, and names a client does not know SHALL be ignored.

#### Scenario: Newer client, older daemon
- **WHEN** a bundled client requires a revision above the running daemon's
- **THEN** it shows a blocking diagnostic naming the revision gap and the daemon restart command, and sends no request that the daemon may mishandle

#### Scenario: Contract not served
- **WHEN** the backend lacks a need of a contract's methods
- **THEN** the handshake omits that contract and a request for its methods refuses before any effect

#### Scenario: Unknown contract name
- **WHEN** a daemon advertises a contract a client was not built with
- **THEN** the client ignores it and its own contract checks are unaffected
