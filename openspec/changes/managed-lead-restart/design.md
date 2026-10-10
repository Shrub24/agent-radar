# Design

## Context

See proposal.md. Today the daemon registry identifies publisher/process incarnations and separately publishes mutable session UUID context; daemon-spawn edges record a parent identity and are durable, but there is no managed restart operation. Existing `agent.context` cannot take over a registration, and Herdr foreground/session metadata is not sufficient to distinguish multiple live attaches. The daemon's private launch specification is inert. Existing worker restart remains owner-routed and lead/standalone restart is explicitly refused.

## Goals / Non-Goals

**Goals:** define a stable daemon-owned logical runtime subject for a managed lead/session; restart the lead against its explicitly stored session launch data; bind the replacement process incarnation to the same logical subject; preserve daemon-owned child relationships and leave already-live children untouched; keep operation outcomes durable and uncertainty explicit.

**Non-Goals:** restart child processes; revive arbitrary Pi in-memory state that was never persisted to its session file; infer restart recipes from argv, titles or scrollback; adopt foreign panes; auto-restart, batch restart, general process kill, or change assignment authority.

## Decisions

### Session UUID is logical parent identity; exact process is its live binding

Use the canonical Pi session UUID as the stable logical parent identity for new daemon-spawn edges. A spawn caller supplies both the exact registered `parent` agent UUID and `parent_session_id`; admission verifies that this exact registration has fresh published context equal to the supplied session UUID. Do not search by session to pick a process. This makes the caller's choice of parent attach explicit while keeping multiple attaches distinct. Store the session UUID on the edge as the semantic parent and retain the exact process registration as its provenance/live parent binding.

On restart, only the authorized lifecycle transaction may replace the session's lead process binding. Ordinary `agent.context` or registration must not take over the binding. The replacement launch receives a private one-time correlation tied to the restart operation and session UUID; successful registration consumes it and associates the new incarnation with the existing logical session. Old process identity remains historical evidence, not current state. A UUID match without correlation and positive process verification cannot bind.

Alternatives rejected: session-only selection among registrations (ambiguous when multiple attaches exist); use process incarnation alone as logical parent (restart makes children appear re-parented); mutate/reuse the old incarnation registration (loses auditability and lets stale writers impersonate the replacement).

### Restart the lead and preserve children; do not restart them

The first operation targets one explicitly managed lead/standalone Pi session after TUI confirmation. It validates the exact session identity, daemon-owned launch provenance, fresh process evidence, and positive confirmation that the previous lead process has stopped before launching. It launches the replacement into the same session, correlates registration using a private restart token, updates the session's live process binding only on verified match, and returns a durable result. If stop or launch may have dispatched without confirmation, record unknown and do not replay the physical effect.

Existing child panes/processes remain alive and unchanged. Their spawn edges refer to the stable parent session UUID. After restart, independently refresh child process/location evidence; stale or missing observations mark the child unresolved, not dead or detached. Restarting children, preserving in-memory state not durably stored in their own sessions, or reconstructing a child relationship from pane metadata is not part of the first version.

Old-process stop is graceful only: resolve the exact registered process birth identity to a Linux pidfd, request SIGTERM through that descriptor, and wait a bounded interval for that process to exit. Never fall back to a PID-only signal or SIGKILL escalation. A stale registry lease or pane closure is not process-exit evidence. If this build cannot obtain a pidfd for the exact identity or cannot confirm exit by the deadline, do not launch the replacement; return refused only when dispatch is proven not to have happened, otherwise unknown. This lets normal Pi shutdown persist session state while refusing to turn an unresponsive process into a duplicate.

### Keep assignment authority separate

The daemon owns physical dispatch, restart correlation, current process binding and durable physical outcomes. Radar determines whether a subject is physically restartable from daemon-owned launch provenance; Herdsman retains assignment eligibility and reconciliation, but its publisher does not need to assert restart authority or re-report the replacement PID. For a daemon-managed logical session, the daemon already owns the launch specification and runtime relationship: an explicit confirmed restart targets that stored subject, the daemon stops the recorded incarnation, relaunches with a private one-use correlation, verifies the replacement incarnation/session, and updates its own binding. The correlation token is evidence that the new registration belongs to that daemon-initiated restart, not an authority grant from the child.

Restart eligibility must come from daemon-owned provenance that the logical subject was admitted/created as restartable; never infer it from `idle`, a child-provided role/label, session presence, assignment writer provenance, or caller-supplied source/incarnation. An approved TUI capture workflow binds the exact process/session identity and captures executable, argv, cwd and the complete environment from Linux procfs. Store the environment only in the private durable launch record, bounded in size and mode 0600; never return it in public reads or emit it to UI, logs or diagnostics. Strip an old one-use Radar correlation and mint a fresh token for relaunch. Publisher-supplied launch specs alone do not qualify as provenance. However, Herdr currently exposes pane creation and input but no positive pane-ready/shell-ready acknowledgement. Radar cannot safely know that a fresh pane's shell will accept the command atomically. Do not infer readiness from timing, output text or marker injection, and do not expose an action that creates a pane then refuses. Keep restart unavailable until Herdr offers a bounded positive readiness acknowledgement after pane creation; absent/timeout must be distinct from ready. Once available, launch the exact saved command using `cd` plus `exec env -i` and POSIX quoting, in a fresh nonfocused pane, never by signaling or sending command text to the old pane. A physical restart does not complete an assignment or answer pending asks; Herdsman retains that authority and reconciles its own state separately.

Restart is offered only from Radar's TUI after explicit operator confirmation, and targets are revalidated against the selected row's exact logical session and current process birth identity immediately before dispatch. It is not a Herdsman lifecycle operation and this change adds no public restart API, CLI command or public daemon protocol method. The internal runtime-provider path uses the same mux abstraction as other Radar operations. The local Unix socket remains a same-UID trust boundary for existing backend operations; confirmation is an operator-intent safeguard, not a security boundary against another process running as the same user. Do not add a separate owner grant mechanism in this slice.

### Preserve exact operation outcomes and expose no hidden fallback

Use one durable restart operation id bound to exact request content and exact logical subject. Return `completed` only after the replacement incarnation is verified and attached; distinguish pre-dispatch refusal from unknown after possible physical effect. Same-id replay reads stored outcome and never dispatches another stop/launch. Radar does not retry an unknown restart via Herdr or direct launch.

## Risks / Trade-offs

- [The saved session is attached elsewhere] → Require operation-scoped replacement correlation and birth-identity checks; refuse ambiguous live attaches instead of trusting UUID equality.
- [A new pane's shell is not ready for command input] → Require a positive upstream readiness acknowledgement; never use a delay, output heuristic or marker command. Restart stays unavailable until then.
- [The old process ignores graceful termination] → Do not force-kill or launch a duplicate; report refused/unknown and leave the logical subject and children intact.
- [Pidfd support is unavailable or races identity lookup] → Fail closed before signal dispatch; never degrade to PID-only signaling.
- [Replacement launches but fails before registration] → Keep the logical subject and child edges durable, report partial/unknown outcome, and do not launch again automatically.
- [Child outlives the lead but loses fresh registry reporting] → Preserve its edge and last facts with stale freshness; do not claim it stopped or detach it.
- [Legacy child edges are process-parented] → Preserve v1 edges unchanged and never infer their session parent. A session with a legacy v1 child edge is not eligible for restart until an explicit, evidence-backed migration exists; v2 session-parented edges are preserved unchanged.

## Migration Plan

1. Add stable logical runtime subject and parent-edge association compatibly; retain process-incarnation IDs and existing records as historical/live bindings.
2. Obtain from Herdr a documented bounded pane-ready/shell-ready acknowledgement for a newly created nonfocused pane. This is a prerequisite: do not synthesize readiness with sleeps, output matching or injected markers.
3. After readiness is available, add daemon-owned restartable-subject admission, exact-session replacement correlation and durable restart outcomes; subjects without provable launch provenance remain unavailable.
4. Verify persistence/replay/refusal paths in disposable daemon tests and a gated live Herdr smoke proving the lead changes while child pane/process identities and edges remain unchanged.
5. Expose an explicitly confirmed lead restart in Radar only after the Herdr pane-readiness contract and daemon implementation are accepted; retain existing worker restart route.

Rollback disables the new restart operation and UI action; it must not delete logical subjects, child edges, process history or private launch specifications. Existing owner-routed worker controls remain unchanged.

## Resolved Questions

- Legacy daemon-spawn edges remain explicitly process-parented v1 records; do not infer session identity. Until a separately designed, evidence-backed migration exists, a lead with a v1 child edge is not eligible for restart.
