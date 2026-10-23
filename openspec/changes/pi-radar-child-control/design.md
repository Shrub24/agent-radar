## Decisions

**1. The subject is the child; the registration is looked up.** pi-radar reads the edges per operation and takes the current edge's bound registration. A cache would keep the stale-key failure this change exists to remove.

**2. Current edge by `bound_at`.** A resumed subject gains an edge per launch. The daemon already records `bound_at`; ordering by it needs no new field. Before relying on this, confirm that a launch resuming a session reserves under the session's existing subject, so the edges share a subject at all; if it does not, the edge needs the session's subject at bind and this change stops until that is settled.

**3. pi-radar replays assignments.** The daemon channel is addressed to a registration and a restarted child has a new one, so today an assignment is lost with its process. pi-radar is already the writer. Holding the last state per subject and republishing it on the new registration makes "an assignment follows the agent" true without a wire change. The alternative, a daemon channel keyed by subject, is deferred until something needs it.

**4. A held assignment is not authority.** The replay publishes only for a registration the edge currently binds to the subject; a duplicate holder of the session never receives it.

## Risks

- Replaying an assignment after a restart could republish work the new process has already finished. The writer republishes the last state it was given, and the owner retires it on completion, as it does today.
- An edge with no subject (legacy) is unaddressable and reported so.
