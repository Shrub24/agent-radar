## Why

Radar needs a supported way to clear transient state left by old models, dead processes and stale tmux topology. Manually deleting directories risks leaving records that refer to one another and durable data that should survive. Reset should create a clean operational slate without erasing the durable agent identities and communication that belong to those agents.

## What Changes

- Add `radar reset`, which previews the reset and requires confirmation before making changes.
- By default, purge registrations, claims and attachments, process/scope evidence, launch and lifecycle history, spawn-operation records, ephemeral publications and event state.
- Preserve durable subject IDs, their Pi-session association, subject-owned metadata and parent/child relationships, plus durable handoffs and inbox contents addressed to those subjects.
- Orchestrate a complete reset: stop the daemon, gracefully stop verified Radar-managed scopes, clear the selected state, restart Radar's served tmux server, then start the daemon. If a managed process survives graceful stop, refuse before clearing state or restarting tmux.
- Add `radar reset --hard` to also remove durable subjects, their relationships, handoffs and inboxes.

## Non-goals

- Selectively pruning only records proven stale (`--stale`); reset is the whole transient-state boundary.
- Migrating old process or lifecycle history.
- Touching tmux servers Radar does not serve, user configuration, or processes outside Radar-managed scopes except for the normal consequences of restarting Radar's served tmux server.

## Impact

New spec `state-reset`. Code: reset orchestrator, selective durable/transient state handling, graceful scope stop, and restart of the Radar-served tmux and daemon services. No wire change.
