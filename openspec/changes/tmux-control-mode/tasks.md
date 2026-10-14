# Tasks

## 1. Verify on the tested tmux

- [ ] 1.1 On a disposable server under its unit, verify that an observer with `read-only,ignore-size,no-output` changes no window size, selection or option, receives no pane output, ends with the session, and that `destroy-unattached` and `detach-on-destroy` behave as the design assumes. Record the results in `verification.md`.
- [ ] 1.2 Check whether any current guard or report depends on session attachment and make it ignore control-mode clients through `client_control_mode`.

## 2. Observer

- [ ] 2.1 Add a bounded notification parser that ignores unknown notifications and discards output notifications. Test known notifications, unknown ones, a malformed line and an over-long line.
- [ ] 2.2 Run an observer per session with `tokio::process`, subscribe to the narrow format set, and map notifications to sampler wake requests. Test a window add, a layout burst, a subscription change and a session created and destroyed.
- [ ] 2.3 Fence observers to the verified serving process identity and end and re-create them when it changes; refuse a foreign server. Test a replaced server with reused ids and a foreign server on the configured name.
- [ ] 2.4 Report observer health as a snapshot source with state, last notification time and loss reason; keep the timer on loss and retry with bounded backoff. Test an observer that exits, one that cannot start, and the session bound.

## 3. Documentation and acceptance

- [ ] 3.1 Document the observer, its health source and its non-goals in `docs/daemon.md`; update `ideas.md` so the 3.8 row covers only OSC 133 and `set-hook -B`.
- [ ] 3.2 Run fmt, strict Clippy, the locked test suite, both PTY smokes, tmux acceptance with an observer attached, strict spec validation and the checked Nix build; record real exit codes. Live acceptance is the operator's.
