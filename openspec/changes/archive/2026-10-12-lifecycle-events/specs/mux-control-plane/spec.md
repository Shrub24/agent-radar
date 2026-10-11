## ADDED Requirements

### Requirement: Committed daemon facts append ordered events
The daemon SHALL append one event with a monotonic sequence immediately after each registry, publication, spawn or close fact it records is durable, and SHALL start a new journal epoch after an unclean stop so a possibly missing event is never silent. Events SHALL carry kind, subject identifiers and time only, never environment values, tokens or request bodies.

#### Scenario: Publication committed
- **WHEN** an execution publication is committed
- **THEN** exactly one event with the next sequence names that agent

#### Scenario: Unclean stop
- **WHEN** the daemon restarts after a stop that did not complete cleanly
- **THEN** the journal epoch changes and consumers resynchronise

#### Scenario: Private launch data
- **WHEN** a spawn with environment values is recorded
- **THEN** its event contains no environment value or token

### Requirement: Events are read by resumable long poll
The daemon SHALL provide `events.read` taking a sequence `after`, optional last-seen `epoch`, bounded `limit` (1..=500, default 100) and bounded `wait_ms` (0..=1000, default 0). It returns ordered events after the cursor, or an empty list when the wait ends. The result carries current epoch, events, object cursor `{epoch, sequence}` and earliest retained sequence. It SHALL preserve JSON-lines framing and worker bounds, and advertise `events_v1`.

#### Scenario: Waiting reader
- **WHEN** a reader waits and a fact commits within the wait
- **THEN** the reader receives that event before the wait ends

#### Scenario: Shutdown during wait
- **WHEN** the daemon stops while a reader waits
- **THEN** the wait ends and workers are joined

### Requirement: Lost history is explicit
When retention has dropped events after the caller's sequence, or the supplied epoch differs from the journal epoch, the daemon SHALL return successful typed `cursor_expired` carrying the current epoch, cursor and earliest retained sequence, and SHALL NOT return a partial sequence as complete.

#### Scenario: Expired cursor
- **WHEN** a consumer resumes from a dropped sequence
- **THEN** it receives `cursor_expired` and resynchronises from full reads (`agent.list`, `spawn.list` and request listings), then resumes from the returned cursor as `after` and `epoch`

#### Scenario: First read after retention dropped history
- **WHEN** a consumer reads with `after` zero after older events were dropped
- **THEN** it may receive `cursor_expired` rather than a partial sequence

#### Scenario: Stale epoch after an unclean restart
- **WHEN** a consumer resumes with an epoch the journal no longer has
- **THEN** it receives `cursor_expired` with the current epoch and cursor

### Requirement: Events report facts, not inferences
Events SHALL NOT report process exit, readiness or assignment outcome that the daemon has not recorded, and absence of an event SHALL NOT imply freshness or liveness.

#### Scenario: Pane vanished
- **WHEN** a launched pane disappears
- **THEN** no exit event is appended
