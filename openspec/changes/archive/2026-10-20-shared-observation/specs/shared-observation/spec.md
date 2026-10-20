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
A snapshot SHALL be identified by an epoch-qualified revision, SHALL carry a collection time, and SHALL report for each source (inventory, registry, processes, tasks) its freshness, its last successful time when it has one, and its failure when there is one. The revision SHALL be meaningful only within its epoch, because a restarted daemon counts from one again. A published snapshot SHALL NOT change; a newer sample SHALL publish a newer revision.

#### Scenario: One source fails
- **WHEN** the inventory source fails and the registry source succeeds
- **THEN** the new snapshot marks the inventory stale with its last-good observation and the failure, and the registry current

#### Scenario: No change
- **WHEN** a sample finds nothing changed
- **THEN** readers holding the current revision are told it is not modified

### Requirement: Reads are by revision and bounded
`observation.get.v1` SHALL return the current snapshot, or `not_modified` when the caller names the current revision, and MAY wait up to a bounded time for a newer revision. It SHALL be bounded in both wait and response size.

#### Scenario: Waiting for a change
- **WHEN** a reader waits and a newer revision publishes within the wait
- **THEN** the reader receives it before the wait ends

#### Scenario: A revision from another daemon process
- **WHEN** a caller names an epoch that is not the running daemon's
- **THEN** the read returns `snapshot_expired` and the caller restarts from the current revision

#### Scenario: A snapshot too large for one response
- **WHEN** the snapshot exceeds the response bound
- **THEN** the read returns `snapshot_too_large` and no truncated snapshot

### Requirement: A revision the daemon does not hold is expired, not current

A revision or epoch the daemon does not hold SHALL return a typed `snapshot_expired` naming the current epoch and revision. A snapshot too large for one response SHALL return a typed `snapshot_too_large` and SHALL NOT be written as a truncated line. The first read of a daemon that has never sampled SHALL answer `not_modified` at revision zero rather than a snapshot its own demand caused.

#### Scenario: The daemon restarts
- **WHEN** a caller names the revision it held from the previous daemon process
- **THEN** the read returns `snapshot_expired` and the caller restarts from the current revision

#### Scenario: Nothing has been sampled yet
- **WHEN** a caller reads before any demand has ever been recorded
- **THEN** the answer is `not_modified` at revision zero, not a snapshot the read itself caused

### Requirement: Sampling is demand-driven
The daemon SHALL sample a data class only while a reader has recently asked for it, SHALL sample nothing while no reader has, and SHALL keep at most one sample in flight, coalescing requests that arrive during it. The fleet, registry and task classes SHALL be collected for any read; the process class SHALL be collected only while a reader asks for it. An unknown class name SHALL be refused rather than ignored. The contract SHALL NOT promise a sampling interval.

#### Scenario: Process table requested
- **WHEN** one reader asks for the process table and another does not
- **THEN** the daemon samples processes and both readers see that revision, and the second reader's view is unchanged in its other sources

#### Scenario: No readers
- **WHEN** no reader has asked for a snapshot within the demand window
- **THEN** the daemon does not sample, and the first read afterwards reports revision zero rather than a sample

#### Scenario: A class this daemon does not collect
- **WHEN** a read asks for a class name the daemon does not collect
- **THEN** it is refused `bad_params` naming the class, rather than left waiting for data that can never arrive

#### Scenario: Requests during a sample
- **WHEN** several wake requests arrive while a sample is running
- **THEN** one further sample follows and no two samples run at once

### Requirement: Failed collection preserves the last-good snapshot
When a source fails the daemon SHALL keep that source's last-good observation, mark that source stale and report its failure, SHALL NOT treat failure as proof that panes or agents disappeared, and SHALL bound each sample with a deadline that cancels its runtime calls. A failed registry read SHALL be reported as the registry source and SHALL leave the fleet observation and the presented agents standing. A sample SHALL NOT delay lifecycle requests.

#### Scenario: Runtime unreachable
- **WHEN** the runtime does not answer within the deadline
- **THEN** the sample is cancelled, the last inventory remains stale in the next revision, and control requests are still answered

### Requirement: Snapshots carry nothing per client
A snapshot SHALL contain no selection, fold state, filter, focus target or window shown by any client, and SHALL be identical for every reader that asks for the same data classes. Its payload SHALL be the daemon's fleet projection with its sources, and SHALL contain no launch values, environment overlay, private recipe or request body.

#### Scenario: Two clients, one session
- **WHEN** two clients show different windows of the same session
- **THEN** both read the same snapshot and each applies its own view locally

#### Scenario: A launch with environment values
- **WHEN** a managed launch with an environment overlay is part of the daemon's state
- **THEN** the snapshot carries no launch value and no environment overlay

### Requirement: Observation is not a durable event
Publishing a revision SHALL NOT append to the durable event log, and the daemon SHALL NOT expose a revision through `events.read`. A reader that needs a change wake SHALL use the revision wait.

#### Scenario: Frequent samples
- **WHEN** samples publish every second for an hour
- **THEN** the durable event log grows only by committed facts

### Requirement: Diagnostics are structured and carry identifiers only
Daemon and display diagnostics SHALL be structured events with request, sampler-pass and operation identifiers, SHALL go to the daemon's stderr or the display's log file and never to the terminal the display draws on, and SHALL NOT contain launch values, tokens or request bodies. The observation read SHALL log identifiers only and no snapshot payload.

#### Scenario: Launch with environment values
- **WHEN** a launch with environment values is admitted and logged
- **THEN** the diagnostics name its identifiers and contain none of the values

#### Scenario: Display logging
- **WHEN** the TUI emits a diagnostic while drawing
- **THEN** it is written to its log file and the screen is unaffected
