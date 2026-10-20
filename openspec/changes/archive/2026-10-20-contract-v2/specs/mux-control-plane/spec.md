## ADDED Requirements

### Requirement: The wire contract is one namespaced version-2 scheme
The daemon SHALL serve protocol version 2 with namespaced methods: `daemon.ping`; `mux.observe`, `mux.process_info`, `mux.output`, `mux.focus`, `mux.close`, `mux.create`, `mux.input`, `mux.report`; `mux.request.get` and `mux.request.list`; the `agent.*` methods; `spawn.create`, `spawn.get`, `spawn.list` and `spawn.close`; and `events.read`. A request naming protocol version 1 or a removed method name SHALL be refused with a typed error naming the supported version, and no alias SHALL be served.

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

## MODIFIED Requirements

### Requirement: Events are read by resumable long poll
The daemon SHALL provide `events.read` taking a sequence `after`, optional last-seen `epoch`, bounded `limit` (1..=500, default 100) and bounded `wait_ms` (0..=1000, default 0). It returns ordered events after `after`, or an empty list when the wait ends. The result carries the current epoch, the events, an object `cursor` `{epoch, sequence}` that is the position after the last delivered event (the request's `after` when none were delivered), the journal `tail` sequence, `more` stating whether events exist beyond `cursor`, and the earliest retained sequence. Following `cursor` SHALL never skip an undelivered event. It SHALL preserve JSON-lines framing and worker bounds and advertise `events.v1`.

#### Scenario: Waiting reader
- **WHEN** a reader waits and a fact commits within the wait
- **THEN** the reader receives that event before the wait ends

#### Scenario: Bounded prefix
- **WHEN** more events exist than `limit` or the response byte bound allows
- **THEN** the result delivers a prefix, `cursor` names the last delivered event, `tail` names the journal head and `more` is true

#### Scenario: Shutdown during wait
- **WHEN** the daemon stops while a reader waits
- **THEN** the wait ends and workers are joined
