# Herdsman migration brief

## Objective and boundary

Remove Herdsman's direct Herdr dependency using Radar's public daemon contracts. Keep assignment, result, mailbox, readiness and supervision policy in Herdsman. Radar owns physical launch, process control, mux observation and durable operation outcomes.

There is no requirement for Herdsman to become an event-sourced application. Radar's internal lifecycle log and reducers do not replace Herdsman's authority or require a new publisher protocol.

## Availability and authorization

This is a discussion brief, not authorization to migrate, repin or add lifecycle integration. Manual live verification belongs to the user; do not start container/TUI/tmux smoke automation.

Herdsman's tested daemon pin remains `f87348032a808967c50a4e84b7e1c766a64ebfe3`, with tmux 3.7c required at that pin. The Herdsman owner reports that real user verification has exercised delegation, token binding/startup acknowledgment, first result, and close followed by saved-session continuation. Phase 1 is substantially implemented; integration/test debt remains, and no full green gate is claimed.

The locally recorded `main`/`main@origin` pin is `8f9102e68cde1dc87312ff113b6f1c93d65d0c33`. It includes the previously shipped protocol-v2, facts channels, registry-backed dashboard and tmux compatibility work. This is a reference pin, not a claim that your daemon is already running it. Record your actual tested pin and binary deployment separately; `daemon.ping` identifies contracts/backend, not the build commit.

The daemon-managed lifecycle and typed event foundation are **uncommitted and unshipped**. Do not repin to an anonymous working-copy revision or call new lifecycle methods against the shipped pin. Keep the existing tested pin. Any later lifecycle adoption needs explicit authorization and a separately published, documented lifecycle pin.

Read `docs/daemon.md`, `docs/control-plane.md`, `docs/agent-registration.md` and `docs/mux.md` for the shipped contracts. For the pending lifecycle work, read this change's `design.md`, `tasks.md` and `event-foundation-verification.md`; these are not a deployment release.

## Phase 1 — existing contract audit, not a new migration

The following describes the existing integration contract. Reconcile remaining debt against it; do not restart completed work or initiate a new migration.

1. Use protocol 2 and `daemon.ping`; negotiate the advertised contracts. Use `spawn.create`, `spawn.get/list`, `spawn.close`, `agent.*`, `events.read` and namespaced `mux.*` methods. Do not retain old aliases or fall back to Herdr.
2. Launch workers through `spawn.create`, transporting literal argv, cwd and environment overlay. Name the exact parent registration and its freshly matching Pi session UUID. Keep private token bootstrap; persist and read back the exact bound child `agent_id`. Registration is not application readiness: wait for the child-authored readiness acknowledgment before assignment.
3. Use registry facts for agents and `mux.observe` for panes. Adopt the existing execution/assignment/self-facts channels where supported. Keep their provenance and freshness separate. Missing facts stay unknown; do not infer readiness, assignment completion or process exit from UI activity.
4. Replace the Herdr lifecycle subscription with `events.read`. Process each page before saving its last-delivered cursor (`result.cursor` on Radar's wire; Herdsman's adapter exposes the delivered position as `reply.sequence`). Never advance to `tail`. Preserve epoch information and immediately drain subsequent pages while `more` is true; resynchronize on cursor expiry/epoch change and retain periodic reconciliation. Events remain public invalidation pointers even though lifecycle events now have an internal replay authority. Never make cursor sequence arithmetic or infer state from an event kind alone.
5. Preserve uncertain launch/close reservations and replay the same mutation request ID. `outcome_unknown` means an effect may have happened; it is not permission for a new spawn or direct retry. `spawn.close` is guarded pane cleanup, not verified process termination.
6. Distinguish retained reuse from relaunch. A proven idle retained worker with a matching launch fingerprint can accept a new assignment in the same process/pane; `agent_continue` does not inherently relaunch it. Relaunch of a stopped/lost generation uses a new pane rather than preserving the old one. Herdsman reports this relaunch path already implemented: `agent.get` absent/mismatched process verification permits relaunch; verified/unavailable/not_found refuses it. Neither pane disappearance nor `spawn.close` is process-exit authority. Lifecycle stop would extend the supported cases rather than create this existing continuation path.

Complete removal of direct Herdr reads, environment assumptions, adapters, subscription handling, packaging and smoke dependencies. Attribute failing tests against the actual baseline rather than declaring them obsolete.

## Phase 2 — after Radar publishes the lifecycle contract

The pending implementation exposes subject references and `lifecycle.get/list`, `lifecycle.cancel` and `lifecycle.stop`. Consume the release's final documentation and canonical fixtures, not names guessed from this brief.

- Persist the returned subject reference alongside the spawn request and exact bound registration. The pending public spawn edge adds optional `subject_id` (`src/control_plane/registry/spawn.rs:230–247`). Missing references mean explicitly untracked/legacy records; never manufacture ownership from a pane or Pi session UUID.
- Cancel a launch that never binds through its managed lifecycle reference. This does not require a child `agent_id`. Durable revocation competes with binding under one commit fence; late token use cannot resurrect a revoked launch.
- For supported retained-worker relaunch, stop the exact subject and require verified old-process exit before a fresh saved-session spawn. Stop is pidfd-bound SIGTERM only, with no SIGKILL/PID-only fallback. Timeout/unavailable evidence stays unresolved and retains the reservation.
- Read durable operation outcomes through the documented request-read contract. Keep revocation, signal attempt, verified exit and pane cleanup distinct. A revoked token does not prove termination; a closed pane does not prove process exit. Unbound pane cleanup must not be improvised through a generic close that bypasses ownership checks.
- Continue publishing execution and assignment facts through the existing public methods. Treat new lifecycle event references as optional identifiers; do not consume Radar's private source-log files or request/recipe storage.

Do not integrate lead enrollment/restart into Herdsman in this phase. It remains operator-oriented and has outstanding Radar acceptance. Manager placement, generic background-job supervision, foreign-agent compatibility and OSC capture are also outside scope.

## Radar-side dependencies still open

- Publish a documented, capability-gated lifecycle release before phase 2; finish consumer docs/fixtures and the remaining release gates.
- Finish restart/generation/recovery acceptance and daemon-backed TUI controls. These do not block phase 1.
- Migrate reported facts and broader pane/session observations to internal reducers. Preserve public publisher semantics; this is Radar work, not a Herdsman prerequisite.
- Define sanctioned cleanup of an unbound launch's own pane after cancellation/revocation. Fence ownership against the creating tmux-server incarnation and exact launch-owned pane; define refusal for replacement, missing or ambiguous evidence, and the conditions for dead-pane cleanup. Persist cleanup outcomes separately from token revocation and verified process exit. Same-ID replay after `outcome_unknown` must not dispatch cleanup again. Whether cleanup belongs to cancel or a separate semantic operation is still a Radar design decision; the tested `spawn.close` refuses unbound edges.

## Acceptance and return artifact

The user owns manual live verification. Record the already reported delegation/binding/readiness/result and close/continue evidence without rerunning it through automated container/TUI/tmux harnesses. Any further live cases require the user's direction. Rust fixture success is not integrated Pi acceptance, and focused green tests do not erase the reported broader integration failures.

For existing authorized integration work, record changed paths, exact tested Radar pin/binary, test results with failure attribution, user-provided live evidence, remaining direct Herdr calls and concrete missing daemon contracts. Do not initiate migration, repin or broaden lifecycle scope from this brief.

Reporter status, as reported by the Herdsman owner: activity, waiting reason and freshness publication exists; permission/questionnaire bridges are implemented but not fully live-certified. Auth/login has no wait producer, typed wait-kind is absent, and some provider-error last-outcome coverage remains incomplete. These do not require access to Radar's internal event log.
