# Proposal

## Why

Radar draws a fleet ordered the way Herdr reports it and can only move one row at a time with `j`/`k`. On a fleet of thirty panes that means scrolling to find anything: the row that needs a human is wherever it happens to fall, and the mouse does nothing at all.

## What Changes

- `s` cycles the row order: source order as reported, attention-first by state, or by name. Ordering applies within a level, so a child never leaves its parent.
- `n`/`N` jump to the next and previous row that needs attention; `w`/`W` jump to the next and previous working row. Both wrap.
- The mouse is captured: the wheel scrolls, a click selects a row, a click on a workspace heading folds it, and a click on the row that is already selected focuses its pane (the `Enter` action from `focus-selected-pane`). The hint line names the active order.

Out of scope: reordering by hand; sorting that crosses levels; a key that repeats the last filter match; drag, hover or per-cell affordances; any action on an agent.

## Capabilities

### New Capabilities

- `tree-sorting`: The three orders, what each level means, and what a sort never changes.
- `row-navigation`: The jump keys and what counts as a row worth stopping on.
- `mouse-control`: Mouse capture, hit-testing the drawn tree, and what each gesture does.

### Modified Capabilities

None. `initial-fleet-overview` is not archived yet and its specs stay as they are; `focus-selected-pane` supplies the focus action this change reuses.

## Impact

Touches `src/app.rs` (keys and order state), `src/tree.rs` and `src/theme.rs`-adjacent rendering only where ordering is applied, `src/ui.rs` (hit-testing needs the drawn rows and their screen area), and `src/main.rs` (mouse capture alongside raw mode). No new dependency. Sorting is presentation only, so the observation, the continuity rules, the bus join and the details panel are untouched. Mouse capture takes the terminal's own text selection; the README says so.
