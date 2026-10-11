# Tasks

## 1. Backend selection and tmux observation

- [x] 1.1 Add explicit daemon backend selection (`herdr` default, `tmux` with a named server) and refuse unknown names; declare capabilities per backend. Verify default unchanged and refusals without fallback.
- [x] 1.2 Implement the tmux adapter for inventory, focus, create, input, output and guarded close with stable qualified handles. Verify against a disposable named tmux server, including handle stability and uncertain-outcome semantics.

## 2. Launch and exit observation

- [x] 2.1 Implement tmux direct-argv launch with cwd/environment overlay, spawn token privacy and `remain-on-exit`. Verify argv fidelity, secret privacy and refusal when exit capture cannot be armed.
- [x] 2.2 Record the typed one-shot exit observation on v2 spawn edges, exposed through spawn.get/list and `exit_observation_v1`; current edges without exit and non-capable backends report `{state:"unknown"}`, while legacy v1 edges omit the field. Verify code, signal, vanished pane, restart persistence and unchanged publications.
- [x] 2.3 Verify a bound dead tmux child remains separate from close: recorded exit does not replace the fresh foreground birth-identity gate; `child.close` refuses before durable admission/dispatch and leaves execution/assignment publications unchanged. Record dead-pane cleanup as deferred pending its own exact-pane-dead proof.

## 3. Documentation and verification

- [x] 3.1 Update `docs/mux.md`, `docs/control-plane.md`, `docs/daemon.md`, the agent-registration contract and canonical fixture for backend selection, tmux guarantees and exit observation.
- [x] 3.2 Run fmt, strict Clippy, full locked tests, PTY and publisher smokes, a disposable-tmux smoke, strict OpenSpec and checked Nix; record in `verification.md`.
