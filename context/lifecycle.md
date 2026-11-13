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

## A claim is the attachment authority; a subject derives its grant from claims

**Id:** 4b8e51c7-2d64-4d9a-9f13-6a7c0e5b8d21
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** design review of the unified execution lifecycle, where the maintainer ruled that a subject's claim state is the source of truth and a query must not reconstruct ownership, 2026-11-10
**Verification:** corroborated — the whole suite (576 library, 176 control-plane, every integration target) passes with no subject pointer written anywhere, and `a_captured_claim_is_spent_and_survives_reopen` shows presence alone granting no authority
**Revisit when:** a read needs a subject's attachment history in an order the recorded sequence cannot express, or publication cost measured on a real fleet makes per-claim derivation too slow

`SubjectRecord` holds the durable identity: subject id, session, parentage, owner/run/label. It holds no attachment authority. A claim records its registration, its subject, its session and its **standing** — reserved, pending, granted, duplicate, released — as a recorded transition on the claim's own lifecycle stream. A subject's current grant, its reservation and its previous grant are queries over those claims in recorded order: the newest claim that has not ended is its current attachment, the newest granted one is what authorises, and a released claim keeps its identity and its granted history rather than being deleted.

**Reason:** authority had two writers. The claim's standing lived in `claims/<claim>.json` and `claims/current-<registration>.json`, while the log held the claim's identity and process facts, and `SubjectRecord.current_claim`/`last_claim` were written by whoever moved them last. `release_attachment` cleared the pointer without recording a terminal standing, so a released claim could still read as current, and a reservation wrote `current_claim` at reserve time — which made the pointer mean "reserved or granted" and let a restart displace an incumbent that was still running. Keeping the state in the log gives ordering, replay and one writer; the pointers became a derived index.

**Rejected alternative:** keep subject pointers and treat the claim files as authority for standing only. That was the arrangement that produced both defects, and it leaves two records to keep in step through every transition.

**Rejected alternative:** a separate reservation-hold entity beside claims. Rejected on review: reservation is a claim standing like pending and denied, and a hold would need its own exclusion and grant-eligibility rules, another concept, and the same fencing.

**Rejected alternative:** rebuild ownership by inspecting the process at query time (`release_if_gone`). Rejected: a query must not reconstruct state, and a missed observation would silently change who holds a subject.

**Consequence:** every caller that asked "is this claim current?" now asks two different questions — whether a live process holds the subject (`is_attached`, which a reservation answers yes) and whether this claim may authorise operations (`grants_authority`, which only a grant answers). Tests that asserted pointer movement were restated as authority assertions.

## A reservation is fulfilled by the process that reserved it

**Id:** 9d2f7a14-5c38-4e61-b7a0-3f18c9d2e640
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer ruling after the launch-identity redesign: session start sends registration and session and receives its claim, 2026-11-10
**Verification:** corroborated — `a_captured_claim_is_spent_and_survives_reopen` (a mismatched session cannot present on the reservation; the matching one presents on the reserved claim and gains no authority from it)
**Revisit when:** a client needs to present on a claim it holds before the daemon has resolved the registration

Radar mints the claim id a successor holds, including the reserved claim a restart creates. The launched process carries its registration, not a claim: on presence Radar resolves the registration's reservation and returns **that** claim, and enrollment records the grant against it. No claim id travels in the launch environment, and the client no longer mints an attachment identity of its own.

**Reason:** the reserved claim could never be fulfilled otherwise. The daemon reserved claim C2 for the successor, then the client generated its own id and presented with it, so `presence::begin` created a second pending claim and the reservation survived as an orphan that nothing could grant. The path only appeared to work because the grant minted a fresh claim and bypassed the reservation entirely.

**Rejected alternative:** inject the reserved claim id into the child through an environment variable, so the client presents with it. Rejected: it buys nothing a registration does not already carry — the daemon can resolve the reservation from the verified registration — and adds an environment contract that must stay in step with the record.

**Rejected alternative:** keep client-minted ids and drop reservations. Rejected: a restart must name its successor before it exists, which is what reservation is for.

**Consequence:** a named claim that does not match is refused. A registration presenting another session than its own reservation intended does not fulfil it: the reservation is settled as a failed intent (see "A reservation that cannot be fulfilled fails its intent, visibly") and the presented session attaches under the ordinary rules.

## The event log's lock is never held across a read of claims

**Id:** c6a14e58-3b27-4f80-9e5d-8b40f2a71c93
**Type:** constraint
**Status:** active
**Evidence:** confirmed
**Source:** three hangs found by the integration suites while claim standing moved into the log, 2026-11-10
**Verification:** corroborated — `a_reservation_with_no_spawn_edge_is_withdrawn_when_it_never_presents` ran over 300 s before the fix and 0.07 s after; the same test is the negative control for the nested-read class
**Revisit when:** the log's mutex is replaced by finer-grained state, which would change what can be held where

`registry.events` is a non-reentrant mutex, and claim lookups (`current`, `attachment`, `granted_claim`, `reserved_claim`, `decision`) all take it. Any loop that enumerates the log must finish that enumeration, release the guard, and only then consult a claim; a filter that calls a claim reader inside the guard deadlocks its own thread.

**Reason:** this failed three times in one change, each time where an enumeration carried a predicate that had previously been cheap — `unclaimed` calling `settled`, the subject listing filtering on a claim's subject, and `bind_locked` reaching `fence_registration`. It is an incident rather than a theory: `std::sync::Mutex` reports nothing, the thread simply waits on itself, and the symptom is a suite that stops making progress rather than a failing assertion.

**Consequence:** the pattern is "collect under the lock, decide after it", applied in `unclaimed` and the subject listing. A claim read inside a log guard is a defect even when it appears to work, because it only hangs on the path that reaches it.

## The attachment identity is Radar's to mint, and the client only carries it

**Id:** 7e39b2d6-4a18-4c05-8f57-2d91c6b03e84
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer ruling that session start sends registration and session and receives its claim, then a second ruling that session shutdown releases the claim, 2026-11-10
**Verification:** corroborated — `a_claimless_report_recovers_the_attachment_it_already_made` fails with the discovery branch disabled, and `a_session's_shutdown_completes_its_release_before_the_next_session_starts` fails when the hook wrapper discards its handler's promise
**Revisit when:** a client needs to present on a claim before the daemon has resolved the registration, or a session-start reason decides whether a reload deserves a new claim

A presence report carries the process and the session; the claim id it names is optional and only ever one Radar answered with earlier. A claimless report resolves the registration's reservation, else its live claim for that same session, else releases the claim the registration still holds for a previous session and mints a new one. A named claim resumes that attachment, is refused if it has been released, and is refused if it names a different session than the one the registration holds.

**Reason:** while the client minted the id, the reserved claim a restart created could never be fulfilled — the daemon installed the client's id as a second pending claim and the reservation survived as an orphan. Client-minted identity also meant "the daemon returned an answer" and "the client persisted it" could disagree with no way to tell which was current.

**Rejected alternative:** keep client-minted ids and drop reservations. Rejected: a restart must name its successor before either exists, which is what a reservation is for.

**Rejected alternative:** accept whatever claim id a report names, including an unrecorded one. Rejected: an attachment identity that any caller can invent is not an identity, and the daemon cannot then tell a retry from a new attachment.

**Consequence:** a lost first reply is recovered by reporting again, which is why the claimless path must be idempotent while an attachment is live for (registration, session) — that idempotence is the recovery story, not a retry token. A released claim is never reopened, so a same-session reload after a shutdown gets a new claim. The lifecycle boundary is therefore load-bearing: Pi awaits a handler's return value, so the extension's hook wrapper must return its handler's promise or the next session starts before the previous release lands. A session-start reason is context only; nothing branches on it yet, so no field is carried.

## An exit settles the attachments it held; a claimant never reconciles

**Id:** 2f6c8d41-95b7-4e3a-8c02-4d1a7be69f35
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Source:** maintainer ruling that a registration exit or shutdown implicitly releases the claims it held, that authentication's supervisor is the primary exit signal, and that the "release if gone" case can never happen, 2026-11-10
**Verification:** corroborated — `supervisor_completion_ends_the_execution_and_releases_its_claim` reads `Reserved` without the settlement and `Released` with it, and `an_execution_that_ended_releases_the_claim_it_held` fails with the exit settlement disabled
**Revisit when:** an exit can only be observed after a claim is already needed by someone else, which would mean the observation cadence, not the model, is the problem

`record_exit_locked` is the one settlement: it establishes the end and releases the claims the execution still held. `record_exit` (the lock-taking recorder) and `record_completion` (the authenticated supervisor's report, which reaped the program it started) both delegate to it, so an exit and a completion end attachment authority the same way. Presence admission reads who holds a session from recorded state and answers `duplicate` when someone does; it never inspects a holder's process and never releases on the caller's behalf.

**Reason:** a claim whose process had ended kept "holding" its session until the next process arrived and asked for it, so authority ended as a side effect of a request rather than as the consequence of the exit. That made the record wrong between the exit and the next claimant, and it put a process inspection in the middle of claim admission — the one place that should be a pure read of recorded state.

**Rejected alternative:** keep `release_if_gone` at admission as well as releasing on exit. Rejected: two writers for one fact, and the admission-time one runs at exactly the moment a duplicate answer is the honest one.

**Rejected alternative:** let `is_attached` decide by asking the process table whether the holder is alive. Rejected: reads would then reconstruct authority per query, which the same change removes elsewhere; observation belongs to the exit sweep and the pane-death wake.

**Consequence:** both daemon-created and operator-run launches already use the same supervisor: its child enters the scope and becomes the program; its parent waits and reports code/signal through `launch.complete`. A dedicated pane retains the supervisor's final output through `remain-on-exit`; `pane-died` supplies an additional wake after the pane command ends. The wake checks the recorded program's process identity, so pane death alone is not proof of program exit. Ordered session shutdown normally releases the claim before either notification. If shutdown is missed, the supervisor's completion settles the execution and its claims on its own; a lost completion report leaves the wake and the sweep as fallbacks into that same settlement. Managed agent admission through `radar run --agent` is distinct from passive observation: unwrapped commands and TUIs remain observable without being granted managed lifecycle authority.

## One creator records an execution; an attachment joins it

**Id:** 3b7d1c4e-2a58-4f0b-9d3e-6c1a8f5b2e07
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Verification:** corroborated — `a_subject_admits_one_successor_at_a_time` fails with creation moved back ahead of the refusal (a refused reservation leaves an execution for the ceiling to withdraw) and passes with the check first; `attachment_views_share_process_evidence_across_replay` asserts the stale attachment still reads the recorded signal
**Revisit when:** an execution needs facts at creation that only identification can supply, which would mean the split is in the wrong place rather than the order

`ExecutionDraft::record` builds the record once, `create_execution` records it from its registration alone, and `identify_execution` records the verified process afterwards, storing the supervisor's key before the transition that depends on it. Adopted, admitted, configured, spawned and successor executions all reach the log through that one creator, and the three former entry points are compositions over it. A claim's first append joins the process facts the log already holds rather than restating them, so an attachment cannot disagree with the execution it names.

**Reason:** three constructors each built a record and then mutated the parts that made them different — origin, kind, placement, key, intent — so what a new execution held depended on which caller ran, and only some of them validated before recording. Moving that into one record builder makes the fields a caller cannot know (correlation, scope, ordering) one decision, and makes "check, then record" the only order available.

**Rejected alternative:** keep the per-entry-point record assembly and share only the final append. Rejected: it leaves three definitions of what a new execution is, which is what diverged in the first place.

**Rejected alternative:** keep refusing a claim whose snapshot differs from the recorded facts. Rejected: it preserves a second copy as a thing that could be right, and the refusal fired on facts the attachment never owned (`bound_registration_id`), which is the wrong question to ask a join.

## A claim owns who is attached; an execution owns what happened

**Id:** 8f2e6a51-4c93-4d17-b0a8-1e5c7f3d9a24
**Type:** decision
**Status:** active
**Evidence:** confirmed
**Verification:** corroborated — `a_claim_reads_its_execution_instead_of_carrying_a_copy` records a completion only on the execution and reads it through the claim; the event log's own `attachment_views_share_process_evidence_across_replay` now records process evidence on the execution stream and finds no execution facts on an attachment
**Revisit when:** a consumer needs a point-in-time execution snapshot as it stood when an attachment was made, which is history the log already orders rather than state a claim should carry

A claim's recorded facts are its registration, subject, managed metadata, session, standing and grant time. Identity, birth, binding, observation, placement, signal, exit, completion and pane cleanup are recorded on the execution's stream and read from there. `read_claim` composes the execution; the claim synchronisation that used to copy it (`sync_claim`) is gone, and `write_attempt` records execution changes on the execution stream rather than projecting a copy into the claim.

**Reason:** one record had two copies of the same facts and a maintenance path between them, so a claim could disagree with the execution it named and a crash between the two writes left it that way. The copy also created work with no owner: the divergence check compared a claim's snapshot against the log and refused writes over facts the attachment never owned, such as a binding.

**Rejected alternative:** keep the copy and synchronise it more carefully. Rejected: a second copy of a fact is a second answer to the same question, and the synchroniser had no consumer that could not read the execution directly.

**Consequence:** a claim transition now validates itself against the execution rather than mutating it, so `Granted`, `Presence` and `Reserved` consult the execution record to refuse an ended one — the guard survives without the copy. Execution transitions validate their own preconditions, which is why `Placed`, `AbsencePending` and `EndedExternally` keep their original refusals.

## A reservation that cannot be fulfilled fails its intent, visibly

**Status:** active
**Evidence:** confirmed
**Source:** maintainer ruling "record the intended restart as failed, allow ordinary attachment to the actual session, error on tui, no automatic retry for now", 2026-11-09
**Verification:** corroborated — `a_registration_presenting_another_session_settles_its_own_mismatched_reservation` fails with the settlement removed; the intent is published on the execution

A registration whose own reservation named one session but which presented another settles that reservation with a recorded reason (`Transition::Failed`) and is then evaluated as an ordinary attachment. The reason is published on the execution as `failed_intent` and the TUI names it on the row and in details. Effects that succeeded (the stop, the launch) are not rewritten. A reservation held for another registration is an exclusion conflict that this registration never clears.

**Reason:** leaving the reservation in place stranded the session that actually came back; silently overwriting its intended session lost the evidence that the restart did not happen.

**Rejected alternative:** refuse the mismatched presentation. Rejected: the actual session is blocked until the reservation's deadline.

**Rejected alternative:** a general "process failed" claim standing. Rejected: the execution did not fail, only the intended attachment.

**Consequence:** no automatic retry. A retry mechanism is future work and would consume `failed_intent`.

## Claim authority is a maintained cache over the claim log

**Status:** active
**Evidence:** confirmed
**Source:** maintainer ruling "the index is just the cache; the claim is the source of truth", 2026-11-09
**Verification:** corroborated — the one-reservation and one-live-attachment guards each fail `the_log_refuses_a_second_reservation_and_a_second_live_attachment` when removed, and the one-grant guard fails `replay_refuses_two_grants_on_one_subject`

A subject's grant, its exclusive reservation, its most recently granted claim and each registration's attachment are indexes the log maintains as it appends and rebuilds on replay. Writes and replay share the invariants, so an inconsistent journal is refused when it is opened rather than resolved by choosing a winner. Cancelling a reservation does not change granted history.

**Reason:** every reader previously rebuilt ownership by scanning and sorting claims, with different rules in different places.

**Rejected alternative:** persist authority pointers or index files. Rejected: that reintroduces multi-file consistency and a repair story.

## The pane names its live occupant; rows do not join

**Status:** active
**Evidence:** confirmed
**Source:** maintainer ruling "I would make the Pane own the process ref itself", 2026-11-10; earlier agreement that topology panes carry the registration link
**Verification:** corroborated — `a_pane_names_the_one_live_execution_that_holds_it`, `the_joined_agent_occupies_its_pane_and_carries_a_failed_intent` and `the_pane_keeps_its_row_when_its_launch_presents` each fail when the guard they cover is removed

`Pane.registration_id` is resolved once in the collector. A joined agent is its pane's occupant, because that join is verified against the pane's foreground process; otherwise a live execution whose placement names this backend and pane is, provided one stands out — a grant over a reservation, and equal standing names none. An ended or withdrawn execution occupies nothing. Placement stays recorded once on the execution. The occupied pane is the row, so its identity survives presence and restart; an execution that occupies no pane is its own row and offers no pane close.

**Reason:** the earlier client-side join by placement duplicated rows, chose an execution by key order and could not know the serving generation. Breaking an equal-standing tie by registration id would have been the same arbitrary choice.

**Rejected alternative:** key every row by registration. Rejected: a restart reuses the pane and creates a new registration, so a registration-keyed row changes identity at each restart.

**Revisit when:** occupancy needs the serving instance — the collector would compare each placement's instance with the server now serving, once its per-refresh cost is measured. Until then destructive effects stay fenced by their own instance checks.
