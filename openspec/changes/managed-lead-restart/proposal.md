# Proposal

> Unimplemented restart execution is superseded by
> `../daemon-managed-lifecycle/`. The shipped exact-parent/session spawn slice
> and its verification remain valid; do not apply the older Herdr-readiness or
> session-as-logical-subject plan in parallel.

## Why

Radar cannot yet safely restart a managed lead or standalone session: restart is limited to eligible idle workers, while the daemon has no launch/reattach operation. As Radar takes ownership of lifecycle orchestration, it should restart the lead without needlessly restarting live children, preserving the logical session and runtime tree while replacing only the lead's live process binding.

## What Changes

- Add an explicitly confirmed Radar TUI action to restart an eligible managed lead/standalone runtime subject through Radar's existing mux/runtime-provider seam, with durable operation outcomes and no retry after uncertain dispatch.
- Keep restart internal to Radar and available only from the TUI in this change; do not add a public daemon protocol method, external API, CLI command or Herdsman restart action.
- Treat the Pi session UUID as the stable logical identity for parent/child relationships, separate from each attached live process incarnation and mux location. A successful restart reattaches the replacement process incarnation to the same session identity; it does not create a new logical parent or rewrite child links. Session UUID identifies the logical session, not a unique live process.
- Preserve daemon-spawned child processes, panes, registrations and topology during lead restart. Reconcile their current process and mux evidence after restart; do not restart children in this change.
- Require an explicit saved-session launch specification; request graceful termination of the exact old process via pidfd and launch only after verified exit. Never escalate to SIGKILL. Missing, ambiguous or conflicting evidence refuses or remains unknown rather than guessing.
- Keep assignment eligibility and assignment state with Herdsman. The daemon's physical restart eligibility for a managed lead derives from its own launch/subject provenance; the operator explicitly requests the restart. Restarting does not imply assignment completion, answer pending questions, or rewrite execution/assignment publications.
- Defer child restart policy, general recovery of arbitrary/unmanaged sessions, automatic startup restart, batch restart, and a new physical backend.

## Capabilities

### New Capabilities
- `managed-lead-restart`: Daemon-owned restart of a registered lead/standalone logical session, replacing its live process binding while preserving live managed children and durable runtime topology.

### Modified Capabilities
- `managed-child-spawn`: Require an exact parent process registration plus an explicitly supplied session UUID, verify they match through fresh context, and persist the session UUID as logical parent identity while retaining process provenance.
- `lifecycle-actions`: Permit a confirmed managed lead/standalone restart through the daemon's explicit lifecycle operation while retaining exact-target confirmation and refusing busy, uncertain or unsupported targets.

## Impact

- Internal TUI-to-runtime operation and durable restart record, using the existing backend-neutral runtime-provider/mux seam; no public daemon protocol operation. Coordinate with session-context registry, private launch specifications, process verifier and mux backend.
- Registry/data-model changes to bind stable Pi session identity to one or more live process incarnations and parent topology; existing worker restart and owner-routed controls remain compatible.
- Consumer contract/docs and disposable-daemon plus gated live-Herdr tests. Herdsman need not re-report the replacement process or grant daemon-owned restart eligibility; the daemon owns launch provenance, physical execution and binding. The operator caller trust boundary remains to be documented.
- No change to process-level state authority: Pi remains execution-evidence publisher, Herdsman remains assignment authority, and the daemon reports physical lifecycle outcomes.
