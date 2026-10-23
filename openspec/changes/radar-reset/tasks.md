## 1. Reset contract

- [x] 1.1 Add `radar reset` with a preview for default and hard modes; require explicit confirmation and verify an unconfirmed preview leaves state unchanged
- [x] 1.2 Stop the daemon before reading process state or draining scopes; on refusal, restart it against unchanged state
- [x] 1.3 Retain durable subject identity, Pi-session association, metadata/parentage and the communications store by default; clear claims and all process/launch/operation/publication/event state; verify a fresh claim reattaches the session to the same subject
- [x] 1.4 Add `--hard` to remove subjects, relationships and communications too

## 2. Stop managed processes and reset topology

- [x] 2.1 After daemon stop, enumerate Radar's active/failed/deactivating scope units, bind each name to its persisted registration record, verify scope ownership, then gracefully stop the set; refuse on unknown units, unverifiable membership or survivors, without SIGKILL
- [x] 2.2 Stop the daemon, drain scopes from persisted records, selectively clear state, restart only the configured tmux unit, then start the daemon; report each phase
- [x] 2.3 Make cleanup and service retry idempotent after partial completion; a retry must not restore old state

## 3. Documentation

- [x] 3.1 Document the default preserved/deleted state, hard reset, managed process stop, tmux restart consequence and daemon restart/start behavior in `docs/managed-execution.md` and the README

## 4. Operator acceptance

- [ ] 4.1 Run default and hard reset against the served fleet; verify scope stop, tmux replacement, daemon restart, retained subject/inbox behavior, and that a second tmux server is untouched
