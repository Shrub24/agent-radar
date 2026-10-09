# Design

## Context

See proposal.md. The daemon already exposes `create` (workspace, tab,
pane-split), `input` (literal text and named keys) and `report` behind a `direct`
or daemon transport, and owns a durable agent registry with registration,
separate execution/assignment channels, writer fencing and freshness. Herdsman's
launch path is recorded in pi-extensions ADR 0029 and the `herdsman-child-command`
spec: create or reuse a pane, type the resolved command, poll `agent get <pane>`
for the child's own registration, then apply and verify the alias.

## Goals / Non-Goals

**Goals:** a durable parent/child runtime edge authored where the child is
created; child identity bound from the child's own registry record; honest
partial outcomes; topology reads that survive a daemon restart.

**Non-Goals:** stop, resume, restart, lead recovery, adoption of panes the
daemon did not create, assignment authority, tmux, or any policy about which
children to recreate.

## Decisions

### 1. Spawn is one durable operation, not a sequence the caller composes

`create` + `input` + `observe` composed by the caller would record nothing
durable: a caller that dies between the pane and the launch leaves an
unattributable pane, and no record says which parent it belongs to. One
operation records the intent first, then reports each effect. Reject exposing
a "spawn" as a client-side helper (no durable edge) and reject recording the
edge only after success (a crashed spawn would leave nothing to reconcile).

### 2. The edge binds through a private correlation token, never a guess

The daemon mints a token per spawn and passes it to the child in its launch
environment. The child carries it in its registration; the daemon binds the
pending edge to that exact `(source, incarnation)` and marks the token spent.
This replaces Herdsman's `agent get <pane>` polling with the child's own
registry record, so binding survives a rename, a pane move and a title change,
and two panes that look alike cannot be confused. Rejected: binding by pane
id at launch (the child may exec, move or reuse), by alias or title (not
identity), and by label or session UUID (shared context, not identity).

A token that never arrives leaves the edge `unbound`: the pane is recorded as
created, the child is not claimed, and nothing is inferred. A second
registration presenting a spent token is refused, so one spawn cannot bind two
children.

### 3. Launch is a backend capability and is terminal input on Herdr

Herdr cannot start an arbitrary command: `agent start --kind pi` picks from
Herdr's own table, so the resolved child command reaches the pane as typed
input under ADR 0029's measured quoting rules (single-quoted argv on one line).
A newline-containing argument is refused before input: the measured private
script fallback cannot safely be cleaned up because the daemon cannot know when
the pane shell has read it, and indefinite scripts or a TTL that can delete an
unread script are not acceptable. Supporting that shape needs an adapter-owned
bounded consumption acknowledgment.

This deliberately reopens the earlier "no launching through terminal input"
exclusion for exactly the create-and-launch step. The exclusion stays for
everything else: no stop, resume or restart through input, and a backend that
does not declare `launch` refuses `spawn` before dispatch. A future tmux backend
may implement `launch` without typing.

### 4. Partial effects are separate outcomes

`created`, `launched` and `bound` are reported separately, each `completed`,
`refused` or `unknown`. A launch that may have been typed but is unconfirmed is
`unknown` and never reported as launched; a spawn whose launch failed leaves the
created pane named in the record and does not clean it up silently, because the
operator can see and close a pane but cannot see a silently deleted one. The
operation is not retried as a whole: a caller retries with a new request id and
a new token, and the earlier record stays readable.

### 5. Reads report the topology, not a conclusion

A `spawn` read returns the parent, the child when bound, the edge state
(`unbound`, `bound`, `unresolved`), the created location and its freshness.
Stored edges survive a daemon restart and are re-verified against the backend
on read; a location that no longer exists reads unresolved, which is not a
statement that the child stopped. Edges the daemon did not author are not
created from observation: Herdsman-spawned children stay absent from this
topology until Herdsman routes spawning through the daemon.

## Risks / Trade-offs

- Herdsman must pass the token into the child environment, a real coupling
  between the two writers → it is exported once at launch, and Herdsman's
  existing `PI_HERDSMAN_CHILD_COMMAND` plumbing is the natural carrier.
- Typed launch is Herdr-specific and shell-sensitive → confined to the adapter,
  behind the capability, with the ADR 0029 rules as the contract it must meet.
- A parent that dies before its children are bound leaves unbound edges →
  deliberately recorded rather than guessed; reconciliation reports them.

## Migration Plan

1. Spawn edge records and token binding (1.x), then the backend `launch`
   capability (2.x), then the `spawn` operation and reads (3.x), then the
   independent gate (4.x).
2. A consumer first checks daemon protocol compatibility and the required
   `creation` and `launch` capabilities. Method names are versioned by the
   compatible protocol rather than enumerated in `ping`; after preflight the
   consumer may call `spawn`, but it must not fall back after an unknown or
   uncertain effectful result. Herdsman keeps today's path until it adopts the
   daemon operation explicitly.
3. Rollback: stop calling `spawn`; existing records stay readable, and today's
   Herdsman path still works because nothing about it was removed.
