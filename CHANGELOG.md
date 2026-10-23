# Changelog

Radar is alpha: breaking changes are expected and bump the minor version. The wire revision (`radar --version` prints it) is the compatibility contract with pi-radar; the version is for people.

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
