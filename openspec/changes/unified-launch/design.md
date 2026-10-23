## Decisions

**1. `radar run` claims a recorded launch.** The two existing chains differ only in how the process proves it is the launch and in who wraps the scope. A daemon-created launch proves itself with the correlation secret in its environment plus its birth identity, as the bootstrap does today; an operator launch proves itself as the pane's verified descendant. Everything after the proof is shared: the daemon answers with the registration and the program, and `radar run` wraps the scope, sets `PI_RADAR_REGISTRATION_ID` for an agent, removes the launch variables and execs. `systemd-run --scope` keeps the pid, so the claimed identity stays valid. Delivering the program from the daemon's record instead of typing it into the pane keeps environment values private and removes shell quoting of argv. This reverses the earlier amendment that `radar run` has no release gate.

**2. One relaunch, args by caller.** `relaunch(subject, program, args, placement)`: the program is executable, cwd and environment; args are the argv. Restart passes the captured kernel recipe; a continue passes what the parent sent, because the parent's environment overlay (generation nonce, mailbox) is new for every launch. Merging extra flags into a recorded argv was rejected: duplicate flags have no defined winner, and Herdsman rebuilds its argv from the agent definition anyway.

**3. The session comes from the agent's record.** `with_session` removes session selectors from args and appends `--session <full id>`; `--session` fails closed when the session is gone, so a deleted session ends the launch instead of creating an empty one. Rejected: Radar minting ids with `--session-id` (encroaches on Pi and silently creates), a session file path (a second locator Pi already resolves), and a subject marker in the session file (a second source of truth for the binding). A first-presence check that the reported session equals the agent's remains as a backstop.

**4. The bare-argument rule applies to both callers.** `resume_launch`'s refusal of a non-flag argument stays, on restart and continue alike, so a message is never replayed. An extension-registered flag that takes a value is refused; that fails closed.

**5. Verb by effect, authority by caller.** Continue starts a child that has no process: `spawn.create { continue_subject_id }`, refused while a claim is live or a successor is reserved, and unfenced because it stops nothing. Restart replaces a live process: `lifecycle.restart`, for the operator (fenced by the claim the TUI shows, with the captured recipe) or the recorded parent (addressed by subject alone, with its own program and args). A parent fence was rejected: a fence read at call time protects nothing, and one carried from the parent's decision would put an id back in Herdsman. The failure it would prevent is a second restart replacing the first's successor, which leaves the later caller's intended process running. Restarts stay serialised by the reserved successor, and a reservation that could not dispatch is withdrawn.

**6. Placement follows the pane.** An agent's own pane, if it still exists, keeps it; otherwise the relaunch splits beside the parent. This keeps operator placement for restart (memory #6130) and gives a continue after close a place.

**7. Launch facts per launch.** A continue under a new run must not carry the old run, and editing the subject would erase what each launch was. Owner, run, label and role are recorded on the launch; reads show the current launch's. The parent link is lineage and stays on the subject.

## Risks

- A nix store path recorded as the executable can be collected; a continue supplies its own program, and restart uses the running process's.
- A session that never had a conversation may never have been written, so continuing it fails at `--session`. Documented.
- Removing the bootstrap touches every tracked launch; the existing lifecycle and scope suites are the guard.
