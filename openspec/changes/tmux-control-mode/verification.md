# Verification

Disposable tmux 3.8 servers, one private `-L` name per run, killed afterwards.
Observer flags: `attach-session -f ignore-size,no-output` under `-C`. `read-only` was tried first and rejected: with it attached, an external `tmux send-keys -t %1` fails with "client is read-only" (found by the control-plane suite), because tmux resolves the command's target client to the best attached client.

## Notifications an attached observer receives

| Action | Notification |
|---|---|
| A client attaches or switches session | `%client-session-changed <client> $s name`; there is no `%client-attached` |
| A client detaches | `%client-detached <client>` |
| A window becomes current in a session | `%session-window-changed $s @w` |
| The active pane changes | `%window-pane-changed @w %p`, naming neither client nor session |
| Zoom, layout, join-pane | `%layout-change`, zoom only as the `Z` flag |
| Renames, session create/destroy | `%window-renamed`, `%session-renamed`, `%sessions-changed` |
| `select-pane` on the already-active pane | nothing |
| A pane's process exits | no `%pane-exited`; exit evidence stays on hooks |

- Notifications arrive for sessions other than the observer's attached one, so one observer covers the server.
- `tmux -C attach` with no session fails with "no sessions"; the observer waits and retries rather than creating one (`an_observer_started_before_any_session_attaches_once_one_exists`).
- `list-clients` reports the observer with `client_flags` containing `no-output`, which is how it is told from an operator; an operator's own control-mode client lacks it.
- `list-clients` reports `session_id`, `window_id`, `pane_id` and `window_zoomed_flag` of the client's own session in one answer, so focus is one read.

## Covered by tests

- `the_observer_wakes_on_native_changes_is_no_operator_and_ends_with_its_stop_flag`: a native `new-window` wakes the observer; focus is refused `NoClient` with only the observer attached; no control client remains after stop.
- `an_inventory_reports_the_one_operators_focus_and_refuses_to_choose_among_several`: `NoOperator`, one `Focused` (with zoom), then `Ambiguous` for two operators.
- `a_requested_refresh_runs_now_instead_of_waiting_out_the_interval` (collector).

## Measured against an attached operator

- Attaching the observer left the operator's client size (111x33), the window size, the active pane and the active window unchanged. The observer reports a width of 80 and no height, so it does not take part in size negotiation.
- The observer exits when its attached session is destroyed (process ended, no client left), so the supervisor's reattach is the recovery path.
- With `destroy-unattached on`, the observer counts as an attached client: after the operator detached, the session survived. A user who sets `destroy-unattached` would therefore keep sessions alive that tmux would otherwise reap. Recorded as a risk; no mitigation chosen yet.

## Not yet verified

- `detach-on-destroy` with only the observer attached.
- The supervisor's reattach after session loss, end to end against the daemon.
