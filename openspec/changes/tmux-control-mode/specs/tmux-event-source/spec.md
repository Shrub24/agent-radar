## ADDED Requirements

### Requirement: The observer is read-only and receives no pane output
The daemon SHALL observe the tmux server it owns through control-mode clients that are read-only, ignore size and receive no pane output. An observer SHALL NOT send commands that change the server, a window, a pane or a selection, and Radar SHALL NOT read or retain pane output through it.

#### Scenario: Observer attached
- **WHEN** an observer attaches to a session
- **THEN** no window size, active window, active pane or option changes and no pane output reaches the daemon

#### Scenario: Output notification
- **WHEN** an unexpected output notification arrives
- **THEN** it is discarded without being stored or logged

### Requirement: Notifications request samples and are never facts
A notification or subscription change SHALL request a sample from the shared sampler and SHALL NOT be recorded as, or derive, an observed fact. State SHALL come from the sample's reads. Unknown notifications SHALL be ignored, and a burst SHALL coalesce into one further sample.

#### Scenario: Window added
- **WHEN** a window-added notification arrives
- **THEN** a sample is requested and the new window appears only when a sample's inventory reports it

#### Scenario: Burst
- **WHEN** many layout notifications arrive within one sample
- **THEN** one further sample follows

### Requirement: Observer health is reported and loss degrades to the timer
The daemon SHALL report the observer as a snapshot source with its connection state, last notification time and loss reason. When an observer ends or cannot start, the daemon SHALL keep sampling on its timer, SHALL re-establish the observer with bounded backoff, and SHALL NOT treat the loss as absence of change.

#### Scenario: Observer exits
- **WHEN** an observer's connection ends
- **THEN** the snapshot reports it lost with the reason, sampling continues on the timer, and the observer is retried

#### Scenario: Observer cannot start
- **WHEN** the installed tmux refuses the observer
- **THEN** the daemon serves with timer sampling and the source reports why

### Requirement: Observers are fenced to the owned server
An observer SHALL attach only to the server whose serving process the daemon verified as its own, SHALL end when that process is replaced, and SHALL be re-created against the new one. The daemon SHALL NOT attach an observer to a foreign server.

#### Scenario: Server replaced
- **WHEN** the owned server is replaced by a new process
- **THEN** old observers end and new ones attach after the new serving identity is verified

#### Scenario: Foreign server
- **WHEN** a server on the configured name is not the one the daemon owns
- **THEN** no observer attaches and the source reports the refusal

### Requirement: One bounded observer per session
The daemon SHALL keep one observer per session, SHALL start and end them from session notifications and inventory, and SHALL bound their number, reporting sessions left unobserved because of the bound.

#### Scenario: A session is created
- **WHEN** a session is created
- **THEN** an observer attaches to it before the next timer sample is due or the source reports why not

#### Scenario: Bound reached
- **WHEN** the observer bound is reached
- **THEN** further sessions are sampled on the timer and reported as unobserved

### Requirement: Observers are not operator clients
The tmux adapter SHALL NOT count control-mode observers as operator clients when it reports attached clients or sessions' attachment, and an observer's presence SHALL NOT change any guard that depends on operator attachment.

#### Scenario: Only observers attached
- **WHEN** a session has only Radar's observer attached
- **THEN** the session is reported as having no operator client

### Requirement: Notifications are not process-exit evidence
A pane-related notification SHALL NOT be recorded as a process exit, and the existing verified exit observation and wake hooks SHALL remain the only exit evidence.

#### Scenario: Pane disappears from a layout
- **WHEN** a layout notification shows a pane gone
- **THEN** no exit is recorded until the verified exit path observes it
