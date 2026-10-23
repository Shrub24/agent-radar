# Managed execution

Radar can admit a command durably, run it inside a systemd user scope, and stop
it gracefully. A scope is one cgroup of processes; it is never an
application outcome. Exit status keeps coming from the typed tmux observation,
because systemd reports `success` for a nonzero exit and `inactive` for a
process that never started.

## Vocabulary

| Term | Meaning |
| --- | --- |
| registration | One process incarnation. Process evidence (recipe, birth identity, scope) belongs to it. |
| scope | The systemd cgroup a registration was launched into. Its unit and slice are named from the registration. |
| launch | The act of starting a registration. Not a stored record. |
| subject | The durable agent. |
| claim | One attachment of a registration to a subject's session. |

## Agents and commands

`radar run --agent -- PROGRAM` is an agent launch. The registration is exported
as `PI_RADAR_REGISTRATION_ID` so the session can present it, and the agent then
gets a subject, a claim, a parent edge and fenced controls.

`radar run -- COMMAND` is a managed command. It has a registration and a scope,
which `radar reset` stops. It has no subject, no claim and no session, so no read
reports it and no control reaches it yet; those wait for a consumer. Nothing is exported to its environment, so a
program it starts cannot present its identity, and a daemon asked to attach a
command's registration refuses.

## Starting one

```sh
radar run -- pi
```

`radar run` executes in the pane it was invoked from: no pane, window or
workspace is created, and no shell interprets the command. Arguments are passed
as written, so a `$VAR`, a glob or a newline reaches the program unchanged.

Before anything executes, `radar run` asks the daemon to admit the launch. The
daemon refuses unless the process is the named pane's own descendant and its
birth identity verifies, so a pane Radar cannot verify starts nothing. On
admission Radar records the process (its registration, recipe, scope) and
answers with the registration id. The command then runs as
`systemd-run --user --scope …`, so the scope exists before the command does and
a scope that cannot be created runs nothing.

`L` launches through the same path after choosing its tab, and so do restart
and Herdsman spawn. The pre-minted registration is handed to the child as
`PI_RADAR_REGISTRATION_ID`; pi-radar captures it and removes it before it
launches children.

## Names

One scope per process lifetime:

```text
radar.slice
  radar-<registration>.slice
    radar-<registration>-main.scope
```

The registration id is a UUID that is never reused, so a name cannot be
recycled and a lost reply cannot create a second scope under a fresh name. A
reload or `/resume` replaces the session's claim and leaves the scope alone;
nothing is renamed or migrated. Unit descriptions name the unit only — argv,
paths and environment never appear in unit metadata.

## Stopping

A managed stop is one `systemctl --user stop` for every scope in the set, so a
workspace with several agents waits one window rather than one per agent.
Managed scopes set `SendSIGKILL=no` and `TimeoutStopSec=2s`: Radar sends TERM
and never escalates on its own.

A stop whose scope still has members is reported as unresolved, with the
surviving programs named. The unit's `failed`/`timeout` state is expected there
and is not an outcome: a process that ignores TERM stays running and Radar says
so rather than claiming it exited. Independently scoped tasks and subagents are
outside a main-scope stop.

An observed agent — one started outside the managed launch path — keeps the
signal path it always had. It is not stopped through a scope it never entered.

## Observed and managed

Every Pi session keeps its durable subject, children, inbox and the controls
that need no scope, whether or not Radar launched it. A managed row also has a
scope, so its resource grouping is tracked; an observed row is marked and
offers no new scope-dependent capability. The route to managed is a restart
through `radar run`; there is no in-place promotion, and no capability is
removed from an observed session because it is unobserved.

## Requirements

- A systemd **user manager** for the user running the daemon. Without one,
  `systemd-run` and `systemctl --user` fail and managed launches refuse before
  the command runs — there is no unscoped fallback.
- **cgroup v2** with the unified hierarchy at `/sys/fs/cgroup`. Group
  membership and survivor inspection read `cgroup.procs` from there.
- `systemd-run` and `systemctl` on the daemon's `RADAR_SYSTEMCTL`/`PATH` as for
  any other operator command.

Platform behaviour that follows from this: a scope outlives a daemon restart, so
recovery reattaches to it instead of signalling anything; a transient unit that
has already been collected is reported as unknown rather than as a clean exit;
and closing the pane's PTY can still end an interactive program even while its
scope exists, so the two facts are reported apart.

## Resetting Radar state

`radar reset` previews its effects and requires `--yes` to apply them. It stops
the daemon, gracefully stops verified managed scopes (TERM, two seconds, no
SIGKILL), clears transient registrations, claims, process records, launch
history, operations and publications, restarts Radar's served tmux server, then
starts the daemon. The tmux restart removes its panes and layout; arbitrary pane
processes receive normal tmux shutdown behavior.

By default, subject IDs, their Pi-session associations, durable subject metadata
and parent relationships, and subject-addressed handoffs/inboxes survive. Claims
and process authority are cleared; the next process for a retained session must
establish a fresh claim. `radar reset --hard` also discards subjects and
communications. Reset does not
back up state. If a managed scope survives or cannot be verified, reset refuses
before clearing state. Run `radar reset` first to inspect counts and consequences.
