# Session-bound subjects and unified Pi enrollment

## Status and intent

Agreed scope for `unified-pi-registration`, reflected in `proposal.md`, `design.md`, the `agent-registration` spec delta and `tasks.md`. This supersedes the earlier draft's process-following subject model, its refusal of ordinary session-based reattachment, and its suggestion that cross-tab placement requires reunion before identity can be established. Implementation has not started.

The goal is a first-class managed Pi session whether Radar launched it or the operator started it. Reuse the existing registration, context publication, lifecycle records and process verification. Delete redundant provenance paths rather than build another identity service or recovery framework.

## 1. High-level model

### Subject: the durable identity of a tracked Pi session

A subject represents one Pi session UUID within Radar's state. Radar retains its own subject ID and associates it durably with that session. It is not the Pi process, tmux pane, workspace or session-file path.

The subject owns Radar's logical parent relationship, communication recipient identity and association with managed lifecycle history. Herdsman continues to own assignment semantics, completion validity and application history. This change does not transfer those domains to Radar.

A session that Radar has never tracked gets a new subject on successful adoption. Radar does not reconstruct previous authority from conversation text or copied custom entries.

### Registration and execution: the current process attachment

A registration describes a process incarnation. Verified process birth identity distinguishes it from PID reuse. The same process may select different Pi sessions over its lifetime.

A managed binding associates a subject with its current execution/registration. Existing execution IDs, generation fields and writer leases may remain daemon-internal machinery where used; no new Pi-persisted identity is required. A session attachment change must fence outstanding operations even when the PID is unchanged.

One process has one current active Pi session association; one session subject has at most one accepted active process attachment. Baseline observation does not itself grant subject ownership.

### Location: placement, not logical ownership

The current verified pane/tab/workspace is execution placement. Moving a pane or manually resuming elsewhere does not change the session's subject, children or inbox identity. No topology effect follows merely from registration.

## 2. One enrollment and reconciliation path

Use one daemon-owned resolution and binding path, reached through pi-radar's existing process registration/current-context reporting. Explicit `lifecycle.enroll` and existing daemon launch-binding/reconciliation entry points become adapters to the same rules, not separate enrollment policies. Do not make Herdsman another registrar.

The path uses the reported current session, process incarnation, verified process/pane evidence and optional launch correlation:

| Situation | Result |
| --- | --- |
| Eligible manual Pi; session not tracked | Create a subject for the session and bind the process as an adopted lead |
| Eligible Pi resumes a tracked, unattached session | Bind to that session's existing subject and retained relationships |
| Managed launch supplies valid correlation | Validate against the admitted launch and bind through the same path; retain its recorded parent/metadata |
| Supplied correlation is invalid | Refuse managed binding; never silently fall back to unrelated adoption |
| Same process repeats its report or reconnects | Return the established binding without duplicate subjects or execution transitions |
| Daemon loses volatile observations but retains records | Reverify and restore the binding using the same path |
| Physical verification or usable session is missing | Keep baseline reporting; expose management as pending rather than claim success |

Registration can precede session availability. Reuse the existing context retry/heartbeat driver when enough evidence becomes available; no additional enrollment polling loop.

Managed launch dispatch, verified registration and application readiness remain separate milestones. This change does not make launch requests wait for health or assignment acceptance.

## 3. Session changes and process replacement

### `/resume` within an existing process

The process registration remains, but its active subject changes to the subject for the newly selected session, creating one if untracked. The previous subject retains its own children, inbox and history; the new conversation does not inherit those because it occupies the same PID or pane.

A subject is not kept exclusively attached to a process that has verifiably switched away. In-flight effects and application writers must not retain authority from the previous attachment after the switch. Validate the current attachment at effect boundaries; old writer/context generations cannot publish into the new subject accidentally.

### New process resuming a tracked session

A fresh, uncontested attachment can regain the same subject without a special Radar restart or a session-file identity marker. The previous attachment must be released or independently established as no longer active for that session. An expired heartbeat or failed scan alone is not sufficient evidence.

Radar-controlled restart preserves the session subject and establishes a new execution attachment. Existing restart placement remains anchored to the previous lead's pane/tab where available. A future resume feature should preserve placement by default, but implementing it is not part of this change.

### Duplicate live session: unsupported, visibly preliminary

If another process opens a session with an accepted live attachment:

- Keep the incumbent's subject binding.
- Continue baseline tracking of the duplicate, with a visible duplicate-session warning and no managed subject/inbox authority.
- Do not terminate either process, choose between histories, merge them or promote the duplicate when the incumbent later exits.
- Require the duplicate to perform a fresh resume/reload of persisted session state, or restart, before it can be reconsidered after the conflict is resolved. Ordinary reports and heartbeats do not clear the conflict.

The adapter must distinguish a real fresh session load from a heartbeat/reconnect. This is a duplicate-admission condition, not a new session-history validator. If the host cannot signal the needed reload reliably, require a process restart rather than guess.

No fork investigation or additional branch model is included. A genuinely new session UUID is a new subject; copied conversation entries confer no existing parent or inbox rights.

## 4. Parent/child identity, communications and placement

Parent relationships are Radar-owned durable relationships between session subjects. Initial managed child registration obtains its parent from the validated launch, not a caller-selected fact or session entry. An adopted untracked session is a lead with null parent and absent owner/run/assignment metadata.

A tracked child resuming its session retains its recorded parent identity. A lead resuming elsewhere retains its children. Absence of a recorded parent means a lead in Radar's managed model; do not add uncertainty for hypothetical children from unrelated extensions.

Execution-specific spawn evidence remains historical evidence of the original effect. Logical relationships must not depend on the original parent registration remaining live. This is especially important because the current tree resolves spawn parents by registration ID.

Handoff inbox access follows the current accepted subject binding. A new execution attached to the same session subject can recover its pending handoffs; a different session in the same process cannot. Published sender/recipient provenance stays immutable, and receipt still does not imply assignment settlement or permission to close a worker.

For a lead manually resumed in another tab, retain the subject/child references and provide a minimal readable association to the parent outside that tab. Keep rows in their physical tabs. No cross-tab tree framework, synthetic nesting, automatic pane movement or requirement to reunite before binding.

Explicit child gathering is parked in `ideas.md`. Do not add a command stub, backend move primitive or multi-pane operation in this slice.

## 5. Public contract and pi-radar behavior

### Daemon

Expose the result of session-subject binding through the existing reporting/read surface: accepted managed identity, pending verification, duplicate conflict or correlation refusal. Reuse existing error/result conventions; add only the fields or capability needed by the production consumer.

Keep ordinary baseline registration possible before management resolves. Make the difference visible rather than return a nominally managed subject before verification. Deduplicate under the existing transition serialization, and reuse existing durable records after restart.

Keep recipes private. Never expose environment values or launch credentials in public identity, warnings, schemas or observation snapshots.

### pi-radar

- Request automatic enrollment by default for eligible Pi processes through the existing reporter.
- Publish fresh current-session changes and update the shared runtime's subject association accordingly.
- Expose the daemon-issued subject to consumers such as Herdsman without a caller-selected identity or recipient override.
- Present pending/conflicting management state without breaking ordinary facts publication.
- Keep process-local registration continuity across extension reloads, but do not treat reload alone as proof that a conflicting session was freshly loaded.
- Remove active session-marker reads/writes and marker-only environment plumbing. Opening an old session must not reinstall an old execution identity.

Use the existing typed DTO/schema workflow only for wire shapes actually changed and consumed. Negotiate additive behavior for older daemons instead of raising unrelated clients' global minimum revision. Do not silently emulate management when the daemon lacks the capability.

### Herdsman

Consume pi-radar's current managed subject. Do not call a separate enrollment loop or maintain a second identity mapping. On session switches, stop using cached subject/parent/inbox handles from the prior session and restore the new session's application state using Herdsman's existing mechanisms.

Keep completion validation, assignment recovery, worker eligibility and disposition in Herdsman. Subject reattachment makes the correct communication identity available; it does not certify that application state is restored or work may be reassigned.

## 6. What we drop or simplify

- Mandatory Pi-session launch identity entries and the API support needed only to write/read them.
- Routine marker matching/stale/contradictory projections as a prerequisite for management.
- Pi-persisted launch IDs and execution generations. Retain launch correlation for initial validation and daemon-side fencing where there is an actual consumer.
- Separate management eligibility for launched versus adopted sessions. Origin remains history, not a privilege tier.
- Manual `E` as a prerequisite for ordinary Pi management. Keep explicit enrollment as a compatible adapter where still useful.
- Special manual-resume identity recovery from session breadcrumbs.
- Restart refusal solely because the process changed its current Pi session since initial enrollment.
- Ownership tied to the original process, pane or live parent registration.
- A separate background enrollment driver, generic workflow framework, identity arbitration service or optionality matrix.
- Automatic topology repair, reunion requirements and speculative pane migration.

Do not delete immutable operation history or repurpose historical execution IDs. Public optional legacy fields/schema exports may remain compatible during SDK adoption, but they must not retain an active competing identity path.

## 7. Restart recipe semantics

Standardize the executable restart recipe on the observed, verified process context for either origin, captured when establishing an execution binding rather than on every heartbeat. Retain original admitted launch intent as history.

Restart uses the fresh current session of the selected subject/attachment. Reuse existing argument reconstruction to remove old session selectors and avoid replaying initial prompts or one-use launch credentials. Refuse an ambiguous recipe before terminating anything.

The promise is to reproduce observed executable/argv/cwd/environment, not rerun the user's launcher, refresh credentials or re-evaluate direnv. A process's observed working directory need not equal its original launch directory. These are documented limits, not reasons to deny adoption.

## 8. Impacts and footguns

| Risk or implication | Required boundary |
| --- | --- |
| Same PID switches session while a control is pending | Revalidate subject-to-execution/current-session binding before the effect; process birth identity alone is insufficient |
| Stale duplicate becomes the only live process | Do not promote it without a fresh session load/restart |
| Daemon restart or lost response repeats adoption | Resolve existing durable session binding before creating; no duplicate transition on retry |
| Multiple old subjects already name the same session | Do not silently merge inboxes, history or parentage; surface an unresolved legacy conflict for explicit resolution |
| Stored sessions/relationships are missing | Ordinary adoption may create a new identity; Pi claims cannot recreate missing authority |
| Original parent process disappears | Retain logical parent identity while location/liveness are unavailable |
| Manual resume changes physical tab | Preserve identity and basic links; do not move panes or pretend they occupy the same tab |
| Cached Herdsman/pi-radar state outlives a switch | Invalidate the old active association; do not route publication or inbox acknowledgment through it |
| Auto-adoption enables controls without pressing E | Preserve existing destructive-action evidence/confirmation requirements; never act merely because enrollment succeeded |
| Execution records and legacy schema fields remain | Distinguish compatibility/history from active authority; do not add a parallel provenance pipeline |

Session filenames/paths are locators, not identity. Continuity is limited to Radar's durable store; losing that store is not recoverable by inspecting Pi history.

## 9. Adoption and delivery

1. Use the aligned proposal/design/spec delta/tasks as the implementation plan. Update the earlier lifecycle-authority breadcrumb guidance it supersedes as part of the corresponding implementation/documentation slice.
2. Add the daemon's shared session resolution/binding through existing registration/context and lifecycle transitions, including duplicate handling and logical-parent projection.
3. Update pi-radar's real baseline reporter/shared runtime in the same vertical slice. Remove the superseded marker path and regenerate consumed contracts.
4. Adapt persisted state non-destructively. Reuse an unambiguous session-to-subject association; retain history and refuse ambiguous merges. Subjects without usable session evidence remain unresolved until a verified report supplies it. No genesis replay, database replacement or mass process discovery.
5. Coordinate Herdsman's current-subject/cache handling and handoff consumption with its owner. Ship daemon support before the SDK cutover; publish a new pi-radar version and update its consumers. Do not call the cross-repository cutover complete on Rust tests alone.

Validation follows behavior, not a new matrix: exercise manual adoption/reattachment with lost response or daemon restart; correlated child registration and mismatch refusal; session switching with retained child/inbox ownership; and a duplicate that remains blocked after incumbent exit until a fresh load. Extend existing faithful tests where possible. A minimal live Pi check must establish the host's session-load event semantics; report it as deferred if not run. No speculative fork, layout or artifact suites.

## 10. Deferred and future considerations

- `/radar gather-children`: explicit operator-directed pane movement, honest partial outcomes; parked in `ideas.md`.
- First-class resume command, default pane selection when the old pane is gone, layout migration and richer cross-tab presentation.
- Fork/copy history policy beyond a new UUID being a new subject.
- Session-history divergence detection or merging; duplicates are unsupported rather than reconciled.
- Explicit repair tooling for ambiguous legacy session-to-subject mappings if real migration data requires it.
- Assignment/context restoration, mailbox domains beyond handoffs, and richer orchestration recovery.
- Renaming/removing persisted execution-generation fields or replacing the storage model. Neither is necessary for first-class enrollment.

Success is one verified, session-bound identity flow used by manual Pi, Radar launches and reconciliation, with fewer active provenance paths—not a comprehensive agent recovery platform.
