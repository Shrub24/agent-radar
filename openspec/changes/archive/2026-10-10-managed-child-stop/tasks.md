# Tasks

## 1. Durable request and admission

- [x] 1.1 Add a private, bounded, atomic, idempotent `child.close` request record referencing a spawn edge, exact bound child identity, recorded pane and intent; verify persistence, identical replay/no replay, conflicting request ids, malformed records and token exclusion using temporary roots.
- [x] 1.2 Add strict `child.close` request decoding and explicit not-ready refusal before record admission or backend dispatch while fresh pane/containment verification is unavailable; verify no record is written and no capability is advertised. Valid request admission, replay and target refusals remain with task 2.1, where fresh verification is implemented.

## 2. Backend close and reconciliation

- [x] 2.1 Revalidate the recorded pane and containment against fresh backend inventory and compare the foreground PID's procfs birth identity (PID + boot_id + start_ticks) with the bound child's registered claim; refuse before record/dispatch on any mismatch or missing evidence, then dispatch the existing pane-close primitive only for an exact verified child. Keep managed-close capability unadvertised until outcome persistence lands.
- [x] 2.2 Persist completed/refused/unknown outcomes without retry; preserve spawn edge and registration, report pane closure only (not process exit), and verify lost replies remain unknown and subsequent reads do not infer process exit.

## 3. Consumer contract and verification

- [x] 3.1 Document `child.close`, explicit intent semantics, target/capability requirements, durable outcomes and no-fallback behavior in `docs/daemon.md` and the relevant registration reference; extend canonical fixture and scripted disposable-daemon tests, asserting assignment/execution publications remain unchanged.
- [x] 3.2a Add a live disposable-Herdr close smoke, gated like the existing launch smoke, that creates a pane, registers the bound child with its real process identity, closes it once, and asserts the pane is gone with a durable `completed` outcome and no redispatch on replay.
- [x] 3.2 Independently run fmt, strict Clippy, locked tests/build, both PTY smokes, publisher/scripted daemon tests, the live disposable-Herdr close smoke, checked Nix build/check and strict OpenSpec validation; record exact provenance and limitations.

## Workflow follow-up

- After verification, sync and archive the change, reconcile `plan.md`, commit and push; then coordinate with pi-extensions before Herdsman adopts `child.close` for daemon-created children. The operation is method-level readiness over `observe` + `close`, not a separately advertised capability.
