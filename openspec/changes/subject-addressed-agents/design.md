# Design

## Context

See proposal.md for the boundary being corrected. `radar-children.ts` currently scans `spawnList`, selects the newest `bound_at`, extracts a child registration, and maintains assignment writers keyed by that process. Its list/observe/inspect calls also republish held assignments. `registry_read.rs` independently builds registration and lifecycle lists; `server/registration.rs` exposes registration reads. These are existing implementation choices, not required consumer semantics.

Correction to the preceding discussion: wire `agent.list` does run bounded process verification. Only the in-process sampler's registration read supplies `Unavailable`. There is no intended list-versus-get verification distinction to preserve.

## Goals / Non-Goals

Goals: one daemon-owned subject projection and one subject-addressed child control path; keep process identity available internally for fencing, pane joins and scope stop. Remove client reconstruction rather than introduce a second permanent API.

Non-goals: moving Herdsman's scheduling, mailbox, results or definitions into Radar; moving Pi session storage; changing operator claim-fenced destructive controls; treating command registrations as agent subjects; implementing a new subscription or mux transport.

## Decisions

### 1. Discovery names subjects; process evidence stays internal

Replace `agent.get/list` with `subject.get/list`, plus a caller-derived `subject.children`. Subject records come from durable subjects, including subjects with no current process, not from registrations or a latest-edge heuristic. Get and list use one record builder. A subject record carries `subject_id`, session, label/managed metadata, relationship, attachment state, current location if established, channel projections and management status. It does not carry registration or claim ids. `scoped` describes recorded management, not freshly proven cgroup membership; unreadable evidence is unknown.

Use attachment states `starting`, `live`, `detached`, `unknown`. `selection=live|retained|all`: live selects verified granted holders; retained selects detached subjects; all also includes starting and unknown. `subject.list` defaults to live; `subject.children` defaults to all because retained workers are valid continue targets. Exit proves detachment; report expiry and failed verification do not. Detachment carries the known reason, without inventing successful task completion. No automatic subject deletion on exit.

Alternative rejected: rename registration pages to subjects. It retains the wrong identity, excludes detached subjects, and duplicates an agent across process incarnations.

### 2. Self authority is distinct from target addressing

Keep the caller's own registration and claim for presence/publication and resolve its granted subject once at the daemon boundary. Child requests name only the target subject. Check recorded parentage there for reads, assignment mutations, restart and inspect. Duplicate/released/pending callers have no child authority. Existing operator trust and operator claim fences remain unchanged; this is not a new local security model.

`subject.get/list` are operator discovery reads on the trusted socket; the SDK child surface uses the parent-authorised `subject.children` and child-operation path, not unrestricted operator discovery. Use the existing current-claim rule and transition boundary for resolution; never choose the target by timestamps.

### 3. Assignment is a subject-owned owner publication

Store the owner's accepted assignment against the subject, retaining existing source, writer generation, sequence, retirement and lease rules. Resolve it to the granted child attachment inside Radar. Assignment acceptance does not require a child process to be running. A replacement sees the still-valid assignment after grant, independent of parent polling or SDK memory. A refused holder does not receive it. No second independently writable process assignment copy: existing reads and child self reads project the same subject-owned record. Self-authored execution/facts remain process publications composed through the current attachment.

The parent SDK writer remains responsible for its normal publication/renewal, not process replacement. Daemon restart preserves provenance and the existing restored-stale rule; delivery never renews the lease. `children.retire` continues to retire the assignment, not the subject. Deleting durable agent identity is not added here.

Alternative rejected: a daemon worker that copies a process channel on replacement. That preserves two owners and demands synchronization rather than fixing the storage boundary.

### 4. Inspection and observation resolve inside the daemon

Expose child observe/inspect by subject using existing bounded output/process primitives behind one resolved target. If attachment changes during a read, return changed/unknown evidence rather than compose output from two incarnations. pi-radar supplies no child registration, claim or session for target validation. Launch readiness still uses the existing request-id status read, but its consumer projection carries subject and launch state only. Keep raw process/spawn provenance internally for verification and diagnostics; remove child process identifiers from SDK-consumed spawn/read results rather than merely hiding them in TypeScript.

No new event stream: existing subject projections and observation/event machinery report changes keyed by subject. An internal registration event may remain for daemon provenance, but consumers need no process lookup to interpret child changes.

### 5. Process diagnostics are not subject inventory

The sampler still needs registrations to join processes to panes and show duplicate/refused occupants. Keep that internal projection, including standing and scope evidence. Do not mint subjects for duplicate rows, hide them from the TUI, or expose a public process list solely to serve the sampler. Subject-facing APIs describe agent state; diagnostic rows describe why a process has no attachment authority.

## Risks / Trade-offs

- Subject discovery can expose assumptions in TUI joins → preserve internal registration evidence and exercise duplicate visibility alongside retained subjects.
- Assignment storage ownership changes → reuse existing writer/lease machinery, reset obsolete transient state, and test delivery after replacement without invoking SDK reads.
- Inspection crosses an attachment change → resolve and validate through existing fencing; report uncertainty rather than retry a potentially different target silently.
- Existing examples and SDK primitives call process reads → remove those routes together, regenerate schema pairs, and update the adoption brief. No aliases or compatibility fallback.

## Migration Plan

Ship a matching daemon/SDK change with wire revision 9, updated minimum/runtime versions and a breaking release note. Use the next unreleased daemon minor and SDK minor, determined from repository/registry state at implementation; do not retag existing releases. Reset transient state as already required by the compatibility removal. Update the Herdsman brief to subject-only child calls. Tags require explicit approval. Rollback uses the prior matching release pair and a reset, not mixed-version support.
