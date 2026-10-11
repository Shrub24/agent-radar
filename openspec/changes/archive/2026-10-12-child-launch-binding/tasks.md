# Tasks

## 1. Launch context through the existing spawn seam

- [x] 1.1 Add optional cwd/environment overlay to spawn and runtime launch requests, with aggregate bounds, reserved-token rejection and side-effect-free selected-backend validation before admission/create. Verify omitted-field compatibility and malformed/unsupported context refusals before any recorded effect; update protocol field documentation.
- [x] 1.2 Implement Herdr cwd/environment transport with the existing single quoted line and guarded directory change. Verify exact cwd, literal/empty/overriding environment and argv, and no command injection through adapter tests; document one-line limits and the absence of a shell-ready guarantee.
- [x] 1.3 Advertise child_launch_context_v1 only for supporting handler/backend combinations and separate private exact request equality from public projections/errors. Verify secret sentinels never occur in responses, request.get/list, topology or failure diagnostics, while changed same-ID cwd/env refuses without redispatch; update capability/reference fixtures.

## 2. Exact child registration and bootstrap contract

- [x] 2.1 Persist the exact agent_id returned by successful token-consuming registration on new spawn bindings, preserving it across identical registration replay and interrupted acknowledgement. Keep agent_id optional for legacy bound records, with no reconstruction from names or sessions. Verify new binding readback through spawn.get/list, replay and legacy fixtures without changing the token confidentiality contract.
- [x] 2.2 Extend the publisher example/scripted smoke to receive launch context, capture/remove the token, register its exact process identity and publish context/execution. Verify the parent resolves the returned bound agent_id through spawn.get and agent.get without aliases, including bound-but-unpublished readiness unknown; fully compare canonical exchanges against a disposable daemon. Pi-extensions deployment remains external; the example and smoke are implemented here.

## 3. Independent integration verification

- [x] 3.1 Run a gated disposable-Herdr spawn/bind/close check through the public daemon spawn path with cwd and shell-sensitive environment values, cleaning up only the created pane. Verify the child sees exact inputs, binds its own agent_id, token values remain private, and the existing child.close/replay still works; record that this tests launch transport, not general shell/application readiness.
- [x] 3.2 Run formatting, strict Clippy, full locked Rust tests/build, both PTY smokes, disposable publisher/fixture smoke, strict OpenSpec validation and checked Nix package/flake gates. Record exact results and skipped live checks in verification.md, preserve the blocked restart change, and prepare the Pi-extensions adoption handoff.
