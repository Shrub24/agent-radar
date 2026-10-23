## Why

Radar keeps durable records of registrations, claims, spawn edges, operations, handoffs and events under one state root. When the model changes, or a fleet is left with records no process, pane or scope backs, the operator has no way to start clean: `reconcile` is effect-free, `abandon` ends one obligation, and the only remaining route is deleting directories by hand, which is easy to get wrong and silently loses handoffs. `managed-scoped-launch` also deliberately chose a reset over a migration and left its destructive scope to be agreed. This change defines it.

## What Changes

- Add `radar reset`, an offline command that moves Radar's control state aside, so the daemon starts empty.
- Make it safe by default: it lists what would be discarded and changes nothing until confirmed, refuses while the daemon answers, and refuses while a managed scope still has members unless the operator asks it to stop them gracefully first.
- State what is lost, by category, including handoffs and inboxes, and keep the moved state so a reset is reversible until the operator purges it.
- Never stop, restart or reconfigure the tmux server, and never signal a process outside a Radar scope.

## Non-goals

- Reconciling or pruning individual stale records (a `--stale` form) is a later slice; it reuses the delete path's proof of absence and needs its own scenarios.
- Migrating records between models.
- Resetting the tmux server, the user's configuration or pi-radar's own stored state.

## Impact

New spec `state-reset`. Code: the `reset` subcommand, a state-root mover, and the existing scope stop. No wire change.
