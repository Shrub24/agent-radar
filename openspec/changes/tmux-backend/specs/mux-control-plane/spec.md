## ADDED Requirements

### Requirement: Daemon physical backend is explicitly selected
The daemon SHALL use exactly one physical backend per process, chosen by explicit configuration, with `herdr` as the default. A tmux backend SHALL target a named tmux server and never the default one. An unknown backend or unreachable server SHALL fail startup or refuse operations, with no fallback to another backend.

#### Scenario: Unknown backend
- **WHEN** the daemon is configured with an unknown backend name
- **THEN** it refuses to start and names the supported backends

#### Scenario: Named tmux server
- **WHEN** the tmux backend is selected with a server name
- **THEN** every effect targets that server and none targets the default tmux server

### Requirement: tmux adapter declares only implemented capabilities
The tmux adapter SHALL advertise observation, focus, create, input, output, close and launch only as implemented, using stable tmux ids qualified by backend and server as location handles. Operations it cannot express SHALL be refused by absent capability before dispatch.

#### Scenario: Stable handles
- **WHEN** a pane is observed, then windows are reordered
- **THEN** the pane keeps its handle

### Requirement: tmux launch uses direct argv
The tmux adapter SHALL launch children from resolved executable and argv without typing into a shell, carrying cwd and the environment overlay exactly and keeping environment values and the token out of diagnostics. It SHALL NOT infer readiness from timing or output.

#### Scenario: Shell-sensitive argv
- **WHEN** argv contains spaces, quotes and newlines
- **THEN** the child receives them unchanged

### Requirement: Physical exit is a typed observation
For panes it launched with a supporting backend, the daemon SHALL record at most one exit observation of `exited` with a code or a signal and an observed time. A missing pane, missing report or unreachable server SHALL remain unknown or unresolved and SHALL NOT be reported as an exit.

#### Scenario: Nonzero exit
- **WHEN** a launched child exits with code 3 and its pane remains
- **THEN** the exit observation reports code 3

#### Scenario: Pane vanished
- **WHEN** a launched pane disappears without a recorded exit
- **THEN** the location is unresolved and no exit is reported

### Requirement: Exit does not imply assignment outcome
An exit observation SHALL NOT change execution or assignment publications and SHALL NOT trigger close, resume or restart. Closing a dead pane SHALL remain the separate guarded `child.close` operation.

#### Scenario: Exited child pane
- **WHEN** an exit is observed
- **THEN** the pane stays until `child.close` is requested and publications are unchanged
