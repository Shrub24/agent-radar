# Proposal

## Why

Radar records a container only for places it created, so on a server with sessions made by hand `a`, `D` and `R` refuse with "not a container Radar records". A workspace Radar makes is the only place they work. The tree also lists registrations from other servers and from agents whose pane no longer exists, because durable registrations never expire, and a failed focus with no client attached records an unknown outcome that then blocks creating anything in that workspace.

## What Changes

- The daemon records every session, window and pane of its own server as an observed container, parent links included. It does this under a verified serving incarnation, only when the server's places have changed, and appends nothing for a place already recorded. It dispatches no effect. This is the existing reconciliation extended with adoption, and it is scoped to the one server the daemon serves by construction.
- **BREAKING**: the requirement that a container Radar only observed is never "adopted implicitly" is modified. Observed containers are now recorded automatically, stay marked `observed`, never hold a recipe and are never reloaded. Deleting one keeps the existing confirmation and live-worker refusal.
- A container whose handle no server reports is marked unresolved by the same pass, as before.
- A tmux focus with no attached client is a definite refusal (`no tmux client is attached to this server`), decided before anything changes, and a focus that ends unknown never blocks a later mutation of its target.
- **BREAKING**: the fleet shows only registrations whose reported pane is in this server's inventory, or that the daemon holds a spawn edge for. A registration of another backend or server, or one naming no place here, is not shown. The `unplaced` group no longer appears.

Non-goals: a durable record of every transition's evidence (`lifecycle-authority`); an event-driven observer in place of the timer (`tmux-control-mode`); removing registrations from the registry.

## Capabilities

### Modified Capabilities

- `managed-topology`: observed containers are recorded automatically.
- `focus-action`: a focus with no client is refused, and a failed focus is harmless to later operations.
- `fleet-overview`: the tree shows only this server's places.

## Impact

- `containers.rs` (adoption), the daemon sweeper, the tmux adapter's focus, the store's unresolved search, `registry_observation::project`, tests and docs.
- Reload of an adopted pane stays refused (no recipe); only `a` and `D` are new there.
