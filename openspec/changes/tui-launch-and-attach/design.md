# Design

## Decisions

### 1. Launch through the existing creation, with a recipe

`topology.create` already makes a place and runs a command in it with no spawn token, holding the command privately and recording only that a recipe exists. `L` is that request with a typed command, so the daemon needs no new contract and the created pane can be reloaded like any recipe pane.

### 2. The dashboard resolves the program

The launch contract runs an absolute executable and never searches a `PATH` of its own. The dashboard holds the user's `PATH`, so it resolves the first word, shows the resolved path in the confirmation, and refuses a program that is not on it before any dialog opens.

### 3. No shell

The line is split by quotes and backslashes only. `$VAR`, `~`, globs, pipes and redirections stay ordinary text, because the daemon runs an argv and refuses to invent a shell. A user who wants a shell types `sh -c '...'` and sees it in the confirmation.

### 4. Attach is the dashboard's job

The daemon's focus refuses with an exact, named refusal when no client is attached. The dashboard recognizes that refusal, opens `<terminal> tmux -L <server> attach-session -t <id>` (tmux resolves a pane, window or session id there), and asks again every 250 ms for six seconds. Any other answer ends the wait. A terminal that exits within three seconds with an error is reported with the setting to check.

### 5. A launched lead is enrolled

Pi started by `L` registers itself, so it appears as an agent with no lifecycle subject and could not be stopped or restarted. The dashboard remembers the place a recipe creation completed in; once an agent joined to that place has a fresh registered session it sends one `lifecycle.enroll` for it, because the operator already confirmed running the command. A place is offered once, so a daemon refusal is not retried. `E` on any joined, registered agent opens the same enrollment in a default-Cancel confirmation for leads started any other way. The daemon still verifies the live foreground process and the session before it records anything, and it fabricates no spawn edge.
