# Changelog

Radar is alpha: breaking changes are expected and bump the minor version. The wire revision (`radar --version` prints it) is the compatibility contract with pi-radar; the version is for people.

## Unreleased

- Removes every compatibility path for state written by earlier builds. A spawn edge must be version 2 with a session parent, an exit observation and a bound `registration_id`; the `agent_id` and `bound_agent_id` read aliases, the pre-migration `IdentityMapped` / `IdentityUnresolved` / `Imported` lifecycle transitions, `execution_generation`, `identity_unresolved`, fingerprint-only operation records, per-execution alias admission and the registration-less `bind_spawn` are gone. Run `radar reset --hard` once after installing; older state is refused rather than adapted.

## pi-radar 0.10.1

- Fixes `children.list`, `observe`, `inspect` and the assignment follow, which refused as unsupported: `spawnList` pinned a `spawn.list.v1` contract no daemon advertises. It now requires `registry.v1`, which serves `spawn.list`.

## 0.5.0

- The daemon withdraws a launch that has not presented within 30 seconds: it revokes the claim, stops a captured process through its scope, closes the launch's pane and records `withdrawn` (`at`, `reason`) on the spawn edge. The subject is then free to continue. Applies to `L`, spawn, continue and restart.
- `lifecycle.restart` answers with `launch_request_id`, the edge of the replacement.
- A subject's authority pointer is the one check behind reserving and granting a claim. The separate `successor` field is gone from stored records, events and the observation read.
- Wire revision 8 — restart the daemon with pi-radar 0.10.0, which drops `timeoutMs` from `launchRadarWorker`, reports `failed` instead of `refused`, and adds `children.restart`.

## 0.4.0

- Every tracked launch (`L`, restart, spawn) starts `radar run --launch` in its pane; the daemon answers the claim with the program from its own record and the process enters its scope. `launch-bootstrap` and `spawn.bootstrap` are gone.
- `spawn.create` accepts `continue_subject_id`: a parent starts its own released child again in the session the daemon recorded, with any session flag in the caller's argv replaced. `lifecycle.restart` also accepts a parent naming itself with the program to restart as, unfenced by claim; the operator's restart keeps its claim fence. Wire revision 7 — restart the daemon with pi-radar 0.9.0.

## 0.3.1

- A spawn edge's `bound` record names the child's `subject_id` beside its registration, so a consumer can address a child by the agent that survives its restart. Additive: the wire revision stays 6. pi-radar 0.8.0 needs this daemon, since it refuses a launch whose edge binds without a subject.

## 0.3.0

- A managed launch pre-mints its process identity: the launch passes `PI_RADAR_REGISTRATION_ID`, and its claim is `reserved` (no authority) until the first verified report replaces it with the session's own claim. Process evidence belongs to the registration. Wire revision 6 — restart the daemon with pi-radar 0.7.0.
- Every tracked launch (`L`, restart, Herdsman) runs in a systemd user scope, and `radar run -- COMMAND` starts one in the current pane. Managed stops go through the scope, are graceful only (`SendSIGKILL=no`, 2s) and name survivors. Observed agents keep their controls and are marked.
- `radar run --agent -- PROGRAM` launches an agent; `radar run -- COMMAND` is a managed command (a scope, no subject, no identity in its environment). Home Manager `programs.radar.managedAgents` and `lib.managedAgent` wrap an agent so it starts under `radar run --agent`.

## 0.2.0

- Sessions attach through one verified presence claim; manually started Pi sessions are adopted without an operator step.
- The agent is published as `subject_id` and an attachment as `claim_id`; `execution_id` and `execution_generation` are gone. Wire revision 5 — restart the daemon with pi-radar 0.6.0.
- `D` is the single delete key and ends nested agents gracefully before closing a place.
- `radar --version`, a clap command line, and shell completions.
