# Design

## Context

`TmuxRuntime` reads the server with one-shot subprocess commands against a named server, installs per-launch exit-wake hooks that run a command, and is sampled on a timer. tmux 3.7c control mode (`-C`) offers command blocks and notifications: `%window-add`, `%window-close`, `%window-renamed`, `%layout-change`, `%window-pane-changed`, `%session-window-changed`, `%sessions-changed`, `%subscription-changed` and others, and `refresh-client -B name:what:format` subscriptions reported at most once a second. `-f no-output,read-only,ignore-size` removes pane output and prevents size or selection effects. tmux 3.8 adds OSC 133 pane-command events and generalizes hooks and subscriptions; this change leaves those for a later additive step.

## Decisions

### 1. Control mode is the notification stream, 3.8 is an extension of it

The observer works on the tested tmux. Nothing is gated on 3.8; if the observer cannot start the daemon runs on its timer and says so. When tmux 3.8 is available, its pane-command events become more notifications mapped to wake requests, with shell-boundary facts added as separate typed evidence in their own change.

### 2. Wake, never fact

Every notification maps to "sample soon". The sampler's reads establish state, which keeps the principle that events are not inferences and keeps correctness independent of the stream: a lost notification costs latency, not correctness. The timer and periodic reconciliation stay.

### 3. One observer per session

A control client attaches to a single session and `%*` and `@*` subscriptions cover only that session, so coverage of a multi-session server needs one observer per session. They start and end on `%sessions-changed` plus the inventory, and are bounded (starting bound: 16, revisited with use). Unobserved sessions are reported and sampled on the timer.

### 4. Subscriptions are narrow

Subscribe to `pane_dead`, `pane_current_command` and window and session names for the attached session. At most once a second per tmux, so subscriptions serve as wake sources, not as measurements. Pane output is never subscribed.

### 5. Run on the transport's runtime

Each observer is a child process whose stdout is read as lines and whose stdin carries `refresh-client` subscription commands, using `tokio::process` on the runtime from `consolidate-daemon-ingest`. Lines are bounded and notification parsing ignores what it does not know. Effects, hooks and snapshot reads stay in the existing synchronous adapter.

### 6. Fence to the verified server

Before attaching, the daemon re-establishes the verified serving process identity (the same identity every generation fence compares). A changed identity ends the observer and starts a new one after verification. An observer never attaches to a server the daemon did not verify as its own.

### 7. Observers are invisible to operator logic

A control client appears in `list-clients` and counts in `session_attached`. The adapter filters on `client_control_mode`, so operator-attachment logic does not see them. Whether any current guard depends on attachment is a task to check, not an assumption.

### 8. Exit wake hooks stay

Per-launch `pane-exited` hooks remain the wake that triggers a verified exit read; control mode does not replace them. A later change can retire hooks only if subscriptions prove at least as timely for a verified-exit trigger.

## Risks

- **Observer attached to the wrong server.** Mitigated by the identity fence and a test that a foreign server on the same name is refused.
- **A subscribed or output flood.** `no-output` removes pane output; notification bursts coalesce; lines are bounded.
- **An observer changing server behaviour.** `ignore-size` and `read-only` are tested for no resize and no selection change; `destroy-unattached` and `detach-on-destroy` effects of an attached client are checked on the tested tmux.
- **Session count.** Bounded, reported and degraded to the timer.
- **Not testable on 3.8.** 3.8 behaviour is out of scope here; verification is on 3.7c.

## Open choices

- The observer bound and the retry backoff.
- Whether the subscription set should include `window_name` and `session_name` or leave renames to `%window-renamed` and `%session-renamed`.
