# Proposal

## Why

Every Radar display collects for itself: inventory, `/proc` samples with CPU history, foreground evidence, the registry read (which re-verifies processes on each call) and the continuity memory that reconciles them. One TUI is tolerable. A pinned sidebar beside a full TUI would repeat all of it, each with its own view of the fleet, and a restart of the TUI loses continuity the daemon could keep. Observation should be daemon-owned so displays are optional, cheap and consistent.

## What Changes

- The daemon runs one sampler. It owns the reconciler (`ObservationState`), the process sampler and the registry projection, reads the registry in process rather than over the socket, and publishes an immutable snapshot with an epoch-qualified revision, a collection time and per-source freshness (inventory, registry, processes, tasks). A failed registry read is reported as its own source and leaves the fleet standing.
- Add `observation.get.v1`: read the current snapshot, or return `not_modified` for a known revision, optionally waiting a bounded time for a newer one. A revision or epoch the daemon no longer holds returns a typed `snapshot_expired`; a snapshot too large for one response returns a typed `snapshot_too_large` rather than a truncated line. A revision is meaningful only within its epoch.
- Sample on demand. A request carries the data classes it wants; the fleet, registry and task classes are always collected, and the process table only while some reader recently asked for it. The daemon idles with no readers, and an unknown class name is refused rather than ignored.
- A sample is requested by a timer, by committed events in the daemon's own log, and later by tmux observation (`tmux-control-mode`); requests coalesce and one sample is in flight at a time. The poll interval is not part of the contract.
- The TUI becomes a fetch-and-render client. It keeps the tree build, folding, selection and filtering locally, and stops sampling `/proc`, reconciling or reading the inventory itself. The `Collector` in the TUI is replaced by a snapshot fetcher, and task sessions arrive as one source of the same revision instead of a second read.
- Task state from `consolidate-daemon-ingest` joins the snapshot as its own source with its own freshness.
- Diagnostics reuse the shipped `tracing` setup: the daemon writes to stderr (the journal captures it) and the dashboard to a log file, never the terminal. Identifiers only, never launch values or tokens.
- **BREAKING**: the TUI no longer works against a daemon that does not serve `observation.get.v1`.

Non-goals: tmux control mode and hooks, deltas between revisions, snapshot paging and retention, durable observation history, a hand-written public DTO set for the snapshot payload, client-local navigation and the sidebar, per-client focus evidence in the snapshot, any new event in the durable log, specta, and async domain code.

## Capabilities

### New Capabilities

- `shared-observation`: the daemon-owned sampled snapshot, its revisions, freshness, paging and demand-driven sampling.

### Modified Capabilities

- `runtime-observation`: collection is the daemon's, and retained associations live in the daemon's memory instead of the display's.

## Impact

- `src/collector.rs`, `src/observation.rs`, `src/registry_observation.rs` and `src/procfs.rs` run in the daemon; the TUI's `main.rs` loop fetches snapshots.
- New daemon module `src/control_plane/observation.rs` for the sampler and the published snapshot; `src/snapshot_fetch.rs` is the dashboard's fetcher. `ObservationState` gains serialization; the client gains the observation read; the advertised contract set gains `observation.get.v1` and the wire revision increases.
- `tracing` and `tracing-subscriber` already landed with `footer-and-logging`.
- Depends on `consolidate-daemon-ingest` for the transport wait and the task source.
