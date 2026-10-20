# Verification

All implementation tasks accepted after owner review and independent gates.

- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings` passed.
- `cargo test --locked` passed: 722 tests. Coverage includes v1/removed-method refusal without effects, delivered event cursor pagination, truthful backend identity, guarded live close, retained dead-pane cleanup/replay, reused tmux pane IDs after server restart, bounded creation-time verification and unknown-close client mapping.
- `cargo build --locked` passed.
- `python3 tests/terminal_smoke.py target/debug/radar` passed both controlled PTY scenarios.
- `python3 tests/agent_publisher.py target/debug/radar` passed full-response registry/event fixtures, the reconnecting event consumer and scripted-Herdr spawn/binding/live-close replay.
- Disposable named tmux integration tests passed within the Rust suite; no live user tmux server or Herdr workspace was used.
- `openspec validate --all --strict` passed 21/21.
- `nix flake check -L` and `nix build .#radar --print-out-paths -L` passed on x86_64-linux. Output: `/nix/store/3rvs4j7xa8g5xj7m36yia1m4smhfl1q3-agent-radar-0.1.0`.

## Boundaries

Protocol version 2 is a clean break without method aliases. Consumers must migrate before using this build. Capability negotiation does not prove a build pin.

Dead-pane cleanup is a physical pane fact with historical pane-command exit evidence, not proof of registered process exit or permission to relaunch. It requires the verified creating server incarnation; old/unfenced edges refuse. Verify and close are separate instants, so same-UID mutation between them remains a documented residual risk.

The package check runs Rust tests, not Python smokes; those were run separately above. The control-plane JSONL fixture remains a shape reference; the agent-registration fixture is compared in full against a disposable daemon. No stop/resume/restart or dashboard registry consumption was added.
