# Design

## Context

Today the daemon is started by hand, the tmux server is created outside Radar, the TUI defaults to a direct Herdr adapter, and `herdr` is the daemon's physical default. Herdsman publishers, the TUI and the CLI all reach the daemon through one socket, and several of them reconnect on their own. The operator has decided the model below; this design records how it is built and where it costs something.

## Decisions

### 1. One config file, with environment overrides

`config.toml` gains a `[daemon]` table read by both the TUI and the daemon:

```toml
[daemon]
backend = "tmux"        # tmux | herdr
tmux_server = "radar"   # the named server; never tmux's default
```

The environment keeps precedence: `RADAR_CONTROL_BACKEND`, `RADAR_TMUX_SERVER`, `RADAR_CONTROL_SOCKET` and `RADAR_CONTROL_STATE` override the file and the defaults. Socket and state paths get no configuration keys. Their XDG defaults already apply on every supported path, and a key would be a second copy of a rule the daemon and TUI already share. `[runtime] backend` and `[runtime] socket` are removed. The loader already refuses unknown keys, so a leftover key fails loudly with no migration path. Backward compatibility is not a goal of this change: the direct adapter, the `[runtime]` keys and the Herdr default may break, and cleanup of the old paths can follow.

### 2. Socket activation is the mechanism for "auto-start unless explicitly closed"

An earlier draft deferred socket activation. The operator's model needs it: a stopped daemon must not restart because a publisher reconnected, but must start again when the operator runs `radar`, and must start on its own when a publisher connects while the socket is listening. Only a listener outlives the daemon's stop, so the socket unit carries the state.

- Socket listening, service down: the first connection starts the service and the connection is served.
- Socket stopped: no listener exists, so nothing can connect. Clients get a missing or refused socket and report "daemon stopped". Only `radar` or `radar daemon start` brings it back.
- Service stopped alone while the socket listens: the next connection restarts it. This is systemd's behaviour and is not what `radar daemon stop` does.

Costs we accept and must verify:
- A connection queued while the service starts is delivered later. A client that timed out and resent produces a duplicate, so mutating requests rely on the existing request-ID replay and readback.
- A start failure or a crash loop reaches clients as a refused connect until `reset-failed`. The diagnostic distinguishes "stopped" from "failed to start" by asking systemd for the unit state.
- The daemon serves an inherited socket and does not unlink a path it did not bind. Its trust checks (user-owned real directory, mode 0700, socket owned by the user) still apply to the inherited path. `DirectoryMode=0700` and `SocketMode=0600` are set on the unit and checked on this machine.

Rejected: stopping only the service and keeping a marker file. Publishers would keep reconnecting to a dead path with nothing to start it for them, and the marker would have to be honoured by every client.

### 3. The tmux server is a template instance, run in the foreground

`radar-tmux@.service` runs `tmux -D -L %i -f /dev/null` with `Type=simple`, `KillMode=control-group` and no automatic restart. The probe on this machine (tmux 3.7c, transient user unit) confirmed three things: the server stays up with no session and no command because `-D` turns `exit-empty` off; a session created afterwards is visible; and stopping the unit kills the server and its panes. The instance name is validated as a tmux socket name and may not be `default`.

`-f /dev/null` keeps the fleet server reproducible and independent of the operator's `tmux.conf`. Server-side options that the operator wants must then be set by Radar. Loading the user's configuration is an open choice; see below.

Rejected: `systemd-run`. Its transient units exist only until reboot or `reset-failed`, cannot be declared by Home Manager, and every start must carry the full command line. Two start paths would double the surface.

### 4. The daemon owns the server

The daemon's `ensure_server` is the single authority for the server:

1. If the unit is inactive and the socket already answers, the server is foreign: refuse, name the socket, and start nothing.
2. Otherwise start `radar-tmux@<server>.service` (idempotent) and verify the serving process by its birth identity.
3. Compare with the recorded identity. A different identity means the server was replaced. Pane bindings recorded against the old identity are refused under the existing fence rules; no separate generation record is kept.
4. Reconcile without creating sessions. Containers create their own sessions.

It runs at daemon bind and on every `radar` launch, which connects through the daemon. The daemon invokes `systemctl --user` for its own unit. The alternative is that the TUI does it, which would put lifecycle authority in the client. That is the thing the operator asked to avoid.

Ownership is not a parent-child relationship. Stopping the daemon never touches the server, because the daemon runs with `KillMode=process` and the server is a separate unit.

### 5. Stop is gated, and the gate is strict

- `radar daemon stop` stops the socket, then the service. It never stops the server.
- `radar server stop` refuses while the server has any pane, live or retained, and while a durable operation is pending. The refusal names the panes and their containers and sends no signal. It stops the unit only when the server is empty.
- `radar server restart` is the gated stop followed by start, under the same gate.

The gate is lifted when managed resume and restart can handle live processes. Until then, a stop that would end an agent is refused rather than offered.

### 6. Units are packaged by Home Manager

`nix/home-manager.nix` declares `systemd.user.sockets.radar-control`, `systemd.user.services.radar-daemon` and `systemd.user.services."radar-tmux@"`, behind the existing `programs.radar.enable`. The socket is enabled, so it listens after login. The tmux instance is not enabled: it starts on demand. Units use the flake's package and a configurable tmux package, and the daemon's `path` includes tmux and systemd, because user units do not inherit the shell's `PATH`.

"Stopped for the current session" follows from this: an explicit stop is runtime state, and the enabled socket listens again at the next login.

### 7. Direct mode is removed

The TUI always reads the daemon. There is no direct adapter, no ordinary-pane listing without a daemon, and no Herdr fallback diagnostic. A daemon that cannot be reached is a blocking diagnostic: no agent rows, the command that starts the daemon, and a reconnect attempt on each refresh cycle, so the TUI recovers when the daemon comes up without a restart. `src/herdr.rs` stays only as the daemon's explicit Herdr physical backend until the follow-up change `remove-herdr-backend` deletes it. Nothing in the TUI depends on it. That follow-up removes the adapter, the daemon's Herdr branch, `HerdrRuntime` and the fake-herdr fixtures. Herdr-named display labels and the Herdsman pane-metadata tokens are separate: the tokens stay until the Herdsman cutover is accepted, because Herdsman still publishes them.

## Open choices

- **Load the operator's `tmux.conf` for the server?** The current choice is isolation. Loading it would give the operator their keybindings and options on the server, at the cost of reproducibility.
- **tmux version.** Current testing used 3.7c. The Nix package's version is not yet pinned against the 3.8 floor that the observation design needs.
- **Socket and state overrides.** These are environment-only. A multi-instance operator would need a config key later, which this change does not add.

## Risks

- Restart and stop ordering: a daemon stop racing an in-flight mutation must answer or refuse cleanly. The acceptance run tests it; the design does not assume it.
- The socket-level startup diagnostic depends on `systemctl --user is-failed` being available to the client.
- A missing `systemctl` (a non-systemd login) fails loudly rather than degrading, which is intended.
