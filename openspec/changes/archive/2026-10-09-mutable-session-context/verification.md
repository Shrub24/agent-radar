# Verification

## Independent final gate — 2026-10-09

Worktree: `/home/saurabhj/Projects/dev/agent-radar`, HEAD `4af0ff6beb6e0d28ddd6e26563dd20f0bf661d70`, 34 changed
working-tree entries (the change's `src/`, `tests/`, `docs/`, `examples/`, `openspec/` and `plan.md` edits).
Tracked-diff digest over `src tests docs examples openspec`: `a731433c2b660c3b334293e1541ac0956cbb0ff39060b3015ced2692687e22a5`.
No implementation, test, documentation or example file was modified while producing this record; only `verification.md`
was added and task 3.1 was marked complete in `tasks.md`.

Primary-session commands and exact outcomes:

```sh
cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked && cargo build --locked
python3 tests/terminal_smoke.py
python3 tests/agent_publisher.py
python3 tests/agent_publisher.py /nix/store/04xd5bf8lm64xk8bzkq0hd4jld9vmhg5-agent-radar-0.1.0/bin/radar
openspec validate --all --strict
nix flake check path:. -L
nix build path:.#radar --print-out-paths -L
```

- Rust gate: exit 0. Formatting clean; strict Clippy with `-D warnings` clean; locked tests passed — 611 tests
  across all suites, 0 failed, including the 341-test library suite, the 60-test control-plane socket suite and the
  context integration suite in `tests/agent_registry.rs`; locked build succeeded.
- Both PTY smokes: exit 0. Scenario one passed source failure/empty/recovery, the stub publisher
  connect/replace/disconnect, details focus and page cycling, a disclosure opened and closed with Space/Enter,
  process-row navigation with `t`, a folded branch on the process table, Escape back to the tree, `q` in filter
  entry, stalled quit with a client connected, and terminal restoration. Scenario two passed a socket another Radar
  owns: the fleet stays observed, the header says `bus off`, and the run quits promptly.
- Publisher smoke over a disposable daemon, run twice: once against the local debug build and once against the
  checked Nix package binary. Both printed PASS for live fixture response schemas on a trusted disposable socket,
  `register/acquire/publish/context/get/list/retire`, session switch and explicit null, credential and launch
  privacy, assignment replay/heartbeat, channel expiry replacement, old-writer fencing and restart freshness, with
  no Herdr invocation. No live publisher or fleet was contacted.
- `openspec validate --all --strict`: 16 items passed, 0 failed: the change validates as `change/mutable-session-context`
  with its 3 delta requirements and 8 scenarios, alongside the 15 standing specs.
- `nix flake check path:. -L`: exit 0, `all checks passed!`. The sandboxed check phase ran the locked test suites
  (the reported run ended `19 passed; 0 failed` for its last binary) and installed
  `/nix/store/04xd5bf8lm64xk8bzkq0hd4jld9vmhg5-agent-radar-0.1.0`. The check omitted the incompatible systems
  `aarch64-darwin`, `aarch64-linux`, `x86_64-darwin`, matching the flake's Linux-only package scope.
- `nix build path:.#radar --print-out-paths -L`: exit 0, exact output path
  `/nix/store/04xd5bf8lm64xk8bzkq0hd4jld9vmhg5-agent-radar-0.1.0`
  (`bin/radar`, sha256 `25f7099daa6b5d03d13e0d364f67c0fc82da9dc2239284272860c80c467e5b9d`), `--help` prints the
  expected usage. The path was copied from the configured remote builder `ssh-ng://dev@home-forge`, the same route
  the earlier Nix-distribution gate used.

## What was verified, and how

- **Storage and fencing (1.1).** `tests/agent_registry.rs` exercises the `{agent_id}.context.json` record through
  temporary state roots: switch without identity/generation change, identical replay with a warning and no lease
  refresh, refusal of conflicting/older sequences and non-incumbent writers, retirement and expiry replacement
  advancing the generation, and explicit failure for malformed, foreign-version, oversized and symlinked records.
- **Endpoint and reads (2.1).** `tests/control_plane.rs` drives the real socket: first publish, session update,
  identical replay, explicit null versus never published, restored/stale reads after a serving-epoch change,
  expected-incumbent replacement, lease expiry and old-writer fencing, plus privacy (no writer handle, serving epoch
  or private launch path/argv). The pre-existing get/list and byte-bounded pagination regressions still pass with
  the additive nullable `context` key, and `ping` still advertises only `agent_registry`.
- **Contract, fixture and example (2.2).** `tests/agent_publisher.py` validates the canonical JSONL exchanges
  against a live disposable daemon and makes the same daemon prove a session switch, an explicit clear, and that no
  writer handle or private launch material leaks into `agent.get`/`agent.list`. The example CLI publishes a UUID with
  `--session` and explicit null without it, and never inspects session files or provider internals.

## Limits — what this does not claim

- **No live publisher.** No Herdsman process, Herdr token, extension or real fleet published context. Adoption by
  pi-extensions on session start/switch/fork is a separate follow-up; this gate only proves the documented contract
  the example implements.
- **No retention or event surface.** Context records accumulate as files; there is no pruning, no subscription,
  no push and no notification path, and daemon-side retention remains a separate later change.
- **No lifecycle behaviour.** Stale or absent context is never process exit, idleness or completion. No launch,
  stop, resume, executor, mux or procfs action is exercised or inferred, and process verification stays independent
  of context freshness.
- **No private material, by construction.** Session context is a canonical UUID or an explicit null; no session
  path, argument, environment or provider error text is representable or readable publicly.
- **Platform scope.** The checked package and the sandboxed check ran on `x86_64-linux`; `aarch64-linux` is
  evaluated but not built, and Darwin systems are outside the flake's package scope.
- **Environment.** The PTY smokes, publisher smoke and Rust gate ran on the host worktree against a locally built
  debug binary and the checked store binary; the Nix check built through the configured remote builder.
