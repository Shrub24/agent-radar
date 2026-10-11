# Verification

Independent gate on x86_64-linux:

- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`: clean.
- `cargo test --locked`: all binaries green (344 library tests, 80 control-plane tests), including concurrent append, retention, clean and unclean epoch restart, long-poll wake, expiry and shutdown joins.
- `python3 tests/agent_publisher.py` ×2: fixture exchanges (incl. normal, empty-wait and expired `events.read`), scripted spawn/close and the reconnecting events consumer passed against a disposable daemon.
- `python3 tests/fixtures/events_consumer_smoke.py`: passed.
- `python3 tests/terminal_smoke.py ./target/debug/radar` ×2: both PTY scenarios passed.
- `openspec validate --all --strict`: 21/21.
- `nix flake check -L` and `nix build .#radar`: passed; output `/nix/store/cbkqqb4ammm7mflyv6r40ii3zcm77zfq-agent-radar-0.1.0`. Nix covers the Rust suite only, not the Python smokes.

Notes: one unexplained, non-reproduced flake of `a_stale_socket_file_is_cleared_and_a_live_one_is_reported` was reported by the worker; it passed in every later run, including this gate. No live Herdr was used.

Limits: events are pointers to committed facts only. An append can be lost after a durable fact; an unclean restart changes the epoch so consumers resynchronise. No physical backend event feed, no exit or readiness events.
