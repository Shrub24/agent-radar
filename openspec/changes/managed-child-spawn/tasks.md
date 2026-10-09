# Tasks

## 1. Spawn edges and token binding

- [ ] 1.1 Record one durable spawn edge per spawn — parent subject, request id, minted token, created location, and the separate `created`/`launched`/`bound` states — under the existing private atomic-record discipline, with a spent token that cannot bind twice; verify with temporary state roots that a replayed request id performs no second effect, a spent or malformed token is refused without changing the existing edge, and a partially written record fails explicitly rather than reading as an empty edge.
- [ ] 1.2 Bind a pending edge to the exact `(source, incarnation)` that registers with its token, leaving the token out of every public projection, and verify that registering with the token binds once, a second registration with the same token is refused, and no pane title, alias, label or session UUID can bind an edge.

## 2. Backend launch

- [ ] 2.1 Add the `launch` backend capability and its refusal path, so a backend that does not declare it refuses a spawn before dispatch with nothing created; verify the refusal, the unchanged capability advertisement, and that no other operation gains launch.

- [ ] 2.2 Implement Herdr launch: execute the resolved child command in the created pane under the quoting rules pi-extensions ADR 0029 measured (single-quoted argv on one line; a newline carried in a private launch script), confirming only what the backend actually reports; verify against a real disposable Herdr session that argv reaches the pane byte-exact, an unconfirmed dispatch stays `unknown`, and input is not used for stop, resume or restart.

## 3. The spawn operation and reads

- [ ] 3.1 Serve `spawn`: validate the named parent and the launch specification at the wire boundary, create under that parent, launch, report created/launched/bound separately, and never claim a later step when an earlier one is uncertain. Record no assignment fact and touch no process lifecycle; verify refusals leave no pane, a launch failure leaves the created pane named, and a duplicate request id replays the recorded outcome.
- [ ] 3.2 Serve the topology read for spawn edges with parent, bound child, state, location and freshness, retained across a daemon restart and re-verified on read; verify that a location missing from the backend reads unresolved rather than stopped, stale facts stay marked, and an agent spawned outside the daemon has no edge and is not an error.
- [ ] 3.3 Document the spawn operation, the token, the outcomes and the reads in `docs/daemon.md` and `docs/agent-registration.md`, extend the canonical fixture and the publisher example, and state the Herdsman adoption boundary: route spawning through `spawn` only when `launch` and `spawn` are advertised, keeping today's path otherwise.

## 4. Independent gate

- [ ] 4.1 Independently verify fmt, strict Clippy, locked tests/build, both PTY smokes, the publisher and spawn examples against a disposable daemon, a real disposable Herdr launch, the checked Nix package and strict OpenSpec validation; record provenance in `verification.md` and state plainly that stop, resume, restart, recovery and adoption of foreign panes are not claimed.

## Workflow follow-up

- Coordinate the token carrier with pi-extensions before Herdsman adopts it: the token must reach the child exactly once, alongside the existing child-command plumbing.
- Keep lead/child recovery, restart policy and the lifecycle executor as later changes; this change creates and records children, it does not manage their lives.
