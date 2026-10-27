# Mux integration contract

This page describes the physical seam used by the Radar daemon and the boundary
that downstream consumers should depend on. For exact JSON, limits and examples,
use [control-plane protocol v2](control-plane.md). For agent identity, publications
and token binding, use [agent registration](agent-registration.md). Start with
[the daemon consumer guide](daemon.md) if you are integrating Herdsman.

## Two interfaces, different consumers

Extensions and dashboards use the daemon's Unix socket. They must not depend on
tmux command names, format strings or socket details.

Backend implementations use the internal Rust `RuntimeProvider` trait in
`src/runtime.rs`. This is the adapter seam, not another public protocol or a
promised stable third-party Rust API. The daemon owns durable operation records,
spawn correlation and target admission; the adapter performs physical effects
and reports their certainty. Assignment decisions remain outside the adapter.

The daemon's physical adapter is tmux on the server named by `[server] tmux_server`
(default `radar`); `RADAR_TMUX_SERVER` overrides the file. A missing server name
refuses startup, with no fallback. The adapter uses only `tmux -L <name>` and does
not touch the default server. The server is started and verified by its own user
unit. The dashboard has no direct mode: it reads the daemon and nothing else.

## Physical operations

| Daemon method | Adapter operation | Current guarantee |
| --- | --- | --- |
| `mux.observe` | `inventory` | Normalized workspace/tab/pane and agent observations; no new registration or daemon-created ownership |
| `mux.process_info` | `foreground_evidence` | Current foreground evidence; inconclusive is a valid answer, not proof of exit |
| `mux.focus` | `focus_outcome` | Focus the exact named location; distinguish confirmed, refused and unknown effects |
| `mux.create` | `create` | Workspace, tab or pane split; report the created identity rather than choosing a nearby resource |
| `mux.input` | `input` | Literal text or named keys; no agent lifecycle interpretation |
| `mux.output` | `output` | Bounded, non-consuming output snapshot; no scrollback/cursor consumption |
| `mux.report` | `report` | Optional forwarding to backend metadata; not direct registry publication |
| `mux.close` | `close_outcome` | Physical pane/tab/workspace close after the daemon's positive-unmanaged containment and identity gate |
| `spawn.create` | `validate_launch`, `create`, `launch` | Daemon-managed child creation/launch with durable edge and private registration correlation |
| `spawn.close` | `inventory`, `foreground_evidence`, `exit_observation`, `server_process`, `close_outcome` | Close only the recorded, token-bound child's pane: a live occupant after exact foreground-identity verification, or a retained dead pane after the edge's recorded exit and the creating-instance fence. `server_process` is not a daemon method: it is the seam by which the daemon fences a created edge to the process that served it |

`mux.launch` is an adapter capability used by `spawn.create`, **not** a public
standalone launch endpoint. `mux.create` only creates a terminal location.
`mux.create` plus arbitrary `mux.input` is not a substitute for `spawn.create`: it
does not mint the private binding token or create a managed-child edge.

Current `spawn.create` placement is a nonfocused right split of the exact registered
parent's pane. Although generic workspace/tab creation exists, managed spawn
into a new tab/workspace, or a caller-selected existing pane, is not implemented.
Consumers needing that placement must request a contract extension, not assemble
an untracked managed launch from generic operations.

`spawn.get` and `spawn.list` include `exit_observation` for current v2 edges.
`spawn.exit_observation.v1` (the backend's `mux.exit_observation`) means the
selected backend can report a retained typed pane
exit; absent that capability the field is `{"state":"unknown"}`. Tmux arms
`remain-on-exit` before launch and records the first observed exit code or signal
with Radar's observation time. This is tmux's retained pane-command status, not
independent proof of the bound agent's process identity. A same-UID client can
`respawn-pane` before Radar's first read and replace the retained status; direct
out-of-band pane respawn is inside the existing same-UID trust boundary. A pane
that keeps no exit status reports unknown. A missing pane is not proof of exit, and exit observation does not change agent execution/assignment facts. It
authorizes dead-pane cleanup only together with the edge's own recorded exit and
the creating-instance fence.

## Adapter duties

- Advertise only implemented capabilities. The default trait implementation
  advertises none; unsupported operations are not attempted. Capability absence
  must never trigger a speculative alternate effect.
- Preserve normalized identifiers and the structured identity returned by
  creation. Locations are opaque handles, not labels, pane positions or session
  UUIDs. Use the same daemon/backend context for observing and acting on them;
  current code does not define multi-server or simultaneous-backend routing.
- Validate representability before accepting context-bearing spawn. Common
  shape/bounds validation is followed by adapter transport validation, including
  encoded command size. Refuse before creation if launch inputs cannot be carried.
- Respect cancellation and command deadlines, clean up adapter-owned command
  processes, and do not leave detached work. Cancellation cannot undo a mutation
  already dispatched.
- Return `refused` only when the requested effect provably did not happen.
  Return `unknown` after possible dispatch without trustworthy confirmation.
  A lost input/create/close reply is not permission to repeat it.
- Provide foreground evidence that can be corroborated with Linux process birth
  identity when managed close needs it. Session tokens and titles do not prove
  which process incarnation occupies a pane.
- Name the process serving the backend's instance through `server_process` when
  the backend addresses one. It is a claim about a pid, and the daemon verifies
  its birth identity before recording or comparing it, but a backend that answers
  none gives the daemon nothing to fence with: the created edge records no
  instance and cleanup of that edge's dead panes refuses rather than guessing.
- Do not derive assignment completion, readiness or process exit from terminal
  disappearance, execution status, pane closure or a successful input call.

The daemon persists accepted work before effectful dispatch. Clients retain the
operation UUID and reconcile durable readbacks; they do not fall back to another
adapter after uncertainty. `events.read` with `events.v1` offers bounded polling
for lifecycle journal pointers, but does not replace exact record reads. Resume
from the delivered `cursor` — the position after the last delivered event — and use
`tail`/`more` only to see whether a backlog remains: while `more` is true, read
again without waiting. On
`cursor_expired`, rebuild from paged registry/topology reads and request listings;
then resume from the returned cursor. Events can be lost after a fact is durable,
so an unclean restart changes epoch. Read the protocol references for event,
generic operation, spawn-edge and managed-close record shapes.

## Launch and binding

A context-bearing `spawn.create` requires compatible protocol plus the `spawn.v1`
and `spawn.launch_context.v1` contracts (`mux.create`, `mux.launch` and
`mux.launch_context`). It names the exact parent `registration_id` and a fresh,
matching `parent_session_id`; duplicate attaches are not selected by UUID alone.

The request supplies a resolved executable and literal argv, optional absolute
cwd and an environment **overlay**, not replacement of the inherited environment.
Radar reserves `PI_RADAR_REGISTRATION_ID`, exports the launch's pre-minted registration id to the
child, and binds it when that exact child presents it. New bindings expose the
actual registration `registration_id`; old bindings may lack that field.

Use flat `spawn.get` readback: `result.spawn.bound.registration_id`, not
`result.spawn.edge.bound.subject_id`. Then read `subject.get` for that exact agent.
Binding says which registered child belongs to this creation; it does not say
that the application is ready to receive an assignment. Execution freshness,
process verification and owner-assignment facts are separate evidence.

The adapter transports a rigorously quoted single shell line in the created pane.
Newlines/control values and over-limit lines refuse before creation. This is not a
documented shell-ready acknowledgement, so readiness is the child's own report.

Environment values and tokens are redacted from public operation/topology
readbacks and daemon launch diagnostics. That is not a sandbox or general output
scrubber: shell/application output can contain sensitive data, and arbitrary
terminal input/argv should not be assumed private. The daemon is a same-UID local
service; filesystem permissions do not isolate it from other same-user processes.

## Lifecycle operations and pane ownership

`lifecycle.cancel`, `lifecycle.stop`, `lifecycle.restart` and `lifecycle.enroll`
operate on a launch this daemon tracked, and `spawn.close` remains a pane-close
fact, never an exit or assignment completion verdict. `already_exited` is the
pane command's recorded exit plus retained-pane cleanup rather than proof about
the registered process. Stored registration launch/resume specifications are not
a promise that the daemon can execute them.

A launch owns the pane it created. Ending the launch removes that pane: a cancel
signals the process, verifies the exit, and only then closes the pane the spawn
edge recorded, gated on the backend reporting that same pane dead and on the
process serving the backend still being the one that created it. A stop leaves
the launch and its pane as they are. A restart stops the predecessor, launches the
successor through the ordinary spawn path, and removes the predecessor's pane
afterwards — the successor is placed first because its pane is split from the
predecessor's. A pane is kept, with the reason in the operation's effects, when
the process may still be alive or the launch never created one.

A restart's readiness is positive evidence that the fresh process reported in, not
that its pane was created. Binding says which child belongs to the creation and
nothing about being ready for an assignment.

## Tmux backend status

The daemon serves a named, server-scoped tmux handle with normalized observation
and mux operations, direct-argv spawn launch, durable child binding, and typed
one-shot exit observation on spawn edges. The adapter's launch requires `remain-on-exit` before dispatch;
uncertain effects remain unknown. `spawn.close` closes a live child pane after the
foreground-identity gate, and closes a retained dead pane as `already_exited`
only when the edge recorded that exit and the tmux server serving now is the one
that served the create. An edge whose creating instance cannot be named is
unfenced, and its dead panes are refused rather than inferred.

Record reads accept the raw separator byte observed on tmux 3.7c and the literal
`\037` separator observed on Debian tmux 3.5a. Only field boundaries are decoded;
the final display field stays as tmux sent it, including literal backslashes.
This avoids general escape decoding, which would alter display text. These checks
establish record-format compatibility, not full lifecycle acceptance on 3.5a.

The registry-backed dashboard and tmux acceptance are shipped. A consumer's own
migration and real Pi acceptance remain its work; dashboard acceptance does not
establish them. Resume/recovery and richer placement remain separate contracts;
do not claim them from a successful spawn.
