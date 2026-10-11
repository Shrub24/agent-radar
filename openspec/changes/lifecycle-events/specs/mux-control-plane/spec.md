## ADDED Requirements

### Requirement: Committed daemon facts append ordered events
The daemon SHALL append one event with a monotonic sequence in the same atomic commit as each registry, publication, spawn or close fact it records. Events SHALL carry kind, subject identifiers and time only, never environment values, tokens or request bodies.

#### Scenario: Publication committed
- **WHEN** an execution publication is committed
- **THEN** exactly one event with the next sequence names that agent

#### Scenario: Private launch data
- **WHEN** a spawn with environment values is recorded
- **THEN** its event contains no environment value or token

### Requirement: Events are read by resumable long poll
The daemon SHALL provide `events.read` taking an after-cursor, a limit and a bounded wait, returning ordered events after the cursor or an empty list when the wait ends. It SHALL keep one-request, one-response framing and the connection and worker bounds, and advertise `events_v1`.

#### Scenario: Waiting reader
- **WHEN** a reader waits and a fact commits within the wait
- **THEN** the reader receives that event before the wait ends

#### Scenario: Shutdown during wait
- **WHEN** the daemon stops while a reader waits
- **THEN** the wait ends and workers are joined

### Requirement: Lost history is explicit
When retention has dropped events after the caller's cursor, or the journal epoch differs, the daemon SHALL report `cursor_expired` with the current cursor and SHALL NOT return a partial sequence as complete.

#### Scenario: Expired cursor
- **WHEN** a consumer resumes from a dropped sequence
- **THEN** it receives cursor_expired and resynchronises from full reads

### Requirement: Events report facts, not inferences
Events SHALL NOT report process exit, readiness or assignment outcome that the daemon has not recorded, and absence of an event SHALL NOT imply freshness or liveness.

#### Scenario: Pane vanished
- **WHEN** a launched pane disappears
- **THEN** no exit event is appended
