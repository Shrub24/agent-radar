## ADDED Requirements

### Requirement: Radar serves only tmux 3.8 or newer
The daemon SHALL read the serving tmux server's version and refuse versions older than 3.8, naming the version and the tmux package setting. It SHALL NOT maintain an older-server fallback.

#### Scenario: Older running server
- **WHEN** the serving server reports 3.7c although the installed executable is newer
- **THEN** Radar refuses that server and names its reported version and package setting

#### Scenario: Supported server
- **WHEN** the owned server reports 3.8 or newer
- **THEN** its version permits service

### Requirement: One no-output observer covers the server
Radar SHALL use one server-wide control-mode observer that ignores size and receives no pane output. It SHALL NOT be read-only, because tmux resolves a command's target client to the best attached client and a read-only one would refuse Radar's own writes. Attaching SHALL NOT change terminal size, active window or active pane. Any additional Radar hooks SHALL use owned slots without replacing user hooks.

#### Scenario: Another session changes
- **WHEN** topology changes in a session other than the observer's attached session
- **THEN** the observer requests a refresh of server state

#### Scenario: Unexpected output
- **WHEN** a pane-output notification arrives unexpectedly
- **THEN** it is discarded without storage or logging

### Requirement: Notifications request reads rather than establish facts
Notifications SHALL request coalesced state reads and SHALL NOT establish focus, placement, absence or process exit themselves. Unknown notifications SHALL be ignored. Notification handling and pending refresh work SHALL remain bounded.

#### Scenario: Burst during a read
- **WHEN** multiple notifications arrive during a sample
- **THEN** a further refresh remains requested without queueing a sample for every notification

#### Scenario: Pane gone from a layout
- **WHEN** a notification suggests a pane disappeared
- **THEN** no absence or exit is recorded without the existing verified evidence

### Requirement: Observer lifecycle tolerates an empty or replaced server
The observer SHALL wait without creating a session when the server is empty, attach when a session exists, and re-establish observation after its attached session disappears. It SHALL attach only to the verified owned serving incarnation. Daemon shutdown SHALL end and reap its observer child.

#### Scenario: First workspace
- **WHEN** the server starts with no sessions and a workspace is subsequently created
- **THEN** Radar begins observing it without a placeholder session

#### Scenario: Attached session destroyed
- **WHEN** the observer's attached session is destroyed while another session remains
- **THEN** observation is re-established against the surviving server

#### Scenario: Serving process replaced
- **WHEN** a new serving incarnation reuses old pane identifiers
- **THEN** old-incarnation observations cannot establish focus in the new incarnation

#### Scenario: Foreign server
- **WHEN** the configured name resolves to a server Radar cannot verify as owned
- **THEN** no observer attaches to it

### Requirement: Observer loss is visible and timer reads recover
Radar SHALL report observer connection state, last notification time and loss reason. Observer failure SHALL retain timer-based sampling and bounded reconnect. Loss SHALL NOT be treated as no change or proof of absent focus.

#### Scenario: Observer exits
- **WHEN** the observer exits unexpectedly
- **THEN** its health reports the loss, timer reads remain available and reconnect uses bounded backoff

### Requirement: The observer is not an operator client
Radar SHALL exclude its own observer, the client that asks for no output, from operator-client counts, focus-client choice and attachment-dependent guards. An operator's own control-mode client is still an operator, because only the observer asks for no output.

#### Scenario: Only the observer is attached
- **WHEN** the only attached client is Radar's observer
- **THEN** Radar reports no operator client and never selects that observer for a focus command

### Requirement: Focus is one typed observed terminal location
Radar SHALL publish the unified operator's focused workspace, window, pane and zoom state from current reads. It SHALL distinguish available focus, no operator, unavailable evidence and unsupported multiple operators. Stale last-good focus MAY be displayed as stale but SHALL NOT authorize implicit targeting.

#### Scenario: Native navigation
- **WHEN** the operator switches sessions, windows or panes using native tmux navigation
- **THEN** a subsequent read publishes the actual focused location to all Radar views

#### Scenario: Read fails
- **WHEN** current focus cannot be established
- **THEN** Radar indicates unavailable focus rather than reporting the observer's attached session or stale focus as current

#### Scenario: More than one operator
- **WHEN** more than one non-control operator client is present
- **THEN** Radar reports the unsupported condition and refuses ambiguous focus-changing or implicit-focus commands without choosing a preferred client

### Requirement: Observer startup establishes a current baseline
Observer establishment SHALL obtain a current state baseline and preserve refresh requests arriving during that read. A transition around establishment SHALL become visible without requiring unrelated input.

#### Scenario: Focus changes during baseline
- **WHEN** focus changes while the observer's baseline read is in flight
- **THEN** the baseline or a subsequent refresh conveys the changed current focus
