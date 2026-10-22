# Spec Delta

## MODIFIED Requirements

### Requirement: L launches the configured managed-agent command
Radar SHALL expose `[tui].launch_command` as an argv vector defaulting to `["pi"]`. Pressing `L` on a recorded workspace or pane SHALL immediately create a tab in that place's workspace and start that command through the same managed launch path as `radar run`, without a command prompt or confirmation. A launch from a pane SHALL open its own tab rather than splitting the pane. The launched command SHALL be a member of its execution's scope before it runs.

#### Scenario: Launch the default agent
- **WHEN** the user presses `L` on a recorded workspace or pane without a launch-command override
- **THEN** Radar starts `pi` in a new tab of that place's workspace inside that execution's scope

#### Scenario: Launch from a pane
- **WHEN** the user presses `L` on a pane
- **THEN** the agent gets a window of its own in that pane's workspace, and the pane it was launched from is left as it was

#### Scenario: Scope fails during a TUI launch
- **WHEN** the scope cannot be established for an `L` launch
- **THEN** the command does not run and Radar reports the refusal rather than starting it unscoped

## REMOVED Requirements

### Requirement: A launched or registered lead can be enrolled
**Reason**: Managed standing now follows an admitted or recovered execution. Promoting a separately started process to full managed lifecycle standing cannot provide the guarantees a managed launch establishes, and maintaining that parity became a second lifecycle path.

**Migration**: A lead started with `L` is bound by its managed launch rather than by a separate enrollment step. For an agent started outside Radar, the operator confirms a restart through Radar, which requires the previous process to have exited before starting its replacement. Existing rows keep their observed facts and durable session identity; no history, inbox or relationship is discarded.

## ADDED Requirements

### Requirement: Externally started agents are observed, not promoted
Radar SHALL show an agent started outside its managed launch path as observed: its facts, session identity, children and inbox identity remain visible and durable. Radar SHALL NOT offer it execution controls that require a managed launch, and SHALL NOT imply its resource grouping, startup admission or exit result are tracked.

#### Scenario: Observed agent selected
- **WHEN** the operator selects an agent Radar did not launch
- **THEN** Radar shows its facts and relationships and states that it is not a managed execution

#### Scenario: Transition through restart
- **WHEN** the operator chooses to place an observed agent under Radar's management
- **THEN** Radar confirms a restart, verifies the previous process has exited, and starts its replacement through the managed launch path

### Requirement: Lifecycle controls name the claim they were issued against
Every lifecycle control that affects a process SHALL name the claim that was current when the operation was requested. Radar SHALL refuse the control when that claim is no longer the subject's current claim, and SHALL NOT retarget a response or a signal onto a successor attempt.

#### Scenario: Stale control after a restart
- **WHEN** an operation naming an earlier claim reaches the daemon after the subject's successor became current
- **THEN** it is refused as stale and no signal targets the successor

#### Scenario: Replayed control
- **WHEN** a control is replayed under its own request id for the claim it named
- **THEN** the recorded response is returned and no second effect occurs

### Requirement: Restart pre-mints its successor before stopping
A managed restart SHALL pre-mint the successor claim and record it as the subject's current claim before it signals the previous process, so the subject is never without a current attempt between the stop and the replacement starting. A restart SHALL refuse before signalling when it cannot pre-mint.

#### Scenario: Continuity during restart
- **WHEN** a managed restart is dispatched
- **THEN** the successor claim exists before the previous process is signalled, and a competing launch cannot claim that subject in the gap

#### Scenario: Successor cannot be admitted
- **WHEN** the successor claim cannot be made durable
- **THEN** no signal is sent and the previous process is left as it was

