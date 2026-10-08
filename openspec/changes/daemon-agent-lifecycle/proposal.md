# Proposal

## Why

The mux daemon abstracts physical controls, but agent identity and status still live in Herdr reports. Restart built on that representation would preserve the dependency we intend to remove. Establish daemon-owned lifecycle records and direct publication before implementing restart.

## What Changes

- Add durable agent registrations separate from mux inventory and mutation records: session context, publisher incarnation, verified process identity, optional owner/run links and backend-qualified location.
- Accept direct execution and assignment snapshots with distinct provenance, sequence and freshness; expose them without requiring Herdr metadata.
- Register an explicit launch/resume specification as private data for later lifecycle execution, without executing it in this change.
- Publish a versioned client contract and fixtures for a Herdsman publisher port. Existing Herdr reporting remains a compatibility path, not the registry's source of truth.
- Keep session launch/stop/resume execution, recovery policy, stale restart batches and automatic restart for subsequent changes. No change to current managed-action eligibility.

## Capabilities

### New Capabilities

- `agent-registration`: durable agent identity, launch specifications, direct source-labelled reporting and fresh registry snapshots.

### Modified Capabilities

None. Existing physical controls, owner-routed managed actions and TUI observation keep their behavior in this foundation change.

## Impact

Radar's `src/control_plane/` gains a registry alongside the durable mutation store and versioned endpoints. Process identity uses the existing birth-identity model; mux location stays backend-qualified. Protocol documentation, fixtures and tests expand. The Herdsman publisher port is coordinated in pi-extensions after this contract is available; removed pi-subagents is out of scope. No new mux backend or generic plugin framework.
