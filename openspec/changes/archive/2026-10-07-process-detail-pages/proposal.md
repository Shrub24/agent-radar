## Why

The flat details panel hides useful facts behind long assignments and repeated task metadata. Known pane processes also expose little beyond PID and age, making it hard to distinguish an idle launcher from work happening in its children.

## What Changes

- Split details into Overview, Processes, Tasks and Source pages, with keyboard focus and scrolling and mouse page selection.
- Keep identity, current PID, activity and model near the top of Overview; make long assignment and task content collapsible within its page.
- Enrich known foreground processes on Linux with birth-verified CPU, RSS, kernel state and observed descendant summaries. Keep ownership nesting in the fleet tree, not a second process browser.
- Include the existing local PID display and exec-replaced-shell correction in this change's verification and eventual commit.
- Preserve existing background-task facts, but do not sample task PIDs until the publisher supplies a verified captured-at-spawn identity contract.

## Capabilities

### New Capabilities
- `detail-navigation`: Focusable detail pages, scrolling and disclosure of long content without changing the fleet selection.
- `process-metrics`: Birth-verified resource observations for known foreground processes and their descendants.

### Modified Capabilities
None. Existing facts remain reachable in the selected row's detail panel; pages change their arrangement, not their authority.

## Impact

`src/app.rs` and `src/ui.rs` own detail navigation and presentation; `src/collector.rs`, `src/procfs.rs` and normalized model facts own sampling. Herdr remains behind the runtime-provider seam. Tests and README cover navigation, freshness, PID reuse and metric limits. No new service, metrics database, process-control operation or mux dependency is introduced. Background-task resource sampling is deferred pending its owner's contract; Tasks retains all existing bus/token facts.
