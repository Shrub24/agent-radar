## MODIFIED Requirements

### Requirement: Launch context support is negotiated
The daemon SHALL advertise the `spawn.launch_context.v1` contract only when its spawn handler and selected backend support the cwd/environment contract. Consumers SHALL require a compatible protocol and that contract before using extended spawn fields. A backend without support SHALL refuse supplied launch context before effects; no fallback is allowed after uncertain dispatch.

#### Scenario: Older daemon
- **WHEN** a consumer observes a compatible daemon without `spawn.launch_context.v1`
- **THEN** it does not attempt the extended launch request and treats launch migration as unsupported

#### Scenario: Backend lacks launch context
- **WHEN** a backend cannot transport cwd or environment supplied by a spawn
- **THEN** the daemon refuses before accepting or dispatching that spawn

## ADDED Requirements

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
