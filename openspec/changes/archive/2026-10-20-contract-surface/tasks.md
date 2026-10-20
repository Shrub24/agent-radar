# Tasks

## 1. Wire

- [x] 1.1 Add the revision and a contracts list to the ping answer and stop advertising `mux.*` names. Test the answer for a tmux backend and a backend lacking a need of one contract.
- [x] 1.2 Make bundled clients (TUI, CLI) require a minimum revision and show the blocking diagnostic on a gap; remove the client whitelist and `intern`. Test an older daemon, a matching daemon and an unknown contract name.

## 2. Typed internals

- [x] 2.1 Add `Contract` and `Primitives` with an exhaustive `needs`, derive the advertised set from them, and remove the contract table, the per-operation capability table and the string constants. Keep a test that every contract's methods are refused when a need is missing.
- [x] 2.2 Return `Primitives` by value from `RuntimeProvider` and update the tmux and Herdr adapters and the capability checks in `server.rs`.
- [x] 2.3 Update the test backends in `tests/control_plane.rs` and the client tests; keep the safety intent of the refuse-before-effect tests.

## 3. Documentation and gate

- [x] 3.1 Update `docs/control-plane.md` and `docs/daemon.md` for the revision and contract list, and record in `ideas.md` that the primitive layer is deleted with the Herdr backend.
- [x] 3.2 Run fmt, strict Clippy, the locked tests, tmux acceptance, strict spec validation and the checked Nix build; record real exit codes.

## Gate record

- `pi-radar`: `npm ci --offline`, 91 package tests, `npm pack`, offline install, and root/extension-subpath import smoke all exited 0.
- `cargo fmt --check`: exit 0
- `cargo clippy --locked --all-targets -- -D warnings`: exit 0
- `cargo test --locked --no-fail-fast`: exit 0
- `openspec validate --all --strict`: exit 0 (33/33)
- `jj git export`: exit 0 (no export needed in the colocated workspace)
- `nix build`: exit 0 (`/nix/store/gp8199gasw2zcaypgbqs10y3bs7sfdj2-agent-radar-0.1.0`)
- tmux acceptance was not run per the owner's earlier instruction to avoid E2E/smoke runs; task 3.2 remains open.
