# Design

## Context

See proposal.md. At `8f9102e6`, `execute_spawn` persists intent, creates a pane, dispatches launch and accepts token-bound registration. `TmuxRuntime::launch` returns dispatch certainty but no process birth binding; it uses direct argv with `remain-on-exit`. `spawn.close` is pane cleanup, not verified process termination. Linux birth identity and a bounded verifier already exist.

`src/control.rs` is Radar's `herdsman-control/v1` file requester. The app owns request publication/outcome polling and requires legacy owner lineage. Registry-only rows are protected but cannot use that route. Existing registry projection and assignment/execution/facts channels must not be replaced.

The older `managed-lead-restart` draft equates session parentage with logical identity and blocks on Herdr shell readiness. Its exact parent-registration/session admission slice shipped; its execution plan did not. This change replaces that unimplemented plan, not the accepted spawn evidence.

## Goals / Non-Goals

**Goals:** one daemon-owned module whose production consumers are launch cancellation, exact stop and operator restart; a compositional lifecycle projection; accepted operations that outlive clients; bounded observation/effects with durable recovery.

**Non-Goals:** full dashboard collector relocation, generic process/job supervision, reparenting arbitrary processes, descendant killing, automatic restart, new layouts, assignment policy or a universal workflow engine. Typed event-led persistence applies first to managed lifecycle; migration of all publisher channels is a later domain slice. Initial process execution is tmux/Linux. Unsupported new controls remain unavailable on Herdr.

## Decisions

### 1. Subject identity is not a session or registration alias

A daemon-generated `subject_id` is the logical managed agent reference. A subject records a monotonic binding generation, optional current incarnation `agent_id`, verified process birth claim, Pi-session association and generation-qualified pane reference. Historical generations and spawn/operation references remain readable. Source observations and desired effects are distinct fields, not one writable lifecycle enum.

Pi session is the conversation identity, not a tmux session. A process can switch Pi sessions; several attaches can share one. Session updates do not overwrite an enrolled resume recipe. A caller must explicitly refresh that recipe for a changed session before restart becomes eligible again.

New tracked spawns reference a subject without rewriting their original edge. Replacement creates a new launch-generation record and registration, not a second binding of the old spawn token. Child relationships use stable subject references where daemon provenance establishes them, while original process/session parent references remain audit evidence. Existing v2 edges may be associated through an explicit enrollment/reconciliation of their exact parent registration and session pair. Never match by UUID search. Legacy v1 edges remain unchanged and block parent restart until separately reconciled.

**Rejected:** redefining registration `agent_id`, treating a session UUID as unique, or rewriting old spawn history. All lose incarnation fencing or hide ambiguous attaches.

### 2. Capture the process before exec, not after observing a pane

A tmux command acknowledgement cannot prove which process later occupies its pane. Reading `pane_pid` after dispatch leaves a respawn/fast-exit race. Use a small bounded internal exec bootstrap supplied by the Radar binary, not a permanent supervisor:

1. Persist launch intent, subject generation, private bootstrap correlation and requested launch context before tmux effects.
2. Tmux starts the bootstrap with direct argv/cwd/environment. It contacts the daemon with its launch correlation and process claim, and waits boundedly before exec.
3. The daemon validates the exact launch correlation, actual Linux birth identity and creating server/pane evidence, obtains and verifies a pidfd, and durably records the process binding before acknowledging permission to exec.
4. The bootstrap execs the requested program without a shell; PID/birth identity remains the same. Requested argv/cwd/environment semantics and `PI_RADAR_SPAWN_TOKEN` remain unchanged.

The launch-bootstrap correlation is distinct from the registration token. Registration must later match the captured birth identity. This gate is physical launch acknowledgement, not Pi application readiness. A cancellation can revoke exec permission before release or signal the captured process afterward. Bootstrap failure/timeout exits without executing the target; a lost acknowledgement remains recoverable through exact process evidence, never a guessed redispatch.

Effective environment and cwd can be captured privately at this verified pre-exec boundary for daemon-created subjects. Bootstrap fields and messages are bounded; command/environment diagnostics are redacted. This avoids inferring ownership from terminal text or relying on timing. The helper is not a generic job runner and does not make the daemon the OS parent.

**Rejected:** registration-only capture (cannot cancel unbound launches), prompt/quiet-time heuristics, and pane-PID sampling as launch provenance. If the backend cannot execute this bounded bootstrap faithfully, it cannot advertise tracked lifecycle support.

### 3. Hooks wake reconciliation; pidfds establish process evidence

One bounded daemon monitor owns pidfds and polls exit readiness. Signals use the verified descriptor, never a saved PID alone. Pidfd exit observation proves that process ended, not its exit code or descendants' termination. Tmux pane-command exit records retain their existing narrower meaning.

Generation-fenced tmux hooks notify the daemon of physical changes using IDs only. Install owned hook entries without overwriting user hooks, and remove only those owned entries. Queue overflow, unavailable notifications, reconnect or server-generation changes schedule bounded physical reconciliation. Hooks are an optimization for timeliness, not the sole correctness source; periodic recovery checks remain. Never parse scrollback/OSC as lifecycle authority or install a new general notification broker.

Persist birth claims and provenance, not descriptors. On daemon restart, reopen a pidfd only after proving the durable tuple still identifies the process. Boot/server changes and unavailable reads are classified explicitly. Active operations are reconciled; possible effects are never automatically repeated.

**Rejected:** detached verification threads per read/request, mandatory control-mode overhaul, and treating pane death or a missed heartbeat as agent exit.

### 4. Cancellation, stop and restart share operation machinery

Use one per-subject/generation mutation lane and the existing bounded effect/admission approach. Persist exact request intent before effects. Cancellation revocation and bootstrap/registration binding must linearize at one authoritative durable subject/correlation transition; independent files without a shared commit fence are insufficient. The authoritative lifecycle event commit is that fence. Legacy public pointer delivery is derived from committed lifecycle events and is not a second authority.

- **Cancel:** revoke pending bootstrap/registration correlations; if a process is captured, run common stop; optionally clean the exact recorded pane after its separate gate. A bound cancellation targets the captured generation rather than erasing registration history.
- **Stop:** validate the frozen subject/process generation, request pidfd SIGTERM and wait boundedly. No SIGKILL or process-group escalation. Report signal dispatch and verified exit separately. An ignored signal does not permit a replacement.
- **Restart:** freeze the current session-specific recipe and relationships; use common stop; only after verified exit launch one replacement into a fresh nonfocused pane through the same bootstrap. Mint new correlations. Commit the new current binding only after registration matches the captured process and fresh expected session context. Preserve children, their processes and immutable edges. No assignment/execution facts are rewritten.

Store effect stages before possible dispatch. Same-ID reads/replays reconcile existing outcomes; different IDs cannot bypass a pending/unknown mutation on that generation. Positive later evidence may refine an unknown result without claiming an unobserved signal delivery. If replacement dispatch is uncertain, do not create another replacement even after a local deadline. Preserve each partial effect rather than claiming whole-operation success.

A missing placement anchor, unknown old-process identity or incomplete recipe refuses before restart signals. Retain the old generation-qualified pane as the default fresh-pane anchor; no flexible-placement expansion is needed. Pane preservation after success is optional cleanup, not agent continuity.

### 5. Restartability requires explicit managed provenance

Daemon-spawned processes have launch provenance but still need an explicit Pi-session resume recipe to restart. For a registered root/lead, an operator enrollment can verify the exact process, current session and pane, capture bounded executable/argv/cwd/complete environment, and attach an explicitly supplied/stored session resume specification. Do not infer a session file or resume command from UUID, title or argv. Publisher-supplied launch data is input, not enrollment authority. No arbitrary unregistered process adoption or fabricated spawn edge.

Capture and recipes use private 0600 records in the trusted state directory. Public lifecycle/spawn/operation reads omit argv, environment, session paths and correlations. Captured old Radar bootstrap/registration tokens are removed before replacement. An environment overlay alone is not a complete restart environment. A changed session or launch-spec revision invalidates the previously frozen eligibility.

### 6. Thin TUI; additive daemon contract

Proposed protocol-2 namespace:

| Method | Role |
| --- | --- |
| `lifecycle.subject.get` / `lifecycle.subject.list` | Bounded current projection/history references; locate by exact subject or registration reference, never session search |
| `lifecycle.cancel` | Exact managed subject/generation, or originating spawn ID resolving to that subject |
| `lifecycle.stop` | Exact managed subject/generation |
| `lifecycle.operation.get` | Durable operation/effect readback by request ID |
| `lifecycle.enroll` / `lifecycle.restart` | Operator-facing requests initiated by explicit TUI confirmation |

Negotiate `lifecycle.observe.v1`, `lifecycle.cancel.v1`, `lifecycle.stop.v1` and `lifecycle.restart.v1` independently when their complete paths exist. Strict bounded parameters carry frozen target generation and immutable request identity. Unsupported capability is undispatched refusal; possible dispatch is `outcome_unknown`. No protocol reset, old aliases or new public restart CLI. These operator wire methods are reachable within the existing same-UID trust boundary: TUI-only describes the supported caller/UX, not an authenticated privilege boundary.

The app opens confirmation, freezes references and submits a request. It does not own pidfds, hook subscriptions, enrollment capture, stop timers, restart state, duplicate suppression or owner-file polling. New reads expose lifecycle evidence to the existing dashboard projection without moving all fleet collection in this change. Remove Radar's owner-file requester once all managed UI routes have cut over; do not move that requester into the daemon as a hidden fallback. Direct mode retains supported unmanaged mux operations but refuses daemon-managed lifecycle when the daemon is unavailable.

**Rejected:** a TUI-local supervisor or daemon proxy to Herdsman control files. Both retain multiple physical lifecycle authorities. Herdsman's mailbox, assignment eligibility/result logic and existing consumer cutover remain independent.

### 7. Typed event log, domain reducers and committed-event delivery

Managed lifecycle uses one daemon-scoped durable event log, not one bus per lead, Pi session or workspace. These identities are event references and subscription filters. Each envelope has a version, sequence, event identity, recorded/observed time, typed source, subject/generation references and operation/causation references where established. Payloads distinguish signals, verified observations, admitted intent and effects; a signal is never promoted to a verified observation by replay.

Deterministic reducers build subject/binding/operation projections and perform no I/O. Private launch recipes and one-use credentials stay in bounded 0600 storage referenced by the projection, never in public events. Existing lifecycle read shapes and `events.read` remain supported projections; the bounded public pointer journal is not the replay authority. Pre-existing durable subject records enter through explicit imported-baseline events, never guessed provenance.

Commits are globally FIFO and serialized; conflicting commands serialize per logical subject while independent external effects use the existing bounded worker lanes. No transition queue uses LIFO. Reconciliation hints may coalesce by target before collecting fresh evidence; committed events never coalesce. Reconciliation emits observations, including unavailable/conflicting evidence, rather than overwriting unrelated source authority. Process, pane, Pi execution and owner assignment remain separate facets.

Effect ordering is commit intent, dispatch bounded effect, commit observed result. Reducer replay cannot dispatch effects. A crash or lost result between these stages remains unresolved until new authoritative evidence arrives; neither replay nor a second request may repeat an uncertain effect. Append failure refuses undispatched controls. Admission and reconciliation are bounded, slow consumers use independent cursors and cannot block effects, and result recording for admitted work must not compete with optional observation hints.

The first production slice moves lifecycle creation/enrollment, capture, binding, revocation, signal intent, verified exit and successor transitions onto the log and reconstructs their reads through replay. Operation admission/results and public subscriber delivery follow through the same boundary. Registry publications retain their existing publisher authority and wire contract until explicitly migrated; messaging and background-task execution are not implemented here. Periodic external reconciliation remains necessary: replay reconstructs recorded knowledge, not current physical reality.

## Risks / Trade-offs

- [Pre-exec bootstrap adds a launch dependency] → Ship it in Radar, bound its handshake and demonstrate literal launch fidelity and fast-exit capture before advertising lifecycle support.
- [Pidfd unsupported or identity acquisition fails] → Refuse effects; never weaken to PID-only signaling.
- [Process forks descendants] → Stop promises only the captured process; descendant cleanup is explicitly not implied.
- [Hooks lost or replaced by another same-UID client] → Preserve user hooks, fence instance/generation and reconcile independently.
- [Daemon crashes between durable intent and dispatch] → Reconcile exact evidence; retain unknown effects instead of reissuing commands.
- [Saved session has another live attach] → Refuse ambiguous replacement; never choose an attach by session UUID.
- [Root enrollment cannot verify pane/process correlation] → Refuse enrollment; do not broaden adoption to make the button work.

### 8. Restart recipe, anchor and successor (decided for section 3)

- **Enrollment** records an `enrolled` subject for one registration whose exact process is verified on the lead's own recorded pane, with a live fresh session context equal to the operator-named session UUID. Its spawn reference is the enrollment request ID, not a spawn edge. It captures executable, argv, cwd and the complete environment privately; the replacement's spawn token and bootstrap correlation variables are excluded. Enrollment refuses a registration that another tracked subject already binds.
- **Resume argv** is rebuilt, not stored: the captured argv with prior session selectors removed (`--session`, `--session-id`, `--fork`, `--continue`/`-c`, `--resume`/`-r`, `--no-session`, and their `=` forms), then `--session <live UUID>` appended. Restart refuses any other bare argument that is not the value of a known value-taking flag, because it could be a positional message replayed on every restart. This allowlist is fail-closed: an unknown valued flag refuses rather than guessing.
- **Anchor** is the lead's own recorded pane. tmux splits a new pane from it, and `remain-on-exit` keeps a dead pane addressable. Restart verifies the anchor exists before sending any signal; an absent anchor refuses with no effect. It does not borrow a live parent.
- **Session check** compares the registration's published session UUID with the recipe's. It is not freshness-gated, because the lead is usually dead by the time a restart runs. A changed session refuses and requires re-enrollment.
- **Successor** is written to the old subject before the replacement is dispatched. A subject with a successor refuses further restart; a replay of the same restart request ID returns the stored outcome. An unverified old exit refuses before any successor is written.
- **Replacement** is a normal spawn-shaped launch: a new edge whose parent is the old incarnation and whose session is the same UUID, a new token, and a bootstrap-captured subject that carries the session. Children keep their immutable edges, and their session-parent relationship survives because the UUID is unchanged. Legacy v1 children whose parent is the old incarnation refuse the restart.
- **Failed replacement** is reported as the restart's own partial outcome (old stopped, replacement refused or unknown). It is not rolled back, and it is not retried automatically.

## Migration Plan

1. Ship the lifecycle record/projection with the first real bootstrap-and-cancellation consumer; keep existing protocol-2 spawn behavior readable and negotiate support.
2. Verify unbound cancellation and common stop on disposable tmux processes; add daemon-restart and binding-race cases.
3. Add explicit enrollment, private recipe handling and restart through that same module; verify unchanged live children and no second replacement after lost replies.
4. Cut managed TUI routes to daemon requests, delete owner-file requester/timers and test operations continuing across TUI exit.
5. Publish consumer fixtures and real disposable Pi/Herdsman evidence separately from Rust/Nix gates. Do not move Herdsman's fixed pin during its existing acceptance.

Rollback hides/refuses new controls without deleting records, tokens' revoked state or subject history. Stop new admissions, drain bounded effects and retain readable outcomes. Already running tmux processes are not killed when presentation or daemon shuts down.

## Planning Supersession

`daemon-managed-lifecycle` is the execution plan for stop/cancel/restart. The old `managed-lead-restart` proposal is historical: keep its checked session-parent tasks and verification, mark unimplemented execution as superseded, and do not apply its Herdr-readiness or session-as-subject design in parallel. Existing main specs are changed only through this change's deltas when it is implemented and synced.
