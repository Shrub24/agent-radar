## MODIFIED Requirements

### Requirement: Provenance distinguishes created containers from observed ones

Radar SHALL record a container as its own only when it created it. A container Radar only observed SHALL be recorded automatically with provenance `observed`, scoped to the one server the daemon serves and under a verified serving incarnation, SHALL keep the existing close safeguards, and SHALL NOT be recorded as owned or reloaded from an invented recipe. A place already recorded SHALL NOT be recorded again, so an unchanged server appends nothing.

#### Scenario: An externally created session appears

- **WHEN** a session exists in the server that Radar did not create
- **THEN** it is recorded as an observed workspace container with its own identity and its windows and panes beneath it, and closing it follows the existing confirmed safeguards

#### Scenario: Reload of an unmanaged container

- **WHEN** an operator asks to reload an observed container
- **THEN** the request refuses without a recipe

#### Scenario: Unverified serving incarnation

- **WHEN** the daemon cannot verify the process serving the server
- **THEN** no container is adopted until it can

#### Scenario: Unchanged server

- **WHEN** a pass sees the same places under the same incarnation
- **THEN** no container is recorded
