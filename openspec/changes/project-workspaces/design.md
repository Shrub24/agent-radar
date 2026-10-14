## Decisions

### The directory is set once
Radar passes `-c <directory>` to `new-session`. It does not follow a later `cd`, so nothing watches the pane's cwd and nothing is recorded to go stale. A container is identified by its id, never by its directory, so a future rule for following the primary pane, or for moving panes and sessions between projects, changes no record.

### Candidates run in the dashboard
The source command runs in the TUI with a 3 s limit, because the TUI has the user's `PATH`, tools and home directory and the daemon's environment is deliberately different. Its output is only a list of paths: each must be absolute, exist and pass the filter, and none is ever run. The `zoxide` crate was rejected because it ships no library; the CLI is the stable interface and is modular, so any command that prints directories (`sesh list`, a `find`) works.

### The daemon checks the directory
The tmux runtime refuses a relative or missing directory before creating anything. Otherwise tmux silently falls back to another directory and reports success.

### The picker owns the terminal
`nucleo-picker` draws its own screen, so the dashboard disables mouse capture and restores the terminal before opening it, re-initialises afterwards and clears, whatever the picker answered.
