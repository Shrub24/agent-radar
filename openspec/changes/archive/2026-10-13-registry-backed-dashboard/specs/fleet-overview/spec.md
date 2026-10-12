## ADDED Requirements

### Requirement: Rows and details keep three authorities separate
The dashboard SHALL present physical lifecycle, execution status and assignment facts as separate, labelled values and SHALL NOT collapse them into one status. Each SHALL carry its own freshness, and a stale value SHALL be marked stale.

#### Scenario: Pane present, execution stale
- **WHEN** a pane is present but its agent's execution report has expired
- **THEN** the row shows the pane present and the execution state stale

### Requirement: Waiting and lost are derived, not published
The dashboard SHALL derive waiting from awaited items and unresolved background tasks, and lost or settling from assignment facts, as a projection that never rewrites a published value.

#### Scenario: Awaiting a child
- **WHEN** an agent is idle with an awaited child
- **THEN** the row is waiting and execution still reads idle in the details
