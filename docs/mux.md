# Mux integration contract

This page describes the physical seam used by the Radar daemon and the boundary
that downstream consumers should depend on. For exact JSON, limits and examples,
use [control-plane protocol v2](control-plane.md). For agent identity, publications
and token binding, use [agent registration](agent-registration.md). Start with
[the daemon consumer guide](daemon.md) if you are integrating Herdsman.

## Two interfaces, different consumers

Extensions and dashboards use the daemon's Unix socket. They must not depend on
Herdr command names, socket schemas, aliases or agent-kind tables.

Backend implementations use the internal Rust `RuntimeProvider` trait in
`src/runtime.rs`. This is the adapter seam, not another public protocol or a
promised stable third-party Rust API. The daemon owns durable operation records,
spawn correlation and target admission; the adapter performs physical effects
and reports their certainty. Assignment decisions remain outside the adapter.

The daemon's physical adapter is tmux on the server named by `[daemon] tmux_server`
(default `radar`); `RADAR_CONTROL_BACKEND` and `RADAR_TMUX_SERVER` override the file. Herdr is
selected only with `RADAR_CONTROL_BACKEND=herdr`. An unknown backend or missing server name refuses
startup, with no fallback. The adapter uses only `tmux -L <name>` and does not touch the
default server. The server is started and verified by its own user unit. The
dashboard has no direct mode and no Herdr fallback.

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
out-of-band pane respawn is inside the existing same-UID trust boundary. Herdr
reports unknown. A missing pane is not proof of exit, legacy v1 edges omit the
field, and exit observation does not change agent execution/assignment facts. It
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
`mux.launch_context`). It names the exact parent `agent_id` and a fresh,
matching `parent_session_id`; duplicate attaches are not selected by UUID alone.

The request supplies a resolved executable and literal argv, optional absolute
cwd and an environment **overlay**, not replacement of the inherited environment.
Radar reserves `PI_RADAR_SPAWN_TOKEN`, passes its private one-use token to the
child, and consumes it when that exact child registers. New bindings expose the
actual registration `agent_id`; old bindings may lack that field.

Use flat `spawn.get` readback: `result.spawn.bound.agent_id`, not
`result.spawn.edge.bound.agent_id`. Then read `agent.get` for that exact ID.
Binding says which registered child belongs to this creation; it does not say
that the application is ready to receive an assignment. Execution freshness,
process verification and owner-assignment facts are separate evidence.

Herdr transports a rigorously quoted single shell line in the created pane.
Newlines/control values and over-limit lines refuse before creation. This is not
a documented shell-ready acknowledgement. A future direct-argv adapter can use
its native launch facility behind the same seam, but must prove that it preserves
cwd, overlay values and literal argv and retains honest uncertain-effect handling.

Environment values and tokens are redacted from public operation/topology
readbacks and daemon launch diagnostics. That is not a sandbox or general output
scrubber: shell/application output can contain sensitive data, and arbitrary
terminal input/argv should not be assumed private. The daemon is a same-UID local
service; filesystem permissions do not isolate it from other same-user processes.

## Unsupported lifecycle operations

No daemon operation currently provides process stop with verified exit,
retained-session resume, lead restart, child replacement or post-owner-restart
reconciliation. `spawn.close` is a pane-close fact, never an exit or assignment
completion verdict, and `already_exited` is the pane command's recorded exit plus
retained-pane cleanup rather than proof about the registered process. Stored
registration launch/resume specifications are not a
promise that the daemon can execute them.

The TUI-only lead-restart change is separately blocked on positive readiness for
a fresh Herdr pane. Completing it is not a prerequisite for implementing a tmux
adapter, and native tmux launch must not be disguised as a Herdr-ready guarantee.

## Herdsman cutover checklist

Direct registry publication has landed separately in pi-extensions. Its physical
client still uses Herdr; replacing publication alone does not complete the cutover.
The current callers are in `pi-extensions/pi-herdsman/extension/herdr.ts`, with
retained-worker close/restart orchestration in `extension/index.ts`.

| Remaining behavior | Required change |
| --- | --- |
| Launch, inspect, focus, input/output and eligible child pane close | Adopt daemon calls, exact binding and capabilities; stop invoking Herdr CLI or resolving aliases |
| Agent-kind launcher | Resolve executable/argv and required environment in Herdsman; the daemon does not own agent-definition policy |
| Startup readiness | Preserve an explicit Pi/Herdsman application-ready predicate; token binding alone is insufficient. A positive Herdr shell-ready contract is still absent |
| Placement parity | Add managed-spawn placement where Herdsman actually requires new-tab/existing-pane/custom-anchor behavior; generic create is not a managed launch |
| Loss/exit watching | Consume resumable daemon events and page spawn topology; typed pane exit is currently available only for tmux-backed v2 spawn edges, while Herdr and missing panes remain unknown |
| Retained-worker stop/resume/restart | Specify and implement physical stop/exit verification and replacement/resume binding while Herdsman keeps run/result/mailbox policy |
| Session facts (model, provider, thinking, context usage, session name, background tasks) | Publish to the `facts` channel after negotiating `registry.facts.v1`; which reporter writes it is pi-extensions' decision |
| Backend detection and dashboard state | Done in Radar: daemon/server selection is explicit and the fleet view reads registry facts (see [dashboard consumption](daemon.md#dashboard-consumption)). What remains is Herdsman's own direct Herdr environment and alias assumptions in its launch and inspect paths |

The first row can migrate now for the supported parent-split launch flow.
Retained-worker resume and the other currently used behaviors are necessary for
*full* Herdsman parity, but do not prevent starting the tmux adapter. A native
backend can solve launch transport without waiting for a Herdr shell-ready API.
Worktree/project choice remains caller policy; pass its selected cwd to spawn.
Keep assignment/result guards and post-close reconciliation in Herdsman.

## Tmux backend status

The daemon now supports explicit Herdr/tmux physical-backend selection, named
server-scoped tmux handles, normalized observation and mux operations, direct-argv
spawn launch, durable child binding, and typed one-shot exit observation on v2
spawn edges. The adapter's launch requires `remain-on-exit` before dispatch;
uncertain effects remain unknown. `spawn.close` closes a live child pane after the
foreground-identity gate, and closes a retained dead pane as `already_exited`
only when the edge recorded that exit and the tmux server serving now is the one
that served the create; Herdr cannot name its serving process, so its dead panes
are always refused. Herdr remains the default and reports unknown exit
observations.

Record reads accept the raw separator byte observed on tmux 3.7c and the literal
`\037` separator observed on Debian tmux 3.5a. Only field boundaries are decoded;
the final display field stays as tmux sent it, including literal backslashes.
This avoids general escape decoding, which would alter display text. These checks
establish record-format compatibility, not full lifecycle acceptance on 3.5a.

The registry-backed dashboard and Herdr-free tmux acceptance are shipped. Full
Herdsman cutover still needs its own consumer migration and real Pi/Herdsman
acceptance with Herdr unavailable; dashboard acceptance does not establish it.
Resume/recovery and richer placement remain separate contracts; do not claim
them from a successful spawn.
