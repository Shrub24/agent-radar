# extension-bus Specification

## Purpose

Accept metadata pushed by local extensions over a private socket, without giving an agent an endpoint to call and without any message that consumes or settles anything.

## Requirements

### Requirement: A private local listener

Radar SHALL listen on a Unix socket in a directory owned by its user with mode `0700`, and SHALL accept connections only from extensions that dial out to it. Radar SHALL NOT connect to any extension. Radar SHALL NOT bind in a directory that fails that check, and SHALL NOT fall back to a looser one.

#### Scenario: An extension connects
- **WHEN** an extension connects and sends a valid `hello`
- **THEN** Radar accepts the connection and holds its messages for that session

#### Scenario: The directory is not ours
- **WHEN** the socket directory exists but is owned by another user, is group- or world-accessible, or is a symlink
- **THEN** Radar runs without the bus and states that in the fleet diagnostic

#### Scenario: Another Radar owns the socket
- **WHEN** the socket file exists and a connection to it succeeds
- **THEN** Radar runs without the bus and states that in the fleet diagnostic instead of failing to start

#### Scenario: A dead Radar left the socket behind
- **WHEN** the socket file exists and a connection to it is refused
- **THEN** Radar removes it and binds a new one

### Requirement: A versioned line protocol

Radar SHALL read one JSON object per line. The first message SHALL be a `hello` carrying the protocol version and the exact Pi session UUID, and optionally the Herdr pane id. A `tasks` message SHALL replace that session's complete list of unresolved tasks; a task that has resolved SHALL be absent from it, not reported in a final state. Radar SHALL ignore unknown fields and unknown message types on a valid connection.

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

Radar SHALL hold a session's bus data only while its connection is open, and SHALL remove it when the connection closes. It SHALL NOT present absent bus data as evidence that the tasks have ended.

#### Scenario: An extension exits
- **WHEN** a connection closes
- **THEN** that session's bus data is removed and the agent row falls back to its token facts

### Requirement: Nothing in the bus consumes or controls

Radar SHALL NOT send any message that acknowledges, settles, stops or otherwise changes a task, and the protocol SHALL carry no receipt, acknowledgement or generation token. `hello` SHALL advertise the operations a client supports, and version 1 SHALL advertise none.

#### Scenario: A completion is pending
- **WHEN** a task has exited and the agent has not yet consumed its result
- **THEN** reading it through the bus leaves that result unconsumed
