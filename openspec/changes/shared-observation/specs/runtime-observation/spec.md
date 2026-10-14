## MODIFIED Requirements

### Requirement: Failed collection preserves stale observations

The daemon SHALL preserve the last successful inventory during collection failure, visibly mark it stale and report the failure. Failure SHALL NOT prove that panes or agents disappeared. A display's reading of a snapshot SHALL NOT block keyboard input or quitting, and successful recovery SHALL restore current observations.

#### Scenario: Herdr unavailable after a successful refresh
- **WHEN** the next collection fails or times out
- **THEN** the last inventory remains visible as stale and its retained associations are not removed as if the runtime were empty

#### Scenario: Failure before the first inventory
- **WHEN** the initial collection fails
- **THEN** the snapshot reports an unavailable-source diagnostic rather than a successful empty fleet

#### Scenario: Recovery
- **WHEN** a collection succeeds after a failure
- **THEN** the next snapshot replaces the stale inventory with the successful observation and clears the source-failure indicator

### Requirement: Positive supersession clears continuity

The daemon SHALL discard a retained agent association when successful inventory proves its pane absent, a different reported agent session replaces it, or positive foreground evidence shows a non-shell replacement command without a current agent observation. The daemon SHALL NOT persist retained associations across its own restarts, and a display restart SHALL NOT clear them.

#### Scenario: Pane closes
- **WHEN** a successful inventory no longer contains the retained pane
- **THEN** its retained association is removed

#### Scenario: Another command occupies the pane
- **WHEN** a retained pane has no reported agent and foreground evidence identifies a non-shell command
- **THEN** the old agent association is removed and the pane is available as an ordinary runtime pane

#### Scenario: New session in the same pane
- **WHEN** a pane reports a different agent session from the previous observation
- **THEN** the old association is replaced by the newly observed session without carrying forward its old ownership or assignment

#### Scenario: Radar restarts
- **WHEN** the daemon starts a new process
- **THEN** no retained associations from its previous process are restored

#### Scenario: Display restarts
- **WHEN** a display starts while the daemon keeps running
- **THEN** it receives the retained associations the daemon holds
