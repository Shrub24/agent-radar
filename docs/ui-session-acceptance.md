# Shared UI-session acceptance

Run with the matching Radar daemon and binary from `wip/shared-ui-session`.
These are manual checks, not results already observed by the implementation agent.
Milestone C migrates workspace creation; other shared commands remain milestone D work.

1. Open a dashboard and `radar workspace` against the same daemon. Move the
   selection and edit the filter: both views should agree. Change details or
   geometry in one view: the other should retain its local layout.
2. Press `A` in the dashboard. Its inline project picker should open, the other
   view should show the shared gate, and ordinary tree input should do nothing.
   Type in the picker: its query/highlight must not appear in the other view.
   Escape should close the picker and release the gate.
3. Press `A` in the compact workspace sidebar. Exactly one tmux popup should
   open over the workspace, running the interaction-addressed presenter.
   Cancel with Escape in the separate dashboard. The popup should close and
   both main trees should become usable.
4. Repeat, then select a project. There should be one created workspace with the
   project-derived label. The shared gate should open before backend work finishes.
   The outcome should identify the domain request; closing the popup afterwards
   must not cancel or repeat the creation.
5. With a claimed popup open, terminate only its initiating Radar sidebar process,
   leaving the outer tmux client/server and popup terminal alive. The popup's
   choice should still create once. Separately terminate only the presenter:
   the remaining dashboard should regain control without creating anything.
   Report host-level popup teardown separately from session behavior.
6. Set a nonexistent `[tui] project_source` program. The picker should show the
   absolute `project_base` (default home) and a diagnostic. Set an unusable base:
   it should instead be empty with a diagnostic. A working source should keep its
   order without a start-directory entry. Run from two directories and compare.
7. Restart the daemon with a picker open. Old presenters must not resolve a new
   interaction or silently resubmit a creation. Surviving dashboards should take
   a fresh attachment and baseline. Check the durable request record separately
   if a creation's admission was uncertain before restart.

Record which case, origin/presenter/host process lost (if any), observed UI and
backend effect, and any request ID. Do not infer successful creation from popup
exit or infer no effect from a connection failure. Leave integrated acceptance
task 5.2 unchecked until these observations are supplied.
