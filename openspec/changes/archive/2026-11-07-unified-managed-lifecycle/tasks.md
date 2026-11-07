# Tasks

Implementation is in progress. A checked task means its whole behaviour and acceptance
criteria are satisfied, not merely that a helper exists or a suite passes.

## 1. Common admitted execution and completion slice

- [x] 1.1 Record verified execution placement through existing records/events at direct admission and tracked claim. An unverified server supplies no destructive authority. Distinguish subject, execution and pane lifetimes.
- [x] 1.2 Prove common supervision with actual program identity, authenticated completion and scope membership: PTY input, Ctrl-C, nonzero status and shell return. `tests/lifecycle_acceptance.py` proves completion, foreground Ctrl-C and shell return with real scopes on the user manager.
- [x] 1.3 Record completion on the execution at the supervisor event and project it into spawn reads. Read-time backend capture and the spawn edge's competing exit field are removed. Missing status never prevents cleanup.
- [x] 1.4 Route configured `L` and spawned launches through managed reservation/release, preserving target cwd, resolution and source-pane isolation. Live: the configured `L` agent is a granted managed subject, restarts keeping its subject and pane, and closes through `D`.

## 2. Common termination and agent close

- [x] 2.1 Share termination through verified process/scope evidence and a shutdown window. Deadline escalation uses `lifecycle::escalate`; an unreadable scope prevents group escalation, not a fenced process TERM. Zombies are not live processes.
- [x] 2.2 Compose termination, completion capture and generation-fenced pane removal in one domain Close (`places::close_place`). Live: `mux.close` of an admitted agent's pane ends its scope, records the signal and removes only that pane.
- [x] 2.3 Route cancel, parent/child close, expiry and revoked-launch cleanup through that composition, preserving authorization and replay. Audited against production callers: child close runs `close_place`; cancel and restart run the shared termination then `cleanup_launch_pane`; expiry and revoked-launch withdrawal (`sweeper::withdraw_unpresented`) call the same `cleanup_launch_pane`. All removals end in `places::remove_place` with the serving-generation fence; authorization stays with each caller.
- [x] 2.4 Publish typed close outcomes across all close adapters. Cancel/stop, `topology.delete`, `mux.close` and child close all carry `closed`, and it reports one entry per execution the close addressed (`executions`, wire 16) rather than one verdict for the whole target.

## 3. Recursive container closure

- [x] 3.1 Discover executions by verified placement, not spawn edges or the filtered UI tree. Live: a pane tmux moved out of a tab survives the tab's close.
- [x] 3.2 Signal all branch members before waiting, share shutdown and capture windows, then apply bounded enclosing-resource removal. Live: three TERM-ignoring agents in one tab are all escalated and gone in 2.3 s, one window rather than three.
- [x] 3.3 Refuse new work in a closing branch and publish the closing state. Holds cover frozen empty panes and discovered placements, canonicalise direct-admission pane IDs, guard topology creation, and reference-count overlaps in either completion order. The held places ride the observation (`closing`, wire 15); views mark those rows `closing`. Verified live with stubborn agents: named during the close, released after. Native tmux mutations are best effort, not transactional.
- [x] 3.4 Route topology deletion through the common container-close composition. `run_container_delete` calls `close_place` and keeps only admission, removal records and replay.

## 4. Subject restart and TUI cutover

- [x] 4.1 Preserve the subject, reserve a new claim, stop the exact predecessor and launch only after verified exit. Existing restart covers those invariants. A spawned launch's own exited pane (same serving generation, anchoring the restart) is launched into again through the common launch sequence; a shell-hosted admission gets replacement placement beside it, never a command typed into its shell. Verified live: the restarted agent keeps its pane (4 repeated runs).
- [x] 4.2 Submit one semantic Close carrying explicit local row identity; resolve exact execution/pane/container in the daemon and freeze confirmation. The command is `close`; selection is frontend-local. Covered: a destructive command acts on the displayed row rather than focus, an omitted row resolves current focus only, a gone row refuses without falling back (`session.rs` tests); palette, mouse and `D` dispatch one `CommandId`. An admitted execution with no subject is closed through its pane row: the daemon finds it by recorded placement (live: admitted pane close).
- [x] 4.3 Render actionable execution/removal failures without optimistic row disappearance; keep missing completion diagnostic. The typed renderer reads `closed` from any operation including `topology.delete`; uncertain removal differs from a kept pane; escalation is reported as attempted. Rows of a branch being closed say `closing` until it settles (3.3). The TUI's drawing during a long shutdown is not exercised by a test and is part of the operator acceptance in 6.2.

## 5. Convergence audit and upgrade integrity

- [x] 5.1 Complete the targeted flow inventory against production callers, not shared-helper names. The design table now records every flow's authorization, shared implementation and remaining policy; topology reload is listed as recipe-only and unrelated to managed close. Two unrelated candidates are deliberately not refactored (`control.rs` `effects` parsing, `targets::containment_of`).
- [x] 5.2 Verify actual record/wire changes and a non-destructive cutover. Spawn edges changed to version 3; child-close versions did not change. Cutover run live from the installed 0.9.0 daemon to this build on one state directory: the new daemon starts over 0.9.0 state and keeps its containers. It found that an unreadable spawn edge left the daemon serving while the sweeper's launch pass failed every cycle; startup now refuses naming the file (tested). Still to check: redaction, unsupported versions in the other record paths, generated contracts and packaging. Preserve unrelated communications and never recommend `reset --hard` merely to upgrade lifecycle records.
  - Done against a copy of a real 0.9.0 state directory. The new daemon started and then failed `subject.list` on a version-3 process recipe, so startup now verifies every spawn edge, subject identity and process recipe and refuses naming the file (`a_process_recipe_of_another_version_fails_verification_naming_its_file`). After the file effects of a default `radar reset` the daemon starts and serves the five old subjects detached. The procedure is in the 0.10.0 upgrade notes.

## 6. Integrated validation

- [x] 6.1 Review gate passed: fmt, strict locked Clippy, all locked Rust suites, two additional full control-plane runs, binary build, real-scope acceptance, SDK contracts/typecheck and 133 tests, package import check, OpenSpec 38/38, rationale lint and whitespace check. The command chain exited 0 with unfiltered subprocess output. One existing control-plane latency probe is ignored; rationale lint has 0 errors and 5 prose-heading warnings. Nix was not run; this review changes no packaging inputs.
- [ ] 6.2 Exercise disposable nested tmux through `L`, shell-hosted `radar run --agent`, spontaneous exit, UI `D`, restart and workspace deletion with responsive/stubborn agents. `tests/lifecycle_acceptance.py` passes real-scope completion persistence, foreground Ctrl-C, shell return and pane-container close. Its original failure was a bad oracle: admission records a process, not a subject before registration. The harness now also covers configured `L`, UI restart and `D`, multi-agent and stubborn branch close, admitted pane close, native pane move and nested workspace close. Live operator Kitty acceptance is still open and gates the archive.
  - The disposable half is done by `tests/lifecycle_acceptance.py`; the Kitty half is carried to `docs/operator-acceptance.md`.

## Workflow follow-up

- Sync predecessor UI and managed lifecycle/topology specs in dependency order before archive; strict validation is not proof that the archive baseline is ready.
- Archive only after implementation, consumer coordination and live acceptance. Interrupted-operation recovery, lead inheritance, pane gathering, all-shell scopes and configurable/manual escalation remain deferred.

## 7. Review follow-ups (2026-11-06)

Raised by a full review pass over this change and fixed before release.

- [x] 7.1 Withdraw a reservation that records no spawn edge, keyed on the claim: the sweeper pages `unclaimed`, and a refused create or launch abandons its reservation in the same request. A configured `L` or container reload that never presented left a pending claim that no path resolved.
- [x] 7.2 Delete the guarded direct-close policy (`Closer`, `close_unmanaged`, `guard`, `registrations_in`, `registered_within`, `DirectClose`). It had no production caller and contradicted the daemon-owned close.
- [x] 7.3 Report every execution a close addressed, per execution, with its own end and completion. A place closing over survivors said which ones, not just that some were unverified.
- [x] 7.4 Name the two execution lookups for what they take (`process_of_claim`, `process_of_registration`), and make `record_exit`, `await_completions` and `still_running` take the record they act on instead of a key that could be either.
- [x] 7.5 Stop losing a cleanup reason: a launch record that cannot be read refuses with that reason instead of reporting "the launch records no pane of this daemon's".
- [x] 7.6 Hold the closing place before its inventory is read, so an admission cannot land in a branch already being torn down.
- [x] 7.7 Name the still-running program from its launch record, not from `/proc/<pid>/comm`, which reports whatever holds that pid now.
- [x] 7.8 Remove `managed_scope::kill` (no deadline, no callers), fix doc comments attached to the wrong items, and make a row operation's match exhaustive instead of defaulting to Close.
- [ ] 7.9 Typed identity for claim/subject/registration ids. Attempted and reverted: it is a staged change of its own (domain types, then interfaces, then wire DTOs), not a review fix. See `context/lifecycle.md`, "Identity stays a string at the wire".
  - Moved to its own change, `typed-identity`.
- [x] 7.10 Pane reuse records a create that never happened: the edge's creation outcome says `Completed` for a pane the restart reused. Needs a provenance value on the reuse path.
  - Closed as not needed. The edge's `created` has two readers, the sweeper and reconcile, and both read it as "a pane exists for this launch", which a reused pane satisfies; the request record's effects already say `reused pane <id>`. A provenance field would be a record and wire change with no reader.
- [ ] 7.11 `a_managed_launch_is_scoped_before_it_runs_and_survives_a_restart` is flaky on its real-time windows. The fix is an injectable `Shutdown` duration, not a wider window: widening it would hide the timing property the test exists to check.
  - Not reproduced: six runs under a concurrent library suite all passed in about 4.3 s. Left unfixed until it recurs, since widening a window or injecting one without a reproduction would not show the cause.
