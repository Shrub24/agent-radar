# Proposal

## Why

The TUI currently coordinates separate action and publication connections, blocking request/reply exchanges and overlapping attachment cleanup. One duplex connection per attachment can preserve independent progress while making connection lifetime, boundedness and cancellation explicit; the current global connection cap also lets long-lived UI traffic exhaust ordinary connection capacity.

## What Changes

- **BREAKING (TUI protocol only):** replace action/subscription `ui.open` roles and the attach-then-subscribe exchange with one attachment opener and correlated binary replies plus publications on one socket. Remove the replaced paths rather than retain permanent dual modes.
- Establish attachment and publication receiver in one actor operation; become ready after coherent baseline adoption. Scope cancellation and cleanup to a connection-owned attachment lease.
- Separate persistent reading, ordered action handling and fair writing. Allow publications and bounded content recovery to progress while an action reply is pending; keep one action-response slot until transmission or connection termination.
- Bridge the existing synchronous actor with async reply receivers, distinguishing rejection before enqueue from uncertainty after enqueue.
- Retain optimistic navigation, ordered input and explicit-row target capture. Make configured Launch follow the same target rule. A gate change fences stale input and reconciles speculation to coherent actor state.
- Run client tasks on the existing I/O-enabled TUI runtime under an async outer lifecycle, with explicit shutdown and nonblocking drop cancellation. Replace timed demand synchronization with desired-value notification.
- Separate opening and established UI/ordinary connection budgets, with modest validated presenter headroom. Preserve handler-lane limits independently.
- Preserve deterministic MessagePack, XXH3-128, segmented immutable payloads, bounded current-only adoption and existing non-TUI protocols.

## Capabilities

### New Capabilities

None. Reuse the capabilities established by the predecessor changes.

### Modified Capabilities

- `daemon-transport`: duplex UI framing, independent I/O progress, class-based connection admission and distinct quiet/partial-frame deadlines.
- `ui-session`: atomic attachment establishment, lease validity, optimistic navigation reconciliation and explicit reply uncertainty.
- `ui-interactions`: uniform explicit-row targeting, including configured Launch.
- `tui-publications`: transmitted-head-relative coalescing and bounded reliable replies alongside latest-state publications.

`ui-session`, `ui-interactions` and `tui-publications` are introduced by pending predecessors, not yet main specs. Apply/sync `shared-ui-session`, then `unified-tui-publications`, then this delta; preserve the existing sidebar prerequisite. Missing baseline requirements block archival, not planning or implementation on this branch.

## Impact

Primary seams: `src/control_plane/transport/`, `server/ui.rs`, `server/ui_binary.rs`, `src/ui_session/{actor,client}.rs`, public UI DTOs, shared framing helpers in `src/content.rs`, both TUI entry paths in `src/main.rs`, and configured-launch target submission. Update protocol and acceptance docs and focused socket tests.

No migration of pi-radar, domain RPCs, observation reads, event feeds, ingest, presence or compatibility codecs. Ordinary connection admission changes deliberately, but their payload contracts and handler-lane reservations remain intact. No new runtime, worker pool, generic RPC framework, adaptive payload thresholds, chunking, navigation debounce, row-ID redesign or tmux/service restart coupling. TUI compatibility declaration and fixtures must match the new opener; no SDK observation-schema churn is intended.
