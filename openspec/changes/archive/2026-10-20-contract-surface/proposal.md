# Proposal

## Why

The handshake advertises two vocabularies in one list: backend primitives (`mux.create`, `mux.launch`) and daemon contracts (`spawn.v1`, `registry.v1`). The client keeps a 31-name whitelist and leaks the filtered list to satisfy a `'static` trait signature. Names are repeated in constants, a contract table, a per-operation table and the client whitelist, and a name missing from the whitelist is silently dropped. With tmux the sole backend the primitives never vary, so the layer protects against a case that no longer exists. What does vary is the version of a long-lived daemon relative to a freshly installed client, because the daemon keeps running across Home Manager switches.

## What Changes

- The handshake advertises a protocol `revision` (an integer bumped whenever a method or field is added) and the daemon contracts external consumers pin. It stops advertising backend primitives.
- In-repo clients (the TUI and the CLI) require a minimum revision. Against an older daemon they show the existing blocking diagnostic naming the version gap and the restart command, and do not probe individual contracts.
- External consumers (Herdsman, the Pi extension library) keep checking contract names. Unknown names in a daemon's answer are ignored, with no client whitelist and no leaked list.
- Contracts and backend primitives become typed in the daemon: an exhaustive contract enum with its serialized name, a primitive set returned by value from the backend, and a `match` giving each contract's needs, so the advertised set is derived and cannot drift. The backend trait stops returning a `'static` slice.
- The primitive layer stays inside the daemon until the Herdr backend is removed, then is deleted with it. Operations a backend lacks still refuse before any effect.
- **BREAKING**: the `capabilities` answer in `daemon.ping` no longer lists `mux.*` names. Consumers that read them must read contracts or the revision.

Non-goals: removing the Herdr backend, a new transport version, per-method negotiation, generated contract types.

## Capabilities

### Modified Capabilities

- `mux-control-plane`: launch context support is negotiated as a contract, and the handshake advertises a revision and contracts instead of backend primitives.

## Impact

- `src/control_plane/ops.rs`, `server.rs`, `client.rs`, `src/runtime.rs`, `src/tmux.rs`, `src/herdr.rs`; the test backends in `tests/control_plane.rs` (a large mechanical change); `docs/control-plane.md`.
- Pairs with typed facts only in sequence; neither depends on the other.
