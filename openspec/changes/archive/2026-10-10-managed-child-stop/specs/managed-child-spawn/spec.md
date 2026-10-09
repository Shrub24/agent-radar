## MODIFIED Requirements

### Requirement: Spawn authority is bounded

The daemon SHALL record edges only for children it spawned. It SHALL NOT adopt a
pane or agent it did not create, SHALL NOT write assignment facts, and SHALL
NOT resume or restart anything. A child-close operation MAY close the recorded
pane of a bound child after positive foreground birth-identity verification.
Close records SHALL remain separate from the immutable spawn edge and SHALL NOT
change assignment or execution facts.

#### Scenario: A child spawned outside the daemon
- **WHEN** an agent is launched by Herdsman through the backend directly
- **THEN** it has no edge in this topology and its absence is not an error

#### Scenario: It does not touch anything else
- **WHEN** a spawn completes
- **THEN** no assignment fact changed and no process was stopped, resumed or restarted

#### Scenario: A bound child pane is closed later
- **WHEN** a caller uses `child.close` for a daemon-created edge
- **THEN** the same edge and child binding remain unchanged, only the pane-close result is recorded separately, and no assignment or execution fact changes
