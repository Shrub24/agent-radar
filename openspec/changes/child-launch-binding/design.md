# Design

## Context

See proposal.md for motivation. `SpawnParams` currently carries parent, parent_session_id, executable and argv. `LaunchRequest` adds a pane and the private daemon token. Herdr transports one quoted shell line via pane input. Registration already accepts `spawn_token`, binds an edge to source/incarnation, and fences repeated consumption. Public binding reads lack the exact registry agent_id.

The working tree contains the accepted, uncommitted session-aware spawn prerequisite from `managed-lead-restart`. This change uses it without revising restart policy. That change remains blocked on positive fresh-pane readiness. Extending today's child-spawn transport does not claim to resolve that blocker.

## Goals / Non-Goals

**Goals:** Carry the context Herdsman needs, make existing registration binding consumable by exact ID, and provide independently tested consumer examples.

**Non-Goals:** Pi-extensions deployment, a new owner-authority model, stop/restart, tmux implementation, expanded placement, foreign-child adoption, or an application-ready promise based on registration alone.

## Decisions

### 1. Extend the existing spawn request

Add optional `cwd` and `env` to the nested spawn request. `cwd` must be an absolute UTF-8 path; `env` is a bounded map of conventional variable names (`[A-Za-z_][A-Za-z0-9_]*`) to UTF-8 values. Empty values are meaningful. Reject NUL and terminal-control characters; Herdr additionally rejects newlines. Values must not be trimmed, expanded or converted lossily. Keep the existing aggregate request/record limits and a maximum of 256 environment entries.

Environment is an overlay: omitted fields preserve existing inheritance, supplied variables override it. This matches the current consumer's need to supply `PI_HERDSMAN_*` launch context; exact whole-environment capture for restart is not needed here. Do not hard-code Herdsman-specific names into the generic mux adapter. Reject `PI_RADAR_SPAWN_TOKEN` in the overlay; only the daemon supplies it.

**Alternative rejected:** adding a second launch endpoint or environment allow-list tied to one extension. That duplicates the owned spawn operation or embeds consumer vocabulary in the runtime seam.

### 2. Validate before creating; reuse the adapter

Extend the runtime launch request and give the selected adapter a side-effect-free launch-context validation path used before spawn admission. The Herdr path must establish that all supplied values and the encoded command fit its existing input limits before a pane is created. Reuse the same validation/building logic during dispatch to prevent drift; this preflight is representability, not a shell-readiness check.

Keep one quoted line using a guarded directory change and environment overlay invocation before the resolved executable. Quote each complete value as an atom; directory failure must prevent program execution. Preserve the inherited environment for both cwd-only and overlay requests. For each supplied overlay variable, use the existing POSIX `env` utility's unset-before-assignment form so the new value replaces any inherited/exported value, including an empty value; always set the daemon token explicitly. Do not clear unrelated inherited values. Preserve the existing create/location and no-retry semantics. No scripts, temporary secret files, delayed retries, interactive readiness probes, or direct process launch outside the mux.

**Alternative rejected:** silently omitting context or retaining a script until the shell might have consumed it. Neither preserves the request safely.

### 3. Advertise an additive feature

Advertise `child_launch_context_v1` only when the daemon handler and backend both implement this contract. Consumers check protocol compatibility plus `creation`, `launch` and the feature. No protocol-wide version change or false claim that this is application readiness. Old requests still work; unsupported extended fields refuse before effects.

### 4. Separate private equality from public readback

Keep complete accepted request content in existing bounded, mode-0600 durable state for same-ID comparison. Public spawn responses, request.get/list paths and diagnostics must omit environment values and tokens, including when backend stderr echoes the launch command. Use a single explicit public projection at the relevant convergence point rather than weakening stored equality or maintaining a second unbounded secret store. Verify typed/Debug/error paths do not accidentally expose values.

**Alternative rejected:** hashing or redacting before replay comparison. It could make two distinct environment requests look identical and would violate the existing exact-content rule.

### 5. Reuse registration; add exact-ID readback

When existing token registration succeeds, persist the returned agent_id with the bound source/incarnation. The existing recovery/idempotency path must retain the same binding if acknowledgement or persistence completion is interrupted. Make agent_id optional when reading old bound records, with no title/session-based reconstruction. New bound edges return it through spawn.get/list so the owner can read agent.get directly.

The child bootstrap example captures the environment token, removes it from the process environment, submits it privately with the child's source/incarnation and Linux birth claim, and retries uncertain registration identically. It retains the token only in memory while registration is unsettled. It does not persist it in Herdsman sidecars. Parent readback needs the edge ID, never the token.

Binding is not assignment readiness. The consumer reads source-labelled execution facts for the bound agent; no execution publication means readiness unknown. Herdsman retains its own assignment-admission decision. The separate Pi-extensions port must implement this bootstrap; Radar alone cannot make an old publisher consume the token.

## Risks / Trade-offs

- **Environment may contain secrets** → Private bounded records; sanitize every public operation/error path; test with a distinctive sentinel value.
- **Shell transport is backend-limited** → Reject unsupported values before creation and prove literal values/cwd through a disposable real Herdr child, not just a string comparison.
- **Herdr lacks positive fresh-shell readiness** → Do not promote launch acknowledgement to readiness. Report launched/bound/fresh execution separately; keep lead restart blocked. A live smoke establishes transport behavior for the tested setup, not a general readiness guarantee.
- **Two active changes touch spawn specs** → Keep this delta additive and preserve the parent/session prerequisite. Reconcile both before eventual sync/archive; no inferred v1 migration.

## Migration Plan

1. Implement optional context, pre-effect validation and privacy in the existing spawn path.
2. Add exact agent_id binding/readback while retaining old records.
3. Publish capability, fixtures and bootstrap example; run disposable daemon and gated real Herdr checks.
4. Hand the pinned contract to Pi-extensions for publisher/launcher adoption. Keep their old launch path when capability negotiation fails before any effect; never fall back after uncertain spawn.

Roll back consumers before the daemon if needed. Existing private records and legacy topology reads must remain readable; rollback never automatically recreates children.
