# Tasks

## 0. tmux 3.8 floor

- [ ] 0.1 Provide tmux 3.8 from the flake as the pinned nixpkgs `tmux` with the 3.8 tag as its source, make it the module's `tmuxPackage` default and the test environment's tmux, and assert `lib.versionAtLeast tmuxPackage.version "3.8"` in the module. Verify `nix build`, the module evaluation and `tmux -V` in the check environment.
- [ ] 0.2 Read `#{version}` from the serving server in `ensure_tmux_server` and refuse one older than 3.8, naming the version and `programs.radar.tmuxPackage`. Unit-test the version comparison, including `3.7c`, `3.8`, `3.8a`, `next-3.9` and an unparseable value.
- [ ] 0.3 Run the existing tmux adapter tests on 3.8 and fix only what 3.8 changes, such as the `\037` escape note in `src/tmux.rs` and the `pane-died` slot listing now that `show-hooks` uses formats. Update the docs that name 3.7c as tested and the `tmuxPackage` description.

## 1. Verify on tmux 3.8

- [ ] 1.1 On a disposable 3.8 server, record in `verification.md`:
  - an observer with `read-only,ignore-size,no-output` changes no size, selection or option, receives no pane output, and ends with its session;
  - `destroy-unattached` and `detach-on-destroy` behave as the design assumes with only an observer attached;
  - which events reach a control client as notifications: window create/close/swap, pane move (`join-pane`/`break-pane`), session create/destroy, OSC 133 command events, and a global `set-hook -gB` monitor firing for a session the client is not attached to.

  Decide server-wide or per-session coverage from the result and record it in `design.md`.
- [ ] 1.2 Check whether any current guard or report depends on session attachment, including the `radar server stop` refusal, and make it ignore control-mode clients through `client_control_mode`.

## 2. Observer

- [ ] 2.1 Add a bounded notification parser that ignores unknown notifications and discards output notifications. Test known notifications, unknown ones, a malformed line and an over-long line.
- [ ] 2.2 Run the observer, or observers, with `tokio::process` in the coverage shape from 1.1. Install Radar's monitors and hooks in appended slots and remove them by slot. Map notifications to sampler wake requests. Test a window add, a pane moved to another window, a layout burst coalescing into one sample, and a session created and destroyed.
- [ ] 2.3 Fence observers to the verified serving process identity, and end and re-create them when it changes. Refuse a foreign server. Test a replaced server with reused ids and a foreign server on the configured name.
- [ ] 2.4 Report observer health as a snapshot source with state, last notification time and loss reason. Keep the timer on loss and retry with bounded backoff. Test an observer that exits, one that cannot start, and an unobserved session being reported.

## 3. Documentation and acceptance

- [ ] 3.1 Document the 3.8 floor, the observer, its health source and its non-goals in `docs/daemon.md` and `docs/mux.md`. Update `ideas.md` so the tmux rows describe what is left: typed OSC 133 facts, and retiring the `pane-died` wake.
- [ ] 3.2 Run fmt, strict Clippy, the locked test suite, strict spec validation and the checked Nix build, and record real exit codes. PTY smokes, tmux acceptance with an observer attached, and the live server restart onto 3.8 are the operator's.
