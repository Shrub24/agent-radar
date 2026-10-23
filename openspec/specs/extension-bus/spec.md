# extension-bus Specification

## Purpose

Accept metadata pushed by local extensions over a private socket, without giving an agent an endpoint to call and without any message that consumes or settles anything.

## Requirements

### Requirement: A private local listener

The daemon SHALL listen on a Unix socket in a directory owned by its user with mode `0700`, and SHALL accept connections only from extensions that dial out to it. The daemon SHALL NOT connect to any extension. The daemon SHALL NOT bind in a directory that fails that check, and SHALL NOT fall back to a looser one. A display such as the TUI SHALL NOT bind this socket.

#### Scenario: An extension connects
- **WHEN** an extension connects and sends a valid `hello`
- **THEN** the daemon accepts the connection and holds its messages for that session

#### Scenario: The directory is not ours
- **WHEN** the socket directory exists but is owned by another user, is group- or world-accessible, or is a symlink
- **THEN** the daemon serves without the bus and reports that in its status

#### Scenario: Another Radar owns the socket
- **WHEN** the socket file exists and a connection to it succeeds
- **THEN** the daemon does not bind a second listener, serves without the bus and reports that in its status

#### Scenario: A dead Radar left the socket behind
- **WHEN** the socket file exists and a connection to it is refused
- **THEN** the daemon removes it and binds a new one

#### Scenario: No display is running
- **WHEN** an extension connects while no TUI is running
- **THEN** its tasks are held by the daemon and readable by a display that starts later

### Requirement: A versioned line protocol

Radar SHALL read one JSON object per line. The first message SHALL be a `hello` carrying the protocol version and the exact Pi session UUID, and optionally the backend pane id. A `tasks` message SHALL replace that session's complete list of unresolved tasks; a task that has resolved SHALL be absent from it, not reported in a final state. Radar SHALL ignore unknown fields and unknown message types on a valid connection.

#### Scenario: The last task resolves
- **WHEN** a live connection sends an empty `tasks` list
- **THEN** Radar shows no tasks for that session and keeps the connection

#### Scenario: A session changes in the same pane
- **WHEN** a publisher closes its connection and a new connection sends a `hello` with a different session UUID
- **THEN** the old session's data is removed and the new session's data replaces it on the same row

#### Scenario: A newer publisher
- **WHEN** a message carries a field or a type this version does not know
- **THEN** Radar ignores it and keeps the connection

#### Scenario: A malformed connection
- **WHEN** a line is not valid JSON, names an unsupported version, exceeds the line cap, or arrives before `hello`
- **THEN** Radar closes that connection and keeps every other one

#### Scenario: Listener limits
- **WHEN** the number of open connections reaches the cap
- **THEN** Radar closes further connections until one ends

### Requirement: Connection data lives with the connection

The daemon SHALL hold a session's bus data in memory only while its connection is open, and SHALL remove it when the connection closes. It SHALL NOT write bus data to the durable event log or to disk, and SHALL NOT present absent bus data as evidence that the tasks have ended.

#### Scenario: An extension exits
- **WHEN** a connection closes
- **THEN** that session's bus data is removed and the agent row falls back to its registry facts

#### Scenario: Frequent snapshots
- **WHEN** a publisher replaces its task list many times a second
- **THEN** no event is appended to the durable log for those replacements

#### Scenario: Daemon restarts
- **WHEN** the daemon restarts
- **THEN** no bus data is restored until publishers reconnect and send their lists

### Requirement: Nothing in the bus consumes or controls

Radar SHALL NOT send any message that acknowledges, settles, stops or otherwise changes a task, and the protocol SHALL carry no receipt, acknowledgement or generation token. `hello` SHALL advertise the operations a client supports, and version 1 SHALL advertise none.

#### Scenario: A completion is pending
- **WHEN** a task has exited and the agent has not yet consumed its result
- **THEN** reading it through the bus leaves that result unconsumed

### Requirement: Task state is read from the daemon
The daemon SHALL serve the held task lists through `tasks.list.v1`, bounded and ordered with a continuation, carrying for each session its connection state and the time of its last accepted list. It SHALL advertise the contract only while the bus is bound. A reader SHALL NOT infer task completion from a missing or disconnected session.

#### Scenario: Reading while connected
- **WHEN** a session's connection is open and its last list is held
- **THEN** the read returns that list marked connected with its receipt time

#### Scenario: Bus not bound
- **WHEN** the bus could not be bound
- **THEN** the contract is not advertised and a display shows that task detail is unavailable rather than empty
