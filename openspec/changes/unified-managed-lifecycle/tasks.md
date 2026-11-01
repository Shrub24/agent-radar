# Tasks

Planning only. Each group lands working behaviour, its relevant tests and docs; no green scaffolding that remains disconnected from production. Use existing tests before adding cases. Do not implement a generic lifecycle framework or compatibility lane.

## 1. Common admitted execution and completion slice

- [x] 1.1 Add claim-scoped verified placement through existing records/events and use it in direct admission. Verify an admitted execution without a spawn edge can be resolved from its pane, and a failed generation/placement check records no destructive authority. Document the subject/claim/pane distinction alongside the lifecycle interface.

  Landed as a recorded `Placement` on the execution, written when a tracked launch claims its program and at in-place admission, and as `placements()`/`placed_at()`. An admission whose serving process cannot be verified records a placement with no instance, so it authorises nothing later. The three lifetimes are documented at the top of `lifecycle.rs`.
- [x] 1.2 Implement the common supervised `radar run` execution chain for direct and claimed entrypoints, with the actual program identity, authenticated completion and existing scope association. Verify with a real PTY that input and Ctrl-C work, a nonzero status is captured, and a shell-hosted agent returns to its shell without pane death. Use an isolated user-manager fixture where a scope is required; never create units on the operator's real manager as a hidden test effect.

  A real tmux pane test covers input, exit-status propagation to the pane's shell, Ctrl-C reaching the program, and the shell surviving the run; it uses a stub scope launcher so no unit is created. The real `systemd-run --scope` path (Ctrl-C across a real scope) has **not** been exercised and remains a live check.
- [x] 1.3 Persist completion on the claim at the event, and project it into existing spawn reads. Remove both read-triggered capture call sites and competing exit authority. Verify a captured code survives subsequent pane deletion and repeated reads need no live backend; unavailable status permits an otherwise valid completion/cleanup and is never zero-filled. Update completion/provenance docs in the same slice.

  Completion is a `Completed` event on the claim, written once by the supervisor's authenticated report; both read-time capture calls, `observe_spawn_exits`, `record_spawn_exit` and the edge's own `exit` field are deleted (stored edge version 3), and spawn reads project the claim.
- [x] 1.4 Route configured `L` through managed reservation/release after its existing target-shell/direnv resolution instead of a naked recipe. Verify `L` and direct `radar run --agent` both reach supervision/registration, and preserve target cwd, command resolution and source-pane isolation. Update launch docs and generated affected DTOs.

  Spawned launches run the same two-role supervisor as `radar run`; the claim carries the supervisor key, and the pane-occupant check accepts the claimed program as a descendant of the pane's foreground. The recipe is persisted at capture so a tracked spawn's report is accepted in production.

## 2. Common termination and agent close

- [x] 2.1 Implement one claim-addressed termination primitive on the existing verified-scope seam: TERM, shared graceful deadline, verified-scope KILL escalation and bounded verification. Verify a responsive execution stops gracefully, a TERM-ignoring execution is boundedly escalated, and a mismatched/unverifiable scope is never killed. Keep real identity/fencing behaviour in scope tests rather than mocking the property under test.

  Escalation is one function, `lifecycle::escalate`, so the deadline that calls it today and the operator request that will call it later cannot drift apart; an emptied verified scope is the evidence the execution is gone, which is what a zombie or an unreaped child looks like to a pidfd. Landed as one `Shutdown` window (graceful + escalation) shared by `cancel`, `stop` and `cancel_many`, with escalation of a verified scope through `systemctl --user kill --signal=KILL`. An unreadable scope withholds escalation but not the TERM, because the process is fenced by its own identity — the old rule settled without signalling and is restated in the unit test. A TERM-ignoring launch inside a real verified scope is escalated and then ends, covered by `a_managed_launch_is_scoped_before_it_runs_and_survives_a_restart` and `a_launch_whose_process_ignores_term_is_escalated_before_its_pane_is_closed`; the escalation is named in the operation's effects as `scope escalated` until typed close outcomes replace that vocabulary.
- [x] 2.2 Implement one domain close path that composes termination, completion capture and generation-fenced pane removal. Verify one Close on an admitted agent removes its pane, missing code cannot hold removal, and a replacement claim/backend generation is not destroyed by stale authority. Ordinary shell closure must not require a managed scope. Document Close versus spontaneous exit.

  The order holds in one place: terminate, capture, remove. Removal is dispatched
  through the single bounded close call, and the authorization that decides whether
  this pane is Radar's to remove stays with each caller, because it needs the
  backend and the verification budget. Close versus spontaneous exit is documented
  in `context/lifecycle.md`. Capture-before-removal is `subjects::await_completion`
  taken by both removal paths, and `a_reported_code_survives_the_pane_a_close_removes`
  reads the code back after the pane is gone.

  Where the acceptance clause does not apply as written: a directly admitted
  `radar run` execution has **no** spawn edge, so its pane is the operator's and is
  deliberately not removed. Only a pane Radar created is removable, which is why
  the removal policy keys on the recorded edge, not on the presence of a claim.

- [x] 2.3 Route lifecycle cancel, spawn/subject child-close, expiry and revoked-launch cleanup through the shared primitives with their existing authorization differences. Replace `cleanup_launch_pane` and child-close dispatch duplication rather than retaining bypasses. Verify operator/parent close parity, foreign-child rejection, already-ended close without a code, and durable same-ID replay without redispatch. Retain stop-only semantics for the explicit Stop intent.

  The launch cleanup no longer carries its own dispatch: it calls
  `bounded_close_dispatch` and translates the answer through `PaneCleanup::from`,
  which is the same translation every caller of that dispatch now shares. Child
  close keeps its own admission, record and refusal vocabulary, and takes the same
  completion grace before dispatching. Stop still removes nothing.
- [x] 2.4 Publish typed close outcomes separating resource removal, execution verification and optional completion diagnostics; update Rust DTOs, schemas, generated SDK types and pi-radar interpretation. Verify normal completion, unavailable status and confirmed survivors have distinguishable meanings. Update protocol/SDK docs and bump actual incompatible versions together; keep labels such as complete/cancel free of assignment-settlement effects.

  Landed: `closed` on the public record with `execution` (`exited` / `unverified` /
  `unsignalled` with its reason), `resource` (`removed` / `absent` / `kept` /
  `unverified`, absent only for an operation that removes nothing) and
  `completion` (present only when a supervisor reported), plus `escalated` as a
  typed fact rather than a word in a prose list. Wire revision 14; Radar 0.10.0.
  An already-absent pane is now reported `absent` instead of dispatching a close
  that answers "can't find pane" and being recorded as unverified.

  No schema or generated SDK change: `PublicRequestRecord` is not part of the
  generated contract surface, and pi-radar reads neither `effects` nor the record,
  so its revision floor stays at 13 rather than forcing an upgrade that gains it
  nothing.

## 3. Recursive container closure

- [x] 3.1 Replace spawn-edge-only `subjects_inside` discovery with verified execution placement and domain topology. Use one recursive close interface for logical children, deduplicating claims. Verify a filtered-out direct-admission agent and a spawned agent are both handled inside the selected container, including verified pane movement; no filtered UI tree or fabricated spawn edge may supply authority.

  Discovery now starts from a verified placement: the pane an execution recorded,
  with the instance that made it, matched against the place being closed. A
  directly admitted `radar run --agent` execution has no spawn edge and is found
  all the same; a bound spawn edge with no placement is no authority at all, so an
  execution admitted before placement existed is not discovered — the upgrade
  procedure says to restart such agents rather than to close a place over them.

  Building this exposed a semantic error in the discovery predicate: revocation is
  cancelled work, not an ended process, so filtering out revoked launches would
  hide a live process inside a place about to be removed. Placement discovery
  ignores only the evidence that says an execution ended.

- [x] 3.2 Initiate leaf graceful termination before waiting, share one deadline across the branch and remove leaves after capture attempts. Apply one bounded root-removal fallback if required. Verify a multi-agent branch does not pay a timeout per child, ancestor auto-removal succeeds, and a failed child discovery/unknown leaf removal does not indefinitely block verified root removal. Preserve unverified process outcomes in the result.

  Every member is signalled before any is waited on, and one `Shutdown` window is
  created for the whole branch, so a five-agent branch pays one grace rather than
  five. Before the root is removed the branch takes one shared completion window
  (`await_completions`) for every execution that has an identity and no report, so
  the codes are captured before the panes carrying their supervisors are removed,
  and a report that never arrives is a diagnostic rather than a block.

- [ ] 3.3 Add an in-flight closing-branch guard to domain admission and the shared publication. Verify an overlapping Radar launch/restart is promptly refused while unrelated branches and UI observation remain responsive; a changed backend generation cannot be removed through the old root reference. Document that native tmux mutations are handled best effort, not transactionally.

  Domain admission is guarded: a close holds the branch it is removing (the target's
  own handle plus the pane of every execution it found, in memory, for the duration
  of the close), and `spawn.create`, `launch.admit` and `lifecycle.restart` refuse
  naming a held place with `busy` before staging any evidence. Overlapping closes
  hold only what they added, so one close cannot release another's branch. The hold
  is memory only on purpose: it says what this process is doing now, and a restart
  mid-close correctly holds nothing, because the durable operation record is what
  says the outcome is still owed.

  Not done in this slice: publishing the closing state on the shared publication, so
  a view can show a branch as closing rather than as gone-but-not-yet-removed. That
  is a wire change of its own; admission — the part that can corrupt topology — is
  guarded now. Native tmux mutations remain best effort: an operator splitting a pane
  by hand inside a closing branch is not transactional, and the close removes the
  enclosing resource regardless.
- [x] 3.4 Wire `topology.delete` to the common container close implementation and remove its separate cancellation/close orchestration. Verify real entrypoint replay, container-removal recording and confirmed-survivor reporting using existing integration fixtures. Update container-close docs without introducing nested public RPC request records.

  There was no second orchestration left to delete. After 3.1 and 3.2 the delete path
  already composes the shared pieces — `executions_inside` for discovery,
  `lifecycle::cancel_many` over one `Shutdown` window, `await_completions` before the
  root is removed, and `lifecycle::close` for the root itself — and holds no copy of
  any of them. The separate thing 3.1 did remove was the spawn-edge scan, which was
  the duplication this task was written for. Replay, container-removal recording and
  confirmed-survivor reporting are covered by the existing fixtures, which now
  exercise discovery through placement.

## 4. Subject restart and TUI cutover

- [ ] 4.1 Compose restart from shared termination/completion/start primitives and existing successor reservations. Verify same subject/new claim, no launch over unverified predecessor termination, and no second signalling of a replacement through an old claim. Reuse eligible dedicated panes; create replacement placement for shell-hosted admission rather than typing into a resumed shell. Update restart placement docs.
- [ ] 4.2 Replace TUI delete/cancel/raw-close branching with one semantic Close request carrying explicit local row identity. Resolve to claim/pane/container in the daemon and retain frozen confirmation. Verify a selected agent different from focus closes that agent, a vanished explicit row refuses, and mouse/palette reach the same close behaviour. No row-owned lifecycle trait or frontend target derivation.
- [ ] 4.3 Present root removal normally, show actionable verified survivors/removal failures and keep missing-status detail diagnostic. Verify the view remains responsive during bounded shutdown and rows reconcile through publications rather than optimistic disappearance. Update key help, README and changelog for one-press `D`.

## 5. Convergence audit and upgrade integrity

- [ ] 5.1 Complete the targeted divergence inventory in design.md against the final production flows: admission/claim, configured recipe, single/batch cancellation, child/subject close, expiry/revoke, restart/continue and topology reload. For each, record its authorization, shared implementation and legitimate remaining policy. Remove unused helpers and overwritten comments; list unrelated candidates with concrete evidence instead of refactoring them opportunistically. Review the delivered diff for lean Rust ownership, minimal state and existing trait seams.
- [ ] 5.2 Add explicit record/wire version checks and the archive/reset upgrade procedure where shapes changed; no silent migration or fallback. Verify unsupported stored records refuse with their location, current records reopen, public projections expose no completion credentials/launch secrets and SDK generated contracts are current. Do not tell users to hard-reset communications merely to upgrade lifecycle state.

## 6. Integrated validation

- [ ] 6.1 Run fmt, strict Clippy, locked Rust suites, SDK tests, schema/type/package checks, strict OpenSpec validation and rationale lint; report exact failures and any existing ignored tests. Run affected Nix evaluation and package checks only if packaging changed or for a release, not as routine Rust validation. No claiming green from filtered output or excluded failing behaviour.
- [ ] 6.2 Exercise disposable nested tmux scenarios through real entrypoints: `L`, shell-hosted `radar run --agent`, spontaneous exit, one-press `D`, restart, and workspace deletion with responsive and stubborn agents. Verify capture before pane loss, shell preservation on spontaneous exit, terminal removal on Close, bounded branch latency and no surviving verified scope after successful escalation. Record which systemd/PTY claims were actually exercised; live operator Kitty acceptance remains a separate check.

## Workflow follow-up

- Review the proposed placement/restart policies and supervisor proof obligations before applying. Do not start implementation from this planning run.
- Sync predecessor UI and managed lifecycle/topology deltas in their dependency order, then reconcile overlapping requirement/scenario changes before syncing this change. Validation of a delta is not proof that its archive baseline is ready.
- Archive after implementation, consumer coordination and the agreed live acceptance. Full recovery of interrupted unknown operations, lead inheritance/pane gathering, all-shell scopes and a separate diagnostics screen remain deferred.
