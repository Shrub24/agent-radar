# Tasks

## 1. Backend selection and tmux observation

- [ ] 1.1 Add explicit daemon backend selection (`herdr` default, `tmux` with a named server) and refuse unknown names; declare capabilities per backend. Verify default unchanged and refusals without fallback.
- [ ] 1.2 Implement the tmux adapter for inventory, focus, create, input, output and guarded close with stable qualified handles. Verify against a disposable named tmux server, including handle stability and uncertain-outcome semantics.

## 2. Launch and exit observation

- [ ] 2.1 Implement tmux direct-argv launch with cwd/environment overlay, spawn token privacy and `remain-on-exit`. Verify argv fidelity, secret privacy and refusal when exit capture cannot be armed.
- [ ] 2.2 Record the typed one-shot exit observation on spawn edges, exposed through spawn.get/list and a capability; Herdr reports unknown. Verify code, signal, vanished pane, restart persistence and unchanged publications.
- [ ] 2.3 Verify `child.close` on a dead tmux pane stays separate and safe; settle the dead-pane gate in design.

## 3. Documentation and verification

- [ ] 3.1 Update `docs/mux.md`, `docs/control-plane.md` and fixtures for the selector, tmux guarantees and exit observation.
- [ ] 3.2 Run fmt, strict Clippy, full locked tests, PTY and publisher smokes, a gated disposable-tmux smoke, strict OpenSpec and checked Nix; record in verification.md.
