# Proposal

## Why

Daemon observation is timer-driven: a topology change reaches a display on the next sample, and every sample re-reads the whole inventory. tmux control mode, available in the tested tmux 3.7c, pushes window, layout, session and active-pane notifications and format subscriptions over one persistent connection. It is independent of tmux 3.8: the OSC 133 pane-command events and `set-hook -B` monitors arrive in 3.8 and extend the same observer, but control mode does not need them. Building it now gives timely wakes today and the observer the 3.8 events later attach to.

## What Changes

- The daemon runs read-only observer clients on the server it owns, attached with `read-only`, `ignore-size` and `no-output`, so no pane output ever reaches Radar and no window is resized or selected.
- Notifications (window add, close and rename; layout change; active window or pane change; session created or destroyed) and a small set of format subscriptions (pane dead, pane current command, window and session names) are wake requests to the shared sampler. They are never facts: nothing is derived from a notification's payload.
- Observer health is a source in the snapshot: connected, last notification time and the reason for a loss. A lost or unsupported observer leaves the timer cadence running and is reported, never read as absence of change.
- Observers are fenced to the verified serving process of the owned server and are never attached to a foreign server; a replaced server gets new observers.
- A control client attaches to one session and `%*`/`@*` subscriptions are scoped to the attached session, so the daemon keeps one observer per session, started and ended from session notifications and bounded in number.
- The adapter excludes control-mode clients from operator client counts.
- Existing evidence is unchanged: exit wake hooks, snapshot reads, the pane-command exit observation and periodic reconciliation. A notification is not process-exit evidence.

Non-goals: OSC 133 events, `set-hook -B` monitors and shell-command boundary facts (tmux 3.8, additive later); reading pane output; control-mode commands for effects (effects keep the current adapter); client-local navigation; any capability gate on tmux versions.

## Capabilities

### New Capabilities

- `tmux-event-source`: the daemon's read-only tmux observer, its notifications as wake requests, its health and its fences.

### Modified Capabilities

None.

## Impact

- New observer module in the tmux adapter, run by the daemon, using the transport's runtime for the persistent child stream (`tokio::process`).
- A new source in the snapshot's freshness set.
- Depends on `consolidate-daemon-ingest` (runtime) and `shared-observation` (the sampler's wake input and the source set).
