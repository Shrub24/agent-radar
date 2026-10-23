## Why

Radar starts a managed process three ways. `L`, spawn and restart type `radar launch-bootstrap -- PROGRAM` into a pane and release it through `spawn.bootstrap`. An operator's `radar run` is admitted through `launch.admit` and wraps its own scope. Restart rebuilds its argv from the captured kernel recipe with `resume_launch`. Herdsman has no way to continue an existing child at all: it spawns a fresh one with its own `--session <path>`, Radar mints a placeholder subject for it, and the first presence report stamps the session onto the placeholder, so the session's agent loses its children and inbox.

One launch chain and one relaunch path remove the duplication, and a declared continuation closes the hole without Radar guessing what an argv means.

## What Changes

- **`radar run` is the only launch chain.** A launch the daemon creates (`L`, spawn, restart, continue) starts its pane with `radar run --launch`, which claims the recorded launch with the launch's correlation secret and its own birth identity. The daemon answers with the registration and the program from its private record, and `radar run` wraps it in the registration's scope and execs it, as the operator path already does. `launch-bootstrap` and `spawn.bootstrap` are removed.
- **One relaunch path.** `relaunch(subject, program, args, placement)` starts an existing agent again: it stops a live incumbent first, takes the session from the agent's record and appends `--session <id>`, and places the process in the agent's own pane when it still exists, otherwise beside its parent.
- **Two entry points, by authority.** The operator's `lifecycle.restart` relaunches with the captured program and argv. A parent's `spawn.create` with `continue_subject_id` relaunches its own child with the program and args it supplies; Herdsman's idle-worker restart and `agent_continue` both use it.
- **Per-launch assignment facts.** Owner, run, label and role are recorded with each launch; the parent link stays on the subject and never changes.
- Wire revision 7.

## Non-goals

- Radar minting session ids, storing session file paths, or writing markers into session files. Pi owns sessions; `--session <id>` fails closed when one is missing.
- Parsing or refusing session flags on a new launch. A client that passes one is wrong, and Radar does not defend against it.
- Merging extra flags into a recorded argv.
- Adopting a running process into a scope.

## Impact

New capability `managed-launch`. Code: `src/control_plane/lifecycle.rs` (`resume_launch` split, bootstrap removal, launch metadata), `src/control_plane/server.rs` (spawn, restart, the launch claim), `src/cli.rs` and `src/main.rs` (`radar run --launch`, `launch-bootstrap` removed), `src/managed_scope.rs`. pi-radar: `launchRadarWorker` gains `continues`. Breaking: revision 7, and a daemon must be restarted with the release. `pi-radar-child-control` depends on this change.
