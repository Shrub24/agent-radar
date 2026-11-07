## MODIFIED Requirements

### Requirement: Radar chooses transport without effectful fallback retries
Radar SHALL use the daemon as its only runtime source and SHALL NOT read Herdr directly. A daemon that cannot be reached or handshaken SHALL be shown as a blocking diagnostic and SHALL NOT switch to another source. Once an operation may have been accepted by the daemon, Radar SHALL not retry it directly. Managed lifecycle eligibility and confirmation SHALL remain unchanged. Radar SHALL start the daemon's socket unit when launched, as `daemon-service` specifies.

#### Scenario: Daemon absent at startup
- **WHEN** the daemon cannot be reached or handshaken after Radar starts
- **THEN** Radar draws a blocking diagnostic with no agent rows and retries on its refresh cycle

#### Scenario: Daemon disconnects during close
- **WHEN** the close response is lost after submission
- **THEN** Radar does not call Herdr directly to repeat close

### Requirement: Daemon physical backend is explicitly selected
The daemon SHALL use exactly one physical backend per process, chosen by configuration or environment, with `tmux` as the default and the named server `radar`. `herdr` SHALL be served only when explicitly selected. A tmux backend SHALL target a named tmux server and never the default one. An unknown backend or unreachable server SHALL fail startup or refuse operations, with no fallback to another backend.

#### Scenario: Unknown backend
- **WHEN** the daemon is configured with an unknown backend name
- **THEN** it refuses to start and names the supported backends

#### Scenario: Named tmux server
- **WHEN** the tmux backend is selected with a server name
- **THEN** every effect targets that server and none targets the default tmux server

#### Scenario: Default backend
- **WHEN** no backend is configured or set in the environment
- **THEN** the daemon serves the tmux backend on the server `radar`
