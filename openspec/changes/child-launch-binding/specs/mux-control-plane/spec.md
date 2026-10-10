## ADDED Requirements

### Requirement: Launch context support is negotiated
The daemon SHALL advertise child_launch_context_v1 only when its spawn handler and selected backend support the cwd/environment contract. Consumers SHALL require compatible protocol, creation, launch and this feature before using extended spawn fields. A backend without support SHALL refuse supplied launch context before effects; no fallback is allowed after uncertain dispatch.

#### Scenario: Older daemon
- **WHEN** a consumer observes a compatible daemon without child_launch_context_v1
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
