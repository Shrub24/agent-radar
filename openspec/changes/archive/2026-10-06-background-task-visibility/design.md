## Context

See proposal.md for the motivation. TaskProjection in src/tree.rs already feeds task children, badges and details. Visibility in src/app.rs currently filters ordinary panes but admits every task. Visible connectors and disclosure hit testing already follow the flattened view.

## Goals / Non-Goals

**Goals:** keep task visibility a presentation choice, preserving the source join, identities, focus checks and selection reconciliation.

**Non-Goals:** no task consumption, new mode, source polling, persisted setting, palette role, lifecycle control or transport change.

## Decisions

1. **Filter rows, not facts.** Add task visibility to the existing Visibility predicate. Keep the full projection and its children, so badges/details and later reveal use current facts. Removing tasks from the projection would make the summary wrong and require rebuilding on toggle.
2. **A choice per existing pane view.** Defaults are agents hidden, running/all shown; b changes only the current view. Choices last for this run. Remembering each mode avoids a toggle unexpectedly changing another mode; resetting on every p or making one global override was rejected. This is a small view-state value, not a new configuration framework.
3. **Every unresolved phase is included.** Processes view is an operator view of outstanding work, not a predicate that a task PID must still be alive. Review/flushing, unknown and token-only phases remain visible; existing command-motion rules still distinguish running from still.
4. **Use the same predicate throughout.** Visibility must gate flattening, filtered matching, visible-child/disclosure detection and hit testing. App::reconcile_selection already falls back from a hidden task to its owner. Preserve folds, source order/state/name sorting and agent-only jumps.
5. **Keep key dispatch and footer idiomatic.** Add b beside e; handle_filter_key continues to receive typed b. Full and terse footers both carry a background-task hint, and README describes view defaults, phases and the toggle.

## Risks / Trade-offs

- [Existing tests assume default-visible tasks] → explicitly enable task rows in tests whose subject is task behavior; add separate default-hidden tests rather than weakening prior identity/rendering/focus coverage.
- [A hidden task leaves a dangling connector or disclosure mark] → frame and pointer tests exercise the predicate through the drawn layout.
- [Phase names are mistaken for live processes] → document unresolved as running, finishing capture or awaiting result consumption; do not change publisher facts or infer completion.
- [Per-view choices are not persisted] → keep this as session view state, matching folds and the other presentation controls.

## Migration Plan

No source or config migration. Update the view state and tests, verify the existing full gate, then sync the delta when accepted. Reverting this change restores always-visible task rows without altering task data.
