## Why

`A` creates a tmux session named `0` in whatever directory the daemon started in. A workspace should be a project: started in a chosen folder and named after it.

## What Changes

- `A` opens a fuzzy picker over candidate project directories and creates a workspace in the chosen one, named for its git root's folder. Escape creates nothing.
- Candidates are the directory Radar was started in, then the output of `[tui] project_source` (default `zoxide query -l`), limited by `[tui] project_filter` (default `git`: only directories containing `.git`; `none` offers all). A missing or failing source leaves the start directory alone and says why in one line.
- The picker is the `nucleo-picker` library inside the dashboard, which gives the terminal back while it runs.
- `topology.create` and `mux.create` accept an optional absolute `directory` for a workspace. The tmux runtime checks it is an existing directory before creating anything and starts the session there. Herdr refuses it.

Non-goals: tracking a later `cd` in the primary pane (the directory is set once and Radar does not follow it), moving panes or sessions between projects, a confirmation dialog (the choice in the picker is the confirmation), and an arbitrary filter expression.

## Impact

- `src/project.rs` (new), `src/main.rs`, `src/app.rs`, `src/config.rs`, `src/runtime.rs`, `src/tmux.rs`, `src/herdr.rs`, `src/control_plane/client.rs`; new dependency `nucleo-picker`.
- `a` and `L` are unchanged: a tab starts where tmux puts it.
