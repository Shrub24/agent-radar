## Why

Herdsman, pi-radar's one consumer, keys every child on `registration_id` (about 21 uses: assign and retire, handoff sender match, persisted spawn entries and provenance, observation evidence, the launch parent). A registration names one process, so a restart or an `agent_continue` resume gives the child a new key while the agent, its Pi session, children and inbox are unchanged. Every identity change in Radar has also forced a Herdsman cutover, although Herdsman owns no decision that needs a registration or a claim.

The subject is the durable agent identity and already survives restart and resume. Herdsman should hold that key as an opaque string, and pi-radar should own the mapping from it to the process.

## What Changes

- pi-radar addresses a launched child by its **subject id** everywhere Herdsman used a registration id: assign, retire, inspect, observe, the launch result, and the handoff sender. The registration for a subject is resolved inside pi-radar and re-resolved after the child restarts.
- `launch` takes its parent from the process's own publication. The caller no longer reads `subject()` and passes `registration_id` and `subject_id` in `RadarLaunchParent`.
- `RadarLaunchParent`, `RadarWorkerLaunched`, `RadarChildEvidence` and the handoff sender drop `registration_id`, `claim_id` and `bound_process` from the types Herdsman consumes.
- The spawn edge's `bound` record gains the child's `subject_id` so pi-radar can map a launch to its subject without another read. Additive, so the wire revision does not change.
- pi-radar 0.8.0. `/client` stays exported for tests and tools; only the runtime surface a consumer imports stops carrying process identity.

## Non-goals

- Decoupling the child key from the subject. It stays a pass-through string. If it must change, pi-radar mints its own key and consumers keep storing a string.
- Peer messaging, general topology reads, or a new consumer identity layer.
- Herdsman's own changes (deleting `support-radar.ts`, rewriting its call sites). They are a separate cutover, handed over as a brief.
- Changing presence, claims, or the daemon's authority rules.

## Impact

Capability `pi-radar-sdk` (new). Code: `pi-radar/extension/radar-launch.ts`, `radar-publication.ts`, `radar-inspection.ts`, `radar-communication.ts`, `radar-runtime.ts`, the client's spawn-edge decoding, and the generated contracts. Daemon: one additive field on `BoundChild`. Breaking for consumers that import the changed types, which is accepted: adopting 0.8.0 means adopting the new contract. `RADAR_RUNTIME_VERSION` goes to 3.
