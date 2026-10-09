# agent-registration Specification (delta)

## ADDED Requirements

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
