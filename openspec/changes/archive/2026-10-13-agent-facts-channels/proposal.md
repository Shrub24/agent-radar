# Proposal

## Why

The dashboard's rich agent rows (model, thinking, context usage, background tasks, assignment text, waiting items, a lead's pending ask, role) come from Herdr pane-metadata tokens that Herdsman writes for Herdr's UI. The registry carries identity, activity, waiting reason, outcome and actions, so a registry-only dashboard would lose most of that. The facts must reach the daemon directly, and they must not all depend on Herdsman: some describe the agent process itself and exist for any Pi session, ordinary or managed.

## What Changes

- Add a self-authored **agent facts** publication channel for facts the agent process owns: model, provider, thinking level, context usage, its own session display name, and its unresolved background tasks (id and phase, oldest start). Any Pi session can publish it with no Herdsman. It is a separate channel with its own writer fencing, sequence and freshness, never merged with execution.
- Extend the owner-authored **assignment** snapshot with the facts only an owner knows: role or definition, run and request identity, assignment display text and start time, outstanding awaited items, and a lead's pending owner ask.
- Both additions are bounded, additive, reject unknown fields, and keep free-form words so new vocabulary survives. Nothing here derives state; activity stays in execution.
- Public `agent.get` / `agent.list` expose each channel's facts with its own provenance and freshness. Publication appends events like existing channels.
- Define the single source for each fact so the dashboard never merges two writers' values for one field.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `agent-registration`: facts channel, assignment-snapshot additions, their reads and provenance.

## Impact

Touches the registry publication types and storage, `agent.acquire`/`agent.publish` channel validation, `agent.get`/`agent.list` projections, the event journal pointers, `docs/agent-registration.md`, its fixture and the publisher example. No physical-backend change. Publisher adoption is separate: pi-extensions decides whether the self-authored channel is published by Herdsman's reporter or by a small standalone Pi reporter; this change defines only the daemon contract. OSC 7501 capture is a separate later change.
