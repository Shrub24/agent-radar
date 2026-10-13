# Event foundation verification

## Scope

Tasks 2A.1–2A.4 are implemented. This is not acceptance of the whole daemon-managed-lifecycle change.

The managed-subject and lifecycle-operation authority is a typed, versioned, daemon-scoped sequence log in `src/control_plane/lifecycle/events.rs`. Subject reads and operation read/list results are projections of committed events. Replay performs no signaling or launch. Private launch recipes and exact request parameters remain in separate private files; event payloads omit credentials and request parameters. Existing subject/operation records are explicitly imported on their next transition, not presented as recovered historical events.

The bounded `events.read` journal is a subscriber view. `src/control_plane/store/delivery.rs` persists each delivery batch and its source checkpoint together. Lifecycle pointers carry optional `subject_id` and `lifecycle_event_id` references. Delivery catches up in batches of 256; `more` also reports undelivered source backlog. The typed source log is not truncated with the public pointer journal.

Stop, cancel and restart admission use the subject ID as their shared suppression key. Physical identity reads occur outside the registry commit fence, with the exact binding rechecked before committing an intent or observation. Duplicate in-flight reconciliation hints coalesce by subject. Observations distinguish running, absent, replaced and unavailable, labelled by read, sweep or tmux-hook source. An unchanged observation does not create another transition; `changed_at` names the evidence change, not a heartbeat. Unavailable evidence neither records exit nor sends a signal.

## Source gate

Background result `bg-1473` completed with exit 0:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked
openspec validate --all --strict
```

The gate passed 397 library tests, all integration suites (including 114 control-plane tests), and all 20 OpenSpec items. The ignored-SIGTERM socket test verifies that an uncertain stop retains the subject lane and a different cancel request cannot revoke or signal that launch.

After that gate, only the existing operation-replay test was strengthened to reopen a started operation before settlement. Its focused run passed, followed by formatting validation:

```sh
cargo test --locked control_plane::lifecycle::events::tests::operation_results_replay_while_private_request_inputs_stay_out_of_events -- --exact
cargo fmt --all -- --check
```

It proves the interrupted record remains started, derives unknown after expiry, reserves its request ID, and later reconstructs its durable result without exposing private parameters in the log.

## Other regression evidence

- Replay restores revoked subjects; a missing sequence refuses reconstruction.
- A failed append preserves the projection and freezes further mutations until recovery.
- Legacy subjects receive an explicit import before their next transition.
- Corrupted pointer delivery does not undo the lifecycle fact; after restoration, delivery catches up without duplicate source IDs, including after reopening.
- A 260-event source backlog is delivered as 256 then 4, with pending backlog reported.
- A live recovery sample records sweep provenance without signaling. Repeated unchanged samples add no transitions.
- Unavailable observation preserves the process claim and can subsequently recover to running.
- A blocked verifier does not block an independent subject's cancellation; a duplicate hint performs no second inspection.

## Unbound pane cleanup (task 1.4)

A cancel that revoked an unbound launch now closes the pane that launch was dispatched into, and records the result on the launch as its own fact. Cancel's effects read "pane kept", "pane cleaned up" or "pane cleanup unverified: <reason>"; refusal reasons name the missing evidence. The record is written once, for dispatched outcomes only: a confirmed close, a backend refusal and an unanswered dispatch are durable. An eligibility refusal dispatches nothing, so it records nothing and a later admitted cancel re-decides it.

Fences, reusing the managed child-close evidence path: the edge must be current, must name the serving backend and the instance that created the pane, and the process that served the create must still be serving. The pane is closed only when the launch's exit is already recorded, or when the edge records a pane-command exit and a fresh bounded read still reports it exited. A bound launch's pane stays with spawn.close, and a subject with no spawn edge has no pane of the daemon's to remove.

Live consequence for consumers: after an unbound cancel, the launch's pane is gone and the launch record names that cleanup; a cancel whose exit or signal was uncertain keeps the pane and leaves the target suppressed, so a retry is refused as in-flight rather than dispatching a second cleanup.

## Not verified or implemented by this slice

Execution, assignment and context publishers still use their existing fact records and pointer notifications; they are not yet reducers of this lifecycle log. Comprehensive pane/session observations and retention/compaction policy also remain outside this foundation.

The remaining section-3 restart acceptance, section-4 TUI extraction and section-5 consumer/integrated acceptance remain open. No new integrated Pi/live smoke or checked Nix gate is claimed here. The user owns live smoke verification. Nothing has been committed, pushed or archived.
