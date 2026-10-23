# Proposal

## Why

Daemon observation is still timer-driven: a topology change reaches a display on the next sample, and every sample re-reads the whole inventory. tmux control mode pushes window, layout, session and active-pane notifications and format subscriptions over one persistent connection, so the sampler can run when something changed instead of only on its timer.

Two things changed since this proposal was first drafted against tmux 3.7c.

**tmux 3.8 is now Radar's floor.** It reworks hooks and control-mode notifications as events with key/value payloads, turns subscriptions into server-side monitors (`set-hook -B`, `show-hooks -B`, `-T`), adds `wait-for -E`, adds hooks for window create/close/swap/zoom, pane move/resize and client create/destroy, and emits `pane-command-started`, `pane-command-finished` and `pane-shell-prompt` from OSC 133. It also fixes the control-mode faults a long-lived observer would hit: clients hanging on exit with queued pty data, notifications sent inside `%begin`/`%end`, a stuck client preventing server exit, and unbounded buffered replies to a client that stops reading. Requiring 3.8 removes the reason to design around 3.7c. There is no capability gate and no 3.7c fallback.

**The model moved.** The daemon owns its server through `radar-tmux@` and verifies its serving identity at bind. Managed executions run in systemd scopes, and their exit evidence comes from `remain-on-exit` and a `pane_dead_status` read. A session's placement is separate from its identity and panes can move between windows. Observation has to keep up with those moves without making any of them evidence.

## What Changes

- **Require tmux 3.8.** The daemon refuses to serve a server whose reported version is older than 3.8 and names the version it found and the setting to change. The Nix module asserts the version of `tmuxPackage`, and the flake provides tmux 3.8 as its default until the pinned nixpkgs carries it. Tests and acceptance run on 3.8.
- **A read-only observer.** The daemon attaches control-mode clients with `read-only`, `ignore-size` and `no-output` to the server it verified as its own. No pane output reaches Radar, and no size, selection or option changes.
- **Notifications are wakes.** Topology, placement (pane move, window swap) and, where delivered, OSC 133 command-boundary events request a sample from the shared sampler. A notification never becomes a fact, an absence or an exit.
- **Coverage is verified before it is designed.** Whether one observer can cover every session through 3.8 global monitors and hook events, or whether coverage still needs one observer per session, is settled on a real 3.8 server first. Either way, every session is observed or reported as unobserved.
- **Observer health is a snapshot source.** It reports connection state, last notification time and loss reason. Losing an observer degrades to the timer and is never read as no change.
- **Observers are fenced** to the verified serving process and re-created when it is replaced. They are not operator clients for any count or guard.
- **Existing evidence is unchanged.** The exit observation (`remain-on-exit`, `pane_dead_status`), the per-launch `pane-died` wake, absence proof by generation or fresh read, and periodic reconciliation stay as they are.

Non-goals: typed OSC 133 shell-boundary facts (a separate change; here they are only wakes), reading pane output, effects through control mode (effects stay in the synchronous adapter), parsing layouts or floating-pane topology, client-local navigation, Pi state (OSC 7501 has no tmux support), and retiring the `pane-died` wake.

## Capabilities

### New Capabilities

- `tmux-event-source`: the tmux 3.8 floor, the daemon's read-only observer, its notifications as wake requests, its health and its fences.

### Modified Capabilities

None.

## Impact

- The tmux 3.8 floor touches `ensure_tmux_server`, the Home Manager module, the flake's packages and the test harness.
- A new observer module in the tmux adapter, run on the transport's runtime with `tokio::process`.
- A new source in the snapshot's freshness set.
- Observers are daemon children, in the daemon's service cgroup: daemon stop ends them and leaves the server, its panes and every managed scope alone.
