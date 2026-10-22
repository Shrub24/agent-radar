# Proposal

## Why

Radar can launch and bind children, but cannot track and stop an unregistered launch or coordinate a verified replacement. Managed controls also still depend on TUI-owned Herdsman file requests; lifecycle authority should reside in the daemon, independently of any view.

## What Changes

- Add a daemon-owned managed subject composed of independently identified process, Pi-session and pane bindings. Keep registration `agent_id` incarnation-scoped and preserve existing spawn history.
- Track daemon launches before registration, using verified Linux birth identity and pidfds. Use generation-qualified tmux notifications to trigger physical reconciliation, not as process-exit proof.
- Coordinate bootstrap cancellation, exact-process SIGTERM, verified exit, optional pane cleanup and operator-confirmed restart through one durable lifecycle module. Each effect retains its own certainty.
- Keep logical subjects and child relationships stable across verified replacement; never infer a successor from session UUID or pane appearance.
- Make typed durable lifecycle events the transition authority, with deterministic replay projections and committed-event delivery. Preserve separate signal, observation, intent and result types; keep launch secrets outside event payloads. Use daemon-wide FIFO ordering, subject-serialized transitions and bounded concurrent effects.
- Move operation admission, monitoring, recovery and lifecycle projection entirely into the daemon. The TUI retains confirmation, selection and presentation only; remove its managed-operation file requester/polling path after cutover.
- Add capability-gated protocol-v2 lifecycle reads and operations. Cancellation/stop support daemon consumers; restart is an operator-facing TUI feature, not a Herdsman restart integration or CLI feature. The socket remains same-UID trust.
- Use tmux direct-argv launch for the first verified lifecycle backend. Herdr remains usable for existing operations, but cannot advertise unsupported new lifecycle controls.
- Supersede the unimplemented execution plan in `managed-lead-restart`; retain its shipped exact-parent/session spawn prerequisite and verification history.

Non-goals: automatic or batch restart, child restart cascades, arbitrary process adoption, SIGKILL/process-group killing, assignment/result management, shared asynchronous work, messaging, full fleet-collector relocation, new storage engines, or flexible spawn placement.

## Capabilities

### New Capabilities

- `managed-agent-lifecycle`: daemon-managed subjects, process tracking, cancellation, stop, operator restart, recovery and source-labelled lifecycle projection.

### Modified Capabilities

- `managed-child-spawn`: attach new launches to managed subjects and capture physical process evidence independently of registration; make bootstrap binding revocable and race-fenced.
- `lifecycle-actions`: confirmed managed controls use daemon records and outcomes rather than TUI-owned Herdsman file transport.

## Impact

- Daemon registry/store/server, runtime adapter and client in `src/control_plane/` and `src/runtime.rs`; Linux identity support in `src/procfs.rs`; a daemon lifecycle module and bounded process monitor.
- Tmux launch/notification integration in `src/tmux.rs`; remove managed-operation orchestration from `src/app.rs` and the Radar requester in `src/control.rs` once daemon routing is accepted.
- Consumer documentation, canonical fixtures, PTY tests and real disposable tmux/Pi lifecycle acceptance. Existing `agent.*`, `spawn.*` and `mux.*` semantics remain available; new lifecycle features are explicitly negotiated.
- Herdsman keeps assignment, mailbox, readiness and result authority. Its fixed-pin cutover is independent; abandonment adoption follows the new verified contract.
