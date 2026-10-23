# Tasks

Consolidate the identity model first — the scope work names an attempt in every unit, slice and control — then establish the scope at the existing launch seam, then cut `radar run` and `L` over to it. Measured constraints from design.md apply throughout: `--expand-environment=no`, direct argv only, no shell or redirection in the pane command, and scope state is never an outcome.

## 0. Identity consolidation (prerequisite)

- [x] 0.1a Give the agent its own record — identity, managed metadata, session and the current attempt — written by the attempt writer and promoted from attempts that predate it; verify an existing chain reads back with its newest attempt current, a late earlier attempt does not displace it, and a chain whose record was removed is promoted on read
- [x] 0.1b Rename the durable agent identity to `subject_id` and the attempt record to a claim across storage reads, the wire and generated contracts, so the agent owns the plain name; verify a stored attempt reads back with its existing id as its claim id
- [x] 0.2 Make a claim the record of one attempt, carrying expected and bound process, recipe, signal and exit evidence and the successor; verify a claim's facts survive a daemon restart and no subject record holds per-attempt facts
- [x] 0.3 Pre-mint a managed launch's claim before dispatch and pass its id in the launched environment; verify the child binds by presenting it and that a presented id whose recorded birth identity differs from the process is refused
- [x] 0.4 Drop `execution_generation` as identity and fence: controls name the claim they were issued against, and a control naming a non-current claim is refused without targeting the successor; verify a stale control after a restart has no effect
- [x] 0.5 Let pre-minting be the exclusive act per subject: verify two concurrent launch paths, one a restart, leave exactly one current claim and the other refused or pending, and that a restart pre-mints before it signals
- [x] 0.6 Move attachment release off the attempt onto the subject's current claim, retire the per-edge spawn token in favour of the pre-minted claim, and verify a release while the process runs leaves the attempt readable and the subject unattached

- [x] 0.7 Record the logical parent subject on the spawn edge when the launch is admitted, so parentage is a durable key rather than a session correlation; verify an edge created by a managed launch reads back with its parent subject and an unmanaged parent records none
- [x] 0.8 Serve a caller's own launches as a derived read — the daemon resolves the caller's current subject and returns that subject's edges — and keep any explicit parent filter on the operator surface only; verify a caller cannot read another subject's launches by naming it and that the refusal names the reason
- [x] 0.9a Pre-mint the registration with a managed launch and pass it in the environment instead of the claim id; reserve the subject's claim with a distinct `reserved` standing that is visible and cancellable but carries no attachment authority; verify the registration's birth identity and session exclusivity at the first report and grant the client's fresh claim; delete pi-radar's first-load rule. Verify first-report admission and handoff routing, fresh claims on reload, refusal of a mismatched process, and that an unverified report neither frees the reservation nor grants scoped-read authority
- [x] 0.9b Complete registration ownership of process-lifetime facts and launch bindings rather than retaining per-claim attempt records; allow a launch without a subject to have no claim; resolve controls through the registration while retaining exact attachment fencing. Verify that session changes leave the process identity and lifetime evidence intact, stale controls cannot target a replacement process, and an undispatched restart reservation rolls back when its incumbent cannot stop

## 1. Scope launch composition

- [x] 1.1 Build the launcher argv that wraps the existing bootstrap in `systemd-run --user --scope` with `--expand-environment=no`, `--quiet`, `-p SendSIGKILL=no`, an explicit `--unit` and `--slice`; verify a unit test asserts the exact flags and that no argv element is exposed in the name or description
- [x] 1.2 Derive unit and slice names from the registration id with a systematic, collision-free encoding; verify two distinct launches never collide, a retried admission reuses its name, and a reload or `/resume` neither renames nor loses the scope
- [x] 1.3 Compose the pane command as direct argv with no shell, redirection or metacharacter; verify a test refuses a recipe that would require them, and that the pane's process is the workload rather than a shell

## 2. Refusal before execution

- [x] 2.1 Refuse the launch before exec when the scope cannot be created or the application cannot be admitted, recording a refusal and executing nothing; verify a test with an unavailable user manager fails closed with no unscoped launch
- [x] 2.2 Keep admission durable and ordered before execution; verify a launch whose record cannot be written does not run

## 3. Execution record and recovery

- [x] 3.1 Record the scope and slice identity against the registration, without a new store; verify a readback returns them and no public projection exposes environment values
- [x] 3.2 Reattach a recorded launch after daemon restart by matching the recorded unit and a verified process birth identity inside the group; verify a test reports it running without signalling it, and that a reused name with a mismatched identity stays unresolved
- [x] 3.3 Report a missing transient unit as unknown rather than a clean exit; verify the swap-in of `--collect` or unit expiry cannot turn into a settled outcome

## 4. Graceful-only stop

- [x] 4.1 Stop the execution's main scope without SIGKILL escalation and report survivors when processes remain; verify a test asserts no escalation, survivor names in the result, and that the failed/timeout unit state is tolerated
- [x] 4.2 Keep independent task and subagent scopes out of a main-scope stop; verify a test shows a sibling group is untouched
- [x] 4.3 Stop a managed execution through its scope in every control that stops one (`D`, cancel, restart), reusing the one-window batch wait; leave the signal path for observed sessions as it is with no new work, and verify a managed stop never takes it

## 5. `radar run` and TUI cutover

- [ ] 5.1 Add `radar run -- COMMAND [ARG...]` that verifies the current pane belongs to this daemon before executing in place; verify an integration test runs a command in an existing pane, keeps its cwd and environment, and creates no pane
  - Implemented: the daemon admits only a caller that is the named pane's own descendant with a verified birth identity, records the process, and answers the registration for the caller to enter its scope with; the command is never executed directly. Refusals are covered by `tests/cli.rs`. The integration check of a real in-place run is left to the operator's live session: driving an interactive pane from the test harness proved unreliable (keystrokes and even a direct `new-window -- sh -c` did not stay observable), and the behaviour it would assert is what a real run shows faster.
- [x] 5.2 Refuse `radar run` on an unverified pane or an empty command before anything executes; verify both refusals
- [x] 5.3 Route `L`, restart and Herdsman spawn through the same managed launch; verify the existing `L` targeting tests still pass and a scope failure refuses before the command runs
- [ ] 5.4 Ship a `pi` launcher for Nix and Home Manager that prefixes `radar run`; verify the launcher runs a managed Pi and that a manually started Pi keeps its identity, children, inbox and controls
  - Implemented as `lib.managedAgent` and `programs.radar.managedAgents`: the wrapper takes the program's own name and names the original by store path, so it cannot resolve back to itself (the first `radar-pi` package could). Built against `pkgs.hello` and its script checked. The run of a real managed Pi is the operator's live acceptance.
- [x] 5.5 Document `radar run`, the managed/observed distinction and the graceful-stop semantics in `docs/` and the README; verify the documented commands run as written

## 6. pi-radar and consumers

- [x] 6.1 Mark an observed session as not first-class, with a hint to restart it under `radar run` for scoped control, from the standing pi-radar already reports; verify it gains and loses no control
- [x] 6.2 Regenerate consumed protocol contracts and update the SDK docs and the Herdsman companion brief; verify generated-contract and package tests pass

## 7. Packaging and platform

- [x] 7.1 Make the systemd user-manager dependency explicit for the daemon and package, and state the failure behaviour when it is unavailable; verify `nix build` and `systemd-analyze --user verify` on the installed units
- [x] 7.2 Record the measured platform requirements (user manager, cgroup v2) and the graceful-stop consequences in the operator docs; verify the doc matches the flags in 1.1

## 8. Integration verification

- [x] 8.1 Prove end to end on a disposable tmux server and disposable daemon: the workload is a scope member before it runs, its exit status reaches the typed tmux observation while the scope reads `success`, a surviving process after a graceful stop is reported, and a daemon restart reattaches without signalling; verify each assertion from real command output
- [x] 8.2 Run fmt, strict Clippy, locked Rust tests, pi-radar package and generated-contract checks, strict OpenSpec validation and the checked Nix build, recording actual exits

## 9. Review follow-ups (2026-10-24)

Found by reading the code and the live user manager, not by running it. First written at `6ca3a383`, re-checked at `37635df4`, and re-verified after `f78558bb`. 9.8 is fixed.

Order: 9.5 → 9.4 with 9.6 folded in → 9.10 → 9.1 → 9.9 → 9.3 → 9.2.

- [x] 9.8 `radar run --agent` could not bind: admission created a process record but no spawn edge, so `launched_registration` refused with "no recorded spawn edge was minted with this token", and the Nix launcher always passes `--agent`. Fixed in `f78558bb`: admission captures the verified birth identity and an admitted agent binds its own registration (`Origin::Admitted`, `bind_admitted`) with no invented edge or parent, while a presented command registration still refuses. Live caveat: this needs Pi to `exec` its node process. If Pi forks, the pid differs from the admitted one and the bind refuses as a mismatch. That is correct behaviour and will look like a regression.
- [x] 9.5 Tests drive the real `systemd-run` on the developer's user manager (`scope_launcher()` in `tests/control_plane.rs`), which created most of the leaked slices, and the path is skipped in the Nix sandbox, so a sandbox run and a dev run do not cover the same code. Use a fake launcher that execs the argv after `--`. Verify a full test run leaves no `radar-*` unit behind. Done narrowly: only the one test that runs a real tmux execs the launcher (the others use a backend that never execs), and a fake cannot stand in because `Scope::verify` reads the real `/proc/<pid>/cgroup`. That test now stops its slices and clears their failed state in a drop guard, including on assertion failure.
- [x] 9.4 Managed launches leave units behind: every launch's `radar-<registration>.slice` stays active and a graceful stop with survivors leaves its scope `failed` (530 `radar-*.slice` units on 2026-10-24). Cleaning at recorded exit alone still leaks, because an exit is recorded only when something observes it: `radar run` installs no pane-death wake, a command carries no claim so the recovery sweep's `read_recorded` never applies to it, and slices older than a state reset have no records at all. Add a sweep — at daemon start and on the existing reconcile cadence, paged like the other passes — that stops each `radar-*.slice` whose cgroup is empty and `reset-failed`s dead `radar-*` scopes. Emptiness is read from `cgroup.procs` and never inferred from a recorded exit, because a slice stop ends anything inside it. Do not use `--collect`: an unloaded unit loses the `ControlGroup` lookup a later re-stop of survivors needs. `radar reset` calls the same sweep, which also replaces the manual cleanup step. Verify with a recording fake `systemctl`. Done: `managed_scope::sweep`, run by the daemon every 60s, first after one interval, and by `radar reset` after its drain. A running daemon stops a slice only after two consecutive empty passes, because a launch creates its slice before its scope joins it; reset has no daemon and stops on first sight. 9.6 is folded in and narrowed: `cancel_many` decides survivors from evidence (`exited` and `remaining()`), never from the stop result, so the stop error is now logged instead of discarded rather than added to the outcome.
- [x] 9.10 A stop reports a clean exit for a scope that still holds members. `stage()` handles `Acquired::Absent | Replaced` by recording the exit and returning `Settled(Exit::Exited)` before it ever looks at the scope, so a backgrounded child or a daemonising command survives unreported and the recorded exit makes the workload look finished. When a scope is recorded, an absent main process must still go through the scope stop and the survivor check, as `Managed stop stays graceful` requires. This is also what makes 9.4 safe. Done in `settle_absent`: while the scope is readable its members are stopped and counted and the exit is recorded only once none remain; an unreadable scope is an unloaded unit with no members, so the verified main-process exit stands. Not covered by an automated test: the survivor check reads the real `/sys/fs/cgroup`, so it needs a live scope. Confirm in acceptance with a command that backgrounds a child and exits.
- [x] 9.1 The daemon answers `{unit, slice}`, both a pure function of the registration id (`Scope::for_registration`), so a naming rule sits on the wire where the receiver can hold a second copy of it. Answer only `registration_id` with typed params and result, and derive the scope in the client. Typed structs add nothing once the field is gone. Verify that a verified pane descendant is admitted and enters the scope its registration names.
- [x] 9.9 `docs/managed-execution.md` says Radar "reports and stops" a managed command "as a scope member". It does not: `cancel_target` resolves a subject or a spawn edge and a command has neither, `registry_observation` has no command path, and only reset reaches a command scope through `verify_unit`. Narrow the docs to what exists. A registration-keyed stop and a report row wait for a consumer.
- [x] 9.3 `design.md` §2 says `radar run` runs through `launch-bootstrap`; it execs the program straight from `systemd-run`. Amend the design to state that this path has no release gate, and why: `spawn.bootstrap` releases a subject-shaped launch, a command has no subject and an admitted agent has none until it binds, and `cancel`/`stop` cannot reach an admitted process at all, so a gate would guard a cancel that cannot be issued. The fail-closed properties do not depend on it.
- [ ] 9.2 Verify that a stop of an admitted process goes through its scope, then close. Admission previously dropped the identity it had just verified, so a stop settled `Unresolved` before reaching the scope; `f78558bb` captures it for both kinds, so the earlier "a command never gains one" no longer applies. One test, no re-implementation.

## Workflow follow-up

- Complete and archive `unified-pi-registration` first; this change revises its adoption policy and consolidates the identity it introduced.
- Agree the reset's scope before rollout, and state what durable history, handoffs and inboxes it discards. The identity consolidation re-keys nothing, so it does not by itself require a reset; the scope work still does.
- Section 0 gates section 1: unit and slice names derive from the claim id, so scopes built before the merge would need renaming afterwards.
- Live acceptance against a real Pi session, and the Herdsman cutover, remain operator-owned follow-ups.
- Sync and archive this change once the gate and integration checks pass.
