# Tasks

## 1. Context record and fencing

- [x] 1.1 Implement the atomic `{agent_id}.context.json` record with strict bounded codec, canonical-UUID-or-null session, writer binding, forward-sequence fencing, bounded lease, retirement and explicit generation replacement, sharing the daemon's freshness rule with the channel records; verify with temporary state roots that a switch republishes without changing the agent id or generation, identical replay is idempotent without lease refresh, older/conflicting sequences and a non-incumbent writer are refused, retirement/expiry replacement advances the generation, and a corrupted, symlinked or foreign-version record fails explicitly.

## 2. Endpoint, reads and contract

- [x] 2.1 Serve `agent.context` and add the `"context"` key to `agent.get` and every `agent.list` entry, distinguishing fresh, stale/restored and absent without exposing any private material or writer handle; keep existing methods, pagination and capability advertisement unchanged and verify over a real disposable socket that old read shapes still parse, byte-bounded pagination is unaffected, and an unsupported daemon refuses the method explicitly.
- [x] 2.2 Extend `docs/agent-registration.md`, the canonical fixture exchanges and the reconnecting publisher example for context publication on session switch/fork, including null-versus-absent semantics and the acceptable (but discouraged) long-lease path; run the example against a disposable daemon with no Herdr calls and record the pi-extensions adoption boundary.

## 3. Independent gate

- [x] 3.1 Independently verify fmt, strict Clippy, locked tests/build, both PTY smokes, the publisher example against a disposable daemon, the checked Nix package and strict OpenSpec validation; record provenance in `verification.md` and state plainly that no live publisher, retention, event surface or physical lifecycle behavior is claimed.

## Workflow follow-up

- Pin the landed commit and hand the exact method, fixture and reference to pi-extensions for Herdsman adoption on session_start/switch/fork under the same writer handling.
- Keep daemon-side retention of accumulated records and the push/event surface as separate later changes; do not infer process state from context freshness.
