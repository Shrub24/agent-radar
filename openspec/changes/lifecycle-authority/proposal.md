# Proposal

## Why

Radar executes operations: it creates panes, launches and stops processes, owns containers and records the outcome. Several of those facts are still maintained twice, once by Radar and once by the agent or publisher that asked, and the two copies are reconciled by display code. Radar should be the single authority for the lifecycle and durable result of what it executes, derive that state from evidence, and say so when it cannot. Observation then drives transitions continuously, and reconciliation shrinks to a narrow recovery path instead of a general synchronizer.

## What Changes

- State the principle as a requirement: Radar is authoritative for the lifecycle and the resulting durable state of operations it executes. External agents supply intent and application-specific evidence, not independently maintained copies of Radar-managed state. Application and domain state stays with its owner.
- Observation drives transitions. A subscribed or sampled observation of a Radar-owned subject that establishes a lifecycle change commits one durable transition with a short evidence summary. The observation itself stays volatile and the transition is edge-triggered, so an unchanged condition appends nothing.
- Transitions need evidence of sufficient strength. Definitive evidence (a verified process exit) transitions at once. Weak evidence (one inventory that lacks a pane) needs confirmation by repetition over a bounded time, or by an identity check, before it changes durable state.
- Reconciliation is defined narrowly: recomputing Radar-owned lifecycle facts that are unresolved. It runs on daemon start, a changed serving identity, an uncertain outcome, a detected inconsistency, and an explicit operator request. It is not periodic.
- An effect that cannot be verified stays an explicit unresolved outcome and is never retried blindly. An operator can abandon a record that can never resolve, which is recorded as a semantic event and releases anything the record blocks (such as the tmux server stop gate).
- Public consumers read semantic events. The internal journal format is not a contract.

Non-goals: moving assignment, execution or other agent-published facts to Radar ownership (a direction, recorded in the design); a durable observation history; shared observation itself (`shared-observation`); tmux event sources (`tmux-control-mode`); snapshots and retention of the journal.

## Capabilities

### New Capabilities

- `lifecycle-authority`: who owns lifecycle state, how observations become transitions, when reconciliation runs, and how unresolved outcomes are surfaced and released.

### Modified Capabilities

None. Existing operation-record and lifecycle requirements already describe at-most-once execution and unknown outcomes; this change adds the ownership and transition rules around them.

## Impact

- Daemon: a transition path that observers (pidfd exit watchers, the sampler, later tmux control mode) and command paths share, serialized per lifecycle record under the existing fence; evidence summaries on committed transition events; a `reconcile` entry point with the listed triggers; an abandon operation for unresolved records.
- `docs/control-plane.md` and `docs/daemon.md`: ownership, transition and reconcile rules.
- Depends on nothing unmerged; `shared-observation` and `tmux-control-mode` become event sources for it.
