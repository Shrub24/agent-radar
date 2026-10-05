# Proposal

## Why

Indentation alone makes ownership hard to read, and background tasks are buried in an agent's details. A connected, foldable tree should expose agents and their unresolved tasks as distinct rows without inventing ownership or task state.

## What Changes

- Draw `├─`, `└─` and continuing `│` connectors from the visible, sorted tree.
- Add selectable background-task children beneath the agent they join, including token-only rows when rich bus detail is unavailable.
- Give a selected task its own details; keep the parent agent's task summary and unresolved count consistent with the same selected source.
- Fold agent branches with Space or their disclosure marker, preserving row-click selection/focus and workspace-heading folding.
- Preserve per-level sorting, filter ancestry, stable row identity, existing agent-state jumps and truthful retained/source freshness.
- Keep the projection independent of Herdr commands and socket types. This change consumes normalized observations and extension facts only.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `fleet-overview`: connected branches and background-task rows, details and interactions in the existing fleet tree.

## Impact

`src/tree.rs`, `src/app.rs` and `src/ui.rs`, with existing task facts from `src/model.rs` and `src/bus.rs`; tests and README. No new dependency, bus protocol change, mux operation or lifecycle action. Pane/tab close and idle-only restart are the next separate change; the Herdr provider interface belongs to that runtime-action work.
