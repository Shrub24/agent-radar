## Why

`pi-radar-child-surface` gives Herdsman the child's subject id, but assigning to, retiring, inspecting and observing a child are still addressed by registration. A registration names one process, so after a child restarts or resumes, each of those addresses a process that no longer exists. Herdsman should be able to treat the subject as the child.

## What Changes

- Assign, retire, inspect and observe take the child's subject id. pi-radar resolves the current registration inside, from the spawn edges, and never caches it across a child's restart.
- A subject has one edge per launch. The current one is the edge with the newest `bound_at` among those naming the subject and a bound registration; the SDK decodes `bound_at`.
- An assignment follows the agent: pi-radar keeps each child's last published assignment keyed by subject, and when the child's registration changes it retires the old writer, opens one on the new registration and republishes that state.
- Once Herdsman has cut over, the registration and `bound_process` leave the launch result and the inspection evidence.

## Non-goals

- Keying the daemon's assignment channel by subject. It stays addressed to a registration; pi-radar owns the replay.
- Choosing among duplicate holders of a session. Only the subject's attached holder is a child.
- Peer messaging and general topology reads.

## Impact

Capability `pi-radar-sdk`. Code: `radar-publication.ts` (assignment writers), `radar-launch.ts` (`observeRadarChild`), `radar-inspection.ts`, the edge decoder. No daemon change of its own; depends on `unified-launch` (continuation, wire revision 7).
