# Superseded — archived without syncing specs

Archived 2026-10-22 with 16/27 tasks done. Do not sync its delta into `openspec/specs/`.

**Why.** This change gave a Pi session its lifecycle identity through launch markers: injected subject/launch/generation variables and a session-persisted marker, with enrollment as the adoption path. That model was replaced:

- `0b76e04a` retired the session marker.
- `ce4fac4d` replaced enrollment with one verified presence claim; the subject follows the Pi session and a process attaches to it.
- `unified-pi-registration` and `managed-scoped-launch` (task 0) then made `subject_id` the agent and `claim_id` one attempt, and dropped `execution_generation`.

Its specs contradict shipped `agent-registration`.

**Landed from it.** The shared pi-radar runtime handle, the baseline extension entry point and the Herdsman migration to that handle.

**Dropped.** Launch-variable injection (3.1), generation-based projection (3.2), the private-cache removal (3.3), and the write-order work for the old edge/lifecycle split (5.6). `managed-scoped-launch` covers whatever of these is still wanted.

**Revisit if** a client needs identity that survives without a daemon-issued claim.
