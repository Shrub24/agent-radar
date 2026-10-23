# Spec Delta

## Purpose
Let the operator start Radar from empty state deliberately, knowing what is discarded, without hand-deleting records and without touching tmux.

## ADDED Requirements

### Requirement: Reset is explicit and shows its effect first
`radar reset` SHALL, without confirmation, report by category the records it would set aside and the consequence of each, and SHALL change nothing.

#### Scenario: Dry run
- **WHEN** the operator runs `radar reset` without `--yes`
- **THEN** Radar lists the categories with counts, states that handoffs, inboxes, subjects and history would be discarded, and the state is unchanged

### Requirement: Reset never races a running daemon
`radar reset` SHALL refuse while the daemon answers on its control socket.

#### Scenario: Daemon running
- **WHEN** the daemon is running
- **THEN** the command refuses, names the daemon, and changes nothing

### Requirement: Reset sets state aside rather than destroying it
On confirmation, `radar reset` SHALL move the control state root to a timestamped sibling in one atomic step so the daemon starts empty, and SHALL keep the moved state until the operator purges it.

#### Scenario: Fresh start
- **WHEN** the operator confirms a reset and starts the daemon
- **THEN** the registry, spawn edges, operations, inboxes and history are empty and the previous state remains on disk

#### Scenario: Purge
- **WHEN** the operator runs `radar reset --purge`
- **THEN** only previous reset directories are removed

### Requirement: Reset does not orphan running processes
`radar reset` SHALL refuse while a Radar scope has members, naming them. With `--stop-scopes` it SHALL stop them through the graceful stop with no SIGKILL escalation and SHALL refuse to proceed if any process survives. When the user manager cannot be read it SHALL refuse.

#### Scenario: Populated scope
- **WHEN** a Radar scope has live members and `--stop-scopes` is absent
- **THEN** the command refuses and names each scope and its members

#### Scenario: A survivor
- **WHEN** a process ignores TERM during `--stop-scopes`
- **THEN** the command reports it and does not set state aside

#### Scenario: Unreadable manager
- **WHEN** the user manager cannot be queried
- **THEN** the command refuses rather than treating the scopes as absent

### Requirement: Reset does not touch tmux or configuration
`radar reset` SHALL NOT stop, restart or change the tmux server, any pane or window, the user's configuration, or any process outside a Radar scope.

#### Scenario: tmux is untouched
- **WHEN** a reset completes
- **THEN** the tmux server, its panes and windows are as before

### Requirement: Handoffs can be kept
`radar reset --keep communications` SHALL leave the handoff store in place and set every other category aside.

#### Scenario: Keep handoffs
- **WHEN** the operator resets with `--keep communications`
- **THEN** undelivered handoffs remain readable and the remaining state is set aside
