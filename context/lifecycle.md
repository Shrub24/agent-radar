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

**Id:** c702a18a-258c-4e67-86e7-bfefeb117b5f
**Type:** decision
**Status:** active
**Evidence:** confirmed

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

## Escalation is one function, and the deadline is only its first caller

`lifecycle::escalate` performs the single SIGKILL step of a shutdown: it kills a
scope that was verified at signal time and reports whether the group emptied.
`await_end` calls it when the graceful window expires. Written this way so the
operator request that will call the same function later (a second close on the
same claim, or a configuration that shortens or disables the deadline) cannot
grow a second escalation path beside it.

Two evidence rules came out of building it:

- After an escalation, an **emptied verified scope** is the evidence that nothing
  which ran there still runs. Requiring the process's pidfd to report as well
  invents survivors: a zombie, or a child whose parent has not reaped it, never
  reports, and the group is already empty.
- A **zombie is not a live process**. `LocalProcfsVerifier` read `/proc/<pid>`,
  which exists until the parent reaps it, so a killed execution could be reported
  as still running. It now reads the scheduler state and treats `Z`/`X` as absent.
  Without that, "survived SIGKILL" would mostly have meant "not yet reaped".

What is *not* handled on purpose: a process in uninterruptible sleep, and a kill
that could not be dispatched at all. Neither is a state an operator can act on —
one ends when its I/O does, the other is an internal fault — so both are reported
as an unverified outcome with the reason, and neither grows a retry workflow.

## Close and a spontaneous exit are different events, and only one removes a pane

A **spontaneous exit** is the program ending on its own. The subject settles on a
verified exit, the pane is deliberately left in place because it is the last place
the failure or the status can be read (`remain-on-exit`), and nothing is removed.

**Close** is an operator or parent act: terminate the hosted execution, capture how
it ended, then remove the pane. The order is the contract. Terminating first is
what makes the pane the daemon's to remove; capturing next is what keeps the
program's code, because the supervisor that reports it lives in the pane; removing
last is taken only on a placement whose backend instance is still the one serving.

Two consequences worth keeping straight:

- A **missing code never holds a removal**. The report is given a bounded grace
  (`subjects::await_completion`), and then the transition proceeds whether or not
  it arrived. "Exit verified, code unavailable" is a valid end state.
- The grace lives where removal is dispatched, not where a caller is: a launch
  cancel (`cleanup_launch_pane`) and a child close (`execute_child_close`) both
  take it before dispatching, because the code is lost to the removal rather than
  to whichever path asked for it.

Pane removal is dispatched through one bounded call
(`verification::bounded_close_dispatch`) and translated into each record's own
vocabulary (`PaneCleanup` for a launch, `CloseResult` for a child). The
*authorization* differs on purpose — a launch pane is removable only for a
bound/revoked launch that has ended, a child pane only with a verified occupant or
dead-pane evidence — and that difference is the caller's, not the dispatcher's.

## A close publishes parts, not prose

`lifecycle.cancel`, `lifecycle.stop` and `topology.delete` publish `closed` on
their record: `execution`, `resource`, optional `completion`, and `escalated`.
The split exists because a consumer acts on the parts differently, and because the
old flat list forced every reader to parse words:

- `execution` says whether the captured process is verified gone. `unsignalled`
  means nothing was signalled because the claim named is not one this operation
  may end — a refusal of authority, not an accident.
- `resource` says what became of the pane, and is **absent** when the operation
  removes nothing at all. A cancel always reports one, including `kept` for a
  launch with no pane of this daemon's; that is a different answer from "this
  operation never removes a pane", and the field's presence carries it.
- A pane that is already gone is `absent`, and nothing is dispatched for it. It
  used to be recorded as an unverified cleanup of a pane that plainly was not
  there, because the backend answered "can't find pane".
- `completion` is the supervisor's report. Its absence is normal, never an error,
  and never a reason to hold a transition.
- `escalated` is a typed fact, not a word: without it, a forced shutdown would be
  invisible to an operator once the prose went away.

Nothing about a close is inferred from `effects` any more. `effects` remains for
operations with no typed outcome of their own (creates, reloads, restart), and
where it names a pane removal it renders the same `ResourceEnd` vocabulary so the
two cannot describe one close differently.

## A place's contents are its executions' placements, not its spawn edges

Closing a branch finds what runs inside it from where each execution recorded
itself: the pane, with the instance that made it. A spawn edge records who asked
for a launch, which is provenance, not where something runs now; a directly
admitted `radar run --agent` execution has no edge at all, and a branch closed over
edges alone would leave it running inside a place the operator just removed.

Two traps, both found by building it:

- **Revocation is not an ending.** A revoked launch is cancelled work; revocation
  alone signals nothing. Filtering revoked executions out of discovery hides a
  process that is still running inside a place about to be removed, so discovery
  excludes only the evidence that says an execution ended — a verified exit, an
  externally established end, or a reported completion.
- **Placement is stored on the execution, and the log holds executions in more
  than one projection** — the attachment of a registered process and the process
  record itself. Reading only one of them silently misses entire classes of
  execution (a registered agent one way, an admitted one the other), so both are
  read and keyed by registration. A subject comes from the attachment, because that
  is the record that has one.

The branch also takes one completion window for the whole set before the root is
removed: the codes belong to executions whose supervisors live in the panes being
destroyed, and a per-member grace would make a five-agent branch pay five.

## Restart is a subject operation, not a close followed by a launch

A restart keeps the subject and replaces the execution: it reserves the successor's
claim first, stops the predecessor through the same termination path a cancel uses,
and launches only on a verified exit. An unresolved or unverified predecessor exit
returns before any launch, so a restart can never leave two executions of one subject
believing they own it — and because the predecessor's claim id is what was signalled,
a replay or a later request cannot signal its replacement through the old id.

The successor reuses the predecessor's pane when that pane is the launch's own — a
spawned launch's recorded placement, of the serving generation that made it, with a
command that has exited. Anything else starts beside it. A shell-hosted
`radar run --agent` execution is never reused: that pane's shell belongs to the
operator, and typing a command into a resumed shell would be a second execution
nobody supervised.

## A close reports every execution it addressed, not one verdict for the place

**Id:** 3b71c9d2-5a84-4e16-9f0c-71d2b8a45e63
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** full review pass over the managed-lifecycle change, then the operator's ruling that ids and outcomes be typed, 2026-11-06
**Verification:** corroborated — `a_survivor_of_a_branch_close_is_named`, `a_place_that_hosted_nothing_reports_no_execution`, and the live-shape assertions in `a_confirmed_deletion_closes_over_a_survivor_and_names_the_unverified_exit` and `a_deletion_never_records_an_exit_it_could_not_verify`, which now read `closed.executions[0].end`
**Revisit when:** a consumer needs a single verdict for a place, which would mean a derived summary field rather than the report going back to one entry

`closed.executions` carries one entry per execution the close addressed: its registration, the subject it runs as when one is recorded, its own `end` and its own `completion`. An empty list means the target hosted none.

**Reason:** a branch holding five agents reported one `execution` for all five. The survivors' names went only to the log, so an operator told "exit not verified" could not tell which agent it was — the one fact a close exists to establish was the one it lost.

**Rejected alternative:** keep one verdict and list survivors in its reason text. That is what the code did, and it is prose an operator has to parse for an identity.

**Consequence:** `ExecutionEnd::NotApplicable` is gone. "No execution here" is an empty list, a different answer from "one execution, nothing to report", which the old variant conflated. Wire revision 16.

## Withdrawal is keyed on the claim, not the spawn edge

**Id:** c4d8a1f6-2b3e-4709-8d55-9e6a0c17b842
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** review pass over the managed-lifecycle change, 2026-11-06
**Verification:** corroborated — `a_reservation_with_no_spawn_edge_is_withdrawn_when_it_never_presents` (a recipe reservation with no edge is found by `unclaimed`, withdrawn once, its pane retired, its subject released) and the pre-existing edge-keyed test, which still passes
**Revisit when:** a launch exists that records neither an edge nor a claim, which nothing does today

`unclaimed` pages reservation claims on the sweeper's own cursor, and `withdraw_unclaimed` withdraws one; the edge path keeps its own bookkeeping and measures the deadline from the edge's `recorded_at`, because a revoke rewrites the claim's. A refused create or launch abandons its reservation in the request that made it.

**Reason:** a recipe launch (a configured `L`, a container reload) records no spawn edge, and every withdrawal path was reached from an edge. A create or launch the runtime refused, or a shell that died before presenting, left a pending claim for ever, and the tree draws one as a subject row nothing resolves.

**Rejected alternative:** record a spawn edge for recipe launches so the existing pass covers them. That gives a recipe launch provenance it does not have — nothing asked for it as a child — to satisfy a scan.

**Consequence:** the third convergence miss of the same shape, after discovery and cleanup. Anything keyed on spawn edges misses admitted and recipe executions.

## Identity stays a string at the wire; typing it is a change of its own

**Id:** e07f2b19-6c48-4a3d-b1f7-58c9d0e6a4b7
**Type:** workaround
**Status:** open
**Evidence:** confirmed
**Source:** review pass, and the operator's ruling that ids should be appropriately typed, 2026-11-06
**Verification:** corroborated — newtypes were added, threaded through the lifecycle domain, and reverted after the compiler showed 220 sites across the registry, the events log and the public DTOs
**Revisit when:** someone plans the typed-identity change, which should be its own change rather than a review fix

Claim, subject and registration ids are canonical UUIDs and are not interchangeable, but all three are `String` in the records and the protocol. `process_for_control` accepts either a claim id or a registration id; `cancel` and `terminate` take a parameter named `subject_id` and are passed claim ids; `cancel_many` takes registration ids.

**Reason it is not done:** `#[serde(transparent)]` keeps the JSON identical and `#[schemars(with = "String")]` keeps the generated schemas identical, so the wire is not the obstacle. The obstacle is that every id at every interface converts once, and the boundary has to be chosen deliberately: domain types first, then the interfaces, then the wire DTOs, each gated. Attempting all three at once produced a 220-error tree that was reverted rather than left half-migrated.

**Consequence:** the class of bug stays reachable — branch close terminated by subject id and resolved nothing, and that was found only by reading the code. Until the change lands, the two lookups are at least named for what they take.
