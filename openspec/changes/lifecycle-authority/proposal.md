# Proposal

## Why

Radar already executes launches and owns the resulting lifecycle, but identity and recovery evidence are split across daemon records, publisher-owned stores, and Herdsman registration. Make Radar the authority for managed subjects and operations, let Pi sessions retain Radar-issued identity breadcrumbs for recovery correlation, and make pi-radar the one baseline publisher so Herdsman extends a Radar subject instead of registering a second copy.

## What Changes

- Make Radar authoritative for managed subject identity, execution generations, ownership/parent relations, and the durable outcomes of operations it admits and executes. External orchestration supplies intent; it does not maintain a competing copy of Radar-managed lifecycle state.
- On an accepted managed launch intent, Radar records the subject, operation, generation and any managed launch metadata such as role, owner, run and label. Herdsman may request those values; Radar commits them as part of its own operation. Later changes require an explicit Radar transition.
- Persist a versioned, non-secret reflection of Radar-issued launch identity in the corresponding Pi session. Reconciliation may use it as correlation evidence, but it cannot recreate missing authoritative state, override a Radar record, prove process liveness, or authorize an effect retry.
- Make pi-radar the single baseline registrar and live-channel publisher for a Pi process, whether or not Herdsman is installed. Herdsman contributes orchestration intent and application-specific evidence through pi-radar/Radar; it does not independently register or republish the baseline.
- Keep baseline and application observations on source-labelled, freshness-bounded live channels. They remain evidence, not managed lifecycle authority.
- Observation drives edge-triggered lifecycle transitions through one fenced reducer. Definitive identity-bound evidence transitions immediately; weak evidence requires bounded confirmation. Reconciliation restores an external observation baseline after continuity or confidence loss and never retries an effect.
- Keep uncertain outcomes explicit. Abandonment ends the recovery obligation and releases only its administrative block; it does not establish the physical outcome or bypass process/pane safety gates.
- Consumers read semantic events, not the private journal format.

**Non-goals:** transferring assignment/mailbox authority from Herdsman in this change; adding a general workflow engine; making Pi-session breadcrumbs a second source of truth; blind recovery retries; migrating the journal to a database; replacing bounded observation polling before an equivalent observer is proven.

## Capabilities

### New Capabilities

- `lifecycle-authority`: Radar-owned managed identity and operation state, single baseline registration, Pi-session recovery breadcrumbs, evidence-driven transitions, reconciliation and safe handling of unresolved outcomes.

### Modified Capabilities

None.

## Impact

- Daemon registry, managed lifecycle and operation records; registration and live-channel APIs; launch environment and Pi-session metadata persistence; `pi-radar` baseline extension and publication lifecycle; Herdsman integration to submit intent and attach application evidence through Radar; event and read projections; docs and migration fixtures.
- The Pi persistence seam is the documented non-context custom session entry (`pi.appendEntry`, read from the active branch with `sessionManager.getBranch`). Host-version compatibility and real restart/reload behavior remain acceptance gates. The marker is versioned, non-secret and evidence only.
- Existing registration identity and TUI enrollment flows need an explicit migration. Do not merge records based only on pane reuse or session UUID, or replay an old launch to fill a missing record.
- Assignment/mailbox state remains with its current owner. A later authority transfer requires an explicit command and contract change.

## Rollout

The daemon identity API now requires protocol revision 3; older clients must be refused before using the changed stable `subject_id` semantics. Coordinate the daemon, pi-radar and Herdsman cutover rather than silently accepting mixed identity meanings. `pi-radar@0.1.0` is already published with the previous contract, so publish the implementation under a new semver version before changing Herdsman's dependency or removing its local copy. Do not enable the baseline extension alongside an unmigrated Herdsman publisher that still creates its own adapter.

## Acceptance

A plain Pi session publishes baseline facts through pi-radar without Herdsman. A Herdsman-managed child has one registration and one Radar subject, with managed metadata recorded by Radar from accepted intent. After daemon restart or observation discontinuity, a matching Pi-session breadcrumb helps correlate the live process to the durable Radar record; a missing or contradictory breadcrumb does not recreate state or trigger a launch retry. Unchanged observations append no events, and weak or stale evidence cannot declare a process dead.

## Deferred

A downstream command-delivery lane (for example, `agent.continue`) is a separate operation-specific change. This change establishes the identity and authority seam it will use; it does not build a general message broker or transfer assignment authority.

## Resolved Design Choices and Remaining Acceptance

Use Pi's versioned non-context custom session entry, scoped to the active session branch. Keep stable logical subject identity distinct from exact execution IDs; migrate only through exact successor request and spawn-edge evidence, and retain legacy request-target aliases for unresolved-operation safety. Accepted owner/run/label/role metadata belongs to Radar's durable record; publication channels cannot overwrite it. The remaining gates are real Pi restart/reload acceptance, Herdsman's use of the shared runtime handle, daemon reconciliation consuming marker evidence without creating authority, fair bounded sweeps, safe abandonment, and the coordinated revision-3 release.