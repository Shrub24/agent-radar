# Proposal

## Why

Herdsman currently creates a managed child itself: it asks Herdr for a pane,
types the resolved child command into it, polls `agent get <pane>` until the
child registers, applies the alias and verifies it. The parent/child
relationship therefore lives in Herdr's pane inventory and in Herdsman's
run records, and no durable record says which pane a given agent was created
under. When a lead's children matter — recovery, diagnostics, a future restart
plan — that relationship has to be reconstructed from pane titles and aliases.

The daemon now owns the agent registry and the physical mux primitives
(create, input, close, focus, reporting). It can therefore create a child,
launch it, and record the runtime edge at the moment the child exists, binding
the edge to the child's own registry identity rather than to a pane name. That
makes the daemon the source of truth for the managed runtime hierarchy it
created, which is the shape later recovery needs.

## What Changes

- Add a `spawn` operation: create the backend pane under a named parent
  location, launch the resolved child command in it, and record one durable
  spawn edge from parent runtime subject to child.
- Mint a private spawn correlation token per spawn, pass it into the child's
  environment, and bind the edge when a child registers with that token. No
  edge is ever bound by pane title, alias or position.
- Add a `launch` backend capability. On Herdr, launching is terminal input
  under Herdsman's measured quoting rules; the daemon states the capability
  and refuses `spawn` before dispatch when the backend lacks it.
- Record partial effects honestly: created, launched and bound are separate
  outcomes, each completed, refused or unknown, and a later step is never
  reported when an earlier one is uncertain.
- Publish spawn topology reads (parent, children, edge state, location) with
  the same provenance and freshness discipline as other records, retained
  across daemon restart and re-verified, never inferred.

## Capabilities

### New Capabilities

- `managed-child-spawn`: the spawn operation, its correlation token, its
  partial outcomes and the durable runtime edges it records.

### Modified Capabilities

- `mux-control-plane`: gains a `launch` capability, which executes a resolved
  command in a pane this daemon created.
- `agent-registration`: registration gains an optional spawn correlation
  token that is never exposed publicly and binds one pending edge.

## Impact

`src/control_plane/` gains a spawn edge record and the spawn operation; the
Herdr adapter gains launch. Herdsman keeps assignment authority and may route
child creation through `spawn` when the daemon advertises it, keeping today's
path otherwise. Out of scope: stop, resume, restart, lead recovery, adoption
of panes the daemon did not create, and any change to assignment records.
