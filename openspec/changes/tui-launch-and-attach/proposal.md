# Proposal

## Why

The dashboard can create empty places but cannot start a managed agent in them. `L` should be the direct managed-agent shortcut; creating an empty shell remains the job of `a`, after which the user can run any command manually. A focus also needs a tmux client, and with none attached it only fails. Radar should be the way to open a project, run an agent in it, and see it.

## What Changes

- `L` on a recorded workspace or pane immediately launches `[tui].launch_command`, an argv vector defaulting to `["pi"]`, in a new tab or split pane through `topology.create`. It does not open a command prompt or a second confirmation. `a` remains the empty-shell action for arbitrary interactive commands.
- Launch uses the workspace's recorded directory or the selected pane's current directory. It starts through the user's login/interactive shell there, so shell startup configuration and that target directory's direnv environment apply. It does not copy the TUI's environment wholesale, which could leak another project's direnv values.
- A lead launched with `L` is enrolled as a lifecycle subject once it registers a joined pane and a fresh session, so it can be stopped and restarted. `E` enrolls any registered lead behind a default-Cancel confirmation.
- A focus the daemon refuses because no client is attached opens a terminal attached to the target's server and asks again until a client exists. The dashboard opens it, not the daemon, because only the dashboard holds the user's desktop session and knows their terminal.
- `[tui] terminal` is the program and leading arguments that run a command in a new terminal. Unset, it is `$TERMINAL -e`; when neither is set the footer names both settings.

Non-goals: arbitrary command entry through `L`, shell expansion of launch config, passing the TUI's current environment into a target project, or detecting a terminal automatically.

## Capabilities

### Modified Capabilities

- `focus-action`: a focus with no client attached attaches one.
- `lifecycle-actions`: `L` launches the configured managed-agent command in the selected project context.

## Impact

- `src/app.rs`, `src/ui.rs`, `src/topology.rs`, `src/attach.rs`, `src/config.rs`, `src/main.rs`, `src/tmux.rs`, tests and docs.
- Depends on `adopted-topology` for the no-client refusal and for every place being a container.

## Decisions

- The configured launch command is argv, not shell text. The target shell resolves the executable after startup and direnv activation, so project-provided commands are available.
- The target shell, not the dashboard process, establishes environment at the launch directory. No environment values are copied into the durable recipe or sent as a TUI snapshot.
- `L` has no prompt or confirmation: pressing it launches the configured agent command. Use `a` for a shell when the user wants to type an arbitrary command.
- The selected directory is a launch-time input; Radar does not follow later cwd changes.
- Exact target-shell startup and direnv behavior remains a live acceptance item; command construction alone does not prove the user's shell configuration.

## Verification

- Unit tests cover config defaults/overrides, target-shell argv construction, immediate `L` dispatch, workspace/pane launch directories, and an empty TUI environment overlay.
- Live acceptance verifies the target shell's login environment and target-directory direnv are active while the configured command runs, without copying a marker from the TUI environment.
- Existing auto-enrollment and attach tests remain in scope.
- Run formatting, strict Clippy, the locked Rust suite, strict OpenSpec validation and the checked Nix build. Live launch, Pi stop/restart and real-terminal attach remain user acceptance.

## Future

The durable-execution and lifecycle architecture is intentionally outside this change; the user will provide that context before that work begins.

## Previous direction superseded

The original draft of this change described `L` as a typed arbitrary command prompt with a confirmation. That direction is replaced by this proposal: `L` launches the configured command (default `pi`) directly, and `a` creates an empty shell for arbitrary commands.

## Related prior history

The original proposal and implementation are retained in the git history; there is no separate replacement change.