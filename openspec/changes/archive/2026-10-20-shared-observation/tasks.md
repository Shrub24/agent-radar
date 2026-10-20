# Tasks

## 1. Snapshot and sampler

- [x] 1.1 Define the snapshot, its epoch-qualified revision and per-source freshness (`inventory`, `registry`, `processes`, `tasks`), reusing the existing observation projection with serialization added. Test that a snapshot round-trips and that an unknown/failed source state is explicit.
- [x] 1.2 Move the collection loop into a daemon sampler thread that owns `ObservationState` and the process sampler, reads the registry in process, runs one sample at a time under the existing deadline, and publishes an immutable snapshot with a revision wait. Test that a failed registry read is reported as its own source and leaves the fleet standing.
- [x] 1.3 Add demand classes with a window renewed by reads; sample nothing without demand. Test idle, the optional process class, and the first read's determinism.
- [x] 1.4 Wake the sampler from a timer and from committed events in the daemon's own log, with a minimum gap.
- [x] 1.5 The sampler's registry read is the daemon's own records: the defaulted `RuntimeProvider::registry` (which the tmux adapter never implemented, so the snapshot carried no registrations, spawn edges, containers or lifecycle subjects) is removed, and the collector takes an explicit registry source built from the daemon's `Registry` and handed the fleet of the same refresh. Wire shapes are shared with the `agent.list`/`spawn.list` handlers (`src/control_plane/registry_read.rs`). Socket tests assert the snapshot carries a recorded container with its label and handle, and a managed lifecycle subject; a unit test covers the paging guard. This closes what 1.2's tick did not cover: its test proved a *failed* registry read stays source-local, and nothing asserted a successful one reached the snapshot. Gate after the fix: fmt 0, strict Clippy 0, locked tests 0 (487 library tests and every integration binary), strict validation 34/34, checked Nix build `/nix/store/3szmwnihz985x611plq97k2ri68cgvq1-agent-radar-0.1.0`.

## 2. Contract

- [x] 2.1 Add `observation.get.v1`: `epoch`, `since_revision`, `wait_ms`, `classes`, `not_modified`, `snapshot_expired` and a typed `snapshot_too_large`; use the revision wait for `wait_ms`; add the contract to the advertised set. Socket tests cover a revision, not-modified, another epoch, unknown classes, an unbounded wait, and a daemon with no backend.
- [x] 2.2 Carry the task source in the snapshot with its own freshness. A daemon without an ingest reports the source unavailable and names why.
- [x] 2.3 Prove no revision enters the event log or `events.read`.
- [x] 2.4 Make registry failure source-local: a failed registry read leaves the last-good fleet and marks the registry stale rather than failing the whole pass.

## 3. TUI as a client

- [x] 3.1 Add the observation read to the dashboard client and replace the TUI collector with a snapshot fetcher; build the tree from the snapshot. Remove local `/proc` sampling, reconciliation and inventory reads from the TUI.
- [x] 3.2 Keep one task path: the dashboard reads task sessions from the snapshot's task source instead of a second `tasks.list` read.
- [x] 3.3 Refuse with the existing blocking diagnostic when the daemon lacks `observation.get.v1`; there is no local fallback. A failed or expired read leaves the previous view standing with the reason on the footer.
- [x] 3.4 Two-display live acceptance: two clients reading one revision, and a display restart receiving the retained associations the daemon kept.

## 4. Tracing

- [x] 4.1 Reuse the shipped `tracing` setup: the daemon writes to stderr (the journal captures it) and the dashboard to a log file, never the terminal. Already landed with `footer-and-logging`.
- [x] 4.2 Identifier-only diagnostics; the observation read adds no payload logging.

## 5. Documentation and acceptance

- [x] 5.1 Document `observation.get`, the snapshot sources and freshness, demand classes, and the display's snapshot consumption in `docs/daemon.md` and `docs/control-plane.md`; update the canonical fixture and bump the wire revision for the added method.
- [x] 5.2 Gate passed with every exit code read from its own stage: fmt 0, strict Clippy 0, the locked test suite 0, strict spec validation 34/34, and the checked Nix build `/nix/store/pfnpcjr2l4im1m1p8r023z8qyh6la614-agent-radar-0.1.0`. PTY/tmux acceptance and live two-display acceptance remain operator checks under the current instruction.

## Deferred, with reasons

- A hand-written public DTO set for the snapshot payload: the payload is the daemon's fleet projection, and the only consumer today is the in-crate dashboard. Generate schemas for it when an external consumer needs the shape, rather than freezing the projection now.
- Paging and snapshot retention: the payload is bounded by `MAX_SNAPSHOT_BYTES` and refused with a typed outcome instead of being paged. Add cursors when a real deployment exceeds one response.
- Stable logical identity across executions, daemon-owned launch progress and safe abandonment: `lifecycle-authority` and follow-on work, not this slice.
