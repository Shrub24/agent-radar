## Decisions

**1. The subject is the child; the registration is looked up.** pi-radar reads the edges per operation and takes the current edge's bound registration. A cache would keep the stale-key failure this change exists to remove.

**2. Current edge by `bound_at`.** A resumed subject gains an edge per launch. The daemon already records `bound_at`; ordering by it needs no new field. Before relying on this, confirm that a launch resuming a session reserves under the session's existing subject, so the edges share a subject at all; if it does not, the edge needs the session's subject at bind and this change stops until that is settled.

**3. pi-radar replays assignments.** The daemon channel is addressed to a registration and a restarted child has a new one, so today an assignment is lost with its process. pi-radar is already the writer. Holding the last state per subject and republishing it on the new registration makes "an assignment follows the agent" true without a wire change. The alternative, a daemon channel keyed by subject, is deferred until something needs it.

**4. A held assignment is not authority.** The replay publishes only for a registration the edge currently binds to the subject; a duplicate holder of the session never receives it.

**5. A continuation is declared, not inferred.** Today a launch carries an opaque argv, `draft` mints a placeholder subject P, and the child's first report names its session S. When S belongs to a released subject Q, `record_launch_session` stamps S on P and `enroll_subject` picks the latest subject recorded for S, which is P: the session's agent Q, its children and inbox are orphaned. The daemon cannot read intent out of `--session` in argv, so the caller declares it with `continue_subject_id`, and the daemon takes the session from its own subject record exactly as restart does (`resume_launch`). A session id on the request was rejected: it would let the caller name a session that is not the subject's. A separate method was rejected: it would repeat the parent check, idempotency, edge, scope and bind that `spawn.create` already does.

**6. The fresh-launch hole is closed at presence, not only by the new field.** A placeholder subject is never granted a session that another subject already holds a record for; the report is refused and the launch stays unattached. Unlaunched processes keep reattaching to the session's subject through enrollment, unchanged.

**7. Revision 7.** pi-radar 0.8.0 depends on the edge subject and on continuation, so the floor moves rather than leaving a 0.3.x daemon to answer `unresolved` or `bad_params`.

## Risks

- Replaying an assignment after a restart could republish work the new process has already finished. The writer republishes the last state it was given, and the owner retires it on completion, as it does today.
- An edge with no subject (legacy) is unaddressable and reported so.
- A continuation replaces any session selector in the caller's argv and refuses a bare argument, as restart does, so a caller that passes an initial prompt in argv cannot continue that way.
