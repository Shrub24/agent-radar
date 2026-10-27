# Lifecycle and deletion semantics

Why a stop, a cancellation and a deletion decide what they do — the parts the code
alone cannot explain.

## A confirmed deletion closes over a survivor and never records an unverified exit

**Id:** 5c0a7f43-9c2e-4d31-8a6b-2f0e5d7c9b14
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** regression found while writing the through-daemon deletion test, then maintainer review of where consent belongs, 2026-10-27
**Verification:** corroborated — `a_confirmed_deletion_closes_over_a_survivor_and_names_the_unverified_exit` (a SIGTERM-ignoring bound agent: the place closes once, the message names the unverified exit, the agent is still running) and `a_deletion_never_records_an_exit_it_could_not_verify` (a managed launch whose scope cannot be confirmed is reported unverified, not exited; this failed before the fix by claiming "after 1 agent(s) exited")
**Revisit when:** an operator needs a close that refuses over survivors, which would be a new request shape rather than a retry flag

`cancel_many` counts only `Exit::Exited` as stopped; `Unresolved` and `Unverified` verdicts are survivors and are never recorded as exits. `topology.delete` does not refuse over survivors: it closes the place and its outcome says how many exits were never verified. The request has no `force` field and the daemon has no `agents_alive` refusal. The UI session asks before dispatching whenever its own observation shows agents running, and the daemon sees one plain delete.

**Reason:** two separate faults had been tangled. The false exit was a real safety bug: the stop path recorded a verified exit for an agent whose exit was never established, which falsifies the durable record and, with a managed scope that could not be read, let a place close over a live agent while claiming otherwise. Consent is a different matter. "The operator agreed to end these agents" is a fact about the conversation with the operator, so the actor owns it; the confirmation already lists what runs. A daemon that also asks twice duplicates the question, forces the actor to remember a hold across daemon restarts, and needs a flag on a published contract for what is only a UI sequence.

**Rejected alternative:** keep the daemon's two-step refusal with a `force` retry, and have the actor remember the hold. That keeps a UI conversation inside the lifecycle contract and loses its state whenever the actor restarts.

**Rejected alternative:** record the hold on the container as a published daemon fact so the actor can derive `force`. It fixes the lost state but adds an event, a published field and a schema change for a flag nothing outside the TUI needs; reverted before release.

**Rejected alternative:** have the actor re-ask after seeing the survivor. That keeps a second prompt and actor state for no safety the first confirmation lacked.

**Consequence:** the actor's decision to ask is advisory. An agent can start between observation and execution; the daemon still stops and verifies every agent inside the place, and reports rather than refuses. The confirmation is the operator's consent to what it named, not a guarantee about what runs at the instant of the delete.
