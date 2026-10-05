# Tasks

## 1. Focus

- [x] 1.1 Add a focus module that focuses a workspace, or a workspace/tab/pane location, through Herdr off the UI thread with the collector's timeout and child-reaping rules; verify tests with a fake `herdr` on `PATH` cover success, a nonzero exit, a hang past the timeout, and that quitting is not delayed by an outstanding request.
- [x] 1.2 Bind `Enter` in the tree (not during filter entry) to focus the selected row's target, refuse when the inventory is not current or the row's pane is absent from the observation, with a message; a retained row focuses the pane it was last seen on, show the transient result line, and add the hint; verify render and key tests cover an agent row, a workspace row, a retained row, filter entry and a failure message that clears.
- [x] 1.3 Update the README and `plan.md` (move focus out of the roadmap) and verify `cargo fmt --check`, Clippy with `-D warnings`, `cargo test --locked` and the PTY smoke pass.
