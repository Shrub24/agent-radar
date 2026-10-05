# Design

## Context

See proposal.md for motivation. FleetTree::build already nests normalized agents by exact lineage within a workspace. App::visible_rows applies pane visibility, filters and sibling ordering; ui::row_item currently receives depth but no sibling continuation information. Task detail is joined only in ui::bus_lines. App::apply_bus_event updates BusState without rebuilding the tree.

The accepted baseline specs are in openspec/specs/. No transport change is required for this slice.

## Goals / Non-Goals

**Goals:** one task projection shared by tree rows, parent counts and details; connectors that describe the view actually drawn; task updates applied independently of runtime polling.

**Non-Goals:** task output, lifecycle controls, tabs as tree levels, a runtime/agent mode split, new animation/config roles, a second mux adapter or bus protocol revision. No Herdr command or wire type enters the projection.

## Decisions

### 1. Project tasks once beside normalized agents

Extend the existing row model with a task kind. Move the exact-session join, with the existing pane fallback only when no session UUID is published, from presentation into the projection. A connected complete bus list wins, including an empty list; otherwise use distinct unresolved token ids and their published phases. Never combine rich bus rows with leftover token rows.

Use only an unambiguous join. Multiple pane-fallback candidates yield no rich join, not whichever HashMap entry happens to come first. A bus session unmatched to an agent remains undisplayed. Reuse current normalized task types where possible; the tree exposes no Herdr token names or socket requests.

Rejected: decoding tokens in row_item or making details and rows independently choose sources. Those paths would disagree on empty lists and disconnects. Revisit if another independent task publisher appears.

### 2. Stable task identity, not a command label

A task identity includes its owner row, the exact session UUID where known, and task id. A token-only row without a UUID has an explicitly pane-scoped fallback identity. Enriching a task with the same known identity does not change selection. A newly known or changed session is an identity change, not evidence that a same-numbered task continued.

Task text uses command when published, otherwise id; the id and phase remain accessible in details. Optional age and program mark use the existing duration/configuration paths. Only observed running tasks animate using appearance.command; retained token facts remain still and labelled last-observed.

Rejected: task id alone (publishers reuse ids) or command as identity (it may be absent or edited). The existing pane-scoped agent identity is not refactored here.

### 3. Derive connectors after ordering and visibility

Build sibling continuation metadata while flattening the visible tree. The same flattened rows feed rendering, hit-testing and disclosure-marker positions. A single child uses a last-child connector; hidden or filtered siblings do not leave a vertical continuation behind.

Keep rows single-line and include connector width in existing Unicode-aware fitting. Workspace roots have no connector. Reuse the configured subtle colour for connectors; do not add a palette knob merely for a new glyph.

Rejected: drawing connectors from raw child indices, which is wrong after filtering or sorting. No separate generic tree widget or new dependency.

### 4. Branch folding without sacrificing focus

Keep Space and arrow folding. Clicking a workspace heading retains its existing behaviour; clicking an agent branch's disclosure marker folds it. Clicking elsewhere selects, and a second click focuses as before. A task leaf has no disclosure control. Enter or a second click on a task focuses its parent's currently observed pane through the existing focus action, with the same stale/absent refusal.

Branches start expanded, as today. Existing agent-state jumps retain their meanings; tasks are reachable by ordinary navigation and filtering. No extra toggle hides task rows independently of their parent branch.

### 5. Refresh at both observation and bus changes

Recompute task children and reconcile selection when inventory or bus state changes, not only on the next Herdr poll. Preserve folds by row identity. An explicit bus empty list removes the children immediately; a disconnect rebuilds token fallback without claiming completion. Parent badges and task details consume the same projection.

Sibling source order follows the chosen source's task order after existing agent children. Name order uses displayed task labels. State order ranks review/flushing as needing attention, running as moving, and unfamiliar or absent phases as unknown; this is a display rank, never an input to agent activity derivation.

## Risks / Trade-offs

- A token-only task has no command or age → show only reported id/phase and source basis.
- A token/bus switch can change rows → preserve selection only for surviving identities and fall back to the parent when a selected task disappears.
- Deep nesting spends horizontal space → fit from actual rendered prefix width; narrow-terminal tests must not panic or emit control text.
- Runtime freshness and live bus freshness differ → label each task's source; do not freeze fresh bus facts merely because runtime collection failed, and never enable focus from stale location evidence.
- The accepted background-task details remain on the parent → reuse the projected task values rather than remove the existing summary contract.
