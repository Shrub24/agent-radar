# Tasks

## Checkpoint and revised scope

The initial implementation is committed at `5482f789` on
`wip/unified-execution-lifecycle`, wire 18. Its gate and live acceptance passed.
That evidence applies to the checkpoint, not to the revised grant/reservation and
joined-row semantics in design.md. Reopened tasks below retain their identifiers;
sections 6–9 cover the approved rework. No revised implementation is complete.

## 1. One execution constructor

- [ ] 1.1 Finish shared execution creation with optional placement and launch intent; compose identification and optional claim reservation around it. Reopened: adoption and launch still have separate construction entry points. Verify adoption, fresh launch and successor creation through their real callers; remove redundant constructors rather than asserting helper names.
- [x] 1.2 Launch claiming, capture, pane-death wake, withdrawal and sweeper cursor address registrations at the initial checkpoint; library, control-plane and live lifecycle checks passed.
- [x] 1.3 Configured `L` and container recipes reserve executions without placeholder subjects; presence enrolls them. Verified by the initial control-plane and live acceptance runs.

## 2. Restart

- [ ] 2.1 Reserve the successor against the known subject without replacing the incumbent grant; stop the exact predecessor and grant only after verified exit and release. Reopened: the checkpoint replaces `current_claim` at reservation. Verify authority preservation, failure cancellation and no launch before verified exit; see section 7.

## 3. Spawn

- [ ] 3.1 Finish fresh and continuing child launch on the shared creator, preserving the registration-keyed edge and binding the subject at grant. Reopened for constructor and reservation rework. Verify parent-child queries and continuation of the same child subject with no placeholder subject.
- [x] 3.2 Edge exit observation, topology reads and child close use the registration at the initial checkpoint; initial Rust and live lifecycle checks passed.

## 3a. Execution-addressed control

- [x] 3a.1 Stop/cancel resolve an exact execution; subject-addressed requests require the selected claim while unattached launches need none. Initial control-plane suite passed. Recheck reservation-related fencing in section 7.
- [x] 3a.2 Registration fencing uses its caller's transition lock instead of reacquiring it. The formerly hanging control-plane tests and full suite passed at the initial checkpoint.
- [x] 3a.3 Settled withdrawal is not repeatedly offered or processed: revoked, no process, no live attachment and cleanup recorded. Initial control-plane suite passed; recheck optional reservation cleanup in section 7.

## 4. Observation and display

- [ ] 4.1 Replace unmatched-subject fallback rows with registration-rooted joined rows and authoritative pre-presence execution observation. Reopened for the full projection, not merely hiding fresh-launch subjects. Verify the cases in section 8.
- [x] 4.2 Initial spawn binding moved to registration and both peers use wire 18. New observation changes require another revision in section 8.
- [x] 4.4 Initial control-plane fixtures read unattached launches through executions/spawn reads and subjects only after attachment; supervisor completion is reported against the execution. Checkpoint result: 176 passed, 1 ignored.

## 5. Initial checkpoint evidence

- [x] 5.1 Initial docs and changelog reflect execution-keyed launch behaviour. These do not document the revised model; update the affected documentation in sections 6–8.
- [x] 5.2 Initial gate passed: fmt, strict Clippy, all Rust targets (576 library; 176 control-plane passed, 1 ignored), pi-radar contracts/133 tests/package check, OpenSpec strict and checked Nix package build. Do not count this as the revised gate.
- [x] 5.3 Initial disposable lifecycle acceptance passed configured `L`, subject-preserving restart, `D`, branch close and shared escalation, admitted pane close, native move, nested workspace close, completion and Ctrl-C. Earlier unresolved/unplaced failure reports are superseded by this run; revised row semantics remain unverified.

## 5a. Verified hook contract and scoped execution policy

Pi awaits shutdown before reload/session replacement and exposes startup/reload/new/resume/fork reasons. Radar's hook wrapper currently drops asynchronous handler promises, and session-start reporting is fire-and-forget. Correct those seams; do not invent an upstream hook race. Implementation and behavioural validation remain in 6.3.

The broader durable work/event/terminal-response execution model is recorded in plan.md as a subsequent refactor. Do not add a separate engine, per-function journal or independent effect-completion flags in this change. Use existing durable transition and recovery seams; claim-derived indexes replace the superseded subject-pointer repair plan.

## 6. Rework: execution and attachment ownership

- [x] 6.1 Claims own attachment identity and standing on the one ordered log record; subject grant, reservation and history are derived from them, the presence decision store is gone, a reserved claim is fulfilled by its own registration and `sync_claim` remains the only projection. Library (576), control-plane (176) and every integration target pass; `context/lifecycle.md` records the decision, its rejected alternatives and the log-lock constraint the change exposed. Complete task 1.1 and make claims reference their registration rather than embed process snapshots. Unify the presence decision and lifecycle claim into one authoritative attachment record. Retain reserved/pending/granted/denied/released semantics; add shared derived standing predicates and central granted-attachment resolution, not stored policy flags or a generic exclusivity boolean. Remove claim synchronisation and fallback execution ownership. Verify composed reads, completion after release and replay through existing behavioural seams; do not add source-layout assertions.
- [ ] 6.2 Apply accepted launch metadata at grant with nonempty intent taking precedence and absent intent retaining continued subject metadata. Verify parentage and continuation through real enrollment; update managed-execution documentation with ownership and precedence.

- [x] 6.3 Preserve the hook promise chain and serialize session boundary work per registration. Shutdown releases the exact claim; start sends registration/session and persists Radar's returned ID; heartbeats/release name it. Resolve reserved IDs instead of replacing them. Carry start reason as context, not a retry token. Remove client-minted authoritative IDs, generic previous-claim replacement and its Reserved skip, with no adapter or mandatory attempt subsystem. Verified: the hook wrapper returns its handler's promise, so `session_shutdown` does not return until the release answers (a held-open release keeps the boundary open; discarding the promise fails that test), the report names Radar's claim or none at all, a lost first reply recovers the attachment it already made (fails with the discovery branch disabled), a released claim cannot be reopened, a report for another session is refused without displacing the incumbent, a same-session reload reuses the attachment, and a different session releases the held one. Wire 19, because the presence report's claim identity moved to the daemon. Retain bounded existing transport-failure handling without silently abandoning queued releases or adding automatic intent retry.

## 7. Rework: independent reservation and grant

- [ ] 7.1 Remove independently writable subject current/reserved/last authority pointers. Maintain subject grant/reservation/history indexes derived from authoritative claim records, updating during serialised transitions and rebuilding at startup. Reject incompatible grants/reservations; no per-query rebuild or opportunistic process observation. Verify replay, interrupted index update, incumbent authority under reservation, competing grant/reservation refusal and cancellation preserving the incumbent. Update lifecycle documentation alongside the change.
- [ ] 7.2 Complete task 2.1 and continuing-child reservation semantics in task 3.1. Verify no successor dispatch before verified exit and release, grant conflict refusal, stop failure settling only the successor reservation, and successor failure leaving a detached subject without restoring an exited grant. Do not add an early-successor retry workflow.
- [x] 7.3 Unify exact claim release and registration finalisation for session shutdown, verified execution end and launch withdrawal; remove pointer-only release and admission-time release_if_gone. Verify non-graceful exit settles claims without a new claimant, withdrawal settles reservations/resources once, interrupted finalisation resumes via existing durable evidence, and stale release cannot affect a successor. Verified: `record_exit` and `record_exit_locked` — the two places an exit is established, reached by every stop, close, pane death, withdrawal and sweep — release the claims the execution held; `release_claim` is idempotent and pointer-only release no longer exists. `release_if_gone` and its admission-time use are deleted with their unit tests, because a claimant no longer reconciles state it does not own. `an_execution_that_ended_releases_the_claim_it_held` fails with the settlement disabled and passes with it.
- [ ] 7.4 Handle own-reservation identity mismatch in the shared attachment operation: settle reservation, fail intended restart/continuation with a retained TUI error, then apply ordinary eligibility to the actual session. Do not retry automatically. Verify another registration's reservation is preserved and an actual-session grant cannot falsely complete the original intent.

## 8. Rework: public observation and joined rows

- [ ] 8.0 Measure the existing publication path and representative execution-backed collection cost before adopting performance optimisations. Record workload/results. The maintained claim-derived attachment index is required for model ownership; additional publication indexes require evidence.
- [ ] 8.1 Publish public execution summaries with placement and distinct optional reserved/granted attachment references before presence, including recipe/container launches without spawn edges. Advance changed record versions and wire to 19, retaining unreleased Radar 0.12.0 and pi-radar 0.16.0; regenerate contracts for exact matching peers, with broad alpha reset and no compatibility or migration paths. Verify representative producer/consumer round trips and absence of private launch data; update public contract documentation.
- [ ] 8.2 Complete task 4.1: one included registration row joined to its optional claim/subject and topology; no duplicate plain pane or default detached-subject rows. Use a derived row record composing registration and optional claim/subject, not a durable row store or another placement owner. Hide revoked never-presented launches; accept one-time row-key selection/fold reset. Verify launch-to-presence row identity, a reserved successor sharing a subject, unplaced executions and unregistered panes under existing history filters; update dashboard documentation.
- [ ] 8.3 Preserve exact registration and attachment identity in selected/frozen actions as row enrichment and grants change. Verify confirmation cannot retarget a successor and verified pane moves change containment without changing execution identity.

## 9. Rework integration acceptance

- [ ] 9.1 Run disposable live acceptance on revised `L`, fresh child, continued child, restart and branch close, including a launch without a spawn edge. Observe one row through launch and grant, no separate reservation/subject row, and no successor dispatch before verified predecessor exit. Record any operator-only acceptance still outstanding rather than claiming it passed.
- [ ] 9.2 Run fmt, strict Clippy, all Rust targets, pi-radar contracts/typecheck/tests/package checks, strict OpenSpec validation and the checked Nix package build on the revised tree. Review the diff for copied state, parallel lifecycle paths and leftover compatibility before claiming completion.

## Workflow follow-up

- The general execution-policy refactor in plan.md is subsequent work, not an unchecked prerequisite silently expanding this change.
- Review the revised implementation and operator acceptance evidence before archive.
- Sync and archive this existing change only after the revised tasks pass.
- Merge, tags, release and live deployment require separate explicit approval.
