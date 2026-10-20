# Tasks

## 1. Launch

- [x] 1.1 Add `[tui].launch_command` as an argv config with default `["pi"]`; test defaults, overrides and empty values. Resolve the program in the target shell after direnv activation.
- [x] 1.2 Make `L` immediately launch the configured argv in a new tab/pane without prompt or confirmation; app tests cover workspace/pane target directories. Keep `a` as an empty shell.
- [x] 1.3 Start the command through the user's login/interactive shell at the target directory and apply direnv there; unit tests cover shell argv, target cwd, and no copied TUI environment.
- [x] 1.4 Live acceptance: launch with `L` in a workspace and pane; verify target cwd, shell environment and direnv, and that a TUI-only marker is absent.

- [x] 1.5 Enroll a lead started with `L` once it registers, and `E` for any registered lead; tests for the control, the guards and the launched-place memory.
- [x] 1.6 Live acceptance: a Pi started with `L` becomes stoppable and restartable.

## 2. Attach

- [x] 2.1 `[tui] terminal` in the configuration, the `$TERMINAL -e` default, and the printed document; tests.
- [x] 2.2 Open a terminal attached to the target's server on the no-client refusal and ask again; command construction and handle parsing tested.
- [x] 2.3 Verify against a real terminal on the user's machine (live acceptance): a focus with no client opens it, attaches and focuses; a missing terminal names both settings.

## 3. Documentation and gate

- [x] 3.1 Document direct `L` launch, `[tui].launch_command`, target-shell/cwd behavior, `a` for an empty shell, `[tui] terminal` and attach behavior in the README, `docs/daemon.md` and `docs/control-plane.md`.
- [x] 3.2 Run fmt, strict Clippy, the locked tests, strict spec validation and the checked Nix build; all gates exited 0.
