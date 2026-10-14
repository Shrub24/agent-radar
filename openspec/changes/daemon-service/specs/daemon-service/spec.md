## ADDED Requirements

### Requirement: The daemon is a systemd user socket and service
Radar SHALL declare `radar-control.socket` and `radar-daemon.service` as user units. The socket SHALL listen on the documented control socket path in a 0700 directory with mode 0600. The service SHALL run `radar daemon` with `KillMode=process` and `Restart=on-failure`. The daemon SHALL serve a listening socket that systemd passes and SHALL NOT unlink a socket path it did not bind.

#### Scenario: Socket activation
- **WHEN** a client connects while the socket unit listens and the service is not running
- **THEN** systemd starts the service and the connection is served once

#### Scenario: Service stopped alone
- **WHEN** the service stops or crashes while the socket listens
- **THEN** the tmux server unit is unaffected and the next connection starts the service

### Requirement: Explicit stop holds until an operator starts Radar
`radar daemon stop` SHALL stop the socket unit and then the service. While the socket is stopped no listener exists, so no publisher or client can start the daemon. `radar` and `radar daemon start` SHALL start the socket unit. An explicit stop SHALL persist only for the current login session: the enabled socket listens again after login.

#### Scenario: Explicit stop with a publisher connected
- **WHEN** an operator runs `radar daemon stop` and a publisher then connects
- **THEN** the connection is refused and the daemon stays stopped

#### Scenario: Launch after an explicit stop
- **WHEN** an operator runs `radar` after an explicit stop
- **THEN** the socket starts, the daemon serves, and the TUI draws the registry

### Requirement: The tmux server is a named template instance
Radar SHALL provide `radar-tmux@.service` running `tmux -D -L <name> -f /dev/null` with `KillMode=control-group` and no automatic restart. An instance name SHALL be a valid tmux socket name and SHALL NOT be `default`. The server's lifetime SHALL be independent of the daemon's and the TUI's.

#### Scenario: Daemon stopped
- **WHEN** the daemon service stops while the server runs
- **THEN** the server and its panes keep running

#### Scenario: Reserved name
- **WHEN** an instance is requested with the name `default`
- **THEN** Radar refuses before starting any unit

### Requirement: The daemon ensures its tmux server
When it serves the tmux backend, the daemon SHALL start the configured server's unit if it is not active, verify the serving process by birth identity, and treat a changed identity as a replaced server. Ensure SHALL NOT create sessions. A server that answers on the socket while its unit is not active SHALL be treated as foreign: the daemon SHALL refuse it, name the socket, and start nothing.

#### Scenario: Foreign server
- **WHEN** a tmux server answers on the configured socket but was not started by its unit
- **THEN** the daemon refuses to serve it and reports the socket

#### Scenario: Replaced server
- **WHEN** the server's serving identity differs from the recorded one
- **THEN** the daemon treats the server as replaced, and bindings recorded against the old serving identity are refused on use

### Requirement: Server stop is gated on an empty server
`radar server stop` and `radar server restart` SHALL refuse while the server has any pane, live or retained, or while a durable operation is pending. The refusal SHALL name the panes and their containers and SHALL send no signal. `radar daemon stop` SHALL NOT stop the server.

#### Scenario: Pane present
- **WHEN** an operator stops the server while a pane exists
- **THEN** the stop is refused and the pane is named

#### Scenario: Empty server
- **WHEN** an operator stops the server with no pane and no pending operation
- **THEN** the unit stops and the stop is reported

### Requirement: Radar commands wrap the user units
`radar daemon` and `radar server` subcommands (start, stop, restart, status) SHALL act through `systemctl --user` and report the unit state.

#### Scenario: Daemon status
- **WHEN** an operator runs `radar daemon status`
- **THEN** the socket and service unit states are reported without starting either unit

### Requirement: Configuration is one file with environment overrides
Radar SHALL read `[daemon] backend` (default `tmux`) and `tmux_server` (default `radar`) from `config.toml`, read by both the TUI and the daemon. The environment variables `RADAR_CONTROL_BACKEND`, `RADAR_TMUX_SERVER`, `RADAR_CONTROL_SOCKET` and `RADAR_CONTROL_STATE` SHALL override the file. `[runtime] backend` and `[runtime] socket` SHALL NOT be read; a configuration that still sets them SHALL be refused as containing unknown keys.

#### Scenario: Removed key
- **WHEN** a configuration sets `[runtime] backend`
- **THEN** Radar refuses to start and reports the unknown key

#### Scenario: Environment override
- **WHEN** `RADAR_TMUX_SERVER` is set and the file names another server
- **THEN** the environment value is used

### Requirement: An unreachable daemon is a blocking diagnostic
When the TUI cannot reach the daemon after its launch, it SHALL draw a blocking diagnostic with no agent rows. The diagnostic SHALL name `radar daemon start` and, when the unit failed rather than stopped, say so. The TUI SHALL retry the connection on its refresh cycle and SHALL NOT fall back to another source.

#### Scenario: Daemon stopped
- **WHEN** the TUI starts while the daemon is explicitly stopped and the start does not reach it
- **THEN** the diagnostic names the start command and no row is drawn

#### Scenario: Daemon recovers
- **WHEN** the daemon becomes reachable while the diagnostic is shown
- **THEN** the next refresh draws the registry without a restart of the TUI
