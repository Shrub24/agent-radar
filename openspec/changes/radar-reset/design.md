## Context

The control state root holds `agents` (registrations), `lifecycle` (subjects, claims, process records, private recipes), `spawns` (parent edges), `requests` (operations), `closes`, `publications` (context and facts), `communications` (handoffs and inboxes) and `events`. Records reference one another by id with no shared transaction, so removing part of them leaves dangling references the daemon then reports as unresolved. A reset is therefore all-or-nothing per state root.

## Decisions

### 1. Offline, and the state is moved, not deleted
The command runs without the daemon. It refuses when the control socket answers, so no writer races it. The state root is renamed to a sibling `control.reset-<UTC timestamp>` in one atomic step, and the daemon recreates an empty root. The moved directory is kept until `--purge`, which removes previous resets. A reset is thus reversible by moving the directory back, and the operator keeps provenance of what was discarded.

### 2. Dry run by default
Without `--yes` the command prints, by category, how many records would be set aside and states the consequence of each (handoffs and inboxes are discarded; subjects are forgotten, so a resumed session registers as new; operations and history are gone) and exits without changing anything.

### 3. Scopes are stopped first or the reset refuses
Records are the only link from a scope back to its owner. Clearing them while a scope has members leaves running processes Radar can no longer name. The command lists `radar-*.scope` units in the user manager. If any has members it refuses and names them, unless `--stop-scopes` is given, in which case it uses the existing graceful stop (TERM, two seconds, no SIGKILL) and refuses to proceed if any process survives. A reset never escalates.

### 4. Boundaries
It does not touch tmux (no server stop, no pane or window change), the user's configuration, or processes outside Radar scopes. An unreadable user manager is not "no scopes": the command refuses and says why.

### 5. Categories can be kept
`--keep communications` leaves the handoff store in place for the operator who must not lose undelivered handoffs; everything else is set aside. Other categories cannot be kept alone, because they reference each other.

## Risks

- A subject forgotten by a reset means the next report from a still-open Pi session creates a new subject. The dry run says so.
- Resetting with a scoped process alive but the user manager unreachable cannot be proven safe, so it refuses.

## Open decisions

- Whether `--stale` ships in this change or after it.
- Whether the moved state is pruned automatically after a period or only by `--purge`.
