# Spec Delta

## ADDED Requirements

### Requirement: Idle agents learn of pending work through the heartbeat
An idle Pi process SHALL hold no daemon connection between requests and SHALL learn of pending work from the attention hint on its heartbeat reply. It SHALL deduplicate wakes by journal position and reconcile from current reads.

#### Scenario: Idle agent
- **WHEN** a process is idle and a handoff arrives for it
- **THEN** it holds no connection and learns of the handoff within one heartbeat

### Requirement: Active agents may hold a short-lived wake subscription
A process whose activity is working or blocked MAY hold a wake subscription, SHALL close it after a bounded idle hold, and SHALL reopen it when activity resumes. Correctness SHALL NOT depend on it: a missed or refused push SHALL be recovered by the next heartbeat hint. Only this connection SHALL use MessagePack, with integers beyond the safe range preserved exactly.

#### Scenario: Active lead
- **WHEN** a working lead's subscription receives a newer position
- **THEN** it reconciles from reads without waiting for the heartbeat

#### Scenario: Idle hold
- **WHEN** the process has been idle for the hold
- **THEN** it closes the subscription and reopens it from the stored position when activity resumes

#### Scenario: Refused or lost subscription
- **WHEN** the subscription is refused or drops
- **THEN** the process continues on heartbeat hints and retries after a backoff
