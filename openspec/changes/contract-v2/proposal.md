# Proposal

## Why

The daemon's wire contract grew change by change and mirrors Herdr-era habits: mux methods are flat (`observe`, `close`), registry methods are dotted, `child.close` sits in a `child` namespace of one method, capabilities mix bare words, `_v1` suffixes and one derived flag, the handshake reports `backend: "runtime"` whatever backend is wired, and `events.read` returns the journal tail as `cursor` even when the events it delivered are a bounded prefix, so a consumer that follows the cursor silently skips events. The only consumer, Herdsman, has not deployed and is adopting the contract now. Fixing names and semantics now costs one pin; fixing them after deployment costs a migration.

## What Changes

- **BREAKING** — Protocol version 2. One namespaced method scheme: `daemon.ping`, `mux.*` for physical operations and reads, `mux.request.get`/`mux.request.list` for durable request records, `agent.*`, `spawn.*` (including `spawn.close`), and `events.read`. A version-1 request is refused with a typed version error; there are no aliases.
- **BREAKING** — One capability scheme: `<namespace>.<name>` with a `.v<N>` suffix on versioned contracts, no derived or bare words. Backend capabilities are `mux.*`; daemon contracts are `registry.v1`, `events.v1`, `spawn.v1`, `spawn.close.v1`, `spawn.launch_context.v1` and `spawn.exit_observation.v1`.
- **BREAKING** — `events.read` returns `cursor` as the position after the last delivered event (the request's `after` when none), a separate `tail`, and `more`, so following `cursor` can never skip an event.
- The handshake reports the real backend identity (`herdr`, `tmux` or `none`) and the tmux server instance.
- Close of an exactly bound child whose pane is reported dead with a matching recorded exit is a defined, durable, idempotent `already_exited` result that closes the retained pane, not a refusal. A live occupant, a missing pane or an unmatched exit still refuse as today.
- Audit and align result keys, outcome words and error codes across the surface; record the table in the contract reference.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mux-control-plane`: method and capability namespace, handshake identity, event cursor semantics.
- `managed-child-stop`: dead-pane close result.

## Impact

Touches the protocol and method classification, capability constants and negotiation, the handshake, the events read, the close path, the daemon client, all fixtures and the validator, and every doc that names a method or capability (`docs/control-plane.md`, `docs/mux.md`, `docs/daemon.md`, `docs/agent-registration.md`). Behaviour of every operation other than the dead-pane close is unchanged. Herdsman pins the resulting commit once; the old names are not retained.
