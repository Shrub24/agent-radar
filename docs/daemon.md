# Radar daemon: consumer guide

Start here when adopting the daemon from Herdsman or another local client. This
page describes the available surface and how its parts fit together. Exact v2
wire schemas and bounds live in the linked references; the architecture and
migration sequence here are a working direction, not a permanent division of
responsibility. The [mux integration contract](mux.md) describes the physical
adapter seam, capability guarantees and second-backend boundary.

## What is available

The daemon exposes physical control, registry reads/publications, and volatile background-task detail. The versioned control socket serves the physical and registry APIs plus `tasks.list.v1`. Until `pi-bash-processes` migrates, its existing publisher protocol is accepted through a temporary compatibility socket entry into the same daemon transport; the TUI never owns that listener.

| Surface | Use | Reference |
|---|---|---|
| Physical mux adapter | Observe terminal locations/process evidence; focus, create, input, read output and guarded unmanaged close | [Control-plane protocol](control-plane.md) |
| Direct agent registry | Register agent incarnations; publish execution, assignment and session facts; read current snapshots and process verification | [Agent registration](agent-registration.md) |
| Background-task read | Read volatile task snapshots grouped by exact Pi session UUID | [Publisher bridge](radar-bus.md) |

The physical backend defaults to tmux on the server named `radar`. Herdr is
selected only explicitly with `RADAR_CONTROL_BACKEND=herdr`. The server name comes from
`RADAR_TMUX_SERVER` or the `[daemon] tmux_server` setting. The daemon runs the
executables named by `RADAR_TMUX` and `RADAR_SYSTEMCTL` (absolute paths in the
Home Manager unit, `tmux` and `systemctl` from `PATH` otherwise), so its unit
needs no `PATH`: tmux gives a new session its creator's `PATH`, and a unit with
a minimal one would give every pane that instead of the user's profiles. A pane
started before this change keeps its old `PATH`; make a new workspace. Registry calls work without a mux backend and do not publish
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
process could respawn the pane in the window between them). The daemon tracks the launches it makes: a captured
process is stopped or cancelled through its own verified path, a lead can be
enrolled as a restartable subject, and a recovery sweep re-observes every tracked
launch by verified identity. Process stop, cancellation and restart are their own
contracts (`lifecycle.*`) and claim nothing about assignments or registration. A
created pane is not a bound child until that child registers with the private
spawn token, and private launch/resume specifications stored on registrations
remain inert unless explicitly used by a supported operation.

## Stopping and refusals

`radar daemon stop` stops the control socket, then the daemon. While the socket is
stopped a connection is refused and nothing restarts the daemon, so a publisher
must treat a refused connection as the daemon being down.

`radar server stop` and `radar server restart` are refused, with no signal sent, while the
tmux server holds a pane (live or retained) or while a durable operation is
unfinished. The refusal names the panes and operations.

Known limit: an operation whose record never completes, for example after a
daemon crash, keeps the server refused while the daemon listens. The unfinished
records are readable with `mux.request.list` and `"unfinished": true`. No command clears them yet.

## Dashboard consumption

The Radar TUI is an optional display client. It declares `registry.v1` and `spawn.v1`, reads the daemon's pane inventory plus paged `agent.list` and `spawn.list`, and joins each registration to a pane by its backend-qualified location and the exact `process.claim` birth identity of that pane's own foreground process. It reads background-task snapshots through `tasks.list.v1` and joins them only by exact Pi session UUID; it does not bind publisher sockets or ingest messages. A daemon that omits either contract fails the
refresh with that diagnostic rather than showing a fleet from another source.
Herdr's agent records, its display metadata and its native agent status are never
display facts: a registered row is named by its registration and shows the
execution, assignment and facts channels, each with its own provenance and
freshness. A registration at a pane also protects it — and the tab containing it
— from a generic unmanaged close, joined or not; managed controls use daemon lifecycle methods and refuse rather than falling back when the daemon is unavailable. No agent fact in the dashboard comes from a Herdr token once the daemon serves the registry.

## Connect and negotiate

The socket unit starts `radar daemon` on the first connection, or the operator runs it
in the foreground for development. It prints the socket and state paths, serves
until SIGINT/SIGTERM, and removes no socket it did not bind itself. Radar's user
units start and stop it; see [Stopping and refusals](#stopping-and-refusals).

Clients connect directly using Unix-domain UTF-8 JSON Lines, protocol version
`2`, with a canonical UUID request ID and one matching response per request.
Send `daemon.ping` first and check the served protocol and the contracts your
client needs: `registry.v1` for a registry publisher (plus `registry.facts.v1` before publishing the `facts` channel or assignment extension), `events.v1` for a journal reader, `tasks.list.v1` for background-task snapshots, and `spawn.v1`/`spawn.close.v1` for managed children. Do not require mux
capabilities for a registry-only publisher. The handshake states what this build
serves; it cannot prove which build is running, so treat an absent contract or an
`unknown_method` answer as the signal to keep your previous path.

Default control socket resolution: `RADAR_CONTROL_SOCKET`, then `$XDG_RUNTIME_DIR/agent-radar/control.sock`, then `/tmp/agent-radar-<uid>/control.sock`. The daemon temporarily also binds the legacy publisher socket path described in [the publisher bridge](radar-bus.md); it is an ingest-only compatibility entry into the same transport and task state. See the [protocol reference](control-plane.md#start-and-select) for state-path resolution, the `[daemon]` table, and physical-backend selection.

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
| Volatile task snapshot | `tasks.list` | `tasks.list.v1` |
| Register a subject | `agent.register` | `registry.v1` |
| Acquire or explicitly replace a channel writer | `agent.acquire` | `registry.v1` |
| Publish/retire a channel | `agent.publish`, `agent.retire` | `registry.v1` |
| Session facts and assignment extension | `agent.acquire`, `agent.publish` on channel `facts`, or extended assignment fields | `registry.v1`; negotiate `registry.facts.v1` |
| Update current session context | `agent.context` | `registry.v1` |
| Public registry reads | `agent.get`, `agent.list` | `registry.v1` |
| Spawn topology reads | `spawn.get`, `spawn.list` | `registry.v1`; location confirmation additionally uses `mux.observe` when available |
| Close a daemon-created child pane | `spawn.close` | `spawn.close.v1` (`mux.observe` + `mux.close`); dead-pane cleanup additionally needs `mux.exit_observation` and a fenced edge |
| Container reads | `topology.get`, `topology.list`, `topology.reconcile` | `topology.get.v1`/`topology.list.v1`; `topology.reconcile.v1` additionally needs `mux.observe` |
| Container controls | `topology.create`, `topology.delete`, `topology.reload` | `topology.create.v1` (`mux.create`), `topology.delete.v1` (`mux.close`), `topology.reload.v1` (`mux.launch` + `mux.exit_observation`) |
| Tracked launch controls | `lifecycle.cancel`, `lifecycle.stop`, `lifecycle.enroll`, `lifecycle.restart`, `lifecycle.get`, `lifecycle.list` | `lifecycle.*` contracts; `lifecycle.enroll` also needs `mux.observe` |

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

## Containers and their controls

A container is the daemon's own durable identity for a place in the hierarchy: a
workspace, a tab or a pane slot. It carries an id, a parent link, Radar's own
label and its provenance, and it keeps its backend binding — backend name,
verified serving process incarnation, raw handle — separate from that identity. A
handle is evidence, never the identity: a handle reappearing under a different
serving incarnation is not the recorded container, and a container the daemon
removed names nothing at all.

Read them with `topology.get` (one id), `topology.list` (paged, `next_after`) and
`topology.reconcile` (one bounded pass that records per-container observations).
Reconciliation reports drift — present, missing, replaced, unavailable — with the
time it was observed and a source label. It never rewrites a binding, adopts a
foreign handle, deletes a record or dispatches an effect.

Three controls change containers. Each takes one caller-chosen request id, is
recorded before it is answered, and is read back by that id:

- `topology.create` (`topology.create.v1`, needs `mux.create`) makes a tab in a
  workspace or splits a pane, and records the container under the label the
  caller gave it. An optional `recipe` — a command with its cwd and environment
  — is accepted for a pane creation only: it is preflighted against the backend,
  held in the daemon's owner-only store, and run in the created pane by the
  daemon itself, with no spawn token, so the child is not a spawn child. The
  container record says only that a recipe is held; the command and its
  environment appear in no record, read or event.
- `topology.delete` (`topology.delete.v1`, needs `mux.close`) removes a container
  and everything recorded inside it. The request carries the container id and the
  place and occupant the operator confirmed; the daemon refuses when the
  container is unknown, already removed, bound to another place, recorded under
  another serving instance, or when a live managed worker is registered inside
  what it would remove — that refusal names each worker and the stop-then-delete
  sequence. The close itself is the same guarded dispatch `mux.close` uses, and
  the removals are recorded only after the runtime confirms the close. A
  deletion claims no process exit, no unregistration and no assignment outcome.
- `topology.reload` (`topology.reload.v1`, needs `mux.launch` and
  `mux.exit_observation`) runs a pane container's recorded recipe again in its own
  pane slot. It requires the slot's previous command to be provably gone — a
  typed retained exit the backend reports now — and refuses when that command is
  still running or its exit cannot be established, naming the stop-then-reload
  sequence. It records its own outcome, changes no container, and is not a Pi
  session restart, a new container or an assignment fact.

`topology.create` is the same creation body as `mux.create`, which keeps its
primitive semantics and takes no Radar annotations. A repeated request id is
answered with the container it already made and dispatches nothing; the same id
with different content is refused. The dashboard drives these controls through
one socket and one handshake, keeps each operation's request id in its outcome
line, and never falls back to a second transport.

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
