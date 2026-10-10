# Spec Delta

## MODIFIED Requirements

### Requirement: A spawn names its parent and is recorded before any effect
A caller SHALL name both the exact parent process registration and its Pi session UUID. Before effects, the daemon SHALL verify that registration's fresh session context matches the supplied UUID, record both identities, and create under that exact process's current pane; it SHALL NOT select a process by searching for the session UUID or create wherever the backend happens to be focused.

#### Scenario: Exact process and session match
- **WHEN** the caller names a registered parent process and session UUID whose fresh context matches
- **THEN** the daemon records both and creates the child under that exact process's current pane

#### Scenario: A named parent
- **WHEN** a caller requests a spawn under a parent it names
- **THEN** the recorded request names that parent and the created pane is created under it

#### Scenario: An unknown parent
- **WHEN** a caller names a parent the daemon has no runtime subject for
- **THEN** the spawn is refused before dispatch and nothing is created

#### Scenario: The same request arrives twice
- **WHEN** a spawn request id is replayed with identical content
- **THEN** the daemon answers with the recorded outcome and performs no second effect

#### Scenario: An unknown or unbound parent
- **WHEN** a caller names a parent process registration the daemon does not know or whose current pane is absent or ambiguous
- **THEN** the spawn is refused before dispatch and nothing is created

#### Scenario: Missing, stale or mismatched session context
- **WHEN** the exact parent registration has absent, stale, explicitly cleared or different session context
- **THEN** the daemon refuses before dispatch and nothing is created

### Requirement: Topology reads report edges without conclusions
A spawn read SHALL report the exact process parent, the session parent when the edge uses version 2, the bound child when there is one, the edge state, the created location and freshness. Edges SHALL survive a daemon restart and SHALL be re-verified rather than revised by inference. Parent process replacement SHALL NOT rewrite stored edge identities.

#### Scenario: Read after a daemon restart
- **WHEN** edges recorded before a restart are read afterwards
- **THEN** each reports its state and its location's freshness, with stale facts marked

#### Scenario: A location that no longer exists
- **WHEN** a recorded location cannot be found in the backend
- **THEN** the edge reads unresolved, which is not a report that the child stopped

### Requirement: Spawn authority is bounded
The daemon SHALL record edges only for children it spawned. It SHALL NOT adopt a pane or agent it did not create, SHALL NOT write assignment facts, and SHALL NOT restart child processes as a side effect of a lead restart. A child-close operation MAY close the recorded pane of a bound child after positive foreground birth-identity verification. Close records SHALL remain separate from the immutable spawn edge and SHALL NOT change assignment or execution facts.

#### Scenario: A child spawned outside the daemon
- **WHEN** an agent is launched by Herdsman through the backend directly
- **THEN** it has no edge in this topology and its absence is not an error

#### Scenario: It does not touch anything else
- **WHEN** a spawn completes
- **THEN** no assignment fact changed and no process was stopped, resumed or restarted

#### Scenario: A bound child pane is closed later
- **WHEN** a caller uses `child.close` for a daemon-created edge
- **THEN** the same edge and child binding remain unchanged, only the pane-close result is recorded separately, and no assignment or execution fact changes
