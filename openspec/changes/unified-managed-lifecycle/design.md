# Design

## Context

See proposal.md for motivation. Relevant current seams, inspected in the daemon-contracts worktree:

| Seam | Current behaviour / convergence problem |
|---|---|
| `tree.rs::RowId`, `ui_session/targets.rs::selection` | Agent rows are pane-keyed, not claim-keyed; joined registry detail supplies `ManagedSubject { subject_id, claim_id, ... }`. A row ID alone is not process authority. |
| `targets.rs::launch_plan` | `L` constructs a target-shell recipe and `DomainAction::Create`; it does not itself reserve a managed claim. This is a larger admission gap than an alternative spelling of `radar run`. |
| `server/launch.rs::admit_request`, `lifecycle.rs::create_process` | Direct admission verifies a pane/process association, records an admitted process, but discards placement from that verification. |
| `registry/spawn.rs::SpawnEdge` | Holds producer parentage, create/launch/bind progress, binding credential, historical location and exit status. It must not become the universal execution record. |
| `server/places.rs::subjects_inside` | Scans bound spawn edges by location; cannot supply complete managed execution containment. |
| `server/child_close.rs` | Child relationship authorization plus its own mux close verification, dispatch and durable result. |
| `server/subject_control.rs`, `verification.rs::cleanup_launch_pane` | Subject termination followed by a separate launch-pane cleanup path; restart creates a replacement before cleaning the predecessor. |
| `server/sweeper.rs::wake_request` | Pane-death wake verifies subject exit but does not capture code there. Reads in `server/registration.rs` capture spawn exit observations. |
| `managed_scope.rs` | Verified per-registration scopes exist. Current wrappers exec `systemd-run`; scope stop is TERM-only (`SendSIGKILL=no`). Scope membership/emptiness is evidence, not main-process status. |

The graph indexes the original worktree and predates this branch. These findings were checked against source in this worktree. `context/lifecycle.md` confirms that an enclosing place may close over unverified survivors, but only verified exits count as stopped. Keep that distinction.

## Goals / Non-Goals

**Goals:** one execution lifecycle after admission; one domain close interface; explicit claim/pane association; bounded shutdown that fulfils close intent; durable, truthful completion and partial outcomes; small Rust interfaces with no second authority in the TUI.

**Non-goals:** scope every shell, encode tmux hierarchy in slices, inherit spawn edges, move children to a new lead, cold recovery, automatic redispatch after daemon failure, a new runtime, a trait hierarchy for UI rows, or a generic operation engine. No new close-persistence configuration in this change.

## Decisions

### 1. Subject, claim and pane are separate durable identities

A subject owns continuity; a claim owns one execution lifetime; a pane owns a terminal lifetime. `Claim` remains the execution aggregate and `ProcessRecord` its persisted process facts, not another addressable execution object. Add verified placement to the claim's execution facts through the existing record/event discipline. Placement names a generation-fenced pane and current container association, plus provenance (`created` or `admitted`), not a second process identity.

The authoritative pane reference contains the backend identity, verified serving `ProcessIdentity` and physical pane handle. Current tab/workspace membership is derived from fresh topology and published as placement; do not freeze the original tab forever. Update execution placement on verified moves. Historical spawn location remains provenance, not current containment authority.

Do not persist a redundant reverse index: derive pane-to-live-claims lookup from current execution records plus the verified inventory, using an in-memory index only where the existing projection needs one. Execution termination remains possible by claim when placement is unavailable; pane removal requires a valid pane reference.

**Why:** operator shells outlive agents and can host subsequent claims; pane movement is not parentage change. Exclusive `Subject { pane: Pane }` ownership would conflate these lifetimes. Fabricated spawn edges would invent producer/create history.

### 2. Row resolution is a daemon adapter, not lifecycle dispatch on row objects

The frontend sends one Close command with the selected `UiRowId`. Resolve against the canonical tree and current registry facts:

| Row | Resolved domain meaning |
|---|---|
| Joined agent | Read its granted subject/claim association, freeze that exact claim and verified hosting pane. Never infer the process from its pane-keyed row ID. |
| Pane | Generation-fenced pane and the currently hosted managed claims; the common close implementation handles those claims before removal. |
| Workspace / recorded container | The corresponding generation-fenced container; a pane row that represents a tab resolves according to its explicit container projection, not guessed ancestry. |
| Unresolved registration / task / status-only row | Refuse unless it already exposes an explicit eligible domain target. No parent or focused-row fallback. |

Omitted row continues to mean currently observed focus, not a browsing cursor. Preserve the current explicit-row/gate identity checks and stale-input refusals. Agent target validation distinguishes an old claim from a replacement on the same pane before any effect. Remove TUI choices between lifecycle cancel, raw mux close and topology delete; their adapters converge in the daemon.

**Why:** UI row identities are presentation identities, and filters/history must never change domain child discovery. Reusing row traits would leak display structure into lifecycle authority.

### 3. A small Rust module owns orchestration; records remain data

Use the existing control-plane domain modules, with a focused internal close module rather than another server/RPC layer. Illustrative signatures, not a new public wire schema:

```rust
pub(crate) enum CloseTarget {
    Agent { claim: ClaimId, pane: PaneRef },
    Pane(PaneRef),
    Container(ContainerRef),
}

pub(crate) fn close(target: CloseTarget, operation: &mut CloseOperation<'_>) -> CloseReport;
pub(crate) fn restart(subject: SubjectId, expected: ClaimId, operation: &mut RestartOperation<'_>) -> RestartReport;

// Private to the domain implementation:
fn terminate_execution(claim: ClaimId, shutdown: &mut ShutdownBatch<'_>) -> ExecutionOutcome;
fn capture_completion(claim: ClaimId, evidence: CompletionEvidence) -> Result<(), StoreError>;
fn remove_pane(pane: &PaneRef, runtime: &dyn RuntimeProvider) -> RemovalOutcome;
```

Reuse existing ID types or a minimal private newtype at actual ambiguous seams; do not add a parallel family of IDs just to match this sketch. `CloseOperation` holds the admitted operation identity, deadline, dependencies and progress; it is not a service locator or generic workflow framework. Retain existing `RuntimeProvider` and `ProcessVerifier` trait seams. The finite domain target set uses enum dispatch; no async-recursion/row-trait dependency is needed.

**Why:** same-claim termination and pane destruction have different authority and lifetimes. `exit()` obscures observed versus requested exit; `kill()` obscures process versus terminal effects. Multiple RPC entrypoints need authorization adapters, not multiple implementations or internal JSON self-calls.

### 4. One supervision chain after placement/admission

Keep two thin preparatory paths: daemon-created placement (`L` and spawn) reserves an execution before release; direct `radar run --agent` verifies the caller's existing placement. Both then use the same execution supervisor, registration, scope and completion protocol. Preserve target-directory login-shell/direnv resolution for `L`; after resolution it must enter the managed chain, not execute a naked recipe. A spawn additionally records and binds its edge; a lead launch must not fabricate a parent edge.

The common `radar run` supervisor starts the actual program in the verified per-execution scope, waits for its main process, and submits one authenticated completion report containing claim, program birth identity and observed wait status. The registration binds the actual managed program identity, not whichever wrapper PID happened to call admission. Use a claim-scoped private completion credential, never a caller-selected claim plus untrusted numeric code. Bound release/identity handshakes before allowing the program to publish.

Keep the terminal attached: do not detach stdin/stdout, create an unintended session or change shell job-control behaviour. Supervision should be local to the launch, not dependent on a daemon-owned child surviving daemon restart. Prove Ctrl-C/foreground handling and actual program identity in the first vertical slice. If scope layout places the waiter inside the terminated group, it must handle TERM long enough to collect the child; scope-wide SIGKILL may eliminate the waiter and status will then legitimately be unavailable. Do not claim `ExecMainStatus` exists for scopes.

A dedicated tmux pane's exit hook remains an additional notification/evidence source; it observes the pane command, never a hosted agent that returned to its shell. A completion report and hook may race: persist a compatible completion once, without re-running lifecycle effects. An admitted agent's shell returns normally on spontaneous exit; an explicit Close still removes that pane, as agreed.

**Why:** relying on pane death excludes shell-hosted agents. Separate admission implementations may perform different verification, but may not omit supervision, restart or completion capabilities.

### 5. Completion is recorded at the event, never required to close

Authoritative main-program completion belongs to the claim. Public spawn reads project that claim's completion rather than owning a competing exit record. Preserve evidence provenance: pane-command status and supervised program status are different observations, not interchangeable codes. Capture at the supervisor event and in daemon-initiated termination before removing a still-readable pane. Delete read-triggered `observe_spawn_exits` from get/list.

A verified exit can have unknown code. Attempt status writes while evidence exists, but a failed fetch/report/write does not veto requested terminal removal or falsely unset an established process exit. Record unavailable capture/recording diagnostics separately where storage permits; never invent zero. Reading a missing status does not cause a backend query. Missing reports during daemon outage remain a known limitation; no new periodic status-fetch fallback or durable replay engine is added.

**Why:** retained panes are not durable exit storage. Conversely, diagnostic loss must not strand lifecycle progression or cleanup.

### 6. Recursive close, batched graceful wait, enclosing-resource fallback

Resolve the root identity at admission. Discover domain containment from current verified topology and execution placement, not the filtered row tree or spawn edges. A private recursive walk (or an explicit stack with the same semantics) gathers leaf work and starts termination before waiting. Deduplicate exact claims; share the existing two-second graceful window across the operation, rather than spending two seconds per leaf.

Mark the branch closing while the operation is active; reject overlapping Radar create/move/restart requests at domain admission, not merely in the TUI. This is an in-flight conflict guard, not a global topology lock. External tmux mutations may occur. Discovery is best effort and non-atomic; failure is retained as incomplete graceful handling, not silently treated as an empty set and not an automatic veto on removing the verified selected container.

For managed claims, verify the scope association, request TERM for all eligible scopes, wait once, then send KILL to still-populated verified scopes and make one bounded verification attempt. Revalidate identity before escalation; never kill by a `radar-*` name pattern or stop a shared slice. Set an explicit bounded escalation/verification budget (reuse the existing three-second verification ceiling); no polling without a deadline. Ordinary unmanaged shell workloads receive normal terminal teardown, not fabricated managed guarantees.

Remove each selected leaf pane after termination/capture attempts. Removing the last pane may remove its tab/session; verify absence under the same serving generation and accept it as the requested resource state, not proof of process exit. Finally remove the root resource if it still exists. This is an intentional stronger fallback for the confirmed operation, not an unlimited retry of an unknown close. A changed server generation is never a reason to remove a new resource with the same handle.

**Why:** users mean to close the branch, including residual panes. A frozen whole-tree plan does not solve native tmux mutation and adds unnecessary transaction machinery. Shared deadlines preserve responsiveness; final parent removal handles missed/new leaves without blocking on exhaustive discovery.

### 7. Public close removes; restart replaces

`D` on an agent resolves its exact current claim and hosting pane, terminates/captures, then removes the pane. It does not silently expand to sibling panes because the agent row happens to be a tab root. `D` on an actual tab/workspace target closes that container. Ordinary spontaneous process exit leaves its pane according to normal shell/tmux persistence.

Restart is subject-level orchestration: validate the expected old claim, reserve a successor, terminate the old execution through the same primitive, capture its completion, start a fresh claim, then advance `current_claim` through the existing durable transition rules. It never mutates the old claim into a new process. Unknown old-process termination prevents launching a second copy, unlike an operator-confirmed container close which may still remove the place.

Use an explicit placement choice: reuse a verified dedicated dead pane through runtime respawn when safe; never type a replacement command into an operator's resumed shell. For an admitted shell-hosted agent, create the replacement alongside its verified placement and leave the original shell intact. If placement is unavailable, use the existing eligible recorded-parent fallback or refuse without guessing from focus. These are placement differences only; both cases execute the same supervisor and lifecycle transitions. A replacement failure is reported and leaves any surviving diagnostic pane intact.

**Why:** calling public close from restart would remove placement too early; duplicating termination is equally wrong. No generic `restart=true` flag or callback-driven parent notification is needed: operations compose private primitives and return typed results.

### 8. One domain implementation, explicit compatibility break

Adapters retain distinct authorization, not divergent lifecycle:

| Entrypoint | Adapter responsibility | Shared action |
|---|---|---|
| TUI Close | Resolve selected row, freeze identity, show losses/confirmation | close target |
| `lifecycle.cancel` | Resolve subject/expected claim | close agent where placement exists; termination-only result if no pane can be established |
| `lifecycle.stop` | Existing process-stop intent | terminate execution, preserve pane |
| `spawn.close` / subject-child close | Validate caller/child binding and map to exact execution/placement | close agent |
| `topology.delete` | Validate recorded root and encompassing confirmation | close container |
| restart / continue | Validate subject, session, successor reservation and placement | shared termination/start as applicable |
| launch expiry/revocation | Validate launch stage and cleanup authority | shared termination; remove only resources that launch created |

Keep operation admission/result durability and same-ID replay. A top-level result owns aggregate progress; avoid creating public nested RPC requests for each child. Known terminal outcomes replay without effects. Unknown operation records remain inspectable; full interrupted-operation resumption is deferred. The same active operation may escalate TERM to KILL and leaf removal to parent removal without treating an uncertain reply as proof of no effect.

Expose a small typed close result separating resource removal, verified/unverified execution outcomes and optional completion diagnostics. Existing `already_exited` can describe a pre-ended execution with an optional status; absence of a code is not refusal. Confirmed root removal is the normal success presentation; missing status alone is diagnostic. Confirmed surviving managed agents or failed removal are actionable warnings. Results never settle assignments or incorporate handoffs.

**Why:** durable replay guarantees must survive convergence, but recursive structure need not become a second operation journal or user-visible wall of results.

### 9. Scope the duplication audit to evidence

Known candidates to inspect and disposition: direct admission versus launch claims; configured `L` recipes versus managed release; spawn/subject child-close versus `cleanup_launch_pane`; `cancel_many` versus single cancellation; restart/continue successor handling; launch expiry/revocation; topology reload versus execution restart. Necessary convergence lands here. Recipe-only shell reload, observation/focus routing and unrelated delivery work remain separate unless they duplicate the managed behaviour being replaced. Record remaining candidates with source seams and the differing behaviour; no blanket trait/actor rewrite.

## Risks / Trade-offs

- Supervisor changes process and terminal relationships → a real PTY/scope slice verifies stdin, foreground signals, program identity, exit status and shell return before migrating all launchers. Do not ship detached supervision or weakened identity checks to make a test pass.
- SIGKILL can destroy the status reporter → code remains unavailable; scope/process exit is verified separately and cleanup proceeds.
- Native tmux changes during close → final root removal honours selected-container intent; exact generation/claim fences prevent killing a replacement server or execution by stale identity.
- Failed discovery may leave an agent alive outside the removed terminal → use verified scopes where discoverable, reconcile honestly and expose confirmed survivors; do not claim all processes died.
- Broader one-press Close is destructive → retain actor confirmation naming selected resource and current known losses; no new second-D or daemon force-consent state.
- Rust/persistence/wire changes affect SDK consumers → generated contracts and a coordinated version bump; no hand-written parallel TypeScript result model.

## Migration Plan

1. Implement the common supervised execution slice and placement, proving admitted/dedicated parity through real entrypoints. Then move close/restart/container consumers and remove their old paths in coherent slices.
2. Add completion/placement events and DTOs through the existing persistence/schema pipeline. Version incompatible stored shapes explicitly; refuse unsupported records with a path and documented archive/reset procedure. No silent migration, old-model fallback or loss of pending communication records.
3. Update pi-radar child-close interpretation and fixtures with the changed result. Bump Radar/SDK and wire revision as the actual DTO break requires; do not choose a speculative release number in this plan.
4. Sync prerequisite specs in order before archive: `sidebar-mode`, `shared-ui-session`, `unified-tui-publications`, `duplex-tui-attachments`, `tmux-control-mode`, plus overlapping managed-scoped/topology changes. Reconcile this change's deltas against that effective baseline before applying them to main specs. Retired owner-file and tab-all-or-nothing policies are not to be resurrected because main specs lag implementation.
5. Rollback means use the prior binary with its archived prior state. Do not feed newly versioned records into an older daemon or downgrade while live managed executions depend on the new supervisor.

## Open Questions

None required for the proposed behaviour. The supervisor's exact placement relative to `systemd-run` and terminal process groups is an implementation proof obligation in the first slice, with the required behaviour fixed above. Lead inheritance, pane-gathering and a persistence option remain deferred in plan.md.
