# Proposal

## Why

Radar needs observed tmux focus for a sidebar that follows native terminal navigation without making inspection switch terminals. The existing synchronized-selection path provides the distribution mechanism, but its meaning must become shared focus while browsing selection becomes local to each TUI.

## What Changes

- Require a serving tmux server at version 3.8 or newer; audit existing packaging before adding any override.
- Add one supervised, read-only, server-wide control-mode observer. Notifications request coalesced reads; reads establish focus and topology. Timer sampling remains the recovery path.
- Publish typed focus availability and the unified operator client's focused workspace, window, pane and zoom state. Exclude control clients; report multiple operator clients as unsupported and refuse ambiguous focus changes.
- **BREAKING:** evolve shared-selection publication/adoption into shared focus, and extract browsing selection into each frontend. Remove shared browsing-move submission and authoritative-selection adoption; do not keep a compatibility cursor.
- Keep filters, folds, ordering, projection membership and command interactions shared. View-local selection reconciles against the canonical projection.
- Every row-dependent TUI command supplies its selected row. An omitted row resolves observed focus; an invalid explicit row never falls back. Freeze resolved intent before preparation, confirmation or execution.
- Show focus independently of selection. Sidebar defaults to following focus changes; dashboard defaults to independent selection. Following is local and never executes focus.
- Preserve input gates, action receipts, frozen interactions, no-client terminal recovery, and all existing lifecycle evidence.

## Capabilities

### New Capabilities

- `tmux-event-source`: serving-version floor, read-only observer, health, server fencing and typed unified focus observation.

### Modified Capabilities

- `row-navigation`: local row movement and jumps, with no terminal-focus side effect.
- `fleet-overview`: local selection alongside shared filters, folds and projection.
- `focus-action`: explicit selected-row versus implicit focused-place targeting.
- `ui-session`: shared focus replaces shared browsing selection; local following and reconnect behaviour.
- `ui-interactions`: explicit-row and omitted-row resolution converge on one frozen-target path.
- `tui-publications`: gate reconciliation no longer adopts a global browsing cursor.

The last three capabilities are introduced by predecessor changes, not yet synced to main specs. Their MODIFIED deltas target the post-predecessor contract; sync/archive order is documented in design.md and tasks.md.

## Impact

- tmux adapter and daemon observer lifecycle, sampler wake integration, typed observation and client selection.
- UI-session state/actions, frontend navigation and details, focus rendering, attachment baseline and command preparation.
- Public DTO/schema and generated-contract checks; revise the wire revision for the incompatible UI contract, with no old-selection fallback.
- Home Manager packaging/version assertion only where still needed, plus docs and real-tmux tests.
- No hotkey installation, sidebar chrome redesign, workspace recovery, multi-operator-client support, local filters/folds/order, OSC 133 facts or exit-tracking replacement.
