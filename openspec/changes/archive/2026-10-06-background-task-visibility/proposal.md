## Why

Background-task children make the default agent fleet harder to scan. They belong in the running/processes view, including tasks that have exited but whose results remain outstanding, with a way to show or hide them without changing views.

## What Changes

- Start with task children hidden in agents view and shown in running and all views.
- Include every unresolved task in those views, not just the `running` phase; preserve `flushing`, `review`, unknown and missing phases.
- Add `b` to toggle task children in the current view. Keep each view's choice for this Radar run and show the effective choice in the footer.
- Keep parent badges, details, state derivation and source precedence unchanged. Hiding a selected task selects its surviving visible parent.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `fleet-overview`: task children become subject to view-local visibility while their underlying facts remain available.

## Impact

`src/app.rs` visibility and key handling; `src/ui.rs` footer; existing nested-tree/UI tests and README. No publisher, collector, bus, lifecycle or dependency changes. `p`, `e`, filtering, ordering and branch folds keep their existing meanings.
