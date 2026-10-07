# Gate evidence — process-detail-pages (worker run, 2026-10-07)

Raw commands and outcomes from the implementer side, for the lead to re-run
independently and fold into `openspec/changes/process-detail-pages/verification.md`.
This is evidence, not the verification record: no claim here should be copied into
`verification.md` without the lead reproducing it.

Working copy over `6210c1f`, uncommitted. All commands in `nix develop` from the
repository root on `x86_64-linux`.

## Source gate

| Command | Outcome |
| --- | --- |
| `cargo fmt --check` | clean |
| `cargo clippy --locked --all-targets -- -D warnings` | clean, exit 0, no warnings |
| `cargo test --locked` | 0 failed: lib 261, animations 1, bus 20, bus_detail 17, collector 12, control 18, descendants 1, focus 4, lifecycle 12, managed 9, nested_tree 37, runtime 20, stale_binary 11, doc-tests 0 |
| `cargo build --locked` | OK |
| `git status --porcelain -- Cargo.lock` | empty — the one dependency edit (`ratatui` feature `unstable-rendered-line-info`) resolved inside the pinned version, so the lock is unchanged |

## PTY coverage (task 3.2's navigation interactions)

`python3 tests/terminal_smoke.py` — both scenarios PASS:

- scenario 1 covers details focus, page cycling with `←`/`→`, a scroll key
  (`k`) inside the details rather than the fleet, a disclosure opened and closed
  with `Space`/`Enter`, and `Escape` back to the tree, alongside the pre-existing
  bus/source/quit coverage;
- scenario 2 covers a socket owned by another Radar (`bus off`, prompt quit).

The smokes drive the real binary through a PTY; they do not replace the unit and
integration coverage of the geometry (below).

## Package gate

- `nix build .#radar` → `/nix/store/ndfk3yqk68b90gmh536flcayvqfkimjw-agent-radar-0.1.0`,
  deriver `h6alr6c1a74f8v6a3bcrp1mwpiqrv6w9`. `buildRustPackage` runs the crate's
  tests in `checkPhase` (no `doCheck = false` in `nix/package.nix`).
- Forced `nix build --rebuild -L .#radar` so the check phase is observed rather
  than inferred from a cached output path: the sandbox built the crate and ran the
  suite in release — 261 lib tests plus every integration suite, 0 failed — and
  produced that same store path, exit 0.
- `nix flake check` — "all checks passed"; `checks.x86_64-linux.package` evaluates
  to the package derivation. It reports `running 0 flake checks` (the check is the
  package derivation, not a separate build) and warns that
  `aarch64-darwin`, `aarch64-linux`, `x86_64-darwin` were omitted as incompatible.
- `nix flake check` still reports "unknown flake output `homeManagerModules`", a
  pre-existing warning from the packaging change.

## OpenSpec gate

`openspec validate --all --strict` — 12 passed, 0 failed. Two `[INFO]`
`requirements[n]: Requirement text is very long (>500 characters)` notes are
reported against `spec/stale-binary` and `spec/tree-sorting` (pre-existing specs
in this change's path), not against the new deltas; both items still pass.

## Not proven here

- `aarch64-linux` was evaluated, not built. Darwin packaging remains out of scope.
- Descendant sampling and resource semantics are exercised against scripted
  `/proc` tables and real `/proc`, but only on this host's kernel; other kernels'
  `stat`/`status` quirks are unverified.
- No live publisher sends a captured-at-spawn birth identity, so the task-PID
  withholding path is covered by tests only.
- No live runtime, mux session, owner or control directory was touched by any
  command above; every test that needs one injects a stub.
