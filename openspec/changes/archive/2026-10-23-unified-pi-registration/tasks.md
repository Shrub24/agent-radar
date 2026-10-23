# Tasks

The implementation already contains partial session continuity and marker removal. Checkboxes below track the corrected presence/claim contract, not the superseded sweep-adoption implementation. Retain useful existing code, remove competing paths, and do not mark partial behavior complete.

## 1. Durable daemon recognition

- [x] 1.1 Add the flat presence report and typed standing/release DTOs; resolve immutable process identity to `registration_id`, retain mutable location/session context separately, and validate optional launch correlation without adoption fallback. Generate consumed protocol artifacts and rename public `agent_id` references without alpha compatibility aliases.
- [x] 1.2 Persist session-load claim identity and granted/duplicate/released decisions using existing storage and transition serialization. Retry interrupted work without another subject; preserve existing session subjects, inboxes, relationships and history. Document storage/rollback limits and ambiguous legacy handling.
- [x] 1.3 Bind through the recognition request, not a later adoption sweep. Reassertions replay the same claim; a duplicate stays rejected after incumbent exit and daemon restart. Pending verification failures remain retryable. A fresh same-process claim supersedes the old attachment safely.
- [x] 1.4 Implement exact idempotent release. A delayed release cannot revoke a replacement, a released claim cannot reacquire, and graceful release leaves process registration intact. Verify process birth identity for crash recovery, never infer death from pane loss or heartbeat expiry; remove sweep promotion.

## 2. pi-radar production path

- [x] 2.1 Use one report function on `session_start`, renewal and reconnect; create claim IDs only at session-load boundaries, reuse them after failures, and retain private retry/acknowledgment state. Replace separate registration/context management choreography while keeping facts/channel publication working.
- [x] 2.2 Send bounded awaited exact release on `session_shutdown`; preserve unresolved release for retry when the process survives. Invalidate subject/parent/communication handles on the session boundary and expose current granted/duplicate/pending standing through the shared runtime.
- [x] 2.3 Finish the `registration_id` SDK rename, remove obsolete marker/registration-context scaffolding, update public schemas and package docs, and ensure no transcript or managed-identity environment machinery is reintroduced.

## 3. Consumers and display

- [x] 3.1 Fence effects and handoff identity to the current granted attachment, including same-PID session switches. Preserve logical parent/child/inbox continuity on resume and existing-pane restart using verified observed context.
- [x] 3.2 Carry duplicate standing into observation and show a compact inline TUI warning distinct from stale process/binary marks. Preserve minimal logical parent references across tabs without pane movement or tree redesign.
- [x] 3.3 Update the existing Herdsman companion brief with the flat report/release and breaking registration-ID contract; leave cross-repository consumer changes to its owner and report adoption status explicitly.

## 4. Verification and adoption

- [x] 4.1 Exercise a composed acquisition/switch/release/resume flow, including mutable pane location and retained child/inbox identity; prove duplicate stickiness across incumbent exit and daemon restart, then fresh-load acceptance; prove lost-response replay and delayed release fencing. Prefer existing integration seams over fixture matrices. Existing tests already covered reconnect replay, mutable pane location, forged subjects, delayed-release fencing, duplicate stickiness across release and restart, and a fresh process reclaiming a released session; `a_session_switch_frees_its_subject_and_a_duplicate_waits_for_a_fresh_claim` adds the three gaps: a same-process session switch, a duplicate staying a duplicate after its incumbent exits and across a restart, and promotion only on a fresh claim. Child and inbox identity across a process change rests on the subject-keyed design and the managed-handoff test, with no new case.
- [x] 4.2 Run fmt, strict Clippy, locked Rust tests, pi-radar generated-contract/type/package checks, strict OpenSpec validation and checked Nix build. Record actual exits and unavailable checks; no new validation framework.
- [x] 4.3 Operator-run live Pi/tmux smokes and Herdsman cutover remain explicit pending acceptance. No automatic pane movement, general incoming request transport, signing or assignment recovery in this change.
