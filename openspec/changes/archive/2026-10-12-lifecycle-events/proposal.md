# Proposal

## Why

Consumers of the daemon, Herdsman first, must poll `agent.list`, `spawn.get` and `observe` to learn that a child registered, published, was closed or went stale. Polling is wasteful, misses ordering and gives no way to resume after a disconnect. Herdsman adoption lists lifecycle events as a remaining gap for Herdr removal.

## What Changes

- Add a daemon-owned, bounded, durable change journal. Each committed daemon fact appends one event with a monotonic sequence: agent registered, retired or became stale; execution, assignment or context published; spawn edge created, bound or effect recorded; child close outcome recorded; and, later, exit observed.
- Add `events.read {after, limit, wait_ms}`: returns events after a cursor, long-polling up to a bounded wait. Request/response framing, worker bounds and connection limits stay unchanged; no streaming mode.
- Events are pointers, not payloads: kind, subject ids and sequence, never environment values, tokens or request bodies. Consumers re-read the exact record. Missed or truncated history returns an explicit `cursor_expired` result with a current cursor so consumers resynchronise by a full read.
- Advertise an `events_v1` capability.
- Events report committed daemon facts only. They are never inferred exits, readiness or assignment outcomes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mux-control-plane`: event journal, `events.read`, retention and resynchronisation, privacy.

## Impact

Touches the registry and spawn/close commit paths, store retention, server dispatch, protocol docs and fixtures, and a disposable-daemon test. No new dependency. Physical backend event feeds (Herdr or tmux push) are out of scope; a backend-originated fact enters the journal only when the daemon observes and records it.
