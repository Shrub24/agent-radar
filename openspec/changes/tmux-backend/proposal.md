# Proposal

## Why

Under Herdr, Radar launches a child by typing one quoted shell line into a pane. The shell is replaced by the child, so nobody can capture an exit code, and a vanished pane is indistinguishable from an exit. A tmux backend launches direct argv, needs no shell-readiness guess, and can keep a dead pane that reports its exit code or signal. It is also the first step of removing the Herdr dependency.

## What Changes

- Add a production tmux `RuntimeProvider` implementing observation, focus, create, input, output, guarded close and launch against one explicitly named tmux server. Locations are backend-qualified opaque handles.
- Add an explicit daemon physical-backend selector (`herdr` stays the default). A daemon uses one backend per process; no simultaneous-backend routing.
- Launch children with direct argv and the cwd/environment overlay from `child-launch-binding`, with the spawn token in the child's environment. No typed shell line, no readiness heuristic. The daemon sets `remain-on-exit` on panes it launches.
- Report a typed physical exit observation for daemon-launched panes: `exited{code}` or `exited{signal}`, with observed time. Absence of a dead-pane report stays unknown. A vanished pane is still unresolved, never an exit.
- Keep closing a dead pane a separate explicit act through the existing guarded `child.close`.
- Under Herdr, exit stays unknown; the daemon reports that explicitly rather than guessing.
- Spawn uses one implicit `child` launch profile. Named profiles are deferred.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `mux-control-plane`: backend selection, tmux adapter guarantees, exit observation and its capability.
- `managed-child-spawn`: exit observation readback on spawn topology for supporting backends.

## Impact

Touches `src/runtime.rs`, a new tmux adapter module, daemon backend selection and startup, spawn/topology projections, protocol and `docs/mux.md` references, plus a gated disposable-tmux smoke. No new dependency; tmux 3.7c is installed. OSC 7501 capture in tmux, Herdsman migration and tmux window/session layout are out of scope.
