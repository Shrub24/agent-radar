# Operator acceptance

Live checks that only a person at a real terminal can run. Each one came from a
change that is now archived. The automated suites and the disposable harnesses
(`tests/tmux_acceptance.py`, `tests/lifecycle_acceptance.py`,
`tests/agent_publisher.py`) cover everything else. Tick an item here once you have
seen it, and record anything that failed.

## Units and daemon

From `daemon-service`.

- [ ] `radar` starts the socket, daemon and tmux server units, and the server holds no state you created.
- [ ] `radar daemon stop` with agents running: the tmux server and its panes survive, a publisher connection is refused, and `radar` starts the daemon again.
- [ ] Killing the daemon process: systemd restarts it and the agents survive.
- [ ] `radar server stop` with a pane present is refused and names the pane; with no pane it stops.
- [ ] After logging out and back in, the socket listens again, and an explicit stop did not outlast the session that issued it.
- [ ] A request queued while the daemon starts is served once, and replaying its request ID is answered from its record.

## Reset

From `radar-reset`.

- [ ] Default and hard reset against the served fleet: scopes stop, the tmux server is replaced, the daemon restarts, a default reset keeps subjects and inboxes, and a second tmux server is untouched.

## Managed agents and lifecycle

From `managed-scoped-launch`, `managed-topology` and `unified-managed-lifecycle`.

- [ ] A Pi installed through `programs.radar.managedAgents` runs as a managed agent, and a Pi you start by hand keeps its identity, children, inbox and controls.
- [ ] Create, close, reload and focus a container (`A`, `a`, `D`, `R`, `Enter`) with Herdr unavailable.
- [ ] While a branch shuts down its rows show `closing`, and they disappear afterwards.
- [ ] `D` on an agent row, a tab and a workspace ends what the row names.
- [ ] Restarting an `L` agent keeps its pane; restarting a `radar run --agent` agent you started from your own shell gives it a pane beside yours.
- [ ] The dashboard keeps drawing and answering keys during a slow shutdown.

## Views, focus and the sidebar

From `shared-ui-session`, `sidebar-mode` and `tmux-control-mode`.

- [ ] Two views (dashboard and sidebar, inline and detached): shared filter and navigation, their own details and geometry, one cancellation for both, survival of presenter loss and reconnect, and focus that only moves when asked. See [ui-session-acceptance.md](ui-session-acceptance.md).
- [ ] The workspace sidebar in [workspace-prototype.md](workspace-prototype.md): nested Pi rendering, keys, mouse and clipboard; `?`/`A`/`D` popups on the selected row; `Z`/F11 expand and return; the `B` rail; detaching leaves the agents running.
- [ ] Two views sharing one tmux client keep their own selection; native navigation moves the `◆` focus marker; following on and off; a confirmation keeps its target while focus moves; launching with no session.
- [ ] Sustained navigation and bursts of tmux notifications stay responsive and leave no backlog of focus commands.
- [ ] Kitty and nested tmux: the observer never resizes or changes selection, browsing never switches terminals, `Enter` focuses, following survives a reconnect, a daemon shutdown leaves no observer process, and a replaced tmux server is recovered.

## Agent attention

From `agent-attention`.

- [ ] Herdsman consumes attention, and an idle Pi process drops its socket.
