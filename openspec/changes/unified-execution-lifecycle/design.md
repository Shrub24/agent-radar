# Design

## Context

See proposal.md for motivation. Today there are two ways to make an execution.
`create_process` records a registration with no claim or subject; in-place
admission uses it, and the subject is attached later at presence by
`enroll_subject`. `create_with_intent` records a claim, a minted subject and a
reserved presence decision before the process runs; configured launches, recipe
launches, restart and `spawn.create` use it. Claiming a launch (`launch.claim`),
capture, the pane-death wake and withdrawal of an unpresented launch are keyed on
that claim. The spawn edge records the claim too.

Read from the code, not yet run: the presence handler already calls
`record_enrollment` for an execution that holds no subject, and
`enroll_subject` accepts an existing process record whose identity matches. So a
launched execution with no reservation can acquire its subject at first presence
the way an adopted one does.

## Goals / Non-Goals

- One constructor for executions. The inputs that differ between adoption, launch,
  restart and spawn are optional parameters of it, not separate paths.
- Out of scope: the presence wire protocol, pi-radar behaviour, and the model of a
  subject once a session is attached.

## Decisions

**1. The execution is the unit of reservation.** A reservation writes the process
record (registration, scope, placement, intent) and nothing about a subject. The
launch correlation is part of the intent. `draft` stops minting a subject.
Alternative rejected: keep a claim and drop only the minted subject. A claim names
its subject, so a claim without a subject is a second representation of "not yet
attached".

**2. Identify and attach are separate steps.** Identify records the verified
process identity: at creation for adoption, at `launch.claim` for a launch.
Attach gives the execution a subject and claim: at presence for adoption and
launch, up front for restart. One `attach` operation serves all three; restart
supplies the existing subject and its expected claim, as `create_with_identity`
does now.

**3. Keys move to the registration.** `launch.claim`, capture, the exit wake, the
sweeper's withdrawal and its cursor address the registration. They need the
process record and correlation, not a subject. A restart's reserved successor
claim is still withdrawn by revoking that claim.

**4. The spawn edge is composed.** It is a separate record keyed on the child's
registration holding the parent link, the correlation token and the outcome. It is
written by `spawn.create` beside the reservation and bound to the child's subject
at grant, which `record_spawn_granted` does now. A resumed child that reports an
already-owned session continues that subject through `enroll_subject`, so a
placeholder subject can no longer be granted over it.

**5. A launch in flight is its pane.** With no subject row there is nothing to
join. The observation's lifecycle subjects list only attached subjects, and the
pane row carries the launch's state when it is useful. Whether the pane row needs a
published "starting" fact is decided in implementation from what the tree needs.

## Risks / Trade-offs

- Withdrawal, wake and cleanup are the paths that missed launches without a spawn
  edge three times before. Every one of them is re-keyed here, so each needs a test
  that fails if it reads the old key.
- Restart now has the only pre-attached claim; its refusal paths must still free
  the subject. The existing restart tests cover this and must pass unchanged.
- Records of the earlier launch shape are refused at startup; `radar reset` clears
  them. Radar supports the current record version only.

## Migration Plan

Stages that each pass the gate: (1) the constructor and re-keying for `L` and
recipes; (2) restart on the same constructor; (3) `spawn.create` and the edge;
(4) observation and tree. Rollback is the previous binary with the previous state.

## Open Questions

- The observation's spawn binding carries a claim today. If the binding is keyed on
  the registration, the wire revision moves; the target is revision 18 unless the
  implementation shows the binding can stay.
