# Proposal

## Why

The fleet tree shows a workspace's agents directly beneath it, with their tmux window only as a label on each row. A tmux session is the workspace, and its windows are its tabs, but the tree does not draw that level. Panes that belong to one window read as unrelated siblings, and tab-level controls have no row to select.

## What Changes

- Draw each window of a workspace as a tab. A tab is identified by its primary pane: the root agent in that window, whose row is the tab itself.
- Nest the other agents and ordinary panes of the window beneath that root agent, so a window reads as one unit.
- Keep an owner link across windows out of nesting. An owner in another window is not a supported grouping, so it is not used to place the agent; the agent is grouped by its own window.
- Fall back to the current flat placement, unchanged, for any window that has no root agent or more than one root agent, because its tab identity is ambiguous. Plain-pane windows keep their current flat placement until non-agent windows are designed.

## Impact

- `src/tree.rs`: tab grouping inside each workspace root, reusing the existing nesting pass per window. No new row kind; the tab is the root agent's row.
- `openspec/specs/fleet-overview`: the workspace-grouped tree requirement is modified; its owner-across-tabs scenario is replaced by the window rule and the ambiguous-window fallback.
- Tab-level delete and reload on a tab row are not part of this change; they need the tab's container identity on its row and are a follow-up.
