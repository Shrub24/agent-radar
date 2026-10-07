## Why

Descendant totals show that a quiet foreground process has busy children, but not which child is doing the work. A compact tree in Processes will make those totals inspectable without adding rows to the fleet or introducing a general process browser.

## What Changes

- Retain verified per-descendant identity, parentage, observed name, kernel state, interval CPU and RSS from the existing bounded process scan.
- Add an initially collapsed descendant table to Processes with name/PID/CPU/RSS columns and foldable branches.
- Let a selected process supply the compact process detail block; keep root and descendant totals separate.
- Add explicit in-panel process navigation without changing fleet selection, page switching or lifecycle authority.
- Preserve incomplete-scan qualifications and withhold rows when the root or source is no longer current.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `process-metrics`: expose verified individual descendants alongside the existing qualified sums.
- `detail-navigation`: support selection and folding inside the Processes descendant table while preserving page scrolling and fleet focus.

## Impact

`src/procfs.rs` and `src/model.rs` gain a per-member projection from the existing confirmed snapshot. `src/app.rs` and `src/ui.rs` hold and render process selection/folds using the current details line model. Sampler, interaction, rendering and PTY tests cover the new behavior. No external process command, tree-widget dependency, new polling loop or publisher contract is needed. Background-task metrics, arbitrary process browsing, mux changes and lifecycle controls remain out of scope.
