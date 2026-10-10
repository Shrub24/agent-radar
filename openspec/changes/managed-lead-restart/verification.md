# Verification

## Status: blocked before restart implementation

The session-aware spawn/topology prerequisite is implemented and verified. The managed lead restart itself remains unavailable: Herdr's documented pane create/input surface does not return a positive shell-ready acknowledgement for a newly created pane. Radar must not infer readiness from delay, output heuristics or marker injection, and must not create a pane only to refuse afterward. No TUI restart action, restart signal, replacement launch, or live-fleet operation was added.

## Landed prerequisite

New spawn requests name both the exact parent registration and `parent_session_id`. Admission requires fresh matching session context on that exact registration; it never searches registrations by session UUID. New v2 spawn edges retain both identities. Existing v1 edges remain readable as process-parented records without inferred session identity. Tests cover missing, stale, null and mismatched context refusals before create, distinct same-session attaches, v1 reads, and preservation of an existing edge when a second attach appears.

Legacy v1 child edges remain process-parented; a session with one is not eligible for restart until an explicit evidence-backed migration is designed. No migration is performed by this change.

## Validation

Independent verification on the working tree:

- `cargo fmt --all -- --check` — passed.
- `cargo clippy --locked --all-targets -- -D warnings` — passed.
- `cargo test --locked` — passed.
- `python3 tests/terminal_smoke.py target/debug/radar` — passed both documented scenarios.
- `python3 tests/agent_publisher.py` — worker-reported pass against the disposable daemon, including the fixture and scripted spawn/child-close checks.
- `openspec validate --all --strict` — passed, 18/18.
- `nix flake check -L` — passed on x86_64-linux; the check omits aarch64-linux and Darwin systems.
- `nix build .#radar --print-out-paths -L` — passed; output `/nix/store/k1gdkk7ar72kbvgcz0hn6zdvjr4cjxm7-agent-radar-0.1.0`.
- `git diff --check` — passed.

No restart-specific or live Herdr restart smoke ran because the required readiness signal is absent. Existing live child-close coverage is not evidence for lead restart.

## Required upstream capability

Herdr must expose a documented bounded result for a newly created nonfocused pane that positively acknowledges its interactive shell is ready to accept one atomic text-plus-Enter launch request. Absence and timeout must be distinguishable from ready. Once this exists, Radar can resume the TUI-only restart implementation using a fresh pane and the approved exact launch capture/quoting rules. Do not substitute timing, output matching, or injected marker input.

## Deliberately absent

There is no pidfd helper left without a production consumer, no public restart protocol/CLI, no Herdsman restart operation, no restart button, and no SIGTERM/launch dispatch implementation. Restart admission, durable outcomes, replacement binding, and child-preservation behavior remain incomplete.