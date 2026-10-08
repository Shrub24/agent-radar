## Why

Radar and Pi extensions currently depend on Herdr's commands and wire formats. A local control-plane daemon gives them a stable mux interface, with Herdr as its first backend and a clear place to add tmux later.

## What Changes

- Add `radar daemon`, serving a versioned local Unix-socket protocol.
- Wrap inventory, foreground evidence, focus, guarded close, creation/splitting, input and bounded output reads behind a backend-neutral interface.
- Provide a metadata-reporting bridge for extension migration without introducing a new registry or agent authority model.
- Record effectful requests durably, execute each request ID at most once, and expose outcomes independently of a connection's lifetime.
- Let Radar use the daemon explicitly, retaining its direct Herdr adapter when selected at startup. Never retry an uncertain daemon action through that fallback.
- Preserve existing managed-owner controls and unmanaged-close checks.

## Capabilities

### New Capabilities

- `mux-control-plane`: local mux transport, backend-neutral primitives, request records and explicit capabilities.

### Modified Capabilities

- `lifecycle-actions`: represent control-plane outcomes without changing existing lifecycle eligibility or owner routing.

## Impact

`src/control_plane/` holds the server, codec, request store, backend interface and client. Herdr decoding/transport remains adapter-owned. `src/main.rs` gains the daemon command and runtime selection; tests exercise the real socket boundary and fixture parity. The existing Nix package continues to ship one binary.

Agent/session policy, lead recovery, resume-command derivation, a publisher registry, topology, automatic restart and a production tmux backend are deferred. This change establishes the mux seam they can use; it does not claim to implement them.
