# TUI extraction verification

## Automated evidence

- `cargo fmt --check` — passed.
- `cargo clippy --locked --all-targets -- -D warnings` — passed.
- `cargo test --locked --no-fail-fast` — passed, including the dashboard-client cancel, stop, restart, child-close and second-client readback paths.
- `openspec validate --all --strict` — 23/23 passed.
- `src/tree.rs` tests verify pending and bound-but-unjoined subjects appear as separate lifecycle rows, revoked subjects are omitted, and no pane identity is invented.
- `tests/animations.rs` verifies a pending lifecycle row participates in the configured loading animation and renders its loading frame.
- `cargo build --locked` — passed.
- `python3 tests/terminal_smoke.py target/debug/radar` — passed; exercises the daemon-backed dashboard, recovery, input handling and terminal restoration, but not a managed lifecycle confirmation/action.
- `python3 tests/tmux_acceptance.py target/debug/radar` — passed; disposable server/daemon, unresolved registrations, managed-action refusal without a daemon subject, no Herdr calls, and cleanup.
- `python3 tests/agent_publisher.py` — passed; registry/events, spawn binding, child close replay and events consumer.
- `nix build .#radar --no-link --print-out-paths` — passed on the final working copy: `/nix/store/kr64cw03rqq230jb8srk4rskp6zwsjs1-agent-radar-0.1.0`.

## Lifecycle control evidence

- App tests cover default-Cancel confirmation and refusal when the process birth identity changes.
- Registry projection joins a managed target only to a joined registration with the exact process identity and matching spawn-edge subject; unjoined pane evidence yields no target.
- The managed-control worker tests prove: missing `spawn.close.v1` refuses before process cancellation; cancellation precedes the separate fenced child close; a refused cancellation never dispatches close; a lost reply is read back by the same request ID rather than redispatched; and the bounded sequence completes during TUI shutdown.
- Socket integration tests exercise the dashboard client's `lifecycle.cancel`, `lifecycle.stop`, `lifecycle.restart`, `spawn.close`, and a second client's durable operation readback.

## Lifecycle visibility

The daemon's typed lifecycle log already records subject transitions and the subscriber view emits lifecycle invalidation pointers (`lifecycle.created`, `lifecycle.captured`, `lifecycle.bound`, `lifecycle.revoked`, and related operation events). The TUI reads the public lifecycle projection, not raw event payloads. Unmatched pending/captured subjects now appear as muted root rows with the configured loading animation; bound-but-unjoined subjects appear as muted unresolved rows. Revoked subjects and subjects already represented by a joined agent row are omitted. These entries carry subject identity, not a fabricated pane identity.

## Still open

- PTY coverage of a managed lifecycle confirmation/action; the existing terminal smoke does not exercise it. Lifecycle status rows are currently informational, not close targets.
- Live Pi/Herdsman acceptance remains operator-owned. Section 3's remaining legacy/replacement refusal cases and integrated restart acceptance are also open.
