# Design

## Context

`TmuxRuntime` reads the server with one-shot subprocess commands against the server `radar-tmux@` runs, and the daemon samples it on a timer into the shared observation snapshot. The daemon verifies the serving process at bind (`ensure_tmux_server`) and fences effects to that identity. Launches arm `remain-on-exit` before dispatch, exit evidence is a typed read of `pane_dead`, `pane_dead_status`, `pane_dead_signal` and `pane_dead_time`, and each launch appends a `run-shell -b` wake to the global `pane-died` hook. Pane absence is proven only by a generation mismatch or fresh absence from a readable server. A session's subject is independent of its pane, and a pane may be moved with `join-pane`/`move-pane`/`break-pane`, which keep its id, process and PTY.

tmux 3.8, from its CHANGES file:

- Hooks and control-mode notifications are events with key/value payloads. `wait-for -v` shows a hook event's keys.
- Subscriptions generalise into monitors: `set-hook -B` takes the `refresh-client -B` syntax, checks a format every second, and fires a hook when it changes, or with `-T` when it becomes true. `show-hooks -B` lists them.
- `wait-for -E` waits for hooks, notifications or user events, and `set-hook -E` fires an event immediately.
- New hooks cover client create/destroy, pane activity, pane mode and prompt changes, pane move and resize, session group changes, and window create, close, swap and zoom.
- OSC 133 triggers `pane-command-started`, `pane-command-finished` and `pane-shell-prompt`.
- Layout strings are a JSON subset including floating panes. Control clients keep the old format unless they set `new-layouts`.
- Control-mode fixes: no hang on exit with queued pty data, no notifications to exiting clients, notifications queued outside `%begin`/`%end`, a stuck client cannot keep the server from exiting, offsets reset on respawn, and buffered replies are bounded for a client that stops reading.

The changelog does not say which of the new hook events reach a control client as `%` notifications, or whether a global monitor's hook can be observed by one client for every session. Those facts decide the observer's shape, so they are verified before it is built.

## Decisions

### 1. tmux 3.8 is the floor, enforced at bind

The daemon reads `#{version}` from the serving server, not `tmux -V` from a binary that may differ from the running server, and refuses to serve one older than 3.8. The refusal names the version found and `programs.radar.tmuxPackage`. The Home Manager module asserts `lib.versionAtLeast tmuxPackage.version "3.8"`. The flake provides tmux 3.8 (the pinned nixpkgs `tmux` with the 3.8 tag as its source) as the module default and in the test environment until nixpkgs carries it; the override is deleted when it does. No capability gate and no 3.7c path, following the earlier decision to require 3.8 rather than gate features on it.

A server already running 3.7c is refused until the operator restarts `radar-tmux@` on 3.8. `keep-old` means a switch alone does not restart it, so the refusal is what tells the operator a restart is needed.

### 2. Wake, never fact

Every notification maps to "sample soon". The sampler's reads establish state, so a lost notification costs latency, not correctness. A burst coalesces into one further sample. The timer and periodic reconciliation stay as the recovery path. This applies equally to placement events: a pane moving between windows wakes the sampler, and the move appears only when the inventory reports the pane in its new window.

### 3. Coverage: verify, then choose

Two shapes:

- **Server-wide.** One observer, with global `set-hook -gB` monitors and global hooks for the topology events, if 3.8 delivers those events to a control client attached to any session. One connection, no per-session lifecycle.
- **Per session.** One observer per session, started and ended from `%sessions-changed` plus the inventory and bounded (starting at 16), if session-scoped `%*`/`@*` subscriptions are still the only reliable delivery.

Task 1.1 settles which. The requirement is shape-independent: every session is either observed or reported as unobserved. Global hooks Radar installs follow the existing `pane-died` rule: appended in Radar's own slots and removed by slot, never by replacing a user's hook.

### 4. Narrow subscriptions

Monitor `pane_dead`, `pane_current_command`, `pane_id` membership per window, and window and session names. That is enough to wake on exit, foreground change, pane moves and renames. Monitors fire at most once a second, so they are wakes, not measurements. Pane output is never subscribed, and the observer keeps the old layout format because Radar does not parse layouts.

### 5. Run on the transport's runtime

An observer is a `tmux -C` child process read line by line through `tokio::process`, on the runtime from `consolidate-daemon-ingest`. Line length is bounded and parsing ignores what it does not recognise. Effects, hooks and snapshot reads stay in the synchronous adapter. Observers live in the daemon's service cgroup, not in `radar-tmux@` and not in any `radar.slice` scope, so stopping the daemon ends them and leaves the server, its panes and every managed scope untouched.

### 6. Fence to the verified server

Before attaching, the daemon re-establishes the verified serving process identity that every generation fence compares against. A changed identity ends the observers and starts new ones after verification. An observer never attaches to a server the daemon did not verify as its own.

### 7. Observers are not operator clients

A control client appears in `list-clients` and counts toward `session_attached`. The adapter filters on `client_control_mode`, so operator-attachment logic, including the `radar server stop` refusal and any attach-dependent guard, never sees them. Whether a current guard depends on attachment is checked, not assumed.

### 8. Evidence stays where it is

Exit evidence stays the typed `pane_dead_*` read over `remain-on-exit`. The per-launch `pane-died` wake stays as the trigger for that read. Absence stays proven by generation mismatch or a fresh read of a live server. Managed stop stays the scope stop. A notification may wake any of these reads and replaces none of them. Retiring the `pane-died` wake is a later change, and only if a monitor proves at least as timely and as reliable.

### 9. OSC 133 events are wakes here

On 3.8, `pane-command-finished` and `pane-shell-prompt` are useful wakes for shell panes: a foreground command ending changes what a delete would lose. Here they only request a sample. Typed shell-boundary facts are their own change, because they introduce new evidence with its own provenance. Pi panes emit no OSC 133, so Pi state stays with its publishers.

## Audit against the current tree

Checked after the launch chain, deadline sweep and reset landed.

- **The floor is not implemented.** `ensure_tmux_server` checks only that the unit is active and the server answers. `tmuxPackage` defaults to `pkgs.tmux` with a description and no assertion, the flake has no 3.8 source, and the local server is 3.7c. Section 0 is entirely open.
- **The wake seam does not exist.** The collector starts a refresh only when `next_refresh` (one second after the last) has passed. A committed event only shortens the sampler's sleep (`EVENT_WAKE_GAP`); it does not pull a refresh forward. An observer therefore needs one new primitive, a coalesced request that makes the next tick refresh now, subject to the same gap. Sampling stays demand-driven, so an idle daemon still reads nothing.
- **Tokio lacks `process`.** `Cargo.toml` enables `rt-multi-thread`, `net`, `sync`, `time`, `io-util`, and the transport is the only user. Observers need the `process` feature and a handle to the transport's runtime.
- **Other timers stay.** The two-second spawn sweep (pane evidence, the 30-second launch deadline) and the 60-second scope sweep are not observation latency problems and are outside this change. `radar reset` stops the daemon before it restarts tmux, so observers end with it and need no reset handling beyond the identity fence.
- **Nothing in the launch chain changes.** `radar run --launch`, scopes, `remain-on-exit` and the `pane-died` wake are unaffected.
- **The benefit is modest.** Today a topology change reaches a reader within about a second while it reads, and nothing is collected while idle. Control mode improves latency and avoids re-reading the inventory on a quiet server; it adds no new fact. It is worth doing for the sidebar's responsiveness, not as correctness work, and it is cheap to stop after the floor and the verification if 1.1 shows the events do not reach one client.

## Risks

- **Observer attached to the wrong server.** Mitigated by the identity fence and a test that a foreign server on the configured name is refused.
- **Output or notification floods.** `no-output` removes pane output, bursts coalesce, lines are bounded, and 3.8 bounds buffered replies.
- **An observer changing server behaviour.** `read-only` and `ignore-size` are verified to cause no resize or selection change, and `destroy-unattached` and `detach-on-destroy` are checked with an observer attached.
- **Global hooks colliding with the user's.** Radar's hooks use appended slots and are removed by slot, as `pane-died` already is.
- **A 3.7c server left running after the upgrade.** Refused at bind with the version named. The operator restarts the server unit.
- **The nixpkgs override drifting.** It is a source-only override of the pinned package, and is deleted once nixpkgs ships 3.8.

## Open choices

- Server-wide versus per-session observers (decided by 1.1).
- The observer bound and the retry backoff.
- Whether renames come from monitors or from `%window-renamed`/`%session-renamed`.
