# Radar daemon: consumer guide

Start here when adopting the daemon from Herdsman or another local client. This
page describes the available surface and how its parts fit together. Exact v2
wire schemas and bounds live in the linked references; the architecture and
migration sequence here are a working direction, not a permanent division of
responsibility. The [mux integration contract](mux.md) describes the physical
adapter seam, capability guarantees and second-backend boundary.

## What is available

The daemon has two independent surfaces on one local control socket:

| Surface | Use | Reference |
|---|---|---|
| Physical mux adapter | Observe terminal locations/process evidence; focus, create, input, read output and guarded unmanaged close | [Control-plane protocol](control-plane.md) |
| Direct agent registry | Register agent incarnations; publish execution, assignment and session facts; read current snapshots and process verification | [Agent registration](agent-registration.md) |

The physical backend defaults to Herdr; the daemon can also select the tmux
adapter explicitly with `RADAR_CONTROL_BACKEND=tmux` and a named
`RADAR_TMUX_SERVER`. Registry calls work without a mux backend and do not publish
through Herdr. The older mux `report` method still forwards state/session/metadata
to the backend; it is **not** direct registry publication.

The daemon can create and launch a child through `spawn.create`, recording a
durable edge to its named parent. Name both the exact parent agent UUID and
`parent_session_id`; fresh context on that registration must match. A shared
session UUID never selects among live registrations.

Spawn accepts optional absolute cwd and an environment overlay, requiring the
`spawn.launch_context.v1` contract (`mux.create`, `mux.launch` and
`mux.launch_context`). Herdr quotes a single launch line and refuses
unrepresentable values before creation. Public operation readbacks omit
environment values. The child captures/removes the private
`PI_RADAR_SPAWN_TOKEN` and registers with it; new bindings persist the returned
`agent_id`. Read `result.spawn.bound.agent_id` from flat `spawn.get` and use that
exact ID with `agent.get`, not aliases or a nonexistent `spawn.edge` object.
Legacy bindings may omit the ID. New v2 edges retain process and session parent
identities; v1 edges have no inferred session parent. Current v2 topology also
reports `exit_observation`: tmux can persist a typed pane-command exit after
arming `remain-on-exit`; Herdr and backends without `mux.exit_observation` report
unknown. Missing panes do not imply exit, and exit evidence never changes
assignment or execution publications.

For children bound to that edge, `spawn.close` closes only the edge's recorded
pane, and only after verification. A live occupant must match the registered
child's process birth identity. A pane whose command has exited is cleaned up as
`already_exited`, but only when the edge recorded that exact exit and the
process serving the backend now is the one whose verified birth identity the
daemon recorded before the create: a pane id outlives the server that answered
the create. An edge with no recorded creating instance — a legacy edge, or an
edge on a backend that cannot name its serving process, such as Herdr — is never
fenced, so its dead panes are refused rather than inferred. `already_exited`
confirms retained-pane cleanup and the pane command's recorded exit; it never
proves the registered process exited and never authorizes relaunch. Neither path
is a process stop or assignment transition, and neither writes execution or
assignment facts. Both fence reads run on the daemon's shared verification
budget — four concurrent verifier jobs daemon-wide, each abandoned at a
three-second deadline, polled so a stop request is honoured — so a stalled
verifier leaves an unfenced edge instead of holding the worker answering a
caller, and verification is deliberately not atomic with dispatch (a same-UID
process could respawn the pane in the window between them). The daemon does not
make itself a lifecycle manager: there is no resume, restart, recovery,
or adoption of panes launched outside the daemon. A created pane is not a bound
child until that child registers with the private spawn token. Private
launch/resume specifications stored on registrations remain inert unless
explicitly used by a supported operation.

## Dashboard consumption

The Radar TUI is a registry consumer. With a daemon selected, it declares
`registry.v1` and `spawn.v1`, reads the daemon's pane inventory plus paged
`agent.list` and `spawn.list`, and joins each registration to a pane by its
backend-qualified location and the exact `process.claim` birth identity of that
pane's own foreground process. A daemon that omits either contract fails the
refresh with that diagnostic rather than showing a fleet from another source.
Herdr's agent records, its display metadata and its native agent status are never
display facts: a registered row is named by its registration and shows the
execution, assignment and facts channels, each with its own provenance and
freshness. A registration at a pane also protects it — and the tab containing it
— from a generic unmanaged close, joined or not; `x` is routed to the owner where
the pane carries the lineage that route needs, and refused otherwise. Direct,
with no daemon, the TUI lists ordinary panes only. No agent fact in the dashboard
comes from a Herdr token once a daemon serves the registry.

## Connect and negotiate

Run `radar daemon` separately; it prints the socket and state paths. It serves
until SIGINT/SIGTERM and removes only its own socket on shutdown. Radar does not
start or supervise it.

Clients connect directly using Unix-domain UTF-8 JSON Lines, protocol version
`2`, with a canonical UUID request ID and one matching response per request.
Send `daemon.ping` first and check the served protocol and the contracts your
client needs: `registry.v1` for a publisher (plus `registry.facts.v1` before publishing the `facts` channel or assignment extension), `events.v1` for a journal reader,
`spawn.v1` and `spawn.close.v1` for managed children. Do not require mux
capabilities for a registry-only publisher. The handshake states what this build
serves; it cannot prove which build is running, so treat an absent contract or an
`unknown_method` answer as the signal to keep your previous path.

Default socket resolution: `RADAR_CONTROL_SOCKET`, then
`$XDG_RUNTIME_DIR/agent-radar/control.sock`, then
`/tmp/agent-radar-<uid>/control.sock`. This is separate from the background-task
bus socket. See the [protocol reference](control-plane.md#start-and-select) for
state-path resolution, Radar's `[runtime]` client transport, and the daemon's
physical-backend selection.

Before connecting, check that the socket and its parent are not symlinks, the
parent is a user-owned real directory with mode `0700`, and the socket is a
socket owned by that user. Filesystem trust is not authentication between
processes running as the same user. Do not expose this socket remotely.

## API map

| Purpose | Methods | Capability |
|---|---|---|
| Handshake | `daemon.ping` | None |
| Physical observations | `mux.observe`, `mux.process_info`, `mux.output` | Respective mux capability |
| Physical mutations | `mux.focus`, `mux.close`, `mux.create`, `mux.input`, `mux.report`, `spawn.create` | Respective mux capabilities (`spawn.create` also `mux.launch`) |
| Mutation readback | `mux.request.get`, `mux.request.list` | None |
| Durable lifecycle events | `events.read` | `events.v1` |
| Register a subject | `agent.register` | `registry.v1` |
| Acquire or explicitly replace a channel writer | `agent.acquire` | `registry.v1` |
| Publish/retire a channel | `agent.publish`, `agent.retire` | `registry.v1` |
| Session facts and assignment extension | `agent.acquire`, `agent.publish` on channel `facts`, or extended assignment fields | `registry.v1`; negotiate `registry.facts.v1` |
| Update current session context | `agent.context` | `registry.v1` |
| Public registry reads | `agent.get`, `agent.list` | `registry.v1` |
| Spawn topology reads | `spawn.get`, `spawn.list` | `registry.v1`; location confirmation additionally uses `mux.observe` when available |
| Close a daemon-created child pane | `spawn.close` | `spawn.close.v1` (`mux.observe` + `mux.close`); dead-pane cleanup additionally needs `mux.exit_observation` and a fenced edge |

Use `events.read` to resume ordered journal pointers with bounded long polling; it does not replace current-state reads. `cursor` is the position after the last delivered event, so following it never skips an event, while `tail` and `more` say whether a backlog remains: while `more` is true, read again immediately instead of waiting. On `cursor_expired`, resynchronise using paged `agent.list` and `spawn.list`, plus `mux.request.list` for operation records, then resume from the returned cursor. Events may be lost between fact durability and append, so an unclean restart changes the epoch. Absence of an event is not evidence of freshness or liveness.

Use the method references for parameter and result shapes rather than treating
this table as a second schema. Unsupported methods/capabilities are not an
invitation to synthesize an equivalent operation with terminal input.

## Agent and publisher lifecycle

1. **Register the subject.** Supply immutable identity/configuration, optional
   location and process birth identity, and optional private launch information.
   Preserve the registration content for identical reconnect retries. The
   returned `agent_id` identifies this record, not a session or pane.
2. **Acquire a writer.** Choose `execution` or `assignment` and provide the
   publisher's source/incarnation and, for an owner projection, reporting owner.
   Preserve the returned binding and sequence. The publisher is separate from
   the subject; restarting a publisher does not require inventing another agent.
3. **Publish complete snapshots.** Keep activity, waiting reason, last outcome
   and advertised actions distinct. Use a newer sequence for changed facts and
   heartbeats. Identical sequence/content replay is idempotent, not a lease
   renewal. Unknown vocabulary is preserved; field shapes and sizes are checked.
4. **Reconnect explicitly.** Repeat identical registration and reacquire the
   current writer. A lost publish reply can be retried with exactly the same
   sequence/content. After daemon restart, persisted reports are stale until a
   newer report is accepted, even if their old lease timestamps look recent.
5. **Retire or replace a writer.** Retirement ends that writer's ability to
   update the channel; it does not stop the process. Replacement requires the
   exact incumbent generation/handle and retirement or lease expiry. A fenced
   writer must stop and reconcile, not automatically take the channel back.

A new subject/process incarnation gets a distinct registration even if it
shares a Pi session UUID or reuses a pane. A publisher replacement leaves the
subject record intact, and accepted old facts retain their original provenance.
See [registration and reconnect semantics](agent-registration.md) for details,
including how an acquired but unreported writer is handled.

Use `agent.context` for an in-process session switch/fork or explicit UUID/null
update. It has its own fenced writer and sequence; it does not change process
identity, launch authority or immutable registration. Never-published context and
explicitly cleared session are distinct. No session path is published here.

Public reads omit writer handles and private launch executable/argv/cwd/session
paths. They expose launch availability/revision, publication provenance and
freshness, and separate process verification. `fresh` does not mean alive;
`stale` does not mean exited. Process verification is local birth-identity
evidence, not a decision that restarting is safe.

## Physical operation lifecycle

Effectful mux requests have durable records. Keep the UUID for each operation
and use `mux.request.get`/`mux.request.list` for generic mux operation readback
after a lost reply. For `spawn.create`, also read its durable edge through
`spawn.get`. `spawn.close` has a separate record: repeat its identical method,
UUID and params to retrieve the stored outcome without redispatch; generic
`mux.request.get`/`mux.request.list` do not index it. An unknown or
unsettled outcome must not be retried under a new UUID or through another
adapter. Same-ID replay retrieves the existing record rather than repeating the
effect. Registry publication uses its own identity/sequence rules; it does not
share the mux mutation lane.

`mux.close` accepts only positively unmanaged targets with a frozen identity that
the daemon rechecks, and refuses any target a registration names, including one made
after the operator confirmed. `spawn.close` is separate: it requires a daemon-authored
spawn edge with a bound child, that child's process identity claim, a fresh pane
and containment match, and then either an exact foreground birth-identity match
for a live occupant or, for a retained dead pane, the edge's own recorded exit,
a fresh dead read and the creating serving-process fence. It requires the
`spawn.close.v1` contract (`mux.observe` and `mux.close`), and dead-pane cleanup
additionally needs `mux.exit_observation`. Foreground evidence is an adapter-side
process read: evidence the adapter cannot provide refuses here rather than being
assumed from pane presence. It refuses before recording or dispatch if any
preflight evidence, capability or fence is missing. It closes only that pane,
never the process group, and never changes the spawn edge, registration,
assignment or execution publications.

The `spawn.close` request id is its durable operation identity. The target pane
and intent (`complete` or `cancel`, audit/display labels only) are recorded
before dispatch. A confirmed mux close is `completed`; a cleaned-up retained pane
whose recorded command had exited is `already_exited`; a positive backend
pre-dispatch refusal is `refused`; and any possibly-dispatched close without
confirmation is `unknown`, answered as `outcome_unknown`. Only a close that
happened returns a result: `refused` and `outcome_unknown` come back as error
objects, so an uncertain answer can never be read as a completed close — and
`outcome_unknown` must never be read as "nothing was dispatched", because the
pane may already be closed. Same-id identical replay returns the stored result without
redispatch; changed content under that id is refused. Never retry an unknown
result under another id or route it through the direct adapter: the pane may
already be closed. `already_exited` confirms retained-pane cleanup with the pane
command's recorded exit; it does not prove the registered process exited, and it
does not authorize relaunch. `unknown` and later missing-pane topology remain
unresolved, not evidence of process exit or assignment completion. See the exact
[method contract](control-plane.md#managed-child-close) and its
[registration/topology context](agent-registration.md#managed-child-close).

## Herdsman adoption

The first port can be small:

- Register lead/worker subjects directly and retain their daemon identities.
- Publish Herdsman's existing owner projection on `assignment`; publish Pi
  execution evidence separately on `execution`.
- Keep optional Herdr metadata mirroring only where an existing consumer needs
  it. Registry publication itself needs no Herdr commands or token parsing.
- Publish the session's own `facts` channel (model, provider, thinking, context
  usage, session name, background tasks) where a dashboard consumer needs it.
  Adoption is additive: a dashboard shows the assignment extension and `facts`
  rows as unavailable until a publisher writes them, and nothing else changes.
- Route child creation through `spawn.create` only after `daemon.ping` confirms
  protocol 2 and the `spawn.v1` contract (the backend's `mux.create` and
  `mux.launch` capabilities), plus `mux.launch_context` when supplying
  cwd/environment. Capture/remove the token in the child and use the returned
  bound `agent_id` for subsequent registry reads. Binding is not application or
  assignment readiness. The server states which contracts it serves; the method
  names themselves are not advertised, so attempt `spawn.create` once after
  those checks. An `unknown_method` response means this daemon does not implement
  the method; keep the existing Herdsman path. Never fall back to another launch
  path after an uncertain/unknown spawn outcome, since the child may already have
  been created or launched.
- Until Herdsman deliberately adopts the managed-close method, its existing
  lifecycle contract is unchanged. `spawn.close` can target only a child bound to
  a daemon-authored spawn edge; registration or advertised actions do not make
  another managed target eligible. It closes a verified live pane or a fenced
  retained dead pane whose edge recorded the exit; neither case is a
  process-exit verdict. A close intent is an audit label: the
  assignment owner still decides and publishes completion.

Today, Pi supplies execution evidence, Herdsman supplies assignment/run facts,
and the daemon supplies physical observations/controls plus opt-in managed
spawn and verified child-pane close operations. This is the starting seam, not a
prohibition on consolidating more lifecycle work later. Spawn creates and
launches; registration with its token establishes the child identity.
`spawn.close`
closes only the verified pane; it does not certify child-process exit. Neither topology freshness
nor a missing pane establishes process exit or authorizes lifecycle action.
Resume, restart, recovery, and adoption of foreign panes are not provided;
recovery of children, pending asks and results must be specified and verified
separately.

## Examples and compatibility

- [Reconnecting owner publisher](../examples/agent-publisher/publisher.py)
- [Reconnecting events consumer](../examples/agent-publisher/events_consumer.py)
- [Registry and spawn exchanges](agent-registration.fixture.jsonl)
- [Mux exchanges](control-plane.fixture.jsonl)

Run `cargo build --locked && python3 tests/agent_publisher.py` for the disposable
daemon smoke. It replays registry and spawn fixture exchanges; spawn uses a
scripted backend to verify create, launch, token binding and redaction without a
live Herdr session. Its close exchanges cover the live-pane path and its same-ID
replay; the dead-pane `already_exited` path needs a retained pane and a fenced
edge, and a scripted Herdr backend can never fence one, so that path is covered
by the isolated disposable-tmux tests instead.
`python3 tests/tmux_acceptance.py target/debug/radar` is the dashboard
acceptance: it runs the TUI against a disposable daemon on a disposable tmux
server, registers agents whose panes run real foreground processes, and asserts
the registry-backed rows, the refusal of a generic close for a registration, and
that no Herdr call is made. It needs no Herdr, and it leaves no server, pane,
daemon or socket behind, on success or failure. The publisher example itself
remains a registry-publisher
reference and does not launch children. It retains publisher identity for one
process lifetime; a production publisher must define durable reconnect state.
The real Herdr launch smoke belongs to the independent final verification gate;
there is no live user-fleet integration.

Consumers should negotiate capabilities, preserve unfamiliar vocabulary, and
switch on error codes rather than human-readable messages. Keep wire-version
compatibility explicit as the API grows. Transport guarantees, identity fencing,
privacy and uncertain-effect handling are requirements of the current contract;
state vocabulary, placement policy, richer topology and recovery responsibilities
can evolve without turning this overview into a fixed domain model.
