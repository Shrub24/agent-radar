## Context

pi-radar already ships a process-bound runtime (`RadarRuntimeServices`: `publication`, `communication`, `execution`). Herdsman still imports the raw client and several helpers (`launchRadarWorker`, `observeRadarChild`, `inspectRadarChild`, `provedRead`), and therefore holds process identity: `registration_id` for each child, `source` and `incarnation` for assignment, and `registration_id` plus `subject_id` for its parent.

Radar's model (memory #6403): the subject is the agent, the registration is one process incarnation, the claim is one session attachment. Herdsman restarts idle retained workers and resumes saved sessions; each gives the child a new process, so a new registration and claim, under the same subject.

## Decisions

**1. The child key is the subject id, as an opaque string.** Herdsman stores and compares it and never parses it. It is Radar's durable identity today; the contract with consumers is only that it is stable for the agent and a string. If it must diverge, pi-radar mints a key and maps it internally.

**2. Registration and claim leave the consumer surface.** Herdsman owns no decision that needs either. A claim never appears in Herdsman's production code; a registration appears only as the child key and the handoff sender match, both replaced by the subject.

**3. (Moved to `pi-radar-child-control`.) pi-radar resolves subject to registration, per call.** It reads the spawn edges (`spawnList`), which carry each child's subject and its bound registration, and picks the registration the edge currently names. It caches nothing across a restart of the child: a stale cached registration is the failure this change removes. After a pi-radar restart the mapping is rebuilt from the same read.

**4. The edge carries the child subject.** `BoundChild` today holds `source`, `incarnation` and `registration_id`. Adding `subject_id` is additive and keeps the mapping to one existing read. The alternative, a separate lifecycle read by subject in the SDK, adds a second source of truth for one fact. The subject is known at bind: a managed launch reserves its attachment for the subject before the child exists. Task 1.1 confirms this against the bind path before anything else is built.

**5. The launch parent comes from the publication.** The process's own publication already holds its registration, subject and location. Exposing them to the caller only for the caller to hand them back is the leak this change closes.

**6. No compatibility shim.** The types change and the runtime version moves to 3; a consumer on the old shape fails at its typecheck (memory #6142).

## Risks

- A child whose subject is not yet on its edge, because the edge is unbound or is a legacy record, has no key. The surface reports it as not yet addressable and never falls back to a registration.
- A second process holding the session is a duplicate and does not own the subject, so a lookup by subject resolves to the incumbent only. That is the intended rule, not a case to handle.
- The subject key is correct only while Radar ties it to the session. If that changes, decision 1's escape hatch applies.

## Scope note

Assignment is a daemon channel addressed to a registration, and a restarted child has a new one. Re-keying assign, retire, inspect and observe by subject therefore changes behaviour, not only types, and is its own change. This change delivers the parts that are pure surface: the parent, the launch result and the sender.

## Migration

Herdsman's persisted spawn entries and provenance hold registration ids. The cutover reads an old entry's registration once to resolve its subject through the edge, writes the subject, and drops the registration. An entry that cannot be resolved is reported, not guessed. This lives in the Herdsman brief, not in pi-radar.
