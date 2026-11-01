# Spec Delta

## MODIFIED Requirements

### Requirement: Spawn authority is bounded
Spawn edges SHALL describe daemon-requested child creation and binding, not provide the universal execution-placement model. Spawning SHALL NOT settle assignments. Later authorized child close SHALL use common managed termination and pane removal, MAY record verified execution completion and SHALL preserve the original edge and binding. Direct admission SHALL NOT fabricate a spawn relationship.

#### Scenario: A child spawned outside the daemon
- **WHEN** a process is admitted without a daemon-requested child creation
- **THEN** it may have a managed execution and verified placement without an edge, and that absence is not an error

#### Scenario: It does not touch anything else
- **WHEN** a spawn completes
- **THEN** no assignment fact changes and unrelated executions are not stopped, resumed or restarted

#### Scenario: A bound child pane is closed later
- **WHEN** an authorized caller closes a spawned child
- **THEN** common close records its execution/removal outcomes while preserving its original edge and binding and leaving assignment settlement to its owner

### Requirement: Spawn topology reports exit observation
Spawn topology reads SHALL project the linked claim's recorded execution completion, distinguishing unavailable status from captured code/signal and preserving observation provenance. Reads SHALL NOT fetch status from the backend. A missing code SHALL NOT prevent verified execution exit or pane cleanup, and pane-command status SHALL NOT be mislabelled as managed-program status.

#### Scenario: A child on a backend that observes no exit
- **WHEN** neither supervised program completion nor valid other evidence records the child's exit
- **THEN** reads report completion as unknown without a backend status fetch

#### Scenario: tmux child exited
- **WHEN** a managed tmux child exits and its supervisor records its program status
- **THEN** the linked edge projects that status stably after pane removal and daemon reopen

#### Scenario: Execution exited without a code
- **WHEN** the execution's exit is verified but status capture was unavailable
- **THEN** reads distinguish verified exit from missing code without zero-filling or blocking cleanup
