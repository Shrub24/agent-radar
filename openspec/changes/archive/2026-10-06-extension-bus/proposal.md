# Proposal

## Why

Herdr caps a pane at 32 token keys and a worker pane already uses about 30, so the tokens can carry only a pointer to an agent's background work: a count, the task ids and the oldest start. Radar can show "1 background task" but not what it is, how long it has run, or whether it is still producing output. That detail belongs to `pi-bash-processes`, which owns the tasks.

The existing channels are the wrong shape for Radar. `pi-bg` talks to a session-private socket and its `get` and `stop` operations acknowledge completions, so a viewer polling it would consume results the waiting agent is meant to receive. The extension should push metadata to Radar instead, so the agent has no endpoint to call and nothing to consume.

## What Changes

- Radar listens on one local Unix socket and accepts connections from extensions that dial out.
- A small versioned line protocol: a `hello` naming the Pi session, then a `tasks` message that replaces that session's list of unresolved background tasks.
- Radar joins a session's tasks to the agent row whose `pi_herdsman_session` is that exact UUID (or, where the row publishes none, whose pane the `hello` names), and shows them in the selected row's details.
- A connection that drops removes that session's bus data. The pane tokens remain the baseline.

Out of scope: lifecycle controls or any message from Radar to an extension; output text or a log path over the bus; a background-task row in the tree and the git-style nesting (`plan.md`); a count or age on the agent row; session-file statistics; Herdsman as a bus client; Windows or non-Unix sockets.

## Capabilities

### New Capabilities

- `extension-bus`: The local listener, the line protocol, and how connections and their data live and die.
- `background-task-detail`: Joining bus tasks to agent rows and showing them beside the token facts.

### Modified Capabilities

None. `initial-fleet-overview` is not yet archived, so its capabilities are not in the main specs.

## Impact

Adds a listener module and a bus state to Radar, a details section in `src/ui.rs`, and a contract document `docs/radar-bus.md` with a fixture. No new dependency. The `pi-bash-processes` publisher is a separate change in that repository, written against the contract; this change ships with a stub client in its tests. The initial change listed an independent daemon as out of scope. A listener inside the Radar process is not one: it exists only while Radar runs and keeps nothing.
