## ADDED Requirements

### Requirement: Agent state comes from the registry
When observing through the daemon, Radar SHALL source every displayed agent fact from the registry. Herdr agent records, display metadata and native agent status SHALL NOT supply display state or fill missing facts. Backend metadata MAY be retained solely for destructive-control protection, outside display continuity. Missing registry facts SHALL remain unavailable without source fallback.

#### Scenario: Registered agent, no execution report
- **WHEN** a registered agent has published no fresh execution snapshot
- **THEN** its state is unknown, not idle

#### Scenario: Unregistered pane
- **WHEN** a pane has no registered agent
- **THEN** it is an ordinary pane with unavailable agent state

### Requirement: Removing metadata preserves destructive-control protection
A registry candidate at a pane SHALL protect that pane and its containing tab from generic unmanaged close, including ambiguous or unresolved joins. A bound spawn edge SHALL establish managed-close eligibility separately. Unsupported registered targets SHALL refuse rather than fall through to unmanaged close.

#### Scenario: Unresolved registered occupant
- **WHEN** Herdr metadata is absent and a registration at the target pane cannot be joined exactly
- **THEN** a generic unmanaged close is refused and no close is dispatched

### Requirement: Registry agents join locations by exact evidence
Radar SHALL join a registry agent to a backend pane only by the agent's backend-qualified location and exact process identity. A pane title, label or session UUID alone SHALL NOT join them, and an unjoinable agent SHALL be reported with its last location as stale or unresolved.

#### Scenario: Sample birth identity
- **WHEN** a registration has a process claim and its pane is a join candidate
- **THEN** the registry read exposes the claimed birth tuple and the collector samples the foreground process independently of legacy agent detection
- **AND** a missing or mismatched tuple leaves the registration unresolved

#### Scenario: Duplicate session
- **WHEN** two registered processes publish the same session UUID on different panes
- **THEN** each joins its own pane and neither is merged with the other

#### Scenario: Pane gone
- **WHEN** a registered agent's recorded pane is absent from a successful inventory
- **THEN** the agent is shown unresolved and no process exit is claimed
