# Proposal

## Why

Herdsman cannot adopt Radar-owned spawn without transporting its child's working directory and launch environment. Radar already binds a private spawn token during child registration, but consumers need exact bound-agent readback and a tested publisher handshake rather than Herdr agent aliases and registration polling by pane name.

## What Changes

- Extend `spawn` with optional absolute `cwd` and an explicit environment overlay. Omitted fields retain current behavior; supplied values reach the child without alteration. Reserve `PI_RADAR_SPAWN_TOKEN` for the daemon and reject caller overrides.
- Carry the launch context through the existing runtime-provider seam and Herdr adapter. Validate backend representability before creation; Herdr keeps the existing quoted single-line transport and refuses values it cannot carry.
- Keep environment values and the daemon token private in storage, operation readbacks, errors and diagnostics, while retaining exact-content duplicate suppression.
- Reuse `agent.register` token consumption and return the exact bound `agent_id` in topology reads for new bindings. Preserve old edges and binding records without inventing identities.
- Document and exercise child startup: receive the token, register the exact source/incarnation and process claim, then publish context/execution. Binding proves registration, not assignment readiness or completion.
- Publish a versioned launch-context capability so consumers can detect support before sending new fields. Keep current parent-agent plus session-context checks and partial-outcome/no-retry semantics.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `managed-child-spawn`: Working-directory/environment transport, pre-effect representability checks, and exact bound-registration readback.
- `agent-registration`: Private token-consuming child bootstrap and binding evidence distinct from execution/assignment readiness.
- `mux-control-plane`: Launch-context feature negotiation and private launch-context operation projections.

## Impact

Touches `src/runtime.rs`, `src/herdr.rs`, daemon spawn/registration/store projections, protocol references, canonical fixtures and disposable-daemon/Herdr launch coverage. No new dependency is expected.

This builds on the uncommitted session-aware spawn prerequisite in `managed-lead-restart`; preserve that work and leave restart blocked. Herdsman publisher/launcher migration is a separate pi-extensions implementation using this contract. No tmux backend, placement expansion, process stop/resume/restart, shell-ready claim, assignment-ready API, or adoption of existing foreign children is included.
