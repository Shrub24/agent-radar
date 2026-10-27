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
6. Set a nonexistent `[projects] source` program. The picker should show the
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

## Destructive commands are the session's (milestone D)

Run these in a dashboard and a compact sidebar attached to the same daemon.

1. In the dashboard, move the selection to an agent that is working. In the
   sidebar, move it to a different agent. In the dashboard press `D`.
   Expect: a question naming the dashboard's row, in both views' session.
2. Move the sidebar's selection while the question is up. Answer Confirm.
   Expect: the dashboard's row is what closes; the sidebar's selection never
   became the target.
3. With a detached popup instead of the inline question (compact sidebar), press
   `D` again and answer in the popup. Expect: one close, one outcome, and the
   popup exits after the session acknowledges the result.
4. Delete an idle pane with no agent and nothing running. Expect: no question,
   one close, and an outcome recorded in `!` with its request id.
5. Delete a container whose agent is still alive. Expect: the question names the
   running agent, and one confirmation sends one delete. If the agent ignores
   its stop window the place still closes, and the outcome says its exit was
   not verified. No second prompt appears.
6. Press `L` on a selected workspace. Expect: a new tab in that workspace running
   the configured command, with no question asked.
7. Detach the dashboard while a close is in flight. Expect: the operation still
   completes, and the daemon keeps the outcome readable by its request id.

