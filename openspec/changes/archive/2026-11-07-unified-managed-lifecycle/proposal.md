# Proposal

## Why

Managed launches, admitted executions and container deletion currently discover targets and terminate or remove them through different implementations. Execution placement and completion must not depend on a spawn edge: an agent started by `L` and one started with `radar run --agent` need the same lifecycle, with explicit terminal-placement differences.

## What Changes

- Establish one claim-addressed execution model with verified pane placement, managed scope and completion facts; keep subject continuity, spawn parentage and terminal-resource identity separate.
- Resolve a displayed row to a domain close target in the daemon. One TUI Close command serves agent, pane and container rows; explicit vanished targets never fall back to selection or focus.
- Compose one execution-termination implementation with pane removal. Container close recursively handles logical children with a shared graceful deadline, verified scope escalation and enclosing-resource removal as fallback. Conflicting Radar mutations are refused while the branch closes; no atomic topology transaction is introduced.
- Route configured `L`, direct admission and spawned launches through one supervised managed-execution chain after their different placement/admission steps. Ordinary shells remain unmanaged.
- Record completion at execution exit and before intentional removal, not on reads. Missing code never blocks lifecycle progression or requested cleanup.
- **BREAKING:** `spawn.close` closes the managed execution and its pane through the common workflow rather than performing an independent mux-only close. Existing complete/cancel labels do not grant assignment authority.
- **BREAKING:** `D` consistently closes the selected agent's execution and pane on one invocation. Restart preserves subject identity and creates a fresh claim using the common termination and launch primitives.
- Remove replaced execution/cleanup branches, read-time capture and legacy adapters. Update affected contracts, generated SDK types and protocol revision; unsupported persisted shapes are refused with an explicit upgrade procedure, not silently migrated.
- Audit adjacent launch/stop/revoke/reload entrypoints for concrete remaining divergence; include fixes that are necessary for the common workflow, list unrelated candidates without expanding scope.

## Capabilities

### New Capabilities

- `managed-execution`: claim-scoped placement, supervision, completion and shared graceful-to-force termination.
- `resource-closure`: domain-target resolution, recursive closure, bounded escalation and truthful aggregate results.

### Modified Capabilities

- `managed-launch`: unified admission/supervision and subject-preserving restart with explicit placement.
- `managed-child-stop`: authorize child closure through spawn relationships, then invoke common managed close; status is not a cleanup prerequisite.
- `managed-child-spawn`: retain spawn provenance while projecting claim-owned completion and allowing close to record verified execution termination.
- `lifecycle-actions`: one daemon-owned close route, current `D` behaviour and container composition instead of retired owner-file/mux-only routing.

## Impact

Rust lifecycle records/events, `managed_scope.rs`, launch wrappers, server launch/subject/child/container handlers, `ui_session/targets.rs`, observation placement projections, public DTOs and pi-radar close consumers. No new runtime, all-shell scopes, workspace recovery, lead inheritance, assignment semantics, general operation framework or full crash-resume engine.

This follows the implemented `tmux-control-mode` cutover. Several main specs still describe retired behaviour; design.md identifies predecessor overlays and archive constraints rather than treating those descriptions as current code.
