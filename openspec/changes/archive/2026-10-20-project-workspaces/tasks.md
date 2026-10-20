## 1. Daemon

- [x] 1.1 Accept an optional `directory` on workspace creation; tmux checks and uses it, Herdr refuses it. Real-tmux test for a created session's path and for refused missing and relative directories.

## 2. Dashboard

- [x] 2.1 `src/project.rs`: candidates, filter, naming and the picker; `[tui] project_source` and `project_filter`. Tests for ordering and filtering, naming, and a missing or failing source.
- [x] 2.2 `A` gives the terminal back, runs the picker, and creates the workspace in the choice.
- [x] 2.3 Live check of the picker in a real terminal: appearance, cancel and the mouse after return.

## 3. Verification

- [x] 3.1 Run fmt, strict Clippy, the locked tests, strict spec validation and the checked Nix build; record real exit codes.
