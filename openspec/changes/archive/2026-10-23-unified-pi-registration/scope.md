# Unified Pi registration scope

## Agreed model

Radar owns a persistent subject for each tracked canonical Pi session UUID. Its exclusive grant to a process is releasable; the subject, relationships, inbox and history survive release. OS process birth identity is boot ID/PID/start ticks; `registration_id` identifies the process registration, not the conversation. Location is mutable context, not identity. No Pi transcript breadcrumb is required or written.

## Public contract

pi-radar sends one flat presence report containing process identity, source/incarnation, current session, session-load claim ID, location and available launch correlation/metadata. It includes acknowledged registration/subject IDs when known. Radar owns internal grouping into registration, context and claim. Its response carries registration ID, claim ID, granted/duplicate/pending standing and subject ID only when granted. The same call serves startup, periodic contact and reconnection. This is a breaking alpha contract: rename public `agent_id` to `registration_id`, without compatibility aliases.

## Session lifecycle

Create a claim at `session_start`, including startup/reload/new/resume/fork. Reuse it on renewal and reconnect. Persist its decision in Radar before acknowledging. A duplicate remains rejected after incumbent exit/release and daemon restart; a fresh session load supplies another claim opportunity. Observation failures remain pending rather than poisoning a claim as duplicate.

Graceful `session_shutdown` releases the exact registration/claim with a bounded awaited request. Keep the process registered; retain unresolved release for retry when possible. New claims from the same verified process can supersede its old attachment even if the shutdown response was lost. A delayed old release cannot release the new grant, and released claims cannot resurrect. A live process without a grant is not an exited process.

Recover held grants before admitting contenders. Verified process exit is the crash backstop. Pane loss, heartbeat expiry and transport silence do not release authority. A sweep observes exits; it never supplies a fresh claim or promotes a rejected duplicate. Lost durable authority stays unresolved, not reconstructed from a client receipt, signature or session transcript.

## Continuity and controls

New processes resuming known sessions receive their existing subjects when eligible; same-process session changes select different subjects. Child links, inbox and history follow the session subject, never pane placement. Validate supplied launch correlation; bad correlation cannot fall back to manual adoption. Use one binding implementation, existing durable records and private verified launch recipes. Fence destructive controls and communication against the current granted attachment, including same-PID session changes.

Restart uses verified observed executable/argv/cwd/env and resumes the subject's current session without initial prompts or one-use credentials. Keep existing pane placement by default. A lead manually resumed elsewhere retains its links; show a minimal parent reference without moving panes or redesigning cross-tab nesting.

## Remove and simplify

Remove register/context/sweep adoption choreography for Pi management, permanent attempted sets, automatic duplicate promotion, marker readers/writers and projections, injected managed execution identities, compatibility-only alpha wire exports, and naming that calls a process registration a durable agent. Reuse existing publication scheduling and persistence helpers; do not add a second registrar in Herdsman or a separate enrollment pump.

## Implementation and migration

Use the existing atomic-file/transition storage with explicit recovery ordering, not an assumed cross-file transaction. Preserve unambiguous existing session subjects and their IDs/history/inboxes; ambiguous old mappings remain pending. Describe actual format and rollback constraints. Generated contracts change with the public DTOs. Reconcile pi-radar's SDK consumers and daemon together; consumers must adopt the new alpha contract.

## Presentation and adoption

Baseline-visible duplicates have a concise inline TUI warning distinct from stale process/binary warnings and no managed/inbox authority. pi-radar exposes standing through its process-shared runtime and clears cached subject/parent/communication handles at session boundaries. Herdsman consumes the SDK; it owns assignment validity, settlement and worker policy, not registration or transport. Update the existing companion brief with rollout status. Live Pi/tmux and Herdsman acceptance remain operator/consumer work, not implied by unit tests.

## Validation

A few composed flows prove acquisition/switch/release/resume, retained relationships/inbox, mutable location, duplicate stickiness across incumbent exit and daemon restart followed by fresh-load acceptance, and lost replies/delayed release fencing. Run existing Rust, SDK, schema, OpenSpec and Nix gates; no speculative fixture matrix.

## Deferred

No signing, transcript identity, takeover/history merging, assignment restoration, per-process durable streams, general incoming request transport, storage replacement or database migration. `/radar gather-children`, pane migration, richer cross-tab rendering and fork history policy remain deferred in `ideas.md`. The bounded shared event journal is notification transport; inbox records remain authoritative and recoverable after cursor expiry.
