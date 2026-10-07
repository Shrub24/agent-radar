# process-metrics Specification

## Purpose
Enrich known foreground processes with birth-verified Linux resource samples and qualified descendant sums, keeping process evidence separate from task or agent ownership.

## Requirements

### Requirement: Current foreground PID is a process fact
Radar SHALL show a known current foreground PID for agents, subagents and ordinary command panes. An original-shell PID equal to the foreground group SHALL NOT suppress a named non-shell program that replaced the shell. Absent or ambiguous evidence SHALL remain unknown. Retained rows and stale source observations SHALL NOT present last-good PID or resource metrics as current.

#### Scenario: Pi replaces the shell in place
- **WHEN** the named foreground leader is Pi and its PID equals the pane's original shell PID
- **THEN** it is treated as a foreground command and its PID is shown

#### Scenario: Foreground really is the shell
- **WHEN** the named original-shell foreground leader is a recognized shell
- **THEN** it remains shell evidence rather than a running command

#### Scenario: Collection fails
- **WHEN** the last successful observation contained resource facts but collection now fails
- **THEN** stale source basis is visible and no last-good metrics are presented as live

### Requirement: Samples are bound to process birth identity
Radar SHALL bind Linux samples to boot identity, PID and kernel start ticks, validating identity before and after sampling. A replaced, unreadable or exited process SHALL yield unknown facts, never resources attributed to its predecessor. Background-task PIDs without publisher-captured birth identity SHALL NOT authorize resource sampling.

#### Scenario: PID is reused between samples
- **WHEN** the same PID has different start ticks or boot identity
- **THEN** earlier CPU history is discarded and no delta crosses those identities

#### Scenario: Task PID lacks birth evidence
- **WHEN** an attached background task only publishes a numeric PID
- **THEN** published task details are preserved but no sampled resources are attributed to that task

### Requirement: Root resources have explicit measurement semantics
Processes SHALL show available RSS, kernel process state and interval CPU for the known root. CPU SHALL be percentage of one CPU using two matching-identity samples and elapsed time; it can exceed 100 percent. First samples, invalid intervals or regressing counters SHALL show unavailable CPU, not zero. Unsupported platforms or inaccessible kernel facts SHALL not prevent fleet operation.

#### Scenario: First sample has no CPU baseline
- **WHEN** a process is observed for the first time
- **THEN** available RSS and state are shown and CPU is unavailable until a second valid sample

#### Scenario: Idle interval is measured
- **WHEN** two valid samples show no increase in process CPU time
- **THEN** CPU is reported as measured zero rather than unknown

### Requirement: Descendant summaries are distinct and qualified
Radar SHALL show observed descendant count and available descendant resource sums separately from root resources, without adding process rows to the fleet. Parentage SHALL be validated against observed birth identities. Totals SHALL be labelled as process-descendant sums, not assignment/workload totals; RSS SHALL warn about shared-page double counting. Incomplete enumeration or missing member samples SHALL be qualified as partial or unavailable, never complete zero.

#### Scenario: Root is quiet but a child is busy
- **WHEN** the root uses no interval CPU and a verified observed descendant uses CPU
- **THEN** root and descendant CPU are shown separately

#### Scenario: New child lacks a baseline
- **WHEN** a descendant has RSS but no matching previous CPU sample
- **THEN** descendant CPU aggregation is incomplete rather than treating that child's CPU as zero

#### Scenario: Scan reaches its budget
- **WHEN** process enumeration is cancelled, bounded or partially unreadable
- **THEN** the fleet remains responsive and affected descendant totals do not claim completeness

### Requirement: Sampling does not change authority or control
Resource collection SHALL be bounded and performed outside rendering at the runtime collection cadence. OS ancestry SHALL NOT establish agent ownership or background-task membership. Metrics SHALL NOT alter activity state, trigger lifecycle actions, consume task results or automatically classify work as stalled.

#### Scenario: A busy descendant is not a managed child
- **WHEN** a process is observed beneath a known root without published agent ownership
- **THEN** it contributes only to qualified process summaries and does not become an agent or task row

### Requirement: Verified descendants are individually inspectable
For a known current foreground root, Radar SHALL expose descendant birth identity, verified parent relationship, observed name, kernel state, available RSS and interval CPU. Rows SHALL use the same bounded, confirmed observation as descendant summaries. Missing CPU baselines SHALL remain unavailable, not zero. Names SHALL be sanitized, never expose arguments or claim original invocation. Ancestry SHALL NOT imply agent ownership or task membership.

#### Scenario: A busy child explains a quiet root
- **WHEN** a verified child consumes interval CPU beneath a quiet root
- **THEN** its own row exposes that CPU separately from the root's metrics and the descendant sum

#### Scenario: A child is newly observed
- **WHEN** a descendant has no matching earlier birth-identity sample
- **THEN** its available name, state and RSS are inspectable but its CPU is unavailable

#### Scenario: A descendant changes during collection
- **WHEN** a descendant disappears, changes birth identity or changes parent relationship before confirmation
- **THEN** it and any subtree whose ancestry depended on it are excluded from the verified rows
- **AND** the existing incomplete-observation qualification remains visible

#### Scenario: Process enumeration is incomplete
- **WHEN** cancellation, a scan budget or inaccessible processes prevent complete enumeration
- **THEN** any confirmed rows remain inspectable as an incomplete observation rather than a complete workload tree
- **AND** no missing member is represented as a complete zero reading

#### Scenario: Process evidence ceases to be current
- **WHEN** the source is stale, the fleet row is retained or the root's birth identity cannot be confirmed
- **THEN** previously observed child rows and metrics are not presented as live

#### Scenario: A task has only a PID
- **WHEN** a background task lacks publisher-captured birth identity
- **THEN** this capability does not attach a process tree or resource samples to it
