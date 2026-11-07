# Proposal

## Why

Radar's daily path still assumes an operator-run daemon, an operator-created tmux server and Herdr as the default physical backend. The TUI silently falls back to a direct Herdr adapter when the daemon is absent, so agent facts can disappear behind a one-line diagnostic. Socket and state paths are documented but every setup repeats them. The result is that opening Radar does not give a working fleet, and stopping the daemon has no defined meaning.

## What Changes

- One configuration file, read by the TUI and the daemon, holds the physical backend and the named tmux server. Environment variables remain overrides. Socket and state paths have no configuration keys: their documented defaults apply, and the environment overrides are kept for tests and separate instances.
- Remove the TUI's direct adapter. The TUI reads the daemon's registry only. An unreachable daemon is a blocking diagnostic with no agent rows and no fallback source.
- Declare the daemon as a systemd user socket and service, and the tmux server as a template instance `radar-tmux@<server>.service`. Home Manager installs them behind the existing enable option.
- The daemon owns its tmux server: it starts the unit, captures the serving process identity, refuses a server it did not start, and reconciles without creating sessions. Ownership is control and lifecycle authority, not a parent process.
- `radar` starts the daemon socket and the server, then opens the TUI. `radar daemon` and `radar server` wrap `systemctl --user` for start, stop, restart and status.
- `radar daemon stop` stops the socket and then the service, so an explicit stop holds until a `radar` launch or a login. Stopping the daemon never stops the tmux server.
- `radar server stop` refuses while the server has any pane, naming them. Managed resume and restart are not yet implemented, so the gate stays strict until they are.
- Herdr is no longer the daemon's default physical backend. It remains selectable only until the follow-up change `remove-herdr-backend` deletes the adapter, which follows this change and is not part of it.

Non-goals: multiple tmux servers under one daemon, socket-activated restart of an operator-stopped service from other clients, persistent closure across logins, client-local navigation, and managed restart of a lead. The daemon and TUI stay Linux-only, as already accepted.

## Capabilities

### New Capabilities

- `daemon-service`: user units, socket activation, explicit stop semantics, the daemon-owned tmux server, the stop gate, configuration resolution and the commands that wrap them.

### Modified Capabilities

- `mux-control-plane`: the TUI no longer selects a direct adapter or draws ordinary panes without a daemon; the daemon's default physical backend becomes tmux and Herdr is explicit only.
- `nix-distribution`: enabling the module installs the daemon and server units; it no longer promises that it creates no service.

## Impact

- `src/config.rs`: `[daemon]` keys; the `[runtime]` keys are removed.
- `src/control_plane/server.rs`, `src/control_plane/client.rs`, `src/tmux.rs`: inherited-socket binding, server ensure and identity, the stop gate and the default backend.
- `src/main.rs` and `src/app.rs`: `radar daemon|server` subcommands, the ensure-then-open startup, and the blocking diagnostic in place of direct mode.
- `nix/home-manager.nix`, `docs/daemon.md`, `docs/control-plane.md`, `README.md`, `--help` text.
- Tests that assumed the Herdr default or the direct adapter are updated; tmux acceptance runs without an operator-created server.
