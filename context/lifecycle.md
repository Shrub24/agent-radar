# Lifecycle and deletion semantics

Why a stop, a cancellation and a deletion decide what they do — the parts the code
alone cannot explain.

## A deletion holds over an agent whose exit is not verified

**Id:** 5c0a7f43-9c2e-4d31-8a6b-2f0e5d7c9b14
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** regression found while writing the through-daemon forced-deletion test, 2026-10-27
**Verification:** corroborated — `a_deletion_is_held_when_an_agents_exit_cannot_be_verified` failed before the change (the workspace was closed and the record claimed "after 1 agent(s) exited" while the process was still running) and passes after; `a_deletion_is_held_by_a_surviving_agent_until_it_is_repeated_with_force` covers the window path, and ignoring `force` in `surviving_agents` fails it
**Revisit when:** a stop can establish an exit for an unverifiable scope, so `Exit::Unverified` stops meaning "unknown"

`cancel_many` counts only `Exit::Exited` as stopped. An `Unresolved` or `Unverified`
verdict for any subject in the set is a survivor, so `topology.delete` refuses
`agents_alive` with nothing closed, and only a repeat that names `force` closes the
place.

**Reason:** the previous arm treated every `Settled` verdict other than `Exited` as an
exit, recording a verified exit for an agent whose exit was never established. The
deletion then closed the place over a live agent, and the durable record claimed an
exit that did not happen. `Unverified` is answered exactly when the managed scope
could not be confirmed, which is the moment the daemon knows least and must claim
least.

**Rejected alternative:** keep counting `Unresolved` as stopped on the grounds that a
revoked launch never ran a program. `stage` returns `Unresolved` both for "no
identity was captured" and for "the record changed, or the process could not be
inspected", so the safe direction is set-wide: a set containing anything unaccounted
for is held.

**Consequence:** a subject whose exit cannot be established holds its place until the
operator asks twice, and the forced answer says how many exits were never verified.
The cost of the mistake is a closed pane over a running agent; the cost of the hold
is one more confirmation.
