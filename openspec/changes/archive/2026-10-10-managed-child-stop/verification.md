# Verification

## Source and behavioral checks

- `cargo fmt --all -- --check` — passed.
- `cargo clippy --locked --all-targets -- -D warnings` — passed.
- `cargo test --locked` — passed; all unit and integration suites passed, including 77 `control_plane` tests and 28 `runtime` tests.
- `cargo build --locked` and `cargo build --release --locked` — passed.
- `python3 -m py_compile tests/agent_publisher.py tests/fixtures/fake-herdr.py` and `bash -n tests/fixtures/live-managed-child.sh` — passed.
- `python3 tests/agent_publisher.py` — passed. This validates the canonical registry/spawn/close fixture against a disposable daemon and scripted Herdr backend. It confirms exact child binding, one pane-close dispatch, same-ID replay without redispatch, and unchanged assignment/execution publications and spawn topology.
- `python3 tests/terminal_smoke.py ./target/debug/radar` — passed twice. Both production-loop PTY scenarios passed, including details navigation, process-tree interaction, terminal restoration and prompt quit when a client is connected.
- `RADAR_HERDR_SMOKE=1 cargo test --locked --test runtime a_real_herdr_managed_child_close_is_durable_and_idempotent -- --exact` — passed independently twice against live Herdr 0.9.3. The smoke creates its own disposable tab and registered parent, uses public `spawn`, registers the actual child with its own `/proc/self` PID/boot ID/start ticks, closes the recorded child pane once, verifies the durable completed outcome, replays without another close, preserves topology and published facts, and removes the disposable tab.
- The same live-smoke test without `RADAR_HERDR_SMOKE=1` — skipped cleanly as intended.
- `openspec validate --all --strict` — passed, 17/17.
- `git diff --check` — passed.

## Nix provenance

- `nix flake check -L` — passed on `x86_64-linux`; package check built and ran the locked Rust test suite. Nix reported the existing `homeManagerModules` unknown-output warning and omitted `aarch64-linux`, `x86_64-darwin` and `aarch64-darwin` from checks.
- `nix build .#radar --print-out-paths -L` — passed; output `/nix/store/m5lj8xf7dfndagwaxcfw3pmp0p4xais2-agent-radar-0.1.0`.

## Limits

- The live smoke is destructive only inside the disposable tab it creates and removes; no user fleet panes are targeted.
- The verification claim is pane close, not process exit. The test observes that this Herdr close ended the child process, but the daemon records no process-exit fact from that observation.
- No Herdsman adoption or live publisher migration was performed. `child.close` is not separately advertised in `ping`; consumers require protocol compatibility plus backend `observe` and `close` and must honor refusal/unknown outcomes.
- This change adds no tmux backend or cross-platform support.
