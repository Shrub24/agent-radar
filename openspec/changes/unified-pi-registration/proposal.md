## Why

Manual Pi sessions require explicit enrollment despite reporting the process evidence Radar needs. Subject identity currently follows execution lineage rather than conversation continuity: manually resuming the same session can lose Radar inbox/child continuity, while switching sessions in one process can retain unrelated relationships. Session breadcrumbs add another identity path without solving that mismatch.

## What Changes

- Define a managed Pi subject as the durable Radar identity of one tracked Pi session UUID. Registrations/executions are its current process attachments; pane and tab are placement.
- Replace register/context/sweep adoption with one flat presence report used for session start, renewal and reconnect. Radar internally separates immutable process registration, mutable location/context and durable session-load claims. Herdsman adds no registrar.
- Rename the process-record identity to `registration_id` with a breaking alpha contract update, not compatibility aliases. Persist granted/duplicate claim decisions; graceful shutdown releases the exact grant, while verified process death is the crash backstop.
- Reattach a fresh uncontested process to an existing session subject. `/resume` in the same process changes the active subject, not the process registration. Children, inbox and history remain with their session subjects.
- Keep duplicate live sessions baseline-visible but unmanaged, with a warning. An incumbent's exit does not promote a duplicate: require fresh session loading or process restart before reconsideration.
- Validate supplied launch correlation; a mismatch cannot fall back to adoption. Retain daemon-side effect fencing and logical parent authority without requiring Pi-persisted execution identity.
- Remove active session-marker reads/writes, marker-only checks and separate enrollment policy paths. No transcript breadcrumbs, dummy subject IDs, cryptographic receipts or legacy alpha wire compatibility.
- Standardize private restart recipes on verified observed process context; resume the current subject's session without replaying initial prompts or one-use credentials.
- Retain subject/child links when a lead manually resumes elsewhere. No automatic pane movement or cross-tab tree redesign; explicit gathering is deferred in `ideas.md`.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `agent-registration`: session-bound subjects, unified verified attachment, automatic adoption, duplicate-session refusal, mutable session selection and private observed restart recipes.

## Impact

Daemon registration/context, lifecycle binding and reconciliation, execution admission, registry projection, communication identity, pi-radar reporter/shared runtime, generated contracts consumed by that runtime, minimal TUI warnings/link display, and registration/lifecycle documentation. Herdsman must consume the current pi-radar association rather than cache a process-lifetime subject.

This supersedes the mandatory breadcrumb design in the shipped but unarchived `lifecycle-authority` change. Preserve durable operation/execution history and adapt unambiguous existing session associations non-destructively; do not silently merge ambiguous subjects or inboxes. No new dependencies, storage replacement, general recovery framework, assignment-authority transfer or pane-movement feature.

See `scope.md` for the agreed behavior, risks and deferred work; `design.md` defines the implementation boundaries and `tasks.md` the delivery slices.
