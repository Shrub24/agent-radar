# Mux integration contract

This page describes the physical seam used by the Radar daemon and the boundary
that downstream consumers should depend on. For exact JSON, limits and examples,
use [control-plane protocol v1](control-plane.md). For agent identity, publications
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

The production adapter is currently Herdr. `Daemon::bind` selects `HerdrRuntime`;
there is no production tmux adapter or daemon physical-backend selector yet.
Radar's `[runtime] backend = "daemon"` selects its *client transport*, not a tmux
backend. The default `direct` client mode and its startup fallback still use
Herdr. Backend selection must be added explicitly when tmux is implemented.

## Physical operations

| Daemon method | Adapter operation | Current guarantee |
| --- | --- | --- |
| `observe` | `inventory` | Normalized workspace/tab/pane and agent observations; no new registration or daemon-created ownership |
| `process_info` | `foreground_evidence` | Current foreground evidence; inconclusive is a valid answer, not proof of exit |
| `focus` | `focus_outcome` | Focus the exact named location; distinguish confirmed, refused and unknown effects |
| `create` | `create` | Workspace, tab or pane split; report the created identity rather than choosing a nearby resource |
| `input` | `input` | Literal text or named keys; no agent lifecycle interpretation |
| `output` | `output` | Bounded, non-consuming output snapshot; no scrollback/cursor consumption |
| `report` | `report` | Optional forwarding to backend metadata; not direct registry publication |
| `close` | `close_outcome` | Physical pane/tab close after the daemon's positive-unmanaged containment and identity gate |
| `spawn` | `validate_launch`, `create`, `launch` | Daemon-managed child creation/launch with durable edge and private registration correlation |
| `child.close` | `inventory`, `foreground_evidence`, `close_outcome` | Close only the recorded, token-bound child's pane after exact current occupant verification |

`launch` is an adapter capability used by `spawn`, **not** a public standalone
launch endpoint. `create` only creates a terminal location. `create` plus arbitrary
`input` is not a substitute for `spawn`: it does not mint the private binding
token or create a managed-child edge.

Current `spawn` placement is a nonfocused right split of the exact registered
parent's pane. Although generic workspace/tab creation exists, managed spawn
into a new tab/workspace, or a caller-selected existing pane, is not implemented.
Consumers needing that placement must request a contract extension, not assemble
an untracked managed launch from generic operations.

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
- Do not derive assignment completion, readiness or process exit from terminal
  disappearance, execution status, pane closure or a successful input call.

The daemon persists accepted work before effectful dispatch. Clients retain the
operation UUID and reconcile durable readbacks; they do not fall back to another
adapter after uncertainty. Read the protocol references for the separate generic
operation, spawn-edge and managed-close record shapes.

## Launch and binding

A context-bearing spawn requires compatible protocol plus `creation`, `launch`
and `child_launch_context_v1`. It names the exact parent `agent_id` and a fresh,
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
reconciliation. `child.close` is a pane-close fact, never an exit or assignment
completion verdict. Stored registration launch/resume specifications are not a
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
| Loss/exit watching | Replace Herdr event subscriptions with daemon-backed observation/reconciliation or a separately specified event feed; distinguish lost observation from confirmed exit |
| Retained-worker stop/resume/restart | Specify and implement physical stop/exit verification and replacement/resume binding while Herdsman keeps run/result/mailbox policy |
| Backend detection and dashboard state | Remove direct Herdr environment/alias assumptions; wire registry facts into Radar's fleet view and make daemon/server selection explicit |

The first row can migrate now for the supported parent-split launch flow.
Retained-worker resume and the other currently used behaviors are necessary for
*full* Herdsman parity, but do not prevent starting the tmux adapter. A native
backend can solve launch transport without waiting for a Herdr shell-ready API.
Worktree/project choice remains caller policy; pass its selected cwd to spawn.
Keep assignment/result guards and post-close reconciliation in Herdsman.

## Second-backend acceptance

A tmux adapter should first satisfy the supported operation matrix above, not
reimplement every Herdr feature. Add explicit daemon backend/server selection and
keep server-scoped location identity unambiguous. Test its adapter boundary and
run the existing daemon/publisher contracts through it: literal launch context,
private exact binding, observation, input/output, focus and verified child close
with durable replay and safe cleanup.

Then prove the consumer cutover with Herdr unavailable: Herdsman delegates,
inspects, receives results and closes a daemon-created child without Herdr CLI,
metadata or aliases. Radar also needs a registry-backed observation path and
backend-neutral location detection before a tmux-only fleet can use it fully.
Resume/recovery and richer placement are separate missing contracts where the
consumer actually requires them; do not claim those from a successful spawn.
