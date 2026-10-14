# Proposal

## Why

Every Radar display collects for itself: inventory, `/proc` samples with CPU history, foreground evidence, the registry read (which re-verifies processes on each call) and the continuity memory that reconciles them. One TUI is tolerable. A pinned sidebar beside a full TUI would repeat all of it, each with its own view of the fleet, and a restart of the TUI loses continuity the daemon could keep. Observation should be daemon-owned so displays are optional, cheap and consistent.

## What Changes

- The daemon runs one sampler. It owns the reconciler (`ObservationState`), the process sampler and the registry projection, reads the registry in process rather than over the socket, and publishes an immutable snapshot with a revision, a collection time and per-source freshness (inventory, registry, processes, tasks).
- Add `observation.get.v1`: read the current snapshot, or return `not_modified` for a known revision, optionally waiting a bounded time for a newer one. Pages are tied to one revision; a page request for a revision the daemon no longer holds returns a typed `snapshot_expired` and never mixes revisions.
- Sample on demand. A request carries the data classes it wants (for example the process table) and the daemon samples a class only while some reader recently asked for it, and idles with no readers.
- A sample is requested by a timer, by committed events in the daemon's own log, and later by tmux observation (`tmux-control-mode`); requests coalesce and one sample is in flight at a time. The poll interval is not part of the contract.
- The TUI becomes a fetch-and-render client. It keeps the tree build, folding, selection and filtering locally, and stops sampling `/proc`, reconciling or reading the inventory itself. The `Collector` in the TUI is replaced by a snapshot fetcher.
- Task state from `consolidate-daemon-ingest` joins the snapshot as its own source with its own freshness.
- Add structured diagnostics with `tracing`: the daemon writes to stderr (the journal captures it) and the TUI to a log file, never the terminal. Spans carry request, sampler-pass and operation identifiers only, never launch values or tokens.
- **BREAKING**: the TUI no longer works against a daemon that does not serve `observation.get.v1`.

Non-goals: tmux control mode and hooks, deltas between revisions, durable observation history, client-local navigation and the sidebar, per-client focus evidence in the snapshot, any new event in the durable log, specta, and async domain code.

## Capabilities

### New Capabilities

- `shared-observation`: the daemon-owned sampled snapshot, its revisions, freshness, paging and demand-driven sampling.

### Modified Capabilities

- `runtime-observation`: collection is the daemon's, and retained associations live in the daemon's memory instead of the display's.

## Impact

- `src/collector.rs`, `src/observation.rs`, `src/registry_observation.rs` and `src/procfs.rs` run in the daemon; the TUI's `main.rs` loop fetches snapshots.
- New daemon module for the sampler; snapshot types gain serialization; `DaemonRuntime` gains an observe call; `KNOWN_CAPABILITIES` gains `observation.get.v1`.
- New dependencies `tracing` and `tracing-subscriber`.
- Depends on `consolidate-daemon-ingest` for the transport wait and the task source.
