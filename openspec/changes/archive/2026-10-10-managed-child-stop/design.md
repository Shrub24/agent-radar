# Design

## Context

See proposal.md. Managed spawn records an exact parent, child binding and created pane location. An audit of the existing evidence established two facts that shape this design:

- The edge-to-child link is exact: the child presents its private spawn token at registration and the edge binds that `(source, incarnation)`.
- Pane *presence* is checkable, but **no current evidence names the occupant's registry incarnation**: inventory carries session-scoped lineage (`pi_herdsman_session`), while registration identity is incarnation-scoped and several live attaches can share one session. Pane presence or session match alone would therefore be a weaker claim than the contract requires.

The runtime provider already closes panes, but the daemon intentionally refuses direct close for managed panes, and existing managed lifecycle controls route through the exact Herdsman owner.

## Goals / Non-Goals

**Goals:** provide one daemon operation to close only a child created and bound by this daemon; verify the exact bound child still occupies the recorded pane before any dispatch; preserve durable idempotency; report the mux close outcome without claiming process termination; leave assignment and Pi execution reports to their respective owners.

**Non-Goals:** killing a process by signal or PID, stopping unbound spawns, closing foreign panes, closing parent leads or standalone sessions, restart/resume, exit-event streaming, changing Herdsman assignment policy or moving assignment authority.

## Decisions

### Close the daemon-owned pane, do not promise a process stop

The request names the durable spawn edge, the bound child `(source, incarnation)` and an explicit `complete` or `cancel` intent. It never names an arbitrary pane id, so a caller cannot retarget the operation. Termination is not attempted and not implied: the result reports pane close only.

`complete`/`cancel` is an audit label. Herdsman decides and publishes assignment state; the daemon writes neither an assignment fact nor an execution status.

Rejected: an OS kill by PID, which bypasses mux ownership and can hit a recycled process; treating `pane.close` as proof the process exited; treating a missing pane as proof the process exited.

### Verify the occupant by birth identity, refusing when it cannot be established

Immediately before dispatch the daemon revalidates, from fresh evidence:

1. the recorded pane still exists in the recorded workspace/tab containment;
2. the pane's current foreground process PID resolves through `/proc` to a birth identity (`boot_id`, `start_ticks`) that equals the process claim held for the bound child's registration.

The compared value is the birth identity; the PID is only the lookup key. Any of the following refuses before dispatch, with no close effect and no accepted record: absent pane, containment mismatch, inconclusive foreground evidence, registration without a process claim, differing birth identity, or a registration whose liveness cannot be re-verified. Session lineage is never accepted as occupant proof, because the registry deliberately keeps several live attaches of one session as distinct incarnations.

This is strictly stronger than the pane-presence check the earlier draft implied, and it is the strongest gate the current evidence supports. Residual risk, accepted and declared: Herdr's foreground report and the daemon's `/proc` read are separate instants, so in principle a child could exit and an unrelated process reuse that PID and pane inside the window; the birth-identity comparison makes that mismatch refuse, and no close is dispatched on unavailable evidence.

### Prefer a producer signal when it exists

The cleaner long-term gate is an incarnation-scoped signal observable on the pane — the child publishing its own incarnation, or Herdsman publishing an incarnation-scoped pane token — making the comparison direct rather than reconstructed. That needs a publisher contract addition and is deferred, not silently substituted. The birth-identity route is used meanwhile and is replaced rather than weakened if such a signal lands.

### Keep the record separate from the immutable edge

`child.close` has its own private, durable, bounded, idempotent request record referencing the spawn edge, the bound child, the recorded pane and the intent. It never overwrites the edge's creation, location or binding facts, and never stores the private spawn token. Identical request id and content replays to the stored outcome with no second dispatch; the same id with different content refuses.

### No hidden retry or state synthesis

A possibly dispatched close with a lost answer stays unknown and is never retried, never rerouted to direct Herdr, and never reported as closed. Topology reads continue to reflect current evidence: a location that cannot be confirmed reads unresolved, and neither a pane close nor a pane disappearance is reported as process exit.

### Do not add a separate managed-close capability

`child.close` is a versioned registry operation whose readiness is determined by the backend's existing `observe` and `close` capabilities plus per-request foreground verification. The daemon does not add a separate `managed_close` capability to `ping`; clients check protocol compatibility and required backend capabilities, then handle explicit refusal/unknown outcomes. No supported backend may be selected for this method unless the verified path and durable outcomes are available.

## Risks / Trade-offs

- [Herdr pane close may leave the child process alive] → the contract claims pane close only and never synthesizes a process-exit fact.
- [PID reuse or a stale mux report] → birth-identity comparison plus fail-closed refusal; declared as residual risk rather than hidden.
- [A close succeeds but its answer is lost] → durable unknown, no retry, explicit reconciliation required.
- [Intent mistaken for assignment completion] → stored and displayed as intent only; documented owner responsibility.

## Migration Plan

1. Implement the verification path and dispatch for exact, verified targets; keep refusal otherwise.
2. Prove refusals and refusal-before-record, idempotent replay, and no process-exit claim in tests.
3. Verify with a disposable daemon and a disposable Herdr pane; never close live managed agents as a smoke test.
4. Herdsman may adopt `child.close` for daemon-created children after explicit integration; existing owner-routed managed close/restart stays for every other target.

## Open Questions

- Whether an incarnation-scoped pane signal will replace the birth-identity reconstruction. pi-extensions will consider one when adopting `child.close`, but not as a bearer token in general pane metadata: it must be unguessable, bound to the exact child `agent_id` and the pane-creation edge, and publishable only by the daemon or mux coordinator. Child self-report alone is insufficient because session switching, multiple attaches, reloads and stale metadata could misidentify an occupant. The fail-closed procfs check stays in the meantime; a valid coordinator signal would replace the reconstruction, not weaken it.
