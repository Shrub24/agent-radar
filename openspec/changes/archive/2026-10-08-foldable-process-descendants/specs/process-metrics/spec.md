## ADDED Requirements

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
