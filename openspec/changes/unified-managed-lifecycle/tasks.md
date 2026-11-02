# Tasks

Implementation is in progress. A checked task means its whole behaviour and acceptance
criteria are satisfied, not merely that a helper exists or a suite passes.

## 1. Common admitted execution and completion slice

- [x] 1.1 Record verified execution placement through existing records/events at direct admission and tracked claim. An unverified server supplies no destructive authority. Distinguish subject, execution and pane lifetimes.
- [ ] 1.2 Prove common supervision with actual program identity, authenticated completion and scope membership: PTY input, Ctrl-C, nonzero status and shell return. The tmux tests cover terminal behaviour with a stub scope launcher; `tests/lifecycle_acceptance.py` now proves completion, foreground Ctrl-C and shell return with disposable scopes on the real user manager. An isolated-user-manager fixture has not been exercised.
- [x] 1.3 Record completion on the execution at the supervisor event and project it into spawn reads. Read-time backend capture and the spawn edge's competing exit field are removed. Missing status never prevents cleanup.
- [ ] 1.4 Route configured `L` and spawned launches through managed reservation/release, preserving target cwd, resolution and source-pane isolation. Spawned launches use supervision; configured `L` still needs a production-entrypoint audit against the recipe creation path.

## 2. Common termination and agent close

- [x] 2.1 Share termination through verified process/scope evidence and a shutdown window. Deadline escalation uses `lifecycle::escalate`; an unreadable scope prevents group escalation, not a fenced process TERM. Zombies are not live processes.
- [ ] 2.2 Compose termination, completion capture and generation-fenced pane removal in one domain Close. Reopened: `cleanup_launch_pane` still depends on a spawn edge and does not remove an admitted agent's pane. That is contrary to the agreed explicit-Close behaviour; shell preservation applies to spontaneous exit, not Close.
- [ ] 2.3 Route cancel, parent/child close, expiry and revoked-launch cleanup through that composition, preserving authorization and replay. Reopened: `execute_child_close` shares only bounded pane dispatch and completion grace; it does not call execution termination before removal. Explicit Stop must continue to leave the pane.
- [ ] 2.4 Publish typed close outcomes across all close adapters. Cancel/stop carry `closed`; `topology.delete` still uses prose and `effects`, and child close retains its separate result. Complete producer/consumer wiring before claiming typed close convergence.

## 3. Recursive container closure

- [ ] 3.1 Discover executions by verified placement, not spawn edges or the filtered UI tree. Registration-keyed discovery now fences the placement's server generation and stops exact execution registrations. Native pane movement is not reflected in recorded workspace/tab placement yet; verify movement before closing this task.
- [ ] 3.2 Signal all branch members before waiting, share shutdown and capture windows, then apply bounded enclosing-resource removal. Shared TERM/capture windows exist, but `cancel_many` still waits through escalation per member: a slow first escalation can consume the hard deadline before later members are escalated. Verify the multi-scope path, not only a single stubborn child.
- [ ] 3.3 Refuse new work in a closing branch and publish the closing state. Holds now cover frozen empty panes as well as discovered placements, canonicalise direct-admission pane IDs, guard topology creation, and reference-count overlaps in either completion order. Closing-state publication and admission/close race coverage remain open. Native tmux mutations are best effort, not transactional.
- [ ] 3.4 Route topology deletion through the common container-close composition rather than separate policy orchestration. It currently combines `executions_inside`, `cancel_many`, completion grace and root removal itself. Preserve durable replay, removal records and unverified-exit reporting during convergence.

## 4. Subject restart and TUI cutover

- [ ] 4.1 Preserve the subject, reserve a new claim, stop the exact predecessor and launch only after verified exit. Existing restart covers those invariants. A spawned launch's own exited pane (same serving generation, anchoring the restart) is launched into again through the common launch sequence; a shell-hosted admission gets replacement placement beside it, never a command typed into its shell. Verified live: the restarted agent keeps its pane.
- [ ] 4.2 Submit one semantic Close carrying explicit local row identity; resolve exact execution/pane/container in the daemon and freeze confirmation. The shared command is renamed `close` and selection stays frontend-local, but resolution still branches between cancellation and mux/container close, and placement-based resolution for an admitted execution without a subject is not wired. Verify selected-agent versus focus, vanished rows, palette and mouse through the production route.
- [ ] 4.3 Render actionable execution/removal failures without optimistic row disappearance; keep missing completion diagnostic. The typed renderer now distinguishes uncertain removal from a kept pane and reports escalation as attempted rather than guaranteed. Container-close failures/survivors cannot reach it until 2.4 is complete. Verify responsive drawing during bounded shutdown and update operator help with the final behaviour.

## 5. Convergence audit and upgrade integrity

- [ ] 5.1 Complete the targeted flow inventory against production callers, not shared-helper names. The design table records current policy differences; child-close termination, admitted pane cleanup, configured L, movement and container outcome convergence remain unresolved. Remove unused helpers and narrative comments without a generic trait/actor rewrite.
- [ ] 5.2 Verify actual record/wire changes and a non-destructive cutover. Spawn edges changed to version 3; child-close versions did not change. Old spawn edges refuse on read, not necessarily at daemon startup. A tested archive/cutover procedure is still required. Preserve unrelated communications and never recommend `reset --hard` merely to upgrade lifecycle records. Check redaction, reopen/replay, unsupported versions with paths, generated contracts and packaging.

## 6. Integrated validation

- [x] 6.1 Review gate passed: fmt, strict locked Clippy, all locked Rust suites, two additional full control-plane runs, binary build, real-scope acceptance, SDK contracts/typecheck and 133 tests, package import check, OpenSpec 38/38, rationale lint and whitespace check. The command chain exited 0 with unfiltered subprocess output. One existing control-plane latency probe is ignored; rationale lint has 0 errors and 5 prose-heading warnings. Nix was not run; this review changes no packaging inputs.
- [ ] 6.2 Exercise disposable nested tmux through `L`, shell-hosted `radar run --agent`, spontaneous exit, UI `D`, restart and workspace deletion with responsive/stubborn agents. `tests/lifecycle_acceptance.py` passes real-scope completion persistence, foreground Ctrl-C, shell return and pane-container close. Its original failure was a bad oracle: admission records a process, not a subject before registration. Remaining scenarios and live operator Kitty acceptance are still open; do not treat the partial harness as full acceptance.

## Workflow follow-up

- Sync predecessor UI and managed lifecycle/topology specs in dependency order before archive; strict validation is not proof that the archive baseline is ready.
- Archive only after implementation, consumer coordination and live acceptance. Interrupted-operation recovery, lead inheritance, pane gathering, all-shell scopes and configurable/manual escalation remain deferred.
