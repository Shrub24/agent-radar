# Proposal

## Why

Herdsman's direct publication is landed and verified (pi-extensions main
`f5eedd29`, ADR 0031), and it surfaced one contract gap: one live Pi process
can switch or fork its session context without becoming a new process subject.
Registration is deliberately immutable per `(source, incarnation)`, and a
session value baked into it would either freeze obsolete context or force a
fictitious re-registration. Herdsman therefore publishes *no* session field
today, which leaves the join between a live subject and its current session
unanswerable. Close that gap with a mutable, source-labelled current-session
association before any consumer needs it.

## What Changes

- Add an additive `agent.context` write that republishes one agent record's
  current session UUID under the same writer/sequence/lease/freshness
  discipline as the existing channels, without touching the registration,
  the agent id or the writer generation stays intact unless explicitly replaced.
- Add a `"context"` read to `agent.get` and `agent.list` that distinguishes
  fresh, stale/restored and never-reported context.
- Keep session-file paths, launch arguments and any other private material out
  of the public read. A context record names the session UUID and nothing else.
- Extend the registry reference, the canonical fixture exchanges and the
  reconnecting publisher example. Pin and report the landed commit to
  pi-extensions as the adoption contract.

## Capabilities

### New Capabilities

None. `agent.context` is served under the existing `agent_registry`
capability, so old clients ignore the added read key and new publishers can
detect support through ping.

### Modified Capabilities

- `agent-registration`: gains a mutable, fenced current-session association
  alongside immutable registration and separate execution/assignment channels.

## Impact

`src/control_plane/` gains one atomic context record type beside the channel
records. Protocol documentation, fixtures and the publisher example expand.
No change to registration immutability, fencing, freshness, retention or the
mux paths; no physical launch/stop/resume, no readiness inference, no TUI
consumption and no new event surface. Herdsman adopts the pinned commit after
this lands; readers must treat stale/absent context as unknown, never as
evidence of exit or idleness.
