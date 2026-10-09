# Verification

## Scope

`managed-child-spawn` delivers daemon-owned creation of managed child panes and a durable runtime edge bound through the child's private spawn token. It does not deliver stop, resume, restart, lead recovery, assignment authority, or adoption of panes created outside the daemon.

## Source gates

Validated the final source state with:

- `cargo fmt --check` — pass.
- `cargo clippy --locked --all-targets -- -D warnings` — pass.
- `cargo test --locked` — pass, 646 tests across 17 binaries, 0 failures.
- `cargo build --locked` — pass.
- `openspec validate --all --strict` — 16 items passed, 0 failed.
- `python3 -m py_compile examples/agent-publisher/publisher.py tests/agent_publisher.py tests/fixtures/fake-herdr.py tests/fixtures/fake-herdr-socket.py` — pass.
- `python3 tests/agent_publisher.py` — pass against a disposable daemon; registry/context publication, session switch and explicit null, privacy, replay/fencing, restart freshness, scripted Herdr spawn, token binding, topology reads and redaction.
- `python3 tests/terminal_smoke.py` — both PTY scenarios pass, including details navigation, foldable process table, filtering, shutdown, and terminal restoration.
- `RADAR_HERDR_SMOKE=1 cargo test --locked --test runtime -- a_real_herdr_launch` — pass twice against a real disposable Herdr 0.9.3 session; child argv and token reached the pane byte-exact; the test closed its own tab and verified no leftovers.

## Nix provenance

- `nix flake check path:. -L` — pass, all checks passed. The sandboxed check built the release package and ran the locked test suite.
- `nix build path:.#radar --print-out-paths -L` — pass.
- Package output: `/nix/store/4n3d9yci7whqmnnlfxzalx00bg6745k0-agent-radar-0.1.0`.
- Build executed through the configured remote builder `ssh-ng://nixbuild@home-forge`.
- Platform coverage: `x86_64-linux`; aarch64-linux was evaluation-only and Darwin is outside the Linux-only daemon scope. Nix emitted the existing `homeManagerModules` unknown-output warning.

## Harness note

One independent combined control-plane rerun reproduced the existing `a_stale_socket_file_is_cleared_and_a_live_one_is_reported` test race once (72/73 passed): a fork inherits a listener fd after the test drops the listener, making the stale socket briefly appear live. The full locked suite passed immediately afterward, and the checked Nix suite passed. This is a test-harness race, not a spawn-path failure; it remains a follow-up.

## Limits

No live user fleet was contacted. The real Herdr test used a disposable session and cleaned it up. The scripted spawn conformance test uses a local fake Herdr backend. No claim is made for stop, resume, restart, recovery, lifecycle authority, or foreign-pane adoption. Newline-bearing argv is refused before pane input until a bounded cleanup/ack protocol exists. An uncertain spawn dispatch must not be retried or routed through another backend.
