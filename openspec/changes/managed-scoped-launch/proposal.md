# Proposal

## Why

Radar's managed lifecycle needs a boundary before execution: durable admission, resource-group placement and cancellation cannot be reconstructed by fast adoption. Establish a public managed launch and its systemd scope contract together, before adding another observation mechanism or background-job platform.

## What Changes

- Add `radar run -- COMMAND [ARG...]` for in-place managed execution in a verified Radar tmux pane. Ordinary terminals and headless execution are deferred. Keep placement separate: TUI `L`, restart and Herdsman spawn converge on the same scoped bootstrap after choosing their target and environment.
- Require a transient systemd user scope before application execution, inside a shallow per-execution slice. Use `systemd-run --user --scope` initially; preserve argv, cwd, environment and the existing PTY. Refuse required setup failure without an unscoped fallback.
- Keep Radar authoritative for launch operations, session subjects, verified execution bindings and outcomes; use systemd for physical membership, accounting and group signalling. Scope activation is not application readiness or an exit result.
- **BREAKING:** externally started Pi remains observed; presence or explicit enrollment no longer fabricates full managed-launch standing. Reconciliation restores established bindings, not equivalent startup guarantees. Offer an explicitly confirmed restart-through-Radar path when safe.
- Preserve session-bound subjects, child/inbox continuity, duplicate rejection and exact release from `unified-pi-registration`. Managed launches and recovery reuse that machinery; no new identity or session-marker protocol.
- Preserve graceful-only stop. Group signalling must not introduce automatic SIGKILL escalation. Independently scoped tasks and subagents are outside a main-scope stop.
- Update pi-radar standing/negotiation, caller-visible diagnostics, SDK consumers, packaging and operator documentation in the same release. No alias is installed automatically.

## Capabilities

### New Capabilities

- `managed-execution`: public in-place launch, pre-exec scope establishment, execution-group lifecycle, recovery and graceful-only control.

### Modified Capabilities

- `agent-registration`: managed grants require an admitted execution or recovery of an established binding; retain session identity and process reporting without adoption parity.
- `lifecycle-actions`: `L` uses the common scoped launch; external enrollment is replaced by an explicit safe restart-through-Radar transition rather than promotion in place.

## Impact

`src/cli.rs`, main dispatch, `control_plane/lifecycle.rs`, presence handling in `server.rs` and `registry/presence.rs`, tmux launch composition, unit-control integration, pi-radar and its Herdsman consumer, generated consumed DTOs, Nix runtime dependencies and documentation. Do not alter assignment settlement or use systemd unit state as domain success.

Priority: next launch-contract feature after closing the current registration verification/cutover and its directly relevant recovery blockers; ahead of `tmux-control-mode`. Complete and sync `unified-pi-registration` as the predecessor, then revise its auto-adoption policy explicitly through this delta. Do not wait for unrelated UI or historical checkbox matrices.

Scope is one release, not one unreviewable patch. Isolated feasibility is a go/no-go gate before integration. Preserve graceful-only stop. Do not build legacy compatibility or conversion: rollout will use an explicit reset, whose destructive scope must be agreed separately from tmux server stop. The main-exit/result contract is still under discussion: a systemd scope provides group lifetime, not authoritative command exit status. Finish that decision before completing the design/specs/tasks. No cgroup limit UI, automatic freezing, task executor, mux replacement or retroactive scoping.
