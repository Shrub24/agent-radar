# Configuration is scoped by what you configure, not by which process reads it

**Decision.** The configuration file is grouped by the thing being configured:
`[server]`, `[agents]`, `[projects]`, `[tui]`. `[daemon]` is gone.

**Why.** The previous split followed the process that read a key, and that never
matched the surface an operator edits. `launch_command` sat under `[tui]` while
the daemon's launch path was what ran it; `focus_created` would have gone under
`[daemon]` while the surface it changes is "what happens when I create
something". A section named for a process forces the reader to know the
architecture to find a setting.

**Consequences.** `launch_command` moved to `[agents]`, `project_source`,
`project_base` and `project_filter` to `[projects]` as `source`, `base` and
`filter`, and `backend` and `tmux_server` to `[server]`. `[tui] terminal` stayed:
it is how the dashboard attaches. A configuration that still uses `[daemon]`, or
a key that moved, is refused with the section it belongs in rather than silently
defaulted — a mistyped or stale file must not appear to work. `FileTui` therefore
deserializes by hand so it can name where a key went.

# Creating a location does not move you; focusing it afterwards does

**Decision.** A create is dispatched unfocused and the daemon focuses the
location it made afterwards, when `[server] focus_created` is true (the default).
A request may say otherwise with `"focus": false`, which a child agent spawn
does.

**Why.** tmux's `-d` only changes the session's current window, so a client on
another session would not move; only `switch-client` moves the operator, and the
daemon already has that path in the focus operation. Doing it in the adapter
would duplicate that logic and make "what did this create do" depend on the flag.

**Consequences.** The focus is a separate effect: a refused or unknown focus is
reported on the creation's record but never changes its outcome and never repeats
the create, which has already happened. Workspace creation stays unfocused
because tmux refuses to create a session and switch a client in one command.
Tab creation focuses its first pane. A successful focus publishes no outcome —
it changes nothing in the tree, so the dashboard's message is not replaced by
"completed"; a refusal is published, because that is the answer that says nothing
moved. `focus_created` lives in `[server]` rather than `[agents]` because it
applies to adding a pane or a tab as much as to launching an agent.
