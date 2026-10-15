# Design

## Decisions

### 1. `L` is the managed-agent shortcut

`[tui].launch_command` is an argv vector, defaulting to `["pi"]`. Pressing `L` on a workspace or pane immediately uses the existing `topology.create` path to create a new tab or split pane and run that command as the private recipe. There is no command prompt or second confirmation. `a` remains the empty-shell action for arbitrary commands.

### 2. Launch in the target project context

For a workspace, use its recorded session directory. For a pane, use that pane's current directory. Start the command through the user's login/interactive shell at that directory so shell startup configuration runs, then explicitly apply `direnv export` for the target project. Do not snapshot the TUI environment: the TUI may itself be running under an unrelated project's direnv context. Do not persist or expose environment values in public operation data.

The launch argv remains data, not shell source. Store its program name and arguments as positional data in the private recipe; after shell startup and target-directory direnv activation, the shell resolves and executes them without interpolation. This allows project-local tools to be supplied by direnv rather than requiring them on the TUI's `PATH`. Live acceptance verifies target cwd and shell/project environment and proves a TUI-only marker does not leak.

### 3. A launched lead is enrolled

Pi started by `L` registers itself, so it appears as an agent with no lifecycle subject and could not be stopped or restarted. The dashboard remembers the place a recipe creation completed in; once an agent joined to that place has a fresh registered session it sends one `lifecycle.enroll` for it, because the operator chose the managed-agent launch action. A place is offered once, so a daemon refusal is not retried. `E` on any joined, registered agent opens the same enrollment in a default-Cancel confirmation. The daemon still verifies the live foreground process and the session before it records anything, and it fabricates no spawn edge.

### 4. Attach is the dashboard's job

The daemon's focus refuses with an exact, named refusal when no client is attached. The dashboard recognizes that refusal, opens `<terminal> tmux -L <server> attach-session -t <id>` (tmux resolves a pane, window or session id there), and asks again every 250 ms for six seconds. Any other answer ends the wait. A terminal that exits within three seconds with an error is reported with the setting to check.

### 5. Configuration

Add `[tui].launch_command` as an argv vector with default `["pi"]`. Keep `[tui].terminal` as the terminal program and leading arguments for focus attach; unset, it derives from `$TERMINAL` as `$TERMINAL -e`.

## Superseded design

The initial implementation treated `L` as a prompt for arbitrary command text and then showed a confirmation. That is superseded: `L` directly starts the configured managed-agent command; an empty pane for arbitrary commands is created with `a`.