## 1. Checks

- [x] 1.1 Confirm a spawned launch's private record holds the program, cwd and environment overlay in replayable form, and that exit capture does not depend on the bootstrap process.

## 2. One launch chain

- [x] 2.1 `radar run --launch` claims a recorded launch (correlation + birth identity), receives registration and program, wraps the scope and execs; every daemon-created launch uses it.
- [x] 2.2 Remove `launch-bootstrap`, `spawn.bootstrap` and daemon-side scope wrapping.

## 3. One relaunch

- [x] 3.1 Restart and continue share `resume_launch` (the recorded session replaces any selector) and `spawn_child` with `create_with_intent`'s one-successor rule. A separate `relaunch` function was not needed: the two callers differ only in admission and in whether an incumbent is stopped first.
- [x] 3.2 `lifecycle.restart`: the operator with the captured recipe and claim fence; the recorded parent by subject alone with its own program and args.
- [x] 3.3 `spawn.create` `continue_subject_id`, refused while a claim is live or the subject is not the caller's child.
- [x] 3.4 A continued launch records the caller's owner, run, label and role on its own claim; the parent stays on the subject. A parent restart keeps the metadata already recorded.

## 4. Contract and release

- [x] 4.1 Wire revision 7, pi-radar `launchRadarWorker` `continues` and a subject-addressed `restart` client method.
- [x] 4.2 Tests at the owned seams: a continuation binds its edge to the existing subject; restart and continue apply one session rule; existing lifecycle and scope suites cover the launch chain.
- [ ] 4.3 Docs: `docs/managed-execution.md`, the Herdsman brief.

## 5. Launch deadline

- [x] 5.0 One exclusion: the subject's authority pointer. Reserving compares it against the named predecessor; granting and the duplicate decision read it through `attachment`; the session-wide claim scans and the separate successor refusal are gone. `successor` stays only as display data.
- [ ] 5.1 Confirm each launch path (L, spawn, continue, restart successor) has a spawn edge with a stable `recorded_at`; if the restart successor has none, record one.
- [ ] 5.2 Withdrawal in the sweep: a bounded cursor pass over unpresented launches past the 30s ceiling runs the cancel path and records `withdrawn` with a reason on the edge (additive field; wire revision 8).
- [ ] 5.3 One daemon test: a launch that never presents is withdrawn, a late presentation is refused, and the subject accepts a continue afterwards.
- [ ] 5.4 pi-radar: `launchRadarWorker` waits on the edge for granted or withdrawn and drops `timeoutMs`; add `children.restart(subject, program)` and `children.continue` on the same wait; a held assignment follows. One SDK test for the failed outcome.
- [ ] 5.5 Release (breaking changes accepted, including dropping `timeoutMs`): Radar 0.5.0, pi-radar 0.10.0, revision 8, changelog, README, Herdsman brief.
