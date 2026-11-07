# Spec Delta

## MODIFIED Requirements

### Requirement: Observation is not a durable event
Publishing a revision SHALL NOT append to the durable event log, and the daemon SHALL NOT expose a revision through `events.read`. Existing observation-read consumers SHALL retain the revision-wait API. TUI consumers SHALL receive the same daemon-owned facts and source freshness through coherent hash-addressed publications, without a second sampler or frontend collection.

#### Scenario: Frequent samples
- **WHEN** samples publish every second for an hour
- **THEN** the durable event log grows only by committed facts

#### Scenario: TUI publication subscriber
- **WHEN** an active TUI receives a newer observation with its head
- **THEN** that observation originates from the existing sampler and does not require a parallel observation long-read connection

### Requirement: Sampling is demand-driven
The daemon SHALL sample a data class only while a reader has recently asked for it or an active TUI attachment holds demand for it, SHALL sample nothing while no demand exists, and SHALL keep at most one sample in flight, coalescing requests that arrive during it. The fleet, registry and task classes SHALL be collected for any reader or active TUI demand; the process class SHALL be collected only while requested. An unknown class name SHALL be refused rather than ignored. TUI attachment loss SHALL release its demand. The contract SHALL NOT promise a sampling interval.

#### Scenario: Process table requested
- **WHEN** one reader or TUI attachment asks for the process table and another does not
- **THEN** the daemon samples processes and both see that revision, and the second reader's view is unchanged in its other sources

#### Scenario: No readers
- **WHEN** no reader has asked within the demand window and no TUI attachment holds demand
- **THEN** the daemon does not sample, and the first read afterwards reports revision zero rather than a sample

#### Scenario: A class this daemon does not collect
- **WHEN** a read or TUI demand update asks for a class name the daemon does not collect
- **THEN** it is refused `bad_params` naming the class, rather than left waiting for data that can never arrive

#### Scenario: Requests during a sample
- **WHEN** several wake requests arrive while a sample is running
- **THEN** one further sample follows and no two samples run at once

#### Scenario: TUI disconnect releases demand
- **WHEN** the last TUI attachment requesting processes disconnects
- **THEN** its demand ends without cancelling process demand still held by another reader
