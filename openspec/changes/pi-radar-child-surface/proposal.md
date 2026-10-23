## Why

Herdsman, pi-radar's one consumer, keys every child on `registration_id` (about 21 uses: assign and retire, handoff sender match, persisted spawn entries and provenance, observation evidence, the launch parent). A registration names one process, so a restart or an `agent_continue` resume gives the child a new key while the agent, its Pi session, children and inbox are unchanged. Every identity change in Radar has also forced a Herdsman cutover, although Herdsman owns no decision that needs a registration or a claim.

The subject is the durable agent identity and already survives restart and resume. Herdsman should hold that key as an opaque string, and pi-radar should own the mapping from it to the process.

## What Changes

- `launch` takes its parent from the process's own publication. The caller passes the pane it splits and no registration or subject for the parent.
- A launch result names the child by its **subject id**, the key that survives restart and resume. It still returns `registration_id` and `bound_process`, because assignment and inspection stay registration-addressed until the follow-up change `pi-radar-child-control` moves them to the subject.
- A received handoff's sender is matched by subject id. The wire already carries it beside `registration_id`.
- The spawn edge's `bound` record gains the child's `subject_id` so a launch maps to its agent without another read. Additive, so the wire revision does not change; Radar 0.3.1 and pi-radar 0.8.0.

## Non-goals

- Subject-addressed assign, retire, inspect and observe, and the assignment-across-restart rule. They are `pi-radar-child-control`.
- Decoupling the child key from the subject. It stays a pass-through string. If it must change, pi-radar mints its own key and consumers keep storing a string.
- Peer messaging, general topology reads, or a new consumer identity layer.
- Herdsman's own changes (deleting `support-radar.ts`, rewriting its call sites). They are a separate cutover, handed over as a brief.
- Changing presence, claims, or the daemon's authority rules.

## Impact

Capability `pi-radar-sdk` (new). Code: `pi-radar/extension/radar-launch.ts`, `radar-publication.ts`, `radar-inspection.ts`, `radar-communication.ts`, `radar-runtime.ts`, the client's spawn-edge decoding, and the generated contracts. Daemon: one additive field on `BoundChild`. Breaking for consumers that import the changed types, which is accepted: adopting 0.8.0 means adopting the new contract. `RADAR_RUNTIME_VERSION` is unchanged, since the runtime services keep their shape. pi-radar 0.8.0 refuses a launch whose edge has no subject, so it needs Radar 0.3.1.
