# Spec Delta

## Purpose
Clear Radar's transient execution and topology state while retaining durable agent identities and their communication, with an explicit hard reset when those identities must also be removed.

## ADDED Requirements

### Requirement: Reset previews its destructive scope
`radar reset` SHALL show, by category, what it will preserve, discard and stop. It SHALL NOT change anything without explicit confirmation. `--hard` SHALL preview that durable subjects, relationships, handoffs and inboxes will also be discarded.

#### Scenario: Default preview
- **WHEN** the operator runs `radar reset` without confirmation
- **THEN** Radar lists transient categories to be purged, durable subjects and communications to be kept, managed scopes to be stopped, and the served tmux server to be restarted; state is unchanged

#### Scenario: Hard preview
- **WHEN** the operator requests `radar reset --hard` without confirmation
- **THEN** Radar also lists subject identities, session associations, relationships, handoffs and inboxes as discarded; state is unchanged

### Requirement: Reset coordinates daemon lifecycle
`radar reset` SHALL stop the control socket and daemon before reading process state or stopping scopes. It SHALL verify and gracefully stop active Radar scopes from persisted process records, then clear the selected state, restart the configured tmux server and start the daemon against the reset state. If the daemon is already stopped, reset SHALL still perform the state and tmux reset and start the daemon afterward.

#### Scenario: Reset with daemon running
- **WHEN** the operator confirms reset while the daemon is running
- **THEN** Radar stops the socket and daemon, drains verified scopes from persisted records, clears the selected state, restarts the served tmux server, and starts the daemon in that order

### Requirement: Default reset retains durable subject data
On confirmed default reset, Radar SHALL retain each subject ID, its Pi-session association, durable subject-owned metadata, parent-subject relationship and the communications store.

#### Scenario: Preserve durable state
- **WHEN** the operator confirms a default reset
- **THEN** each subject and its session association remain, addressed communications remain available, and transient claims and process records are removed

### Requirement: Default reset removes transient execution state
On confirmed default reset, Radar SHALL remove claims and process attachments, registrations, process and scope evidence, spawn edges and launch history, operations, closes, publications, events and other transient control state. A retained subject SHALL keep its Pi-session association but have no process attachment or authority until a fresh presence claim is granted.

#### Scenario: Reattach after reset
- **WHEN** a retained subject's Pi process reports presence after reset
- **THEN** Radar resolves the retained session to its existing subject, requires a fresh claim and restores authority only after normal verification

### Requirement: Hard reset removes durable subject data
On confirmed `radar reset --hard`, Radar SHALL also remove subject IDs, subject relationships, handoffs and inboxes.

#### Scenario: Forget all subjects
- **WHEN** the operator confirms a hard reset
- **THEN** no Radar subject or subject-addressed communication remains

### Requirement: Managed processes stop before their records are cleared
Reset SHALL stop verified Radar-managed scopes using the existing graceful stop with no SIGKILL escalation after stopping the daemon and before clearing state. If the user manager is unreadable, scope membership cannot be verified, or a process survives, reset SHALL refuse before clearing state or restarting tmux.

#### Scenario: Managed scope survives
- **WHEN** a process ignores TERM during reset
- **THEN** Radar reports the survivor, leaves Radar state and tmux topology unchanged, and restarts the daemon against unchanged state

#### Scenario: Scope evidence unavailable
- **WHEN** Radar cannot query or verify a managed scope
- **THEN** reset refuses rather than treating the scope as absent

### Requirement: Reset restarts only Radar's served tmux server
After stopping managed scopes and clearing the selected state, reset SHALL restart the tmux server served by this Radar instance before starting the daemon. It SHALL NOT target unrelated tmux servers or change user configuration. The preview SHALL warn that arbitrary processes in panes are subject to normal tmux shutdown behavior.

#### Scenario: Stale topology cleared
- **WHEN** a reset completes
- **THEN** the served tmux server has restarted with no old panes or layout, and an unrelated tmux server is untouched

### Requirement: Reset reports partial completion
If a service restart fails after Radar state has been cleared, the command SHALL report which phases completed and SHALL NOT claim full success.

#### Scenario: Tmux restart fails
- **WHEN** the tmux restart fails after state cleanup
- **THEN** the command reports the failed step and does not start the daemon against an unreset tmux server

#### Scenario: Daemon restart fails
- **WHEN** the daemon fails to start after state cleanup and tmux restart
- **THEN** the command reports that state and tmux were reset but the daemon remains stopped

### Requirement: Reset is repeatable after partial completion
A subsequent reset after state cleanup but failed tmux restart SHALL be able to restart the served tmux server without requiring deleted process records or restoring old state.

#### Scenario: Retry topology restart
- **WHEN** reset is retried after a partial completion
- **THEN** it finishes the tmux restart without resurrecting purged state
