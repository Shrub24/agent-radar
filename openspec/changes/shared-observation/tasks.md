# Tasks

## 1. Snapshot and sampler

- [ ] 1.1 Define the snapshot, its revision and per-source freshness, reusing the existing model types with serialization added where missing. Test that a snapshot round-trips and that unknown source states are explicit.
- [ ] 1.2 Move the collection loop into a daemon sampler thread that owns `ObservationState` and the process sampler, reads the registry in process, runs one sample at a time under the existing deadline, and publishes an `Arc<Snapshot>` and a revision `watch`. Test coalescing, the deadline, and that a stalled runtime call does not delay a control request.
- [ ] 1.3 Add demand classes with a lease renewed by reads; sample nothing without demand. Test idle, one class requested, and expiry.
- [ ] 1.4 Wake the sampler from a timer and from committed events in the daemon's own log, with a minimum gap. Test a burst of events producing one further sample.

## 2. Contract

- [ ] 2.1 Add `observation.get.v1`: `since_revision`, `wait_ms`, `classes`, page cursor embedding its revision, `not_modified` and `snapshot_expired`. Use the transport wait for `wait_ms`. Test waiting, not-modified, paging across a publish, and expiry; add the contract to `KNOWN_CAPABILITIES`.
- [ ] 2.2 Add the task source from `consolidate-daemon-ingest` to the snapshot with its own freshness. Test a connected, disconnected and unavailable bus.
- [ ] 2.3 Prove no revision enters the event log or `events.read`.

## 3. TUI as a client

- [ ] 3.1 Add an observe call to the dashboard client and replace the TUI collector with a snapshot fetcher; build the tree from snapshot rows. Remove local `/proc` sampling, reconciliation and inventory reads from the TUI.
- [ ] 3.2 Compare the rendered tree from the old collection path and the snapshot path on the same fixtures (assignment age, foreground evidence, retained rows, processes, tasks) and then delete the old path.
- [ ] 3.3 Refuse with the existing blocking diagnostic when the daemon lacks `observation.get.v1`. Test the diagnostic and a daemon that answers with a stale source.
- [ ] 3.4 Test two clients reading one snapshot and a display restart receiving retained associations.

## 4. Tracing

- [ ] 4.1 Add `tracing` and `tracing-subscriber`; initialize the daemon to stderr and the TUI to a log file under the state directory, filtered by `RADAR_LOG`; replace the daemon's `eprintln!` diagnostics, not user-facing CLI messages.
- [ ] 4.2 Add request and sampler-pass spans with identifier-only fields. Test that a launch with environment values logs none of them and that the TUI writes nothing to the terminal.

## 5. Documentation and acceptance

- [ ] 5.1 Document `observation.get.v1`, the snapshot sources and freshness, demand classes, and logging in `docs/daemon.md` and `docs/control-plane.md`; move the item from `ideas.md` to done.
- [ ] 5.2 Run fmt, strict Clippy, the locked test suite, both PTY smokes, tmux acceptance, the publisher fixture, strict spec validation and the checked Nix build; record real exit codes in `verification.md`. Live acceptance with two displays is the operator's.
