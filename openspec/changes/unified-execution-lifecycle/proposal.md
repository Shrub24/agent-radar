# Proposal

## Why

An execution can exist before any agent session attaches to it. Requiring every
launch to invent a subject and claim confuses process identity with durable agent
identity. It also produces a pending subject row beside the pane that will host
that execution.

The first implementation removed fresh-launch placeholder subjects and keyed
process operations on registrations. Review exposed two remaining problems: a
successor reservation replaces the incumbent's current claim before it is granted,
and the default view still reconciles independently produced agent, pane and
subject rows. Claims also carry copies of execution facts that need synchronising.

A registration identifies an execution. A claim is its optional, versioned join
to a subject. Reserving that join must not grant it, and neither a reservation nor
a subject should introduce another default-view row for the same execution.

## What Changes

- Create executions through one shared composition: optional placement and launch
  intent, identification now or later, optional subject reservation, and an
  optional spawn edge. Adoption, configured `L`, container recipes, restart and
  child launches use those operations.
- Fresh launches reserve registrations without inventing subjects. Restart and
  existing-child continuation reserve claims against known subjects before launch.
- Keep the incumbent's granted claim current while a successor is reserved. The
  reservation excludes competing reservations and grants without displacing the
  incumbent's authority. Only its named execution can fulfil it after release.
- Unify presence decisions and lifecycle claims into one authoritative attachment
  record. Retain reserved, pending, granted, denied/duplicate and released semantics;
  derive policy predicates from standing instead of scattering state filters.
- Radar mints and returns claim IDs, including the pre-minted reserved ID. Preserve
  Pi's awaited session shutdown/start order through Radar's hook promise chain:
  shutdown releases the exact claim, start establishes the next, and heartbeats
  use its returned ID. No claim-ID env injection or mandatory attempt subsystem.
- Derive subject grant/reservation/history lookup from authoritative claims, not
  writable subject authority pointers. Update indexes during transitions and
  rebuild on startup; remove admission-time process reconciliation.
- Keep session shutdown as normal exact-claim release. Use the existing shared
  supervisor's authenticated completion as the primary process-exit signal and
  main fallback release, with wake/sweep verified exits and launch failure entering
  the same registration finalisation. Do not wait for pane death to settle a
  completed execution or equate finalisation with pane removal. Fail a mismatched
  restart/continuation visibly in the TUI, settle its reservation and apply normal
  rules to the actual session, without automatic retry or clearing another
  registration's reservation.
- Store attachment facts on claims and process facts on executions; compose reads
  instead of synchronising execution copies into claims.
- Make registrations the row unit in the default view. Join each execution with
  its optional claim, subject and verified placement, including before presence.
  Retain ordinary topology rows for panes without registrations, not duplicate
  pane rows for placed executions or standalone subject rows. The row record is a
  derived join, not persisted state. Hide revoked never-presented executions.
- Publish the public execution and attachment facts needed for that join without
  leaking private launch credentials or recipes.
- **BREAKING**: support only current records and the exact current wire revision.
  Regenerate contracts and advance versions when the public document changes;
  refuse unsupported state rather than migrating it.

Non-goals: a fresh-subject preallocation API, new subject-first or claim-first
views, workspace recovery, the general durable-work/response execution-policy refactor
recorded in plan.md, or compatibility and cutover mechanisms. The model
allows intentional preassignment without requiring it for ordinary launches.

## Capabilities

### Modified Capabilities

- `managed-execution`: execution-owned process facts, optional attachment and
  registration-primary joined presentation.
- `managed-launch`: independent granted claim and exclusive successor reservation;
  execution-addressed launch control, observation and withdrawal.
- `managed-child-spawn`: execution-keyed spawn provenance for fresh children and
  reserved attachment for continuing an existing child.

## Impact

- Lifecycle construction, claim records, subject pointers, presence decisions,
  restart, cancellation, withdrawal, replay and process observation.
- Public observation and control contracts, generated pi-radar schemas and
  consumers, tree projection and frozen row operation references.
- The initial implementation checkpoint is `5482f789` on
  `wip/unified-execution-lifecycle`, at wire 18. This revised plan is not yet
  implemented; its changed observation contract requires a subsequent revision.
