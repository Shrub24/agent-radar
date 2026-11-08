# Proposal

## Why

A process Radar launches is recorded as a subject with a claim before it runs, and
a subject is the durable identity of a Pi session, which a freshly launched agent
does not have yet. Only a restart or a child launch knows its subject beforehand.
Applying that reservation to every launch gives configured `L` and container
recipes a placeholder subject that shows as an orphaned `pending` row beside the
new pane until the program claims its launch, and it keys withdrawal, the exit
wake and the spawn edge on a claim that nothing yet needs. Adoption already
shows the right shape: an execution exists by its registration alone and acquires
its subject when the session reports presence.

## What Changes

- One execution lifecycle for every way a process comes to be managed. An
  execution is created by its registration, with an optional placement and an
  optional launch intent. It is identified when its process is verified, and it
  acquires a subject and claim only when a session attaches.
- Adoption, configured `L` and recipe launches, restart and `spawn.create` are
  that one lifecycle with different inputs: adoption identifies at creation,
  a launch identifies when the program claims it, a restart attaches a successor
  claim to its existing subject up front, and a spawn composes an optional edge to
  its parent.
- A launch no longer records a subject or a claim before the program runs.
  Claiming, the exit wake and withdrawal of an unpresented launch are keyed on the
  registration.
- A launch still starting appears as its pane, not as a separate subject row.
- **BREAKING**: records of a launch written in the earlier shape are refused, not
  migrated; `radar reset` clears them.

Non-goals: the presence protocol, pi-radar, and the subject and claim model for
sessions that have reported presence.

## Capabilities

### Modified Capabilities

- `managed-execution`: an execution exists by its registration; a subject and claim attach to it.
- `managed-launch`: a launch reserves an execution, not a subject; withdrawal is keyed on the execution.
- `managed-child-spawn`: a spawn's edge is composed onto the execution it launches.

## Impact

- `src/control_plane/lifecycle.rs` (reservation, claim, capture, withdrawal),
  `registry/presence.rs` (the reserved standing), the launch, places, spawn and
  sweeper handlers, the spawn edge, the observation's lifecycle subjects and spawn
  binding, and the tree's lifecycle rows.
- The observation carries a spawn binding keyed on a claim today, so this likely
  moves the wire revision; the design settles that.
