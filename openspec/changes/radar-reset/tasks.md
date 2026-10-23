## 1. Command

- [ ] 1.1 Add `radar reset` with a dry run that lists categories, counts and consequences and changes nothing; verify a test that a dry run leaves the state root byte-identical
- [ ] 1.2 Refuse while the control socket answers; verify the refusal names the running daemon and changes nothing
- [ ] 1.3 Move the state root aside atomically on `--yes` and leave the daemon able to start empty; verify the moved root keeps every record and a fresh daemon reports an empty registry

## 2. Scopes

- [ ] 2.1 List Radar scopes with their members and refuse when any is populated; verify the refusal names them and an unreadable manager refuses rather than counting as none
- [ ] 2.2 With `--stop-scopes`, stop them through the existing graceful stop and refuse to continue if a process survives; verify no SIGKILL is sent

## 3. Handoffs and purge

- [ ] 3.1 Add `--keep communications`; verify the handoff store is left in place and everything else is set aside
- [ ] 3.2 Add `--purge` for previous resets; verify it removes only reset directories

## 4. Documentation

- [ ] 4.1 Document what a reset discards, by category, in `docs/managed-execution.md` and the README, replacing the "explicit reset" placeholder; verify the commands run as written
