# Tasks

## 1. Spawn edges and token binding

- [x] 1.1 Record one durable spawn edge per spawn — parent subject, request id, minted token, created location, and the separate `created`/`launched`/`bound` states — under the existing private atomic-record discipline, with a spent token that cannot bind twice; verify with temporary state roots that a replayed request id performs no second effect, a spent or malformed token is refused without changing the existing edge, and a partially written record fails explicitly rather than reading as an empty edge.
- [x] 1.2 Bind a pending edge to the exact `(source, incarnation)` that registers with its token, leaving the token out of every public projection, and verify that registering with the token binds once, a second registration with the same token is refused, and no pane title, alias, label or session UUID can bind an edge.

## 2. Backend launch

- [x] 2.1 Add the reusable `launch` capability-readiness check and verify a backend without `launch` is refused before effect dispatch without changing capability advertisement; the wire-level `spawn` refusal and no-create proof are verified with task 3.1, when the spawn method exists. The reusable readiness check is delivered in slice 2.1; the wire-level `spawn` refusal is completed with task 3.1, when `spawn` exists as a complete operation.

- [x] 2.2 Implement Herdr launch: type the resolved child command into the created pane as one line under the quoting rules pi-extensions ADR 0029 measured (every argv element single-quoted, the spawn token exported to the child on the same line), confirming only what the backend actually reports; an argv holding a newline is refused before any pane input with a typed refusal, because ADR 0029's private-script fallback is not implemented — it needs an adapter-owned bounded cleanup/ack the daemon cannot yet observe, and a self-deleting wrapper could remove a script the pane shell has not read. Verify against a real disposable Herdr session that argv reaches the pane byte-exact, an unconfirmed dispatch stays `unknown`, and input is not used for stop, resume or restart. Newline-bearing argv is the one measured shape this slice does not support; it is tracked as a follow-up rather than approximated.

## 3. The spawn operation and reads

- [x] 3.1 Serve `spawn`: validate the named parent and the launch specification at the wire boundary, create under that parent, launch, report created/launched/bound separately, and never claim a later step when an earlier one is uncertain. Record no assignment fact and touch no process lifecycle; verify refusals leave no pane, a launch failure leaves the created pane named, and a duplicate request id replays the recorded outcome. The caller resolves the command and names a registered parent whose own registered pane is the create target; the daemon adds only the created pane and the token it minted, records the intent before the first effect and each effect as it returns, and answers with the request record beside the edge's public projection, so `bound` reads absent until a child registers with that token. The spawn read (3.2) and the documentation, fixture and example extension (3.3) remain.
- [x] 3.2 Serve the topology read for spawn edges with parent, bound child, state, location and freshness, retained across a daemon restart and re-verified on read; verify that a location missing from the backend reads unresolved rather than stopped, stale facts stay marked, and an agent spawned outside the daemon has no edge and is not an error.
- [x] 3.3 Document the spawn operation, the token, the outcomes and the reads in `docs/daemon.md` and `docs/agent-registration.md`, extend the canonical fixture and the publisher example, and state the Herdsman adoption boundary: route spawning through `spawn` only when `launch` and `spawn` are advertised, keeping today's path otherwise.

## 4. Independent gate

- [x] 4.1 Independently verified fmt, strict Clippy, locked tests/build (full suite passed; the existing stale-socket fork race also reproduced once and is recorded as a non-feature harness flake), both PTY smokes, publisher plus scripted spawn against a disposable daemon, a real disposable Herdr launch, checked Nix package/check, and strict OpenSpec validation. See `verification.md` for exact provenance and scope limits.

## Workflow follow-up

- Coordinate the token carrier with pi-extensions before Herdsman adopts it: the token must reach the child exactly once, alongside the existing child-command plumbing.
- Design the adapter-owned bounded cleanup/ack protocol that would let a newline-bearing argv launch from a private script (ADR 0029's fallback). Until it exists, `launch` refuses such a command rather than writing unbounded residue.
- Keep lead/child recovery, restart policy and the lifecycle executor as later changes; this change creates and records children, it does not manage their lives.
