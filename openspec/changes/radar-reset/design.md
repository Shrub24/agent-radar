## Context

Radar's control state root contains registrations, lifecycle subjects and claims, process records and recipes, spawn edges, requests/operations, closes, publications, communication records and events. These have different lifetimes. A registration, claim attachment, process recipe, spawn attempt, operation, publication and event describe current or historical execution state; they are resettable. A subject is the durable logical agent. Its ID and parent relationship survive process/session replacement. Handoffs and inbox items are addressed to a subject and are durable across restarts.

The reset must remove legacy/transient state without accidentally dropping identities or communication. It must also stop current Radar-managed processes before their ownership records are cleared, and restart the tmux server Radar serves to clear stale pane topology.

## Decisions

### 1. Reset clears transient state and retains durable subject state
By default, keep each durable subject's ID, its durable Pi-session association, and subject-owned metadata including the parent-subject relationship. Keep the communications store unchanged; since the default retains every subject, valid handoffs and inboxes remain addressed to their owners without scanning or rewriting them. Drop current/last claim pointers, claims, registrations, process recipes and scope evidence, spawn edges and launch history, operations, closes, publications, events and other ephemeral control state. The next process for the retained session must establish a fresh attachment through normal presence. No process or claim is reconstructed from old history.

The implementation uses the lifecycle subject-record writer to clear attachment pointers without losing durable session identity. Communications are outside the lifecycle store and are preserved as a unit on the default path.

### 2. Hard reset also forgets durable identities
`radar reset --hard` discards subject records, their relationships and all communications, in addition to the default reset. This is the explicit way to remove the durable layer. Both forms show their exact scope before confirmation.

### 3. Managed processes stop before state is cleared
The command uses Radar's existing graceful scope-stop path, with its two-second timeout and no SIGKILL escalation. If the user manager is unreadable, a scope cannot be verified, or any process survives, reset refuses before clearing records or restarting tmux. It never signals a process by PID outside a verified Radar scope.

### 4. Restart only the Radar-served tmux server
After managed scopes have stopped and state has been cleared, restart the tmux server configured for this Radar instance. This deliberately removes its panes and layout and clears stale topology. Do not target another tmux server or modify the user's configuration. Reset does not promise graceful shutdown for arbitrary non-Radar processes inside panes; those are subject to the normal tmux server shutdown semantics and are called out in the preview.

### 5. Orchestrate daemon, scope, state and tmux lifecycle
`radar reset` is an external orchestrator, not an RPC that kills its own daemon. It previews the default or hard-reset plan and requires explicit confirmation. It first stops the control socket and daemon, preventing concurrent writes. With the daemon stopped, the reset process reads the persisted process records, verifies every active Radar scope against its registration and birth identity, and gracefully stops them. An unreadable manager, unknown active Radar scope, unverifiable membership or survivor refuses before state cleanup or tmux restart; on this refusal it restarts the daemon against unchanged state. It selectively clears transient state (and durable state for `--hard`), restarts only the configured tmux service, and starts the daemon service against the clean state. If the daemon was not running, it still starts it at the end. It never stops an unrelated tmux server.

Each phase reports its result. If a failure occurs after state cleanup, the command reports exactly which phases completed and can be safely retried; it does not restore deleted transient state or claim success. The state is destructively cleared in place; there is no backup or moved copy. Confirmation is required because the operation stops managed processes and the tmux server, losing panes/layout and potentially terminating arbitrary pane processes.

## Risks

- A preserved subject keeps its Pi-session association but has no claim or process authority after reset. The next process for that session must prove a fresh claim; preserving the ID does not grant authority.
- Restarting tmux can terminate arbitrary pane processes. The preview must make this consequence explicit.
- A failure between clearing durable categories and restarting tmux can leave a partially completed reset; report completed steps, and make a second invocation safe to finish.
- Messages addressed to removed subjects must not remain as unrouteable records after `--hard`.

## Confirmation

Confirmation is explicit `--yes`; without it, the command only previews and does not prompt or mutate state.
