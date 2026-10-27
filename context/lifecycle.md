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

## A held deletion is the daemon's record, and force is the client's consent

**Id:** 9b6d3e18-4a2f-4c81-b7d5-3c9e60f2a714
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer review of where the forced-repeat decision belongs, 2026-10-27
**Verification:** corroborated — `a_held_deletion_is_a_container_fact_that_survives_a_restart` shows the flag in the container read, still set after the daemon restarts, and the repeat closes once; `a_forced_deletion_is_the_same_effect_with_force_set` pins that only a held container produces `force: true`
**Revisit when:** the two-ask flow is replaced by a single pre-emptive confirmation, which would make `force` mean "confirmed in advance"
**See:** lifecycle.md#a-deletion-holds-over-an-agent-whose-exit-is-not-verified — 5c0a7f43-9c2e-4d31-8a6b-2f0e5d7c9b14 — as of 2026-10-27

The daemon records `deletion_held` on the container whose deletion it refused, and publishes it. The UI session derives a repeat's `force` from that container fact; it keeps no list of held containers of its own. `force` stays a field of `topology.delete`, so the caller still states its consent.

**Reason:** two facts were being conflated. "This deletion was refused because an agent survived" is domain truth and only the daemon can durably own it; "the operator has now said close it anyway" is consent and only the session can own it. Keeping both in one in-memory set made consent volatile: after a daemon restart the session forgot the hold while the refusal record still said "delete again to close anyway", so the instruction became false.

**Rejected alternative:** have the daemon close on any repeat without a `force` field, deriving consent from its own record. That removes the client's statement entirely and makes "close anyway" a rule about request history rather than about what an operator was told.

**Rejected alternative:** have the session query the daemon for running agents and decide before dispatching. Observation already answers what is running for the confirmation's own loss list, the answer can change between the check and execution, and "is anything running" cannot express "was the operator already asked".

**Consequence:** `force` can lag by one observation sample, so immediately after a refusal the next press may repeat it once. The daemon's stop-and-verify on the repeat is the safety net, and it is deliberate: the session's reading of a held flag is advisory, never authorisation.
