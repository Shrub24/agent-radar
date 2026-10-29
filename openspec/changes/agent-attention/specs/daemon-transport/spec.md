# Spec Delta

## ADDED Requirements

### Requirement: Wake subscriptions push the latest journal position
The trusted socket SHALL accept a bounded JSON wake-subscription opener that upgrades the connection to length-prefixed MessagePack and pushes the current event epoch and sequence whenever either moves past the last position written to that connection. Delivery SHALL be latest-wins and carry no event content.

#### Scenario: Position advances
- **WHEN** the journal sequence advances while a subscription is open
- **THEN** the subscriber receives the newest epoch and sequence

#### Scenario: Slow reader
- **WHEN** several advances occur while the subscriber is not reading
- **THEN** it receives only the newest position when it resumes, and other subscribers are unaffected

### Requirement: Wake subscriptions are separately bounded
Wake subscriptions SHALL have their own small connection budget, refuse beyond it with a typed `busy`, and SHALL NOT consume ordinary or UI capacity. A stalled write SHALL end that connection only, and pushed frames SHALL have a hard size bound.

#### Scenario: Budget exhausted
- **WHEN** the wake-subscription budget is full
- **THEN** a new opener is refused with `busy` and ordinary and UI connections remain admissible

#### Scenario: Stalled write
- **WHEN** one subscriber stops reading past the write deadline
- **THEN** only its connection closes
