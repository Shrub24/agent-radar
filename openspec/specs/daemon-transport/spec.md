# daemon-transport Specification

## Purpose

The single transport under every daemon socket: one trusted bind and inherited-listener rule, bounded line framing, lane admission with capacity reserved for control requests, and synchronous handlers on a bounded blocking pool so a slow handler or a waiting read never starves the others while the wire contracts stay unchanged.

## Requirements

### Requirement: One transport serves every daemon socket
The daemon SHALL serve its control socket and any compatibility socket through one transport implementation that applies the same directory trust check, bounded line framing, timeouts, connection limits and shutdown to each. A socket directory that is not a real directory owned by the daemon's user with mode `0700` SHALL be refused with no looser fallback, and an inherited listener SHALL never be unlinked.

#### Scenario: Untrusted directory
- **WHEN** a socket directory is a symlink, is owned by another user, or is group- or world-accessible
- **THEN** the daemon does not bind or serve there and names the directory and the failed check

#### Scenario: Inherited listener
- **WHEN** the daemon serves a listener passed by its service manager
- **THEN** it serves it without binding and leaves the socket path in place at shutdown

#### Scenario: Same protections on both sockets
- **WHEN** a client of the compatibility socket sends an over-long line
- **THEN** the connection is closed by the same bound that applies to the control socket and other connections are unaffected

### Requirement: Lines are bounded and connections are timed
The transport SHALL read one line at most 1 MiB long without buffering beyond that bound, SHALL close a connection that exceeds it, and SHALL close a connection that is idle or whose reply cannot be written within its timeout. A closed connection SHALL NOT cancel a request the daemon already admitted.

#### Scenario: Unterminated line
- **WHEN** a client sends more than 1 MiB without a line terminator
- **THEN** the daemon closes that connection and holds no more than the bound for it

#### Scenario: Slow reader
- **WHEN** a client does not read its reply within the write timeout
- **THEN** the daemon closes the connection and keeps serving others

#### Scenario: Disconnect after admission
- **WHEN** a client disconnects after its operation was admitted
- **THEN** the operation completes and its durable record can be read back by its request identifier

### Requirement: Work is admitted by lane with reserved capacity
The transport SHALL admit control requests, publication ingest and waiting reads against separate limits, and SHALL keep capacity for control requests that ingest and waiting reads cannot use. A request over its lane's limit SHALL be refused with a typed busy answer and SHALL NOT delay another lane.

#### Scenario: Ingest saturated
- **WHEN** publication ingest has reached its limit
- **THEN** a lifecycle control request is still admitted

#### Scenario: Waiters saturated
- **WHEN** waiting reads have reached their limit
- **THEN** further waits are refused as busy and control requests are unaffected

### Requirement: Synchronous handlers never run on the transport's workers
Request handling SHALL run on a bounded pool separate from the transport's own workers, and the transport SHALL NOT hold daemon state locks while it awaits. Shutdown SHALL stop accepting, end connections, signal cancellation to running handlers and wait for them within a bound, as the daemon does today.

#### Scenario: Slow handler
- **WHEN** a handler takes seconds to return
- **THEN** other connections continue to be read and answered within their lane limits

#### Scenario: Shutdown with a running handler
- **WHEN** the daemon is stopped while a handler runs
- **THEN** the handler is signalled to cancel, shutdown waits for it within its bound, and no thread outlives the process

### Requirement: Waiting reads do not occupy handler capacity
A read that waits for new events SHALL wait in the transport on the journal's sequence and SHALL run its handler once, after the wake or the end of the wait. Results, bounds and the wait limit SHALL be those the existing event read defines.

#### Scenario: Event during a wait
- **WHEN** a fact commits while a reader waits
- **THEN** the reader receives that event before the wait ends and no handler capacity was held during the wait

#### Scenario: Wait ends empty
- **WHEN** the wait ends with no new event
- **THEN** the reader receives an empty list and the same cursor

### Requirement: The wire contracts are unchanged
Replacing the transport SHALL NOT change any request, response, error or capability the control protocol or the bus protocol defines.

#### Scenario: Existing consumers
- **WHEN** the protocol fixtures, the publisher fixture and the existing socket tests run against the new transport
- **THEN** they pass without edits
