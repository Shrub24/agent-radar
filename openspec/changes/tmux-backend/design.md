# Design

## Decisions

- **Dedicated tmux server.** The adapter targets a named socket (`tmux -L <name>`), never the user's default server. Selection is explicit configuration; the daemon refuses to start with an unknown backend.
- **Handles.** Locations use tmux's stable ids (`$session`, `@window`, `%pane`), qualified by backend and socket name. Labels and indexes are never identity.
- **Mapping.** session ≈ workspace, window ≈ tab, pane ≈ pane. Anything tmux cannot express is refused by an undeclared capability, never emulated.
- **Launch.** `split-window -d -P -F '#{pane_id}' -c <cwd> -e K=V … -- argv…`. Direct argv, no shell. The environment overlay uses `-e`; values never appear in diagnostics. Absolute cwd and token rules from `child-launch-binding` still apply.
- **Remain-on-exit.** Arm it before the child can exit, preferably as a launch-time pane option; the exact tmux 3.7 mechanism is verified in task 2.1. If it cannot be armed, launch refuses rather than losing the exit status.
- **Exit observation.** Read `#{pane_dead}`, `#{pane_dead_status}`, `#{pane_dead_signal}` and `#{pane_dead_time}` from `list-panes`. Polled with the same inventory read; `pane-died` hooks are an optimisation, not required. Observation is recorded once on the spawn edge and is idempotent.
- **Honest absence.** Dead pane → typed exit. Pane gone with no recorded exit → unresolved. Server gone → unknown. Close of a dead pane is `child.close`, unchanged, and still verifies containment; a dead pane has no foreground process, so the birth-identity gate is satisfied by the recorded exit and exact pane id, not by procfs.
- **No resume/stop.** Exit observation is physical fact only; it never completes an assignment or alters execution publications.
- **Herdr parity.** Herdr keeps typed-line launch; its exit observation is always `unknown`.

## Risks

- tmux 3.7 option and format names must be verified against the installed binary in the smoke, not assumed.
- Dead-status reads race pane reuse only if ids are reused; tmux pane ids are not reused within a server lifetime.
- A killed tmux server loses all panes; recovery is out of scope.

## Open questions

- Whether a dead pane's close needs an explicit foreground-gate variant (proposed: exact pane id plus recorded exit).
