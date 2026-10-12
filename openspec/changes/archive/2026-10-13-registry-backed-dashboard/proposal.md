# Proposal

## Why

The TUI still reads agent state, labels and Herdsman facts from Herdr through the runtime path. Herdr removal needs the TUI to run on Radar's own lifecycle state: the daemon registry (identity, activity, waiting, outcome, facts, topology, freshness) joined to backend inventory for locations. Herdr independence must be tested end to end, so the dashboard should stop reading Herdr agent state altogether rather than fall back to it.

## What Changes

- The TUI observes through the daemon: backend inventory for workspaces, tabs, panes and foreground evidence, and the registry for every agent fact. Herdr's agent records, display metadata and native state are no longer read.
- Join registry agents to locations by their reported backend-qualified location and exact process identity, never by pane title, label or session UUID alone. An agent that cannot be joined to a live pane is shown with its last location and a stale or unresolved mark.
- Show the three authorities separately in rows and details: physical (pane present, spawn-edge exit observation), execution (registry activity, advisory), and assignment (owner facts). Missing or stale facts show as unavailable or stale and are never replaced from another source.
- Render freshness and provenance from the registry's own read-time freshness. A registered agent with no fresh execution is shown unknown, not idle.
- Derive `waiting` in the dashboard from awaited items plus background tasks, and `lost`/`settling` from assignment facts, as a pure projection.
- Panes without a registration are ordinary panes with unavailable agent state. There is no Herdr fallback; a compatibility layer for foreign agents is a later explicit change.
- Under the direct (non-daemon) runtime, which cannot read the registry, the TUI shows ordinary panes only.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `runtime-observation`: agent facts come from the registry, not Herdr metadata.
- `fleet-overview`: rows and details present registry state, freshness and the three authorities.

## Impact

Touches the daemon client/runtime provider, `src/model.rs` normalization, the collector's join, `src/ui.rs` rows/details and tests and fixtures. Depends on `agent-facts-channels` for model, context, awaited, background and assignment facts; without it those render as unavailable. OSC 7501 capture, tmux-native layout, the persistent sidebar and the project picker are out of scope.
