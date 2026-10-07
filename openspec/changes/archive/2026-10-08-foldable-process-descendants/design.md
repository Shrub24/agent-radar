## Context

See proposal.md for motivation. The current sampler scans once per refresh, confirms process birth identities and parent links, and computes qualified descendant totals from a shared snapshot. It discards per-member detail at the model boundary. Processes is rendered as lines in a scrolling Paragraph, with disclosure markers and mouse geometry supplied by the same layout. The fleet tree has its own stable row identities.

## Goals / Non-Goals

**Goals:** expose the members behind the existing sums, reuse their confirmed samples, and make process selection independent of fleet selection.

**Non-Goals:** an arbitrary process browser, task attribution without captured birth identity, argv display, process controls, a second polling loop, or a widget-framework migration.

## Decisions

### Project members from the confirmed scan

Extend the resource model with per-descendant samples containing birth identity, parent birth identity, observed name, state, CPU and RSS. Build these rows from the same confirmed snapshot members used by the sums; retain the existing partial/unavailable reasons. Preserve before/after identity and parent-link checks, excluding dependent subtrees when a link fails. Names come from kernel process data, are sanitized for display, and are never described as launcher commands.

This avoids a new per-row /proc scan, disagreement between rows and totals, and synthetic task ownership. Extending the shared scan is preferred to invoking pstree or sampling each descendant independently.

### Keep the table inside the existing detail line model

Project a deterministic preorder tree, with siblings ordered by PID, rooted at the foreground process. The root is a selectable anchor; descendant totals and counts continue to exclude it. Opening the table initially reveals all branches; individual branches can then be folded. Render aligned name/PID/CPU/RSS columns, omitting RSS then CPU as width shrinks and shortening the name rather than wrapping a row. The selected sample drives the compact Process block above the table. Binary identity remains labelled as the foreground root's evidence when a child is selected.

Use the existing page's line/scroll/disclosure geometry rather than adding a stateful widget inside Paragraph. Selection, folds and hit targets use boot ID/PID/start ticks; rendered offsets are never identities. No selection or fold state is inherited across PID reuse.

### Explicit process navigation, not overloaded page navigation

The draft uses t to enter or leave process-row navigation when the table is open. This is a small local mode, visibly labelled. In it, j/k and arrows select rows, Home/End reach row limits, and Enter/Space fold the selected branch. Ordinary page scrolling and disclosure navigation remain unchanged outside that mode. Page switching, Tab hand-off and Escape retain their established meanings; mouse selection enters the local mode. PgUp/PgDn and the wheel continue to scroll. Keep the cursor visible when keyboard selection moves, without forcing the viewport back on every automatic refresh.

Overloading Left/Right for folding would conflict with established page switching; replacing ordinary detail scrolling everywhere would make the other pages inconsistent. t is a proposed local binding, not a global fleet key.

### Local process navigation is a mode of the Processes page

`t` enters and leaves process-row navigation only when the keyboard is in the panel, the Processes page is shown and its table is open, and the table's own label says which mode is on. Every key that leaves the panel leaves the mode, and so does closing the table or switching page. Rejected: a global fleet binding, and a persistent per-row mode that would outlive the row it was entered on.

The selected sample reaches the page through `render`, resolved against the observation being drawn, so a selection the refresh no longer carries draws the root rather than a stranger's metrics under the row's name. Rejected: handing the raw selection to the line builder, which can outlive the observation it was made in.

The cursor is scrolled into sight in `note_layout`, from the rows the draw that the selection produced reported. A selection changes the page's length — the picked process's own block replaces the root's — so scrolling eagerly from the previous layout left the cursor off-screen, and re-scrolling on every refresh would fight the reader's own viewport. Rejected: eager scroll from stale row numbers, and a viewport the fleet's refresh moves.

A row click selects that process and enters the mode, repeatably; a fold-marker click only folds its branch. Rejected: a row click that only selects, which would need a second gesture to navigate, and a marker that selects its own row.

The PTY smoke drives a real sampled tree — the fake CLI answers `pane process-info` with a live leader and child, and the machine's own sampler reads it — rather than scripting `/proc`. Rejected: a scripted process table, which would test the fixtures instead of the sampler.

### Withhold stale rows and reconcile refreshes

A current root with no descendants shows a measured empty observation; an unreadable or incomplete scan shows its qualification, not a fabricated empty tree. Retained rows, stale source observations and missing root identity do not show old process samples as live. On disappearance select the current root; on folding select the ancestor branch if selection became hidden; on root/fleet selection changes reset local state. Leaving Processes exits navigation mode, while selection/folds may persist for the same current root.

## Risks / Trade-offs

- Process trees can change during collection → preserve the existing confirmed-snapshot and incomplete-scan semantics; do not promise an atomic kernel snapshot.
- Kernel names are short and may not uniquely describe a command → label them as observed names, identify rows by PID and birth identity, and do not inspect arguments.
- Extra rows increase retained observation size → reuse the bounded shared scan and avoid extra per-child reads or unbounded history.
- Table rows compete with details for vertical space → start collapsed, keep summaries visible, and verify short/narrow panels and cursor scrolling.
- Descendant RSS double-counts shared pages → retain the existing qualification; rows are process readings, not exclusive workload accounting.

## Migration Plan

No configuration or wire migration is needed. Apply the sampler/model slice first, then rendering, then interaction. Verify each slice independently. Rollback removes the table projection and local navigation without changing process sampling authority or owner controls.
