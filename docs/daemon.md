# Radar daemon: consumer guide

Start here when adopting the daemon from Herdsman or another local client. This
page describes the available surface and how its parts fit together. Exact v1
wire schemas and bounds live in the linked references; the architecture and
migration sequence here are a working direction, not a permanent division of
responsibility.

## What is available

The daemon has two independent surfaces on one local control socket:

| Surface | Use | Reference |
|---|---|---|
| Physical mux adapter | Observe terminal locations/process evidence; focus, create, input, read output and guarded unmanaged close | [Control-plane protocol](control-plane.md) |
| Direct agent registry | Register agent incarnations; publish execution and assignment facts; read current facts and process verification | [Agent registration](agent-registration.md) |

The physical backend is currently Herdr. Registry calls work without a mux
backend and do not publish through Herdr. The older mux `report` method still
forwards state/session/metadata to the backend; it is **not** direct registry
publication.

The daemon can create and launch a child through `spawn`, and records a durable
edge to its named parent. This does not make the daemon a lifecycle manager:
there is no stop, resume, restart, recovery, or adoption of panes launched
outside the daemon. A created pane is not a bound child until that child
registers with the private spawn token. Private launch/resume specifications
stored on registrations remain inert unless explicitly used by a supported
operation. The current Radar TUI still uses its existing observation path;
registering an agent is not yet a way to make it appear there.

## Connect and negotiate

Run `radar daemon` separately; it prints the socket and state paths. It serves
until SIGINT/SIGTERM and removes only its own socket on shutdown. Radar does not
start or supervise it.

Clients connect directly using Unix-domain UTF-8 JSON Lines, protocol version
`1`, with a canonical UUID request ID and one matching response per request.
Send `ping` first and check the protocol and capabilities needed by your client.
Do not require mux capabilities for a registry-only publisher.

Default socket resolution: `RADAR_CONTROL_SOCKET`, then
`$XDG_RUNTIME_DIR/agent-radar/control.sock`, then
`/tmp/agent-radar-<uid>/control.sock`. This is separate from the background-task
bus socket. See the [protocol reference](control-plane.md#start-and-select) for
state-path resolution and Radar's `[runtime]` adapter selection.

Before connecting, check that the socket and its parent are not symlinks, the
parent is a user-owned real directory with mode `0700`, and the socket is a
socket owned by that user. Filesystem trust is not authentication between
processes running as the same user. Do not expose this socket remotely.

## API map

| Purpose | Methods | Capability |
|---|---|---|
| Handshake | `ping` | None |
| Physical observations | `observe`, `process_info`, `output` | Respective mux capability |
| Physical mutations | `focus`, `close`, `create`, `input`, `report`, `spawn` | Respective mux capabilities (`spawn`: `creation` + `launch`) |
| Mutation readback | `request`, `requests` | None |
| Register a subject | `agent.register` | `agent_registry` |
| Acquire or explicitly replace a channel writer | `agent.acquire` | `agent_registry` |
| Publish/retire a channel | `agent.publish`, `agent.retire` | `agent_registry` |
| Public registry reads | `agent.get`, `agent.list` | `agent_registry` |
| Spawn topology reads | `spawn.get`, `spawn.list` | `agent_registry`; location confirmation additionally uses `observe` when available |

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

Public reads omit writer handles and private launch executable/argv/cwd/session
paths. They expose launch availability/revision, publication provenance and
freshness, and separate process verification. `fresh` does not mean alive;
`stale` does not mean exited. Process verification is local birth-identity
evidence, not a decision that restarting is safe.

## Physical operation lifecycle

Effectful mux requests have durable records. Keep the UUID for each operation
and use `request`/`requests` to read it back after a lost reply. An unknown or
unsettled outcome must not be retried under a new UUID or through another
adapter. Same-ID replay retrieves the existing record rather than repeating the
effect. Registry publication uses its own identity/sequence rules; it does not
share the mux mutation lane.

`close` currently accepts only positively unmanaged targets with a frozen
identity that the daemon rechecks. Existing managed-worker close/restart still
routes through the owner contract. Registration or advertised actions do not
make a managed target eligible for mux close.

## Herdsman adoption

The first port can be small:

- Register lead/worker subjects directly and retain their daemon identities.
- Publish Herdsman's existing owner projection on `assignment`; publish Pi
  execution evidence separately on `execution`.
- Keep optional Herdr metadata mirroring only where an existing consumer needs
  it. Registry publication itself needs no Herdr commands or token parsing.
- Route child creation through `spawn` only after `ping` confirms protocol v1
  and both `creation` and `launch` capabilities. Those are the backend
  capabilities required by the operation; the method names are part of this
  protocol version and are not separately advertised in `ping`. Attempt
  `spawn` once after those checks. An `unknown_method` response means this
  daemon does not implement the method; keep the existing Herdsman path. Never
  fall back to another launch path after an uncertain/unknown spawn outcome,
  since the child may already have been created or launched.
- Until Herdsman deliberately adopts this path, its existing create/reuse and
  launch path remains unchanged. Do not infer adoption from merely registering
  agents or reading topology. Existing owner assignment and child execution
  publication remain separate from the spawn operation.

Today, Pi supplies execution evidence, Herdsman supplies assignment/run facts,
and the daemon supplies physical observations/controls and an opt-in managed
spawn operation. This is the starting seam, not a prohibition on consolidating
more lifecycle work later. Spawn creates and launches; registration with its
token establishes the child identity. Neither topology freshness nor a missing
pane establishes process exit or authorizes lifecycle action. Stop, resume,
restart, recovery, and adoption of foreign panes are not provided; recovery of
children, pending asks and results must be specified and verified separately.

## Examples and compatibility

- [Reconnecting owner publisher](../examples/agent-publisher/publisher.py)
- [Registry and spawn exchanges](agent-registration.fixture.jsonl)
- [Mux exchanges](control-plane.fixture.jsonl)

Run `cargo build --locked && python3 tests/agent_publisher.py` for the disposable
daemon smoke. It replays registry and spawn fixture exchanges; spawn uses a
scripted backend to verify create, launch, token binding and redaction without a
live Herdr session. The publisher example itself remains a registry-publisher
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
