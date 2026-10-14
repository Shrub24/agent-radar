# Tasks

## 1. Transport

- [ ] 1.1 Add `tokio` (minimal features) and `rustix`; confirm the locked build and the checked Nix build still pass and record build time.
- [ ] 1.2 Add `src/control_plane/transport/` with one directory trust check, bind and inherited-listener rules, and stale-socket handling, replacing the copies in `bus.rs` and `server.rs`. Test symlink, foreign owner, wrong mode, a stale socket, a live socket, and an inherited listener that is never unlinked (closes `daemon-service` 2.3).
- [ ] 1.3 Add bounded line framing with idle and write timeouts. Test one byte under, at and over 1 MiB, an unterminated line, an idle client and a client that never reads.
- [ ] 1.4 Add lane admission with reserved control capacity and a typed busy answer. Test ingest saturated, waiters saturated, and that neither delays a control request.
- [ ] 1.5 Run handlers on the bounded blocking pool through one synchronous dispatch function. Own the runtime in `Daemon`; keep `start`, `run`, `stop` and `Drop` synchronous. Test stop with a running handler and a request whose client disconnects after admission.
- [x] 1.6 Audit hold times of the `Core` lock at its handler sites and record the result; change none unless a hold spans an effect that a control request must not wait behind. No effectful `RuntimeProvider` operation, process-verifier wait or await occurs under the lock; a few paths read only the current adapters' in-memory provider identity/capability accessors. Synchronous filesystem-backed registry and operation-store work does; see `design.md` for the residual control-latency risk.

## 2. Control socket on the transport

- [ ] 2.1 Serve the control socket, including socket-activated listeners, through the transport and delete the accept, serve and connection-registry code from `server.rs`. The existing socket tests, fixtures and publisher smoke pass without edits.
- [ ] 2.2 Make `events.read` waiting a transport wait on the journal sequence. Keep results and bounds; test an event during the wait, an empty wait, and shutdown during a wait.

## 3. Bus in the daemon

- [ ] 3.1 Bind the existing bus socket path in the daemon through the ingest lane with the existing protocol; keep `BusState` decoding, delete the TUI listener and accept loop, and keep connection-scoped, in-memory state. Test a connection with no TUI running, a disconnect, a session change, a malformed line, and no event-log append for repeated lists.
- [ ] 3.2 Add `tasks.list.v1` with bounds and continuation; advertise it only while the bus is bound; add it to the dashboard client's `KNOWN_CAPABILITIES`. Test the advertisement, paging, and the unavailable case.
- [ ] 3.3 Read task state in the TUI through the daemon client and join publisher sessions only by the registry-published UUID; never use the publisher's `hello.pane` to join. Keep existing `pi_bg_*` facts as a separate token-only projection, not a publisher fallback. Show task-source availability from the daemon, not a TUI listener. Update the terminal smoke's `bus_off` mode to occupy the compatibility socket before daemon startup.

## 4. rustix

- [ ] 4.1 Replace the pidfd syscalls, `kill_process_group` uses, `getuid`, `sysconf` values, `tcgetattr` and the `O_NOFOLLOW` opens with rustix. Keep signal handler installation on libc. The existing process-control, procfs and ownership tests pass without edits; add none that only mirror the new calls.

## 5. Documentation and acceptance

- [ ] 5.1 Update `docs/radar-bus.md`, `docs/daemon.md` and `docs/control-plane.md` for the daemon-owned bus, `tasks.list.v1` and the lane limits; record the later merge into a control-socket `tasks` lane in `ideas.md`.
- [ ] 5.2 Run fmt, strict Clippy, the locked test suite, both PTY smokes, tmux acceptance, the publisher and event-consumer fixtures, strict spec validation and the checked Nix build; record real exit codes in `verification.md`. Live acceptance on the daemon is the operator's.
