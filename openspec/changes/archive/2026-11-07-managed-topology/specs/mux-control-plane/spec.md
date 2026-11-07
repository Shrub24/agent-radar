## MODIFIED Requirements

### Requirement: Close preserves current identity and containment safeguards

Close SHALL require a frozen expected target identity and fresh observation.
Direct mux close SHALL retain the positive-unmanaged containment checks and
SHALL NOT close managed or uncertain panes. A close target SHALL name a pane, a
tab or a workspace; a workspace target covers the tabs and panes inside it, and
its frozen identity SHALL carry the same membership, so a workspace whose
members changed between confirmation and dispatch is not closed. A distinct
managed-child close MAY close a daemon-spawned pane only when its durable edge
names a bound child and fresh evidence verifies that location, containment and
occupant. It SHALL report the pane outcome only. Owner-routed managed
close/restart is unchanged, with no fallback between the routes.

#### Scenario: Replacement occupant
- **WHEN** a different session or run occupies the requested pane at execution
- **THEN** close refuses and leaves that occupant untouched

#### Scenario: Mixed tab
- **WHEN** any tab member is managed or uncertain
- **THEN** the whole direct close refuses without partially closing members

#### Scenario: Mixed workspace
- **WHEN** any pane in the workspace is managed or uncertain
- **THEN** the whole direct close refuses without partially closing it

#### Scenario: Workspace membership changed
- **WHEN** a workspace's membership differs from the frozen identity at execution
- **THEN** close refuses and closes nothing in it

#### Scenario: Unmanaged safeguards unchanged
- **WHEN** a direct close targets a managed, uncertain or mixed-container target
- **THEN** it refuses before dispatch

#### Scenario: Verified daemon-managed child
- **WHEN** managed close targets a bound child whose recorded pane and containment still match current observation
- **THEN** it may request pane closure and records that effect separately, without claiming the process exited
