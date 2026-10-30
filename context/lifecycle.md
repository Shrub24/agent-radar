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

## Adding from the dashboard is always a tab and never a question

**Id:** f8b2b370-c442-42a8-a071-3c1b59e6a0f6
**Type:** decision
**Type:** workaround
**Status:** active
**Evidence:** confirmed
**Source:** acceptance feedback on the live build, 2026-10-29

`a` creates an empty tab in the selected row's workspace whichever row is
selected, and asks nothing. The tmux adapter names the new window's directory as
the session's (`new-window -c '#{session_path}'`).

**Reason:** an earlier build split a pane when a pane row was selected and asked
"this may lose …" before creating a tab from a workspace row. An addition ends no
process, so a confirmation had nothing to protect, and a split from whichever row
the cursor happened to be on made the result depend on where the operator
looked. The confirmed tab then opened in the home directory: tmux gives a new
window the working directory of the process that ran `new-window`, which is the
daemon's, unless `-c` is given. Reproduced on tmux 3.8 against a session created
in another directory. Workspace creation already passed its directory; tab
creation did not.

**Rejected alternatives:** keeping the split for pane rows (a second behaviour
for one key, and the operator asked for tabs); keeping a confirmation (it listed
a "loss" that was the thing being created); using the selected pane's current
path instead of the workspace's (a pane that has since changed directory would
move the tab's start with it).

**Consequences:** `PaneSplit` stays in the runtime contract for `spawn`, which
creates beside its parent; only the dashboard no longer reaches it.

## An unclaimed request that expired is settled, not left unfinished

**Decision:** reconciliation settles a request that expired before it was claimed
as refused `not_executed`. `Store::settle_unclaimed` does it, and
`reconcile::unresolved` calls it first.

**Reason:** a claim is written before any effect is dispatched, so an unclaimed
request past its expiry provably ran nothing. Nothing settled it, so it stayed in
the unfinished set forever, blocked `radar server stop`, and `lifecycle.abandon`
refused it as "never claimed". The operator had no way out. The settlement is
definite, needs no evidence source, and is the outcome the record's own
derivation already named.

**Rejected alternatives:** letting `abandon` accept it (it would record `unknown`
for a request known to have run nothing); filtering it out of the unfinished read
(the stop gate would pass, but the record would still read as pending everywhere
else).

**Evidence:** confirmed
