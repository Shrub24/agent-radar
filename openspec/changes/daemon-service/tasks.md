# Tasks

## 1. Configuration

- [x] 1.1 Add `[daemon] backend` (default `tmux`) and `tmux_server` (default `radar`) to the configuration loader; keep environment precedence.
- [x] 1.2 Remove `[runtime] backend` and `[runtime] socket`; a leftover key is refused as an unknown key. Add tests for that and for the defaults.
- [x] 1.3 Make `physical_backend_in` default to tmux and accept `herdr` only when selected explicitly; update its tests.

## 2. Inherited socket and daemon serving

- [x] 2.1 Serve a listening socket passed by systemd (`LISTEN_FDS`, `LISTEN_PID`); keep the existing bind path when none is passed.
- [x] 2.2 Do not unlink an inherited socket path on shutdown; keep the unlink for a socket the daemon bound itself. Test both.
- [x] 2.3 Apply the existing trust checks to the inherited path. Test a symlinked or foreign-owned directory.
- [ ] 2.4 Verify on a live unit that a request queued during start is served once and that a replayed request ID is answered from its record.

## 3. Daemon-owned tmux server

- [x] 3.1 Implement `ensure_server`: refuse a foreign server, start the unit, verify the serving identity, treat a changed identity as a replaced server (fenced by the existing identity rules, no generation record), create no sessions.
- [x] 3.2 Run `ensure_server` at daemon bind; test refusal of a foreign server and refusal of old-identity bindings on a replaced server.
- [x] 3.3 Implement the stop gate: refuse `server stop` with any pane or pending operation, naming the panes; send no signal before refusing. Test the refusal and the empty-server stop.

## 4. Commands and startup

- [x] 4.1 Add `radar daemon start|stop|restart|status` and `radar server start|stop|restart|status` as wrappers over `systemctl --user`. `daemon stop` stops the socket, then the service.
- [x] 4.2 Make bare `radar` start the socket unit, connect, and open the TUI; keep `radar daemon` as the foreground serve used by the unit.
- [x] 4.3 Map a removed socket to "daemon stopped" and a failed unit to "daemon failed to start"; test both mappings.
- [x] 4.4 Update `--help` and `--print-config` to show the `[daemon]` table.
- [x] 4.5 Make a stop graceful: the transport waits up to `DRAIN_TIMEOUT` (3 s) for the requests already being served to answer before it cuts connections, so a write in flight reports its outcome instead of dying with the connection; a request that arrives while stopping is answered `busy` ("daemon is stopping") without being dispatched, so its caller may ask again; a waiting `events.read`/`observation.get` is woken and answered for the window that ended; and a request that cannot answer within the deadline is cut with its record standing, which is what makes an unresolved outcome readable instead of invisible. A switch restarts this service (`sd-switch` stop/start) and the socket unit is untouched, so connections queued during the gap are served by the new daemon; a clean stop keeps the journal epoch stable. Tested at the transport (`a_stop_lets_a_request_in_flight_answer_and_refuses_one_that_arrives_during_it`) and through a real daemon with a backend holding a request (`a_stop_answers_the_request_in_flight_and_refuses_one_that_arrives_during_it`); the daemon test fails when the pre-drain cut is restored. Three existing shutdown tests changed with the behaviour: a request blocked in a verifier and a long-polling read are now answered (`busy` / an ended window) rather than cut, and their bounded-shutdown assertions are unchanged.

## 5. TUI

- [x] 5.1 Remove the direct adapter from the TUI startup path and the Herdr fallback diagnostic; keep `src/herdr.rs` for the explicit daemon backend.
- [ ] 5.2 Draw a blocking diagnostic with no agent rows when the daemon is unreachable; reconnect on each refresh cycle. Add PTY coverage for the diagnostic and for recovery after start.

## 6. Units and Home Manager

- [x] 6.1 Write `radar-control.socket` (`ListenStream=%t/agent-radar/control.sock`, `SocketMode=0600`, `DirectoryMode=0700`, `RemoveOnStop=yes`), `radar-daemon.service` (`KillMode=process`, `Restart=on-failure`) and `radar-tmux@.service` (`tmux -D -L %i -f /dev/null`, `KillMode=control-group`, no restart).
- [x] 6.2 Declare the units in `nix/home-manager.nix` behind `programs.radar.enable`, with the package and tmux package and the daemon's `path`.
- [x] 6.3 Verify the units with `systemd-analyze verify` and add a checked evaluation of the module. Deferred to live acceptance (9.1): the flake adds no Home Manager input, so the module is not evaluated in checks.
- [x] 6.4 Keep the tmux server across a package change: `radar-tmux@` carries `X-SwitchMethod = "keep-old"`. Without it `sd-switch`'s default for a changed unit stops and starts the template instance, which kills the server and every pane and agent in it. Verified with `sd-switch --dry-run` against the installed unit: the default plans `Stop/Start radar-tmux@radar.service (SwitchMethod(StopStart))`, and `keep-old` plans `Keep active radar-tmux@radar.service (SwitchMethod(KeepOld))`. The daemon unit deliberately keeps the default, so a switch that changes its `ExecStart` path does restart it.

## 7. Documentation

- [x] 7.1 Update `docs/daemon.md`, `docs/control-plane.md`, `docs/mux.md` and `README.md`: the units, the commands, the stop semantics, and the removed direct mode.
- [x] 7.2 Update the canonical fixtures and any consumer example that assumed the Herdr default or the operator-created server.

## 8. Gates

- [x] 8.1 Run fmt, strict Clippy, locked tests, the publisher fixture, both PTY smokes, the tmux acceptance (updated to run without an operator-created server), strict OpenSpec validation and the checked Nix build.

## 9. Live acceptance (operator)

- [ ] 9.1 Start with `radar`; confirm the socket, daemon and server units are active and the server has no operator-created state.
- [ ] 9.2 `radar daemon stop` with agents running: the tmux server and its panes survive; a publisher connection is refused; `radar` starts the daemon again.
- [ ] 9.3 Crash the daemon process: systemd restarts it and the agents survive.
- [ ] 9.4 `radar server stop` with a pane present: refused, naming the pane; with no pane: stopped.
- [ ] 9.5 Log out and back in: the socket listens again; an explicit stop does not persist beyond the session that issued it.

