# Herdsman adoption brief

## Recommendation

No Herdsman change is required just because Radar moved lifecycle state into its internal typed event log. Keep Herdsman's assignment, readiness, mailbox, result and supervision authority. Consume only Radar's documented daemon contracts; do not read its event-log files or reproduce its reducers.

The current published Radar `main` observed in this workspace is `4e62d6a1`. It includes daemon-managed lifecycle contracts and the TUI extraction. This session did not verify which Radar binary or commit Herdsman currently runs; `daemon.ping` reports protocol and capabilities, not the build commit. Record the deployed pin separately before relying on a contract.

## Existing integration

- Use protocol v2 and gate each method on the contract names returned by `daemon.ping`.
- Keep using `spawn.*`, `agent.*`, registry facts, and `events.read` as documented in `docs/control-plane.md` and `docs/agent-registration.md`.
- Treat `events.read` entries as invalidation pointers. Save the delivered cursor only after processing a page, drain `more` pages immediately, and re-read authoritative records before acting. Never consume Radar's private event log.
- Continue to treat assignment, readiness, and results as Herdsman-owned facts. Pane presence, event absence, `spawn.close`, and token revocation do not prove process exit or application readiness.

## Optional lifecycle integration

Only adopt this after deliberately selecting a Radar pin and checking its advertised contracts:

1. Read the optional `subject_id` from the durable spawn edge returned by `spawn.get/list`. If it is absent (for example, a legacy edge), do not invent one from a pane, PID, or Pi session.
2. Use `lifecycle.get/list` to read the public subject projection. Use `lifecycle.cancel` for a tracked launch that may not yet have registered; read uncertain outcomes back using the same durable operation ID.
3. For a bound child that must be closed, process cancellation and pane removal are separate effects: require `spawn.close.v1`, cancel and verify the exact process first, then call `spawn.close` against its recorded edge. If the contract is absent, refuse before signaling. Keep the process-exit and pane-cleanup outcomes distinct.
4. Keep enrollment and restart operator-oriented; do not add automatic lead restart or make Radar's lifecycle projection an assignment authority.

The public capability table and method semantics are in `docs/control-plane.md`; the spawn edge's optional subject binding is in `src/control_plane/registry/spawn.rs`. The end-to-end safety decisions are in `openspec/changes/daemon-managed-lifecycle/design.md`.

## Current verification boundary

The TUI extraction and muted rows for unbound/unjoined lifecycle subjects are now pushed. Focused and full automated gates passed, but a PTY-specific managed-action test and live Pi/Herdsman acceptance remain open. These do not change Herdsman's contract. This brief does not authorize a Herdsman-side pin change or migration.
