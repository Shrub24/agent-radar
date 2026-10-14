# Design

## Context

`src/collector.rs` runs one refresh at a time on a worker thread and applies it to a caller-owned `ObservationState`, which is single-owner and lock-free. A refresh reads the inventory, then foreground evidence for the reconciler's continuity candidates, composes local `/proc` facts with interval CPU history, measures assignment age against one clock read, and reads the registry (which re-verifies processes on every call). The TUI's own loop ticks it once a second. The reconciler keeps in-memory continuity and the process sampler keeps CPU history, so both must live in exactly one place for views to agree.

The daemon already serves the registry, the event log and a transport with waits (`consolidate-daemon-ingest`). The runtime seam, `RuntimeProvider`, is backend-agnostic, so the sampler works for tmux and for the explicit Herdr backend alike.

## Decisions

### 1. The snapshot is reconciled output, not raw inventory

It holds what the display needs after reconciliation: the fleet observation with retained associations, per-pane foreground evidence, process samples, the registry projection (agents, spawn topology, containers, lifecycle subjects), task state and per-source freshness. Sending raw inventory would make each display reconcile on its own and lose agreement.

The TUI keeps `FleetTree::build`, folding, selection and filtering, since those are per view. The snapshot's rows are the existing normalized model types, serialized; types that lack serialization gain it, and the wire shape is the versioned contract `observation.get.v1`.

### 2. The sampler is a plain thread

It owns `ObservationState` and the process sampler, runs one sample at a time under the existing 30 s deadline with cancellation, and publishes an `Arc<Snapshot>` by replacing it under a short lock and bumping a `watch` revision that the transport's waiters observe. It reads the registry through the in-process `Registry`, not the socket. No async code is added outside the transport.

### 3. Demand is a lease carried by reads

A read names the data classes it wants and implicitly renews their demand for a short window (a few seconds). The sampler samples the union of live demand and idles when it is empty. The TUI already reads about once a second, so no separate subscribe call exists in this change.

### 4. Wakes are requests, not facts

A sample is requested by a timer, by committed events from the daemon's own log (an in-process subscriber), and later by tmux observation. Requests coalesce, with a floor on the gap between samples. The timer cadence is an implementation setting and not in the contract, so a faster source can replace it without a protocol change.

### 5. Reads are by revision, with bounded pages

`observation.get` takes `since_revision`, `wait_ms`, `classes` and an `after` page cursor that embeds its revision. The daemon holds the current and previous revision, so a reader paging across one publish usually finishes; a cursor for any older revision returns `snapshot_expired` and the reader restarts. Full snapshots only: no deltas yet.

### 6. Nothing per client in the snapshot

Pane foreground evidence is per pane and shared. Which window or pane a client shows is per client and is not in the snapshot; the sidebar change defines client targeting and local view state. This keeps every reader's snapshot identical for the same classes.

### 7. Observation is not a durable event

Revisions never enter the event log or `events.read`. The log stays an account of committed facts; a reader that wants a change wake waits on the revision.

### 8. Tracing

Add `tracing` and `tracing-subscriber` (formatting and an environment filter, variable `RADAR_LOG`). The daemon writes to stderr, which the journal captures. The TUI writes to a file under the state directory and never the terminal. Spans: one per request (identifier, method, lane), one per sampler pass (revision, per-source outcome, duration) and the existing lifecycle operation and subject identifiers where those are already in scope. Fields are identifiers only; launch values, tokens and bodies are never fields. User-facing CLI messages stay plain output. The event log remains authoritative; tracing is diagnosis.

### 9. The TUI has no fallback

The TUI requires `observation.get.v1`; a daemon that lacks it is a blocking diagnostic, as for any missing required contract.

## Risks

- **Daemon stalls.** The sampler's `/proc` and runtime work moves into the daemon. It runs on its own thread with deadlines and cancellation and never holds the `Core` lock across a sample.
- **Snapshot size.** Many panes with process tables approach the 1 MiB frame. Paging by size is built in; process tables are a separate class so a display that does not ask does not pay.
- **Loss of the TUI's local view.** Anything the TUI derived from raw collection (assignment age, foreground evidence) must be in the snapshot or the TUI regresses. A parity test compares the rendered tree from the old path and the snapshot path on the same fixture before the old path is removed.
- **Cost moves, it does not vanish.** Registry process verification still runs per sample; sampling once instead of per display is the remedy, and the interval floor bounds it.
- **tmux timeliness.** Until `tmux-control-mode`, wakes come from the timer and the daemon's own events; latency is as today.

## Open choices

- The demand window length and the minimum gap between samples.
- Whether to keep a third revision for slow pagers; start with two.
- Whether the sampler's registry projection should stay a recomputation or be maintained from reducers; start with recomputation.
