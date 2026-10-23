## 1. Checks

- [ ] 1.1 Confirm a spawned launch's private record holds the program, cwd and environment overlay in replayable form, and that exit capture does not depend on the bootstrap process.

## 2. One launch chain

- [ ] 2.1 `radar run --launch` claims a recorded launch (correlation + birth identity), receives registration and program, wraps the scope and execs; every daemon-created launch uses it.
- [ ] 2.2 Remove `launch-bootstrap`, `spawn.bootstrap` and daemon-side scope wrapping.

## 3. One relaunch

- [ ] 3.1 Split `with_session` out of `resume_launch`; `relaunch(subject, program, args, placement)` with the incumbent stop, reservation withdrawal and placement rule.
- [ ] 3.2 `lifecycle.restart` through `relaunch`: operator with the captured recipe and claim fence; recorded parent by subject with its own program and args.
- [ ] 3.3 `spawn.create` `continue_subject_id`; refused while a claim is live or a successor is reserved through `relaunch` with the caller's program and args.
- [ ] 3.4 Record owner, run, label and role per launch; parent stays on the subject.

## 4. Contract and release

- [ ] 4.1 Wire revision 7, published schemas, pi-radar `launchRadarWorker` `continues` and a subject-addressed `restart`.
- [ ] 4.2 Tests at the owned seams: a continuation binds its edge to the existing subject; restart and continue apply one session rule; existing lifecycle and scope suites cover the launch chain.
- [ ] 4.3 Docs: `docs/managed-execution.md`, the Herdsman brief.
