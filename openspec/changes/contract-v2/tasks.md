# Tasks

## 1. Wire scheme

- [x] 1.1 Introduce protocol version 2 and the namespaced method table with typed refusal of version 1 and removed names; rename server classification, the client and operation tables. Verify old names are `unknown_method` and no effect occurs.
- [x] 1.2 Replace the capability strings with the `mux.*` backend capabilities and the derived daemon contracts in one place. Verify each derived capability appears only when its dependencies do, and that nothing advertised is refused.
- [x] 1.3 Report the real backend identity and tmux instance in `daemon.ping`. Verify herdr, tmux and none.

## 2. Semantics

- [x] 2.1 Change `events.read` to the delivered-position `cursor`, `tail` and `more`. Verify a limit-bounded and a byte-bounded prefix cannot skip events, empty reads, expiry and epoch change.
- [x] 2.2 Implement `already_exited` close with durable replay, the dead-now plus matching-edge-exit gate and the serving-process generation fence. Verify respawn-after-exit, another server generation with reused pane ids, a missing or changed instance, no recorded exit, absent pane and replay without a second dispatch.
- [x] 2.3 Audit result keys, outcome words and error codes across the surface and align them; record the table.

## 3. Documentation and verification

- [x] 3.1 Update `docs/control-plane.md`, `docs/mux.md`, `docs/daemon.md`, `docs/agent-registration.md`, canonical fixtures and examples to the v2 names and cursor semantics; run the full-comparison validator.
- [x] 3.2 Run fmt, strict Clippy, locked tests, PTY and publisher smokes, a disposable tmux close/events smoke, strict OpenSpec and the checked Nix build; record in `verification.md`.
