# Tasks

Implement against `scope.md` and the aligned design/spec delta. Prefer existing production seams and tests; no parallel enrollment controller, session-marker replacement or pane-move feature. Each group includes the behavior checks and documentation it needs. Items are unchecked: this is a plan, not an implementation report.

## 1. First-class adoption through the real Pi reporter

- [ ] 1.1 Converge manual adoption, explicit enrollment, launch binding and recorded-execution reconciliation on one daemon-owned binding routine, resolving subject by canonical session UUID and validating optional launch correlation before adoption; extend an existing daemon integration flow to verify manual adoption, identical retry, correlated child binding and correlation refusal without duplicate subjects or effects.
- [ ] 1.2 Adapt the existing durable subject/execution records for session association and active attachment without replacing storage or rewriting history; verify reopen restores one binding and that ambiguous legacy session mappings remain unresolved rather than merge IDs, parentage or inboxes; document actual format/rollback constraints.
- [ ] 1.3 Wire automatic management into pi-radar's baseline registration/context driver and expose accepted/pending/refused association through the shared runtime, with additive capability negotiation and generated DTOs only for consumed changes; exercise the real reporter to verify manual Pi becomes managed without E/Herdsman, missing verification preserves baseline publication, and an older daemon gets no simulated identity; update `pi-radar/README.md` and registration docs alongside the slice.

## 2. Session switching and safe reattachment

- [ ] 2.1 Change current-session reporting so `/resume` retains process registration but changes active subject, releasing the prior session attachment and keeping its relationships/history; use one composed session-switch/reattach test to verify a fresh uncontested process resumes the original subject and repeated reports/daemon restart do not create another identity.
- [ ] 2.2 Fence effect dispatch and subject-bound publication/communication against the current attachment, including same-PID session changes; extend existing control/communication checks so stale work for session A cannot act on or acknowledge as session B, while the correct current association still works.
- [ ] 2.3 Update pi-radar runtime consumers to invalidate previous subject/parent/communication handles on session selection and deliver the new association; verify through the actual session event/runtime boundary that switches do not leak cached identity, and document the consumer contract for Herdsman's owner without implementing a second registrar.

## 3. Unsupported duplicate-session handling

- [ ] 3.1 Inspect the installed Pi session-load/resume events and distinguish fresh loading from heartbeat, reconnect and extension reload; record the supported signal in the design/SDK docs with a focused host check, or implement the agreed process-restart-only fallback if reliable distinction is unavailable—no session-history comparison or merge.
- [ ] 3.2 Keep a duplicate process baseline-visible but unbound, retain sufficient conflict state across daemon restart, and allow reconsideration only after fresh loading/restart plus incumbent release verification; extend one duplicate flow through incumbent exit and daemon restart to prove no automatic promotion, then prove the supported fresh-load/restart path can bind.
- [ ] 3.3 Expose a concise duplicate/pending warning in observation and the TUI without displacing the incumbent or granting inbox authority; add a focused existing-view rendering assertion and update operator guidance with how to resolve the duplicate safely.

## 4. Session-owned relationships, inbox and restart

- [ ] 4.1 Resolve logical parent/child identity using subject relationships while preserving original spawn history; extend the reattachment flow to verify child identity and pending inbox survive parent process replacement, and render a minimal out-of-tab parent reference without moving panes or building cross-tab nesting; document the supported placement limitation.
- [ ] 4.2 Use verified observed process context as the private restart recipe for either origin, freeze/revalidate the selected subject's current session and preserve existing restart placement; adapt existing restart coverage for current-session selection, initial-prompt/credential exclusion and refusal before termination on ambiguous arguments; update restart docs with observed-environment limits.
- [ ] 4.3 Remove active session-marker reads/writes, marker-only environment injection, host API requirements and marker projections/tests with no remaining consumer; retain necessary released compatibility exports but ignore old entries; verify baseline/runtime behavior without custom-entry support and run contract drift/type checks after regeneration, then align the superseded lifecycle-authority breadcrumb guidance.

## 5. Integration and adoption handoff

- [ ] 5.1 Run formatting, strict Clippy, locked Rust targets, pi-radar contract/type/package checks, strict OpenSpec validation and the checked Nix build; record actual exits and any unavailable check, using the existing gate rather than adding a validation framework.
- [ ] 5.2 Run a disposable real Pi/tmux check covering manual auto-adoption, `/resume` subject change, fresh-process reattachment and duplicate refusal/reload behavior, with no pane moves; record observable identities/links and any live check left operator-pending rather than infer acceptance from unit tests.
- [ ] 5.3 Deliver the Herdsman adoption brief with the current-association/cache contract and verified local SDK artifact; record the consumer owner's validation or explicitly pending cutover, including a handoff recovered by the same session subject after reattachment. Keep assignment restoration and settlement policy outside Radar.

## Workflow follow-up

- Ship daemon capability support before the pi-radar npm release and consumer pin updates; publication, switching and live consumer acceptance remain explicit release/operator work, not implicit results of passing tests.
- Review the completed change, sync requirements and archive when implementation and required acceptance are recorded.
- Keep `/radar gather-children`, first-class resume, layout migration, richer cross-tab views, fork investigation and legacy repair tooling deferred as listed in `scope.md`/`ideas.md`; do not add placeholders.
