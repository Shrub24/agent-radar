# Spec Delta

## ADDED Requirements

### Requirement: Publication replies carry an attention hint
The daemon SHALL include an optional `attention` object in a successful publication reply for the writing registration, naming its pending handoff count and the current event journal position (epoch and sequence). The hint SHALL describe only the replying registration's subject, SHALL carry no content, and SHALL be additive: a client that ignores it SHALL behave as before.

#### Scenario: Pending handoff
- **WHEN** a registration has an undelivered handoff and publishes its next heartbeat
- **THEN** the reply names a non-zero handoff count and the journal position

#### Scenario: Other subjects
- **WHEN** handoffs are pending for a different subject
- **THEN** this registration's hint does not count them

#### Scenario: Older client
- **WHEN** a client that does not read the hint publishes
- **THEN** its publication behaviour is unchanged
