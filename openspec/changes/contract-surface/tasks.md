# Tasks

## 1. Wire

- [ ] 1.1 Add the revision and a contracts list to the ping answer and stop advertising `mux.*` names. Test the answer for a tmux backend and a backend lacking a need of one contract.
- [ ] 1.2 Make bundled clients (TUI, CLI) require a minimum revision and show the blocking diagnostic on a gap; remove the client whitelist and `intern`. Test an older daemon, a matching daemon and an unknown contract name.

## 2. Typed internals

- [ ] 2.1 Add `Contract` and `Primitives` with an exhaustive `needs`, derive the advertised set from them, and remove the contract table, the per-operation capability table and the string constants. Keep a test that every contract's methods are refused when a need is missing.
- [ ] 2.2 Return `Primitives` by value from `RuntimeProvider` and update the tmux and Herdr adapters and the capability checks in `server.rs`.
- [ ] 2.3 Update the test backends in `tests/control_plane.rs` and the client tests; keep the safety intent of the refuse-before-effect tests.

## 3. Documentation and gate

- [ ] 3.1 Update `docs/control-plane.md` and `docs/daemon.md` for the revision and contract list, and record in `ideas.md` that the primitive layer is deleted with the Herdr backend.
- [ ] 3.2 Run fmt, strict Clippy, the locked tests, tmux acceptance, strict spec validation and the checked Nix build; record real exit codes.
