## ADDED Requirements

### Requirement: The daemon samples once for every display
The daemon SHALL run one sampler whose output every display reads, and SHALL NOT run a separate collection per display. A display SHALL render a snapshot and SHALL NOT sample the runtime, read process tables or reconcile observations itself.

#### Scenario: Two displays
- **WHEN** a full display and a sidebar read at the same time
- **THEN** both receive the same revision of the same snapshot from one sample

#### Scenario: Display restarts
- **WHEN** a display exits and starts again while the daemon runs
- **THEN** it receives retained associations and process history the daemon kept, with no re-learning

### Requirement: Snapshots are immutable and carry freshness
A snapshot SHALL have a monotonic revision, a collection time and, for each source (inventory, registry, processes, tasks), its last successful time, its freshness and its failure when there is one. A published snapshot SHALL NOT change; a newer sample SHALL publish a newer revision.

#### Scenario: One source fails
- **WHEN** the inventory source fails and the registry source succeeds
- **THEN** the new snapshot marks the inventory stale with its last-good observation and the failure, and the registry current

#### Scenario: No change
- **WHEN** a sample finds nothing changed
- **THEN** readers holding the current revision are told it is not modified

### Requirement: Reads are by revision and bounded
`observation.get.v1` SHALL return the current snapshot, or `not_modified` when the caller names the current revision, and MAY wait up to a bounded time for a newer revision. A snapshot larger than one response SHALL be paged, each page naming its revision, and a page request for a revision the daemon no longer holds SHALL return a typed `snapshot_expired` and SHALL NOT combine pages from different revisions.

#### Scenario: Waiting for a change
- **WHEN** a reader waits and a newer revision publishes within the wait
- **THEN** the reader receives it before the wait ends

#### Scenario: Revision replaced during paging
- **WHEN** a newer revision is published and the older one is released between two page requests
- **THEN** the second request returns `snapshot_expired` and the reader restarts from the first page

### Requirement: Sampling is demand-driven
The daemon SHALL sample a data class only while a reader has recently asked for it, SHALL sample nothing while no reader has, and SHALL keep at most one sample in flight, coalescing requests that arrive during it. The contract SHALL NOT promise a sampling interval.

#### Scenario: Process table requested
- **WHEN** one reader asks for the process table and another does not
- **THEN** the daemon samples processes and both readers see that revision, and the second reader's view is unchanged in its other sources

#### Scenario: No readers
- **WHEN** no reader has asked for a snapshot within the demand window
- **THEN** the daemon does not sample

#### Scenario: Requests during a sample
- **WHEN** several wake requests arrive while a sample is running
- **THEN** one further sample follows and no two samples run at once

### Requirement: Failed collection preserves the last-good snapshot
When a source fails the daemon SHALL keep that source's last-good observation, mark it stale and report the failure, SHALL NOT treat failure as proof that panes or agents disappeared, and SHALL bound each sample with a deadline that cancels its runtime calls. A sample SHALL NOT delay lifecycle requests.

#### Scenario: Runtime unreachable
- **WHEN** the runtime does not answer within the deadline
- **THEN** the sample is cancelled, the last inventory remains stale in the next revision, and control requests are still answered

### Requirement: Snapshots carry nothing per client
A snapshot SHALL contain no selection, fold state, filter, focus target or window shown by any client, and SHALL be identical for every reader that asks for the same data classes.

#### Scenario: Two clients, one session
- **WHEN** two clients show different windows of the same session
- **THEN** both read the same snapshot and each applies its own view locally

### Requirement: Observation is not a durable event
Publishing a revision SHALL NOT append to the durable event log, and the daemon SHALL NOT expose a revision through `events.read`. A reader that needs a change wake SHALL use the revision wait.

#### Scenario: Frequent samples
- **WHEN** samples publish every second for an hour
- **THEN** the durable event log grows only by committed facts

### Requirement: Diagnostics are structured and carry identifiers only
Daemon and display diagnostics SHALL be structured events with request, sampler-pass and operation identifiers, SHALL go to the daemon's stderr or the display's log file and never to the terminal the display draws on, and SHALL NOT contain launch values, tokens or request bodies.

#### Scenario: Launch with environment values
- **WHEN** a launch with environment values is admitted and logged
- **THEN** the diagnostics name its identifiers and contain none of the values

#### Scenario: Display logging
- **WHEN** the TUI emits a diagnostic while drawing
- **THEN** it is written to its log file and the screen is unaffected
