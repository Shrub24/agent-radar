# Tasks

## 1. Launch

- [x] 1.1 Split a typed command by quoting alone and resolve its executable from `PATH`; tests for quotes, empty and unfinished lines, no expansion, and resolution order.
- [x] 1.2 `L` opens a prompt that owns the keyboard, Enter reviews the resolved command in the container confirmation, and Confirm sends it as the creation's recipe. App tests for the prompt, the dialog text, the recipe, a missing program and Esc.
- [ ] 1.3 Socket test that a confirmed launch through a disposable tmux server runs its command in the created pane.

- [x] 1.4 Enroll a lead started with `L` once it registers, and `E` for any registered lead; tests for the control, the guards and the launched-place memory.
- [ ] 1.5 Live acceptance: a Pi started with `L` becomes stoppable and restartable.

## 2. Attach

- [x] 2.1 `[tui] terminal` in the configuration, the `$TERMINAL -e` default, and the printed document; tests.
- [x] 2.2 Open a terminal attached to the target's server on the no-client refusal and ask again; command construction and handle parsing tested.
- [ ] 2.3 Verify against a real terminal on the user's machine (live acceptance): a focus with no client opens it, attaches and focuses; a missing terminal names both settings.

## 3. Documentation and gate

- [x] 3.1 Document `L`, the quoting rule, `[tui] terminal` and the attach behaviour in the README and `docs/daemon.md`.
- [ ] 3.2 Run fmt, strict Clippy, the locked tests, strict spec validation and the checked Nix build; record real exit codes.
