## Decisions

**1. `radar run` claims a recorded launch.** The two existing chains differ only in how the process proves it is the launch and in who wraps the scope. A daemon-created launch proves itself with the correlation secret in its environment plus its birth identity, as the bootstrap does today; an operator launch proves itself as the pane's verified descendant. Everything after the proof is shared: the daemon answers with the registration and the program, and `radar run` wraps the scope, sets `PI_RADAR_REGISTRATION_ID` for an agent, removes the launch variables and execs. `systemd-run --scope` keeps the pid, so the claimed identity stays valid. Delivering the program from the daemon's record instead of typing it into the pane keeps environment values private and removes shell quoting of argv. This reverses the earlier amendment that `radar run` has no release gate.

**2. One relaunch, args by caller.** `relaunch(subject, program, args, placement)`: the program is executable, cwd and environment; args are the argv. Restart passes the captured kernel recipe; a continue passes what the parent sent, because the parent's environment overlay (generation nonce, mailbox) is new for every launch. Merging extra flags into a recorded argv was rejected: duplicate flags have no defined winner, and Herdsman rebuilds its argv from the agent definition anyway.

**3. The session comes from the agent's record.** `with_session` removes session selectors from args and appends `--session <full id>`; `--session` fails closed when the session is gone, so a deleted session ends the launch instead of creating an empty one. Rejected: Radar minting ids with `--session-id` (encroaches on Pi and silently creates), a session file path (a second locator Pi already resolves), and a subject marker in the session file (a second source of truth for the binding). A first-presence check that the reported session equals the agent's remains as a backstop.

**4. The bare-argument rule applies to both callers.** `resume_launch`'s refusal of a non-flag argument stays, on restart and continue alike, so a message is never replayed. An extension-registered flag that takes a value is refused; that fails closed.

**5. Authority decides the entry point, not the mechanism.** The operator restarts through `lifecycle.restart`; a parent continues or restarts its own child through `spawn.create { continue_subject_id }`, which checks that the subject's recorded parent is the caller's granted subject, that the subject has a recorded session, and that no successor is reserved. Both are claim-fenced; a released subject is fenced by its last claim. A live incumbent is stopped and awaited first, and a reservation that could not dispatch is withdrawn, as restart already does.

**6. Placement follows the pane.** An agent's own pane, if it still exists, keeps it; otherwise the relaunch splits beside the parent. This keeps operator placement for restart (memory #6130) and gives a continue after close a place.

**7. Launch facts per launch.** A continue under a new run must not carry the old run, and editing the subject would erase what each launch was. Owner, run, label and role are recorded on the launch; reads show the current launch's. The parent link is lineage and stays on the subject.

## Risks

- A nix store path recorded as the executable can be collected; a continue supplies its own program, and restart uses the running process's.
- A session that never had a conversation may never have been written, so continuing it fails at `--session`. Documented.
- Removing the bootstrap touches every tracked launch; the existing lifecycle and scope suites are the guard.
