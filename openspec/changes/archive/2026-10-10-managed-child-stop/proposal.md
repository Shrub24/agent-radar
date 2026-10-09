# Proposal

## Why

The daemon can create managed children and retain the exact registry identity and pane location for each child it created. Ending one still requires Herdsman to call Herdr directly, so mux ownership is split where identity matters. A daemon operation for a child it spawned would complete the first practical lifecycle slice while preserving Herdsman's assignment authority.

## What Changes

- Add a durable, idempotent `child.close` operation targeted only at an exact daemon-recorded spawn edge and its bound child identity.
- Require positive current evidence that the recorded child is still the process in the recorded pane before dispatch; refuse if identity is missing, changed, or ambiguous.
- Record pane-close outcome separately from process-exit evidence. Confirmed pane close does not claim process exit; a missing pane is not itself proof of process exit.
- Accept explicit intent (`complete` or `cancel`) for audit/display, but do not write assignment completion or rewrite agent-advertised execution status.
- Return `unknown` after a possibly dispatched close with no confirmation; never retry automatically or fall back to Herdr.

## Capabilities

### New Capabilities
- `managed-child-stop`: safely close the mux pane for a daemon-spawned, identity-bound child, with durable outcomes. `child.close` readiness uses the existing `observe` and `close` backend capabilities; it is not separately advertised by `ping`.

### Modified Capabilities
- `managed-child-spawn`: clarify that close effects remain separate from immutable creation and binding facts; keep the existing spawn wire format unchanged.
- `mux-control-plane`: expose a distinct backend-neutral operation to close a daemon-managed pane without weakening unmanaged-close safeguards.

## Impact

This affects the daemon protocol and a separate private close-request store, with disposable-daemon and Herdr integration tests. The existing runtime and Herdr adapter provide the foreground process evidence and pane-close primitive; the spawn edge and registration formats remain unchanged. Herdsman remains the assignment authority and caller; physical close never completes an assignment.
