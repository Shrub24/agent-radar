# Proposal

## Why

The TUI still owns the background-task bus: it binds the bus socket, holds each publisher's connection state and shows tasks only while it runs. A second display (the sidebar) would find the socket taken and run without tasks. The daemon already has its own socket stack, and the two copy the same directory trust check, accept loop, connection cap and shutdown logic (`prepare_directory` exists three times). The daemon's transport is also about to change for shared observation and tmux control mode, so the transport should be rewritten once, on the model those changes need, not twice.

## What Changes

- Add one daemon transport module for every socket the daemon serves: directory trust checks, binding and inherited-listener rules, bounded line framing, idle and write timeouts, admission by lane, and orderly shutdown. It replaces the accept and trust code in `src/bus.rs` and `src/control_plane/server.rs`. The wire protocols do not change.
- Build that transport on Tokio, in that module only. Request handlers, `store`, `registry`, `RuntimeProvider`, process verification, and the CLI and TUI clients stay synchronous. Handlers run on a bounded blocking pool and are never called on a runtime worker.
- Admit work by lane, with capacity reserved so telemetry and waiting readers cannot starve lifecycle controls: control requests, publication ingest, and waiting reads.
- Move the background-task bus into the daemon. The daemon binds the existing bus socket path with the existing bus protocol, as a temporary compatibility entry, and keeps task state in memory per connection. The TUI stops binding it.
- Add a daemon read, `tasks.list.v1`, so any display reads task state from the daemon. The TUI reads it through its daemon client.
- Make `events.read` waiting a transport wait on the journal sequence, so a waiting reader no longer occupies a worker. The method and its results are unchanged.
- Adopt `rustix` for the Linux process and file primitives in the transport, process control, procfs and ownership checks, without changing any guarantee. Signal handler installation stays on libc, because rustix does not provide it.
- **BREAKING**: tasks join an agent through the registry's published session, not a Herdr pane token or pane id, and the bus `hello` pane field no longer joins anything.

Non-goals: moving publishers to the control socket (a later change merges the bus into a `tasks` lane there), the observation sampler and revision subscriptions, tracing, tmux control mode, Specta, async domain code, a durable task store, and any new event domain.

## Capabilities

### New Capabilities

- `daemon-transport`: one socket stack with trust checks, bounded framing, timeouts, lane admission and shutdown for every daemon socket.

### Modified Capabilities

- `extension-bus`: the daemon, not the TUI, listens; connection-scoped data is held by the daemon; task state is readable from the daemon.
- `background-task-detail`: tasks join an agent by the registry's published session.

## Impact

- `src/bus.rs` loses its listener and keeps decoding and `BusState`; `src/main.rs` stops binding it.
- `src/control_plane/server.rs` accept, serve and shutdown move to the transport module; handlers are untouched.
- New dependency `tokio` (`rt-multi-thread`, `net`, `sync`, `time`, `io-util`) and `rustix`; the Nix package vendors from `Cargo.lock`, so no hash changes.
- The dashboard client's `KNOWN_CAPABILITIES` gains `tasks.list.v1`.
- `pi-bash-processes` is not changed: it dials the same path with the same protocol.
