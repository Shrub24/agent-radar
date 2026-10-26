# Proposal

## Why

A shared session fixes interaction ownership but does not by itself make Radar easier to read or operate. Use the stable command/presenter boundaries to improve discovery, feedback and local panel interaction without another framework or model rewrite.

## What Changes

- Provide contextual help and a fuzzy command palette from the typed catalogue and effective local bindings, including pending leader-sequence feedback.
- Reuse picker components across inline and detached roots with explicit loading, empty, error and blocked states.
- Make pending/refused/unresolved outcomes and relevant retained lifecycle events inspectable with request IDs, timestamps, failure details and distinct source provenance, while keeping inspection and informational overlays local.
- Add local panel sizing/focus improvements and structured copy, then evaluate panel-scoped text selection.
- Apply one Radar-owned theme/status vocabulary across dashboard, sidebar and presenter. Preserve motion settings; add effects only where they communicate a meaningful state change.
- Evaluate focused Ratatui crates against these behaviors rather than adopting an entire widget suite.

## Capabilities

### New Capabilities

- `command-discovery`: context-sensitive help, fuzzy palette and local sequence feedback over the established command catalogue.
- `operation-inspection`: local operation and retained lifecycle-event inspection, distinguishing record authority, observations and diagnostics without re-execution or invented history.
- `panel-selection`: local structured copy and panel-confined text selection with explicit clipboard capability/failure behavior.

### Modified Capabilities

- `detail-navigation`: local split sizing and focus affordances without changing shared fleet selection or existing detail-page authority.
- `fleet-overview`: consistent presentation of focus, shared blocking, observation freshness and outcomes across frontend roots.

## Impact

This is Slice E, not another ownership change. Implementation depends on `shared-ui-session` milestones A–C: typed command catalogue, projection/input boundary, host effect delivery and both modal roots. It may overlap that change's milestone D only after those interfaces are stable. Archive the core change first; do not reapply its deltas here.

Affected code is primarily frontend input/rendering, `src/picker.rs`, `src/ui/`, local panel state and presentation configuration. Nucleo matching and the key resolver come from the core boundary; evaluate tui-input/list, tui-popup, tui-scrollview, tui-panel-select and rat-widget/rat-text only for a concrete gap. TachyonFX is optional. Keep existing TestBackend tests; a small number of intentional insta snapshots is an option, not a new mandatory testing framework.

No Bubble Tea pivot, terminal emulator, generic window manager, new operation journal, shared detail state, or changed lifecycle authority. Clipboard and real tmux acceptance are performed by the user. New optional dependencies and uncertain API compatibility must be checked before adoption.
