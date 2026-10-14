# Proposal

## Why

The dashboard can create empty places but cannot start anything in them: there is no launch action, so a lead can only be started by running a program by hand in a pane. A focus also needs a tmux client, and with none attached it only fails. Radar should be the way to open a project, run a command in it, and see it.

## What Changes

- `L` on a workspace or pane opens a one-line prompt for a command. The line is split by quoting only, with no expansion, into an executable and arguments; the executable is resolved to an absolute path from the dashboard's `PATH`; the confirmation (Cancel by default) shows exactly what runs and where; confirming runs it in a new tab (from a workspace) or a split pane (from a pane) through `topology.create` with the command as the container's recipe. This is also the recipe input that was deferred, and a created pane can be reloaded.
- A focus the daemon refuses because no client is attached opens a terminal attached to the target's server and asks again until a client exists. The dashboard opens it, not the daemon, because only the dashboard holds the user's desktop session and knows their terminal.
- A lead started with `L` is enrolled as a lifecycle subject once it registers a joined pane and a fresh session, so it can be stopped and restarted. `E` does the same for any registered lead behind a confirmation.
- `[tui] terminal` is the program and leading arguments that run a command in a new terminal. Unset, it is `$TERMINAL -e`; when neither is set the footer names both settings.

Non-goals: shell expansion or aliases; choosing the working directory per launch; detecting a terminal automatically.

## Capabilities

### Modified Capabilities

- `focus-action`: a focus with no client attached attaches one.
- `lifecycle-actions`: a command can be run in a new place from the tree.

## Impact

- `src/app.rs`, `src/ui.rs`, `src/topology.rs`, `src/attach.rs`, `src/config.rs`, `src/main.rs`, docs.
- Depends on `adopted-topology` for the no-client refusal and for every place being a container.
