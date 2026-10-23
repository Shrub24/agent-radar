## ADDED Requirements

### Requirement: Radar serves only tmux 3.8 or newer
The daemon SHALL read the version the serving tmux server reports and SHALL refuse to serve a server older than 3.8, naming the version it found and the setting that selects tmux. Radar SHALL NOT gate individual features on the tmux version or keep a path for older servers.

#### Scenario: Older server running
- **WHEN** the owned server reports version 3.7c
- **THEN** the daemon refuses to serve it and the refusal names 3.7c and the tmux package setting

#### Scenario: Supported server
- **WHEN** the owned server reports 3.8 or a later version
- **THEN** the daemon serves it

### Requirement: The observer is read-only and receives no pane output
The daemon SHALL observe its owned server through control-mode clients that are read-only, ignore size and receive no pane output. An observer SHALL NOT change a server, session, window, pane, selection or user option, and Radar SHALL NOT read or retain pane output through it. Hooks or monitors Radar installs SHALL be added in their own slots and removed by slot, never by replacing a user's hook.

#### Scenario: Observer attached
- **WHEN** an observer attaches
- **THEN** no window size, active window, active pane or user option changes and no pane output reaches the daemon

#### Scenario: Output notification
- **WHEN** an unexpected output notification arrives
- **THEN** it is discarded without being stored or logged

### Requirement: Notifications request samples and are never facts
A notification, monitor or subscription change SHALL request a sample from the shared sampler and SHALL NOT be recorded as, or used to derive, an observed fact, a placement, a pane absence or a process exit. State SHALL come from the sample's reads. Unknown notifications SHALL be ignored, and a burst SHALL coalesce into one further sample.

#### Scenario: Pane moved
- **WHEN** a notification reports a pane moved to another window
- **THEN** a sample is requested and the pane's new window appears only when a sample's inventory reports it

#### Scenario: Pane gone from a layout
- **WHEN** a notification shows a pane gone
- **THEN** no absence or exit is recorded until the existing verified read establishes it

#### Scenario: Burst
- **WHEN** many layout notifications arrive within one sample
- **THEN** one further sample follows

### Requirement: Every session is observed or reported unobserved
The daemon SHALL either observe every session on its owned server or report each session it does not observe, with the reason, and SHALL sample unobserved sessions on its timer.

#### Scenario: A session is created
- **WHEN** a session is created
- **THEN** it is observed before the next timer sample is due, or the source reports it unobserved with the reason

### Requirement: Observer health is reported and loss degrades to the timer
The daemon SHALL report observation as a snapshot source with its connection state, last notification time and loss reason. When an observer ends or cannot start, the daemon SHALL keep sampling on its timer, SHALL re-establish the observer with bounded backoff, and SHALL NOT treat the loss as absence of change.

#### Scenario: Observer exits
- **WHEN** an observer's connection ends
- **THEN** the snapshot reports it lost with the reason, sampling continues on the timer, and the observer is retried

#### Scenario: Observer cannot start
- **WHEN** the server refuses the observer
- **THEN** the daemon serves with timer sampling and the source reports why

### Requirement: Observers are fenced to the owned server
An observer SHALL attach only to the server whose serving process the daemon verified as its own, SHALL end when that process is replaced, and SHALL be re-created against the new one after verification. The daemon SHALL NOT attach an observer to a foreign server.

#### Scenario: Server replaced
- **WHEN** the owned server is replaced by a new process
- **THEN** old observers end and new ones attach after the new serving identity is verified

#### Scenario: Foreign server
- **WHEN** a server on the configured name is not the one the daemon owns
- **THEN** no observer attaches and the source reports the refusal

### Requirement: Observers are not operator clients
The tmux adapter SHALL NOT count control-mode observers as operator clients when it reports attached clients or session attachment, and an observer's presence SHALL NOT change any guard that depends on operator attachment.

#### Scenario: Only observers attached
- **WHEN** a session has only Radar's observer attached
- **THEN** the session is reported as having no operator client
