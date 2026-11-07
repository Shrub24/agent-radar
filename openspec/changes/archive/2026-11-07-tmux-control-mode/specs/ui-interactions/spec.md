## MODIFIED Requirements

### Requirement: Captured intent cannot be retargeted by shared selection
Every row-dependent TUI invocation, including Delete, Restart, Focus and configured Launch, SHALL explicitly identify its local selected row. An intentional invocation without a row SHALL resolve observed shared focus instead. The actor SHALL validate eligibility and freeze target, identities and preconditions before preparation, confirmation or execution. Invalid explicit targets SHALL refuse without focus fallback. Domain execution SHALL revalidate preconditions without retargeting.

#### Scenario: Another frontend changes selection
- **WHEN** one frontend invokes a command on B while another selects C and tmux focus is A
- **THEN** the command validates B or refuses, never substituting A or C

#### Scenario: Target disappears
- **WHEN** the explicit row is absent from the actor's canonical tree
- **THEN** it refuses without acting on a neighbour or the focused row

#### Scenario: Implicit target frozen before preparation
- **WHEN** a rowless invocation resolves A and focus changes to B during preparation or confirmation
- **THEN** the interaction and execution retain A subject to domain revalidation

#### Scenario: Configured launch
- **WHEN** a frontend prepares configured Launch for selected B
- **THEN** frontend command/environment preparation cannot change its frozen target to later focus or selection

#### Scenario: Replacement before confirmation
- **WHEN** a frozen execution identity is replaced before confirmation
- **THEN** incompatible execution is refused rather than redirected to the replacement

#### Scenario: Rapid navigation before command
- **WHEN** a view moves its selection to B and immediately invokes Close, Restart, Focus or configured Launch while publications are delayed
- **THEN** the command names B, the row that view displayed, and is ordered after the move
