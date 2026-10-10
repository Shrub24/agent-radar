## ADDED Requirements

### Requirement: Spawn bootstrap reuses exact registration
A spawned publisher SHALL present the delivered token privately in agent.register with its own source, incarnation and process claim. Successful registration SHALL consume the token for that exact registration, with identical retry remaining idempotent. The publisher SHALL remove the token from its launch environment after capture and SHALL NOT persist it in owner sidecars, facts, labels or logs.

#### Scenario: Child bootstrap
- **WHEN** a spawned child registers with its own incarnation and the delivered token
- **THEN** the returned agent_id identifies that registration and the spawn edge binds to it

#### Scenario: Registration answer is lost
- **WHEN** the child repeats the identical registration after an uncertain acknowledgement
- **THEN** it receives the existing registration and no second binding or registration is created

#### Scenario: Token reused by another incarnation
- **WHEN** another incarnation attempts to consume an already-bound spawn token
- **THEN** registration refuses and the original binding remains unchanged

### Requirement: Binding does not imply application readiness
Token binding SHALL establish only the exact registration associated with a daemon spawn. It SHALL NOT certify assignment readiness, process exit or assignment completion. Consumers SHALL read execution and assignment reports separately with their own freshness and provenance; never treat launch dispatch, pane presence or a Herdr alias as application readiness.

#### Scenario: Bound but no execution publication
- **WHEN** a child is bound but has not published fresh execution facts
- **THEN** topology reports the binding while execution readiness remains unknown

#### Scenario: Bound with fresh execution publication
- **WHEN** a bound child publishes execution facts
- **THEN** a consumer can read those facts using its exact agent_id without looking up a Herdr agent alias
