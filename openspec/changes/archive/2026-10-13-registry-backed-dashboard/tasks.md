# Tasks

## 1. Registry observation

- [x] 1.1 Read paged registry agents and spawn topology through the daemon client and normalize them to the existing model with provenance and freshness. Verify pagination, stale/unavailable and the bounded refresh.
- [x] 1.2 Join registry agents to inventory panes by location and exact process identity; report unjoined agents unresolved. Verify duplicate sessions, a replaced process and a missing pane.

## 2. Presentation

- [x] 2.1 Remove Herdr agent state, metadata and native status from the daemon-backed path and show registered state, unknown for no execution report, ordinary panes otherwise. Verify no Herdr agent fact supplies display state; any legacy metadata retained for containment is safety-only.
- [x] 2.2 Render the three authorities and per-value freshness in rows and details, with derived waiting/lost/settling. Verify against fixtures and the PTY smoke.

## 3. Verification

- [x] 3.1 Run fmt, strict Clippy, locked tests, both PTY smokes, a disposable-daemon and tmux run with Herdr absent, strict OpenSpec and checked Nix; record in `verification.md`.
