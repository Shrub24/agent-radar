# Agent registration and direct publication

For the overall surface, lifecycle and adoption path, start with the
[daemon consumer guide](daemon.md).

**Registry contract `registry.v1` with additive `registry.facts.v1`, served on protocol version 2.** This is the
normative wire reference for direct agent
registry producers. The records it documents carry their own `version`, which is
independent of the control-plane protocol version: registrations, close records
and context records are `1`, and a spawn edge is `2` — see [the version table](control-plane.md#versions). The canonical
request/response exchanges are in
[`agent-registration.fixture.jsonl`](agent-registration.fixture.jsonl); the
stdlib-only reconnecting owner publisher is
[`examples/agent-publisher/publisher.py`](../examples/agent-publisher/publisher.py),
exercised against a disposable daemon by
[`tests/agent_publisher.py`](../tests/agent_publisher.py). The fixture uses
`<daemon-time>`, `<daemon-time-plus-1s>`, `<daemon-time-plus-30s>`,
`<opaque-uuid>`, `<replacement-opaque-uuid>`, `<replay-warning>`,
`<spawn-request-id>`, `<child-agent-id>`, `<child-incarnation>`, `<boot-id>`
and `<private-spawn-token>` placeholders for generated values, identities and
timestamps, plus `<journal-epoch>` for the daemon's current journal epoch and
`<old-journal-epoch>` for one no daemon still has, so the fixture can show
`cursor_expired` resynchronisation. Fixture validation replays registry and
journal exchanges against a disposable daemon and compares each response
against its template, substituting only generated and clock-derived fields.
The registry exchanges include self facts with no owner, an extended owner
assignment and an independent facts replacement; freshness is checked per channel.
The spawn exchanges use a disposable scripted Herdr socket; these validate the
served daemon's operation, token binding and public redaction without a live
mux or partial whole-object comparison. The final
`spawn.close` exchanges close the spawned child pane after a scripted
foreground-process identity match and replay the same request id, which must
return the recorded result instead of dispatching again. Those exchanges cover
the live-pane path; the dead-pane `already_exited` path is not expressible
against a scripted backend that cannot name its serving process — such an edge is
never fenced — so it is covered by the isolated disposable-tmux tests instead.
The smoke publishes
both child channels first and asserts those publications and the spawn edge are
unchanged afterwards. The `agent.list` entry is a shape reference, compared
against the validated `agent.get` template. `agent.get` and `agent.list`
differ intentionally: writer-facing channel/context replies return the writer
binding to the publisher, whereas public get/list projections omit writer handles (and
channel accepted-snapshot handles).

This documents the durable agent-registration records and direct publication
channels served on the trusted control socket. They are a **foundation**: private
storage, writer fencing and freshness rules are implemented and tested. Registry
operations are independent of mux capabilities and make no Herdr calls. They
perform no lifecycle controls.

The registry is backend-independent. It holds agent identity separately from
mux inventory and durable mutation records, and mutable channels and current
session context separately from identity. Writing or publishing dispatches no
physical control and enters no mux mutation lane. The additive managed-spawn
methods are described below: `spawn.create` is a physical operation requiring backend
capabilities, while topology reads are registry reads.

## Registration record

One registration is one JSON file, `<state root>/agents/<registration_id>.json`, mode
`0600`:

```json
{
  "version": 1,
  "registration_id": "8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22",
  "registered_at": "2026-01-01T00:00:00.000Z",
  "request": {
    "source": "herdsman",
    "incarnation": "1c2d3e4f-5678-4abc-9def-0123456789ab",
    "session": "c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
    "owner": null,
    "run": null,
    "label": "worker",
    "location": {
      "backend": "herdr",
      "instance": null,
      "workspace": "wA",
      "tab": "wA:t1",
      "pane": "wA:p1"
    },
    "process": { "boot_id": "b3e0…", "pid": 4242, "start_ticks": 99311 },
    "launch": {
      "executable": "/usr/bin/pi",
      "argv": ["--resume"],
      "cwd": "/home/dev/proj",
      "session": { "uuid": null, "path": "/home/dev/.pi/sessions/…" },
      "provenance": "herdsman",
      "revision": "7"
    },
    "spawn_token": "<private launch token>"
  }
}
```

Radar exposes one `agent.presence` operation. A report presents the process birth identity (boot ID, PID, start ticks), source/incarnation, current session UUID, session-load `claim_id`, mutable location and optional launch correlation/managed metadata. Radar resolves the process's `registration_id`, current context and durable session subject internally. The response is `granted`, `duplicate` or `pending`, with a `subject_id` only for a grant. A retry of one claim returns its stored decision; only a fresh Pi `session_start` creates another claim opportunity. A managed launch's pre-minted `registration_id` identifies the process; every session load, including the first, presents a fresh `claim_id`.

The process birth tuple and `registration_id` identify the process registration. Session/location are mutable context; the subject ID follows the session and survives release. A duplicate claim is sticky across incumbent release and daemon restart. Procfs or storage uncertainty remains pending, never duplicate. Supplied launch correlation must match the recorded launch; it never falls back to manual adoption. The daemon validates process identity before recording a registration. Private argv/environment and token fields are never projected. A launch's pre-minted registration id is exported to the child as `PI_RADAR_REGISTRATION_ID`; presenting it grants no authority without the captured process birth identity.

`agent.release` names the exact registration and claim. It releases the current attachment but retains the process registration and durable session subject. It is idempotent; a delayed release cannot revoke a later claim. The process-exit sweep releases only when birth identity is verifiably absent or reused, never from pane loss or heartbeat expiry.

Records are bounded (64 KiB, 1 KiB text fields, max 256 argv entries), private
(mode 0600), written atomically and read only as bounded regular non-symlink
files. Corrupt, oversized, wrong-version and filename-mismatched records fail
explicitly. Registration listings are bounded to 100 and use stable agent-id
order. One daemon owns the state root; independent `Registry` instances over one
root are unsupported for concurrent writes.

## Socket methods

The Pi baseline uses `agent.presence` and `agent.release` for identity; generic
publisher clients may still use `agent.register` and `agent.context`. The trusted
control socket also serves `agent.acquire`, `agent.publish`, `agent.retire`,
`agent.get`, `agent.list`, `spawn.create`, `spawn.get`, `spawn.list` and
`spawn.close`, plus the journal read `events.read`. `daemon.ping` advertises `registry.v1`, `registry.facts.v1` and `events.v1`
regardless of mux backend. `registry.facts.v1` is served whenever `registry.v1`
is, and names the facts channel and the assignment extension below; an older
daemon rejects both as `bad_params`. The `agent.*` methods are
independent of the physical backend and do not call Herdr. `spawn.create` is
different: it requires the `spawn.v1` contract, which the backend's
`mux.create` and `mux.launch` capabilities make servable,
and `spawn.launch_context.v1` when cwd or environment is supplied;
`spawn.get` and `spawn.list` are registry reads, available with
`registry.v1` even when no backend is wired. `spawn.close` requires a bound
child on a daemon-authored spawn edge plus the `spawn.close.v1` contract
(`mux.observe` and `mux.close`), and it needs `mux.exit_observation` and a fenced
edge to clean up a retained dead pane; it refuses before recording when a
capability, a fence or verification evidence is missing. All method parameter objects
reject unknown fields. The exact canonical exchanges, including a successful
managed close and its same-ID replay, are in the JSONL fixture; field semantics
and transitions follow.

### Spawn operation

`spawn.create` is an effectful operation, not a composition of caller-issued
`mux.create`
and `mux.input`. It requires the `spawn.v1` contract (`mux.create` and
`mux.launch`, which the daemon checks before recording
intent or dispatching a create), plus `spawn.launch_context.v1` when cwd or
environment is supplied. Its exact parameters are:

```json
{"request":{"parent":"<registered agent UUID>","executable":"/usr/bin/pi","argv":["--child","row one"]},"requester":"herdsman"}
```

`parent` names a registered runtime subject whose own registered location must
name a pane. The daemon splits to the right of that pane without focusing it;
it does not use whichever pane has current focus. The caller supplies the
resolved executable and argv. The daemon does not resolve a command, write an
assignment, or alter process lifecycle. The command is passed to the backend
unchanged, alongside the registration id the daemon pre-minted for that launch,
exported to the child as `PI_RADAR_REGISTRATION_ID`. The child captures that
value, removes it from its process environment, and presents it as its
`registration_id` in its own `agent.presence` request. Keep the captured value
only in bootstrap memory while an acknowledgement is uncertain; retry with
identical source, incarnation, process claim and registration id. The returned `registration.registration_id` is the exact child
identity. A parent obtains the same identity from `spawn.get`/`spawn.list` at
`spawn.bound.registration_id`, then calls `agent.get` with that value. The token is
private, stored
only with the durable edge and consumed on binding; never return or publish it.

A successful response carries the durable operation record beside its public
edge projection:

```json
{"request":{"id":"<request UUID>","method":"spawn.create","outcome":"completed","created":{"kind":"pane","id":"wA:p2"},"effects":["created pane wA:p2","launched child"]},"spawn":{"version":2,"request_id":"<request UUID>","parent":"<agent UUID>","created":"completed","launched":"completed","location":{"backend":"herdr","workspace":"wA","tab":"wA:t1","pane":"wA:p2"}}}
```

The example omits generated fields; use the fixture for complete shapes. A
location carries the producing backend's own name and instance, so a record made
through one daemon is distinguishable from the same coordinates read through
another. The
request record and edge distinguish `created`, `launched`, and `bound`; each is
reported as `completed`, `refused`, or `unknown` where applicable. `bound` is
absent until the child registers with its token. A launch refused after create
leaves the named pane and edge; nothing is cleaned up silently. If a dispatch
may have occurred but confirmation is lost, `launched` and the request outcome
are `unknown`. Do not retry an uncertain spawn under another request id or fall
back to another launch path: the pane or child may already exist. Same request
id and identical request content only reads back the recorded outcome; changed
content is refused. A backend missing either capability refuses before create,
edge creation, or token minting. Herdr refuses argv containing a newline before
pane input; no private-script fallback is implemented.

Before the create is dispatched, the daemon reads the process serving the
backend's instance and verifies its birth identity, then records that instance
privately on the new edge. That private instance is what later fences the edge
for dead-pane cleanup; it is never returned to callers. When the read or the
verification cannot complete within the daemon's shared verification deadline,
the edge records no instance and the create still proceeds unfenced.

### Spawn topology reads

`spawn.get` takes `{"request_id":"<canonical spawn request UUID>"}` and
returns `{"spawn": <topology>}` or `not_found`. `spawn.list` takes optional
`{"after":"<last returned request UUID>","limit":50}`; default limit is
50, accepted range 1–100. It returns `{"spawns":[<topology>,...],"next":null}`
or a continuation request UUID. Resume with that `after`; ordering is lexical by
the durable request id, not timestamp. An empty result is normal. A malformed
stored edge fails the read rather than being silently omitted.

Both topology methods require `registry.v1`, regardless of backend. For
location verification, one `mux.observe` inventory is consulted per response/page
when a backend advertises `mux.observe`; otherwise the durable topology is still
returned with unconfirmed freshness. Edges persist across daemon restarts. The
public topology contains parent, optional bound child `(source, incarnation, registration_id)`. A binding carries the exact ID returned by the registration that spent the token; a child is never resolved by label, pane, or session. It also contains created/launched outcomes, optional created location, `state`,
`freshness`, and `exit_observation`; a tracked edge publishes its claim id as
`claim_id` and no separate token field, because that claim is the credential
and only the process whose birth identity matches can spend it, while an
untracked edge carries a minted token that is never published. It never returns
the private creating instance the dead-pane fence compares.

`state` is `unbound` without a child, `bound` after token binding, or
`unresolved` when a recorded pane is absent from a successful inventory.
`unresolved` takes precedence over `bound`, but the bound child remains present.
`freshness` is `fresh` only when the backend currently reports the recorded pane;
it is `stale` when it cannot be confirmed or is absent, and omitted if there is
no pane to verify. These location fields never imply exit, idleness, completion,
or permission to restart/stop/resume.

Current v2 edges expose `exit_observation` as `{"state":"unknown"}` until the
backend confirms the pane command exited, then as
`{"state":"exited","code":23,"signal":null,"observed_at":"<daemon-time>"}`
(the signal form has `code:null`). `spawn.exit_observation.v1` (the backend's
`mux.exit_observation`) advertises backend
support; without it, current v2 edges remain unknown. Herdr has no
exit-observation capability, and a missing pane is never inferred to be exited.
Exit evidence is the backend's pane-command
status, not proof of the bound agent's process identity after an out-of-band
respawn; it does not change agent execution or assignment publications, and it
authorizes dead-pane cleanup only together with the edge's own recorded exit and
the creating-instance fence. Reads
never adopt an observed pane or bind a child by title, alias, label, position, or
session UUID.

`spawn.create` is a method of this protocol version, not a capability name.
`daemon.ping` reports the contracts those capabilities can serve, not its full
method table. A consumer should
check protocol compatibility plus the `spawn.v1` contract (or both `mux.create`
and `mux.launch`) before attempting `spawn.create`; if the daemon answers
`unknown_method`, keep the prior path. Never
fallback after timeout, malformed reply, internal error, or any otherwise
uncertain spawn result.

### Managed child close

`spawn.close` takes a daemon-authored spawn request id, its exact bound child
identity and an audit intent; the caller does not supply a pane id. The exact
parameters and response are in the
[canonical fixture](agent-registration.fixture.jsonl). A child is eligible only
when its edge is present and bound to that `(source, incarnation)`, the edge has
a recorded pane, and the bound registration has a process identity claim.
Foreign or caller-selected
panes, children outside daemon-authored spawn edges, labels, titles and pane
positions are never targets.

For a pane that still has an occupant, the live path applies. Immediately before
admission, the daemon requires fresh inventory showing that
pane in its recorded workspace/tab containment, then a positive non-shell
foreground PID whose procfs birth identity exactly matches the child's
registered `(pid, boot_id, start_ticks)`. It refuses before recording or
dispatch for an absent/unbound/mismatched edge or pane, missing process claim,
missing or inconclusive foreground evidence, changed containment, or failed
identity verification.

For a retained pane whose command has exited, the dead path applies, because a
pane reported dead has no occupant to protect. It closes the pane and answers
`already_exited` only when three things hold: the edge itself recorded the exit
observation for that exact pane, a fresh read shows that pane `exited` now, and
the verified birth identity of the process serving the backend equals the one
the daemon recorded privately on the edge before its create. That last check is
the generation fence: a pane id outlives the server that answered the create,
and a backend restarting under the same name hands the same ids to panes Radar
never created, so history and a pane id are both insufficient. An edge that
records no instance — an edge on a backend that cannot name
its serving process, such as Herdr — is unfenced and refuses before admission
rather than inferring a generation. Dead-pane cleanup additionally needs
`mux.exit_observation`, and reads the exit `code`/`signal` the edge stored
rather than the code a same-UID respawn could change. Both fence reads run on
the daemon's shared verification budget and its deadline, so a stalled backend
or verifier fails the fence rather than holding the caller's worker. Verification is not atomic with dispatch: a same-UID process could
respawn the pane between the read and the close, and that residual is accepted
inside the local same-UID trust boundary. The live path requires the
`spawn.close.v1` contract (`mux.observe` and `mux.close`); a missing capability
refuses before record or backend effect.

A successful admission persists the target and the `complete` or `cancel` intent
before dispatch, and the outcome that follows it. The intent is an audit/display
label only, and `completed` is a
backend-confirmed pane closure. The durable close
outcome is `already_exited` when the retained pane of an edge whose command had
exited was closed, `refused` only on a positive
backend refusal before applying the close, and `unknown` if dispatch may have
occurred without confirmation (including lost or timed-out replies), which the
daemon answers with the `outcome_unknown` error code. A result is
returned only for a close that happened, so `refused` and `outcome_unknown` come
back as
error objects; `already_exited` carries the exit observation the edge recorded.
A replay
with the same id and content returns that stored result without re-verification
or redispatch; conflicting content under the same id is refused. Unknown is
never retried automatically or through another backend. No outcome claims more
than the mux pane-close result: it never claims foreground-process or process-group
exit and never writes assignment or execution facts, and `already_exited` in
particular is not proof that the registered process exited and is not relaunch
authority. A vanished pane after an
unknown close remains unresolved; topology preserves the original spawn edge
and binding, and missing-pane freshness is not process-exit evidence. `daemon.ping`
advertises `spawn.close.v1`, so a consumer sees whether the method is servable.
Existing managed
lifecycle decisions remain with their owner.

### Who owns what

Three roles, kept apart so no participant keeps a second writable copy of
another's state:

- **Radar is the lifecycle authority** for a managed Pi process. It records the
  subject, the current attachment, the accepted launch intent and its
  outcome, and it alone decides a transition. Owner, run, label, role and parent
  values supplied in an accepted launch are Radar state; a publisher has no write
  path to them.
- **pi-radar is the single baseline registrar and publisher.** One Pi process has
  one registration, one writer lease and one baseline publication. Other
  extensions consume that runtime through the versioned `pi.events` handoff
  rather than creating a second publisher; a missing or incompatible one is a
  visible diagnostic, not a silent duplicate.
- **Herdsman supplies intent and application evidence**, not managed state. It
  asks Radar for a launch and contributes its own observations through the same
  subject; it does not maintain an independently writable copy of Radar's
  lifecycle records.

A Pi session is attached to a managed subject by Radar's attachment rules, which
are evidence-based and never create authority from a session entry alone.

### Public discovery and pagination

`agent.get` takes `{"registration_id":"<registered UUID>"}`. `agent.list` takes
`{"limit":50,"after":"<last returned registration_id>"}`; both fields are optional.
The default limit is 50; accepted limits are 1–100. Listing returns
`{"agents":[...],"next":"<continuation UUID>"}` or `"next": null` when complete.
Pass `next` as `after` for the next page. Responses are also byte-bounded and may
contain fewer entries than requested: completion is determined by `next`, not
the number of entries. Ordering is lexical by stable agent UUID, not receipt
time. Pages are current reads, not a frozen fleet snapshot; concurrent insertion
before a cursor can be seen on a subsequent full scan. Corrupt records fail the
read explicitly. Retirement does not prune registrations.

For process-owned channels, use the exact `registration_id`; durable lifecycle and handoff identity use `subject_id`, not a session lookup or pane. If a process changes sessions, it keeps the same registration and sends a fresh `claim_id` through `agent.presence`. The receipt reports `granted`, `duplicate` or `pending`, and includes `subject_id` only on a grant. A duplicate stays rejected until a fresh session load; reconnecting the same claim does not promote it. No session entry or marker is written or read for identity. Process channels remain source-labelled, leased, replaceable evidence and carry no paths, recipes, spawn tokens or environment.

### One channel is one record

A channel is one file, `<state root>/publications/<registration_id>.<channel>.json`,
holding the current `writer` binding and an optional `snapshot`. Execution,
assignment and facts are three such files, each with its own writer, sequence
and lease. Current session
context is a separate file, `<state root>/publications/<registration_id>.context.json`.
**Acquire,
publish and retire each rewrite that single record atomically**: a mode-`0600`
temporary sibling is flushed, renamed into place, and the containing directory is
synced before the call returns. There is no separate writer file a partial write
could leave behind, so a failed write leaves the previous binding, sequence and
snapshot byte-identical and an identical retry still succeeds.

`AcquireRequest` names a target `registration_id`, a `channel`, a `PublisherIdentity`
(`source`, publisher `incarnation` UUID, optional `reporting_owner`) and an
optional expected incumbent:

- **Absent `replace`** acquires only a channel with no writer. An identical retry
  by the same publisher/target/channel returns the **same** binding; a different
  publisher is refused (`replacement is explicit`).
- **Present `replace`** names the exact incumbent `generation` and `handle` the
  caller observed. If the current writer no longer matches, the call is refused
  (`incumbent writer changed`), so a replayed or delayed replacement cannot
  overwrite a newer writer.

A `registration` `registration_id` is a target identity, **not** a writer credential. A
channel writer is a separately issued opaque UUID bound to actual publisher
source, publisher incarnation, reporting owner, target agent ID, channel,
generation, sequence and retirement state. The **publisher is not looked up**: a
publisher restart is a new incarnation UUID and needs no second agent
registration. A genuinely new subject/process gets a new agent record; session
or pane reuse never implies continuity. Replacement keeps the same target agent
ID and never creates a fake second child.

### Provenance and replacement

`execution`, `assignment` and `facts` are separate streams. Execution is normally
authored by the target agent; assignment may be authored by its registered
owner; facts are authored by the session itself and name no owner. Writer-facing
publication replies name the actual writer handle and source/incarnation. Public
get/list channel projections omit the fencing handle and expose
source/incarnation/generation, separately naming the reporting owner — they do
not copy the target's registration source and call that the publisher. The three
complete snapshots are never merged. Execution and assignment snapshots share
this base:

- `activity`;
- optional, separate `waiting_reason`;
- optional `last_outcome` (`result` and optional `detail`), distinct from activity;
- bounded `actions` the publisher advertises, which certify neither completion
  nor restart eligibility.

All words are free-form bounded strings, so unknown vocabulary survives; object
shapes reject unknown fields. An idle process with a failed last turn and
unsettled owner work can retain all of those facts separately.

A channel that is acquired but not yet reported omits its `snapshot` key: no
invented "unreported" activity is presented. When a writer is replaced, the
binding changes but the accepted snapshot keeps its **own** source, incarnation,
handle, generation and sequence, and reads `freshness: "stale"`: the report was
true when made and is never relabelled as the successor's. Before its first
accepted snapshot the successor's sequence is zero.

### Facts and assignment fields

Facts and the assignment extension are additive to the base. The daemon serves
them whenever `registry.v1` is served, so a client negotiates `registry.facts.v1`
before it publishes them. Each accepted publish replaces the whole channel value
at a newer sequence, so omitting a field withdraws it on read.

Facts (`channel: "facts"`) are self-authored by the session, with no owner:

- `model`, `provider`, `thinking` and `session_name`: optional bounded text.
  `session_name` is a display label; the session UUID is reported through `agent.presence`.
- `context_window`: optional `{"tokens","window"}` of two non-negative integers
  no greater than `9,007,199,254,740,991` (JavaScript's safe-integer maximum),
  the tokens in context and the window size. `window` is above zero and `tokens`
  does not exceed it; anything else refuses the snapshot. A display derives the
  percentage, so none is published. It carries no authority over freshness,
  lifecycle or assignment.
- `background_tasks`: optional list of at most 64 `{"id","phase"}` objects. Both
  fields are bounded text, and `id` is unique within the snapshot. `phase` is
  producer vocabulary.
- `background_started_unix_ms`: optional nonnegative Unix milliseconds no
  greater than `9,007,199,254,740,991`, when the unresolved background work began.

Assignment (`channel: "assignment"`) adds owner-authored optional fields to the
base:

- `role`, `definition`, `run`, `request`, `assignment` and `ask`: bounded text.
- `assigned_started_unix_ms`: optional nonnegative Unix milliseconds.
- `awaited`: optional list of at most 64 bounded text items.

Every text value is nonempty, at most 1 KiB and free of control characters.
Objects reject unknown fields. Each channel accepts only its own shape: base
fields are refused on `facts`, facts fields on execution or assignment, and
assignment fields on execution, each as `refused`. A base-shaped payload on the
assignment channel is an assignment snapshot without owner fields.

`reporting_owner` on the assignment channel is the publisher's claim, not
authenticated authority: the daemon does not compare it with the registration
owner. Owner authorship of these fields is a publisher convention under the
same-UID socket trust described under [errors, trust and limits](#errors-trust-and-limits).

Facts use the same writer rules as every channel: acquire returns a generation-1
binding for the session's publisher identity, a publish needs a positive
sequence, and a replacement names the expired or retired incumbent under
`replace`. Replacement marks the previous facts stale and fences its handle; it
does not touch assignment or execution.

Each accepted facts publish appends a `facts.published` pointer, and each retire
appends `facts.retired`. Execution and assignment append their own
`execution.*` and `assignment.*` pointers. Pointers carry `registration_id` and time
only, never snapshot values.

`agent.get` and every `agent.list` entry carry a `facts` key: `null` when no
facts writer was ever acquired, otherwise the public channel projection with its
provenance, freshness and no fencing handle.

### Presence, claims and release

`agent.presence` combines the Pi process identity, current location, loaded session UUID and session-load claim. pi-radar calls it on `session_start`, renewal/reconnect reuses the same claim ID, and the response supplies the process `registration_id` plus claim standing. A fresh session load creates a new claim. Radar records duplicate outcomes durably so renewal, incumbent exit or daemon restart cannot promote a rejected duplicate. Pending verification/recovery is retried with the same claim.

On graceful `session_shutdown`, pi-radar calls `agent.release` with the exact `registration_id`, `claim_id` and process birth identity. The daemon acknowledges release of that attachment; the process registration and session subject persist. A late release is fenced to its old claim. If the shutdown reply is lost, a new claim from the same verified process can supersede its previous attachment. Process death is independently established from birth identity; missing pane, expired heartbeat and silence are not proof of death.

The daemon stores the grant/decision and projects its current standing through `agent.get`/`agent.list` and observation snapshots. Clients do not persist subject authority in Pi transcript entries or infer it from pane/session contents. Mutable location changes update process context without creating another registration.

#### Pi session contract surface

The JSON Schemas under `pi-radar/schema/public/v1/` are the machine-readable contract of record; they are generated from the Rust wire DTOs and shipped with the package, and a change to a Pi-facing call is not complete until they are regenerated. This table is the reviewer's index of which schema backs each call: change a call and its row changes in the same commit. Serialize and deserialize schemas are separate because Serde defaults, null handling and omission differ by direction.

| Call | Gate | Schemas | pi-radar SDK |
| --- | --- | --- | --- |
| `agent.presence` | `registry.v1` | `presence-report.*`, `presence-answer.*` | `publication.presence.report(session)` |
| `agent.release` | `registry.v1` | `presence-release.*`, `presence-answer.*` | `publication.presence.release()` |
| `communication.handoff` publish/read/ack | `communication.handoff.v1` | `handoff-publish-params.*`, `handoff.*`, `handoff-inbox.*`, `handoff-receipt.*` | `createRadarHandoffPort(client, publication)` |
| `spawn.create` with managed metadata, optionally `continue_subject_id` | `spawn.v1` | `managed-launch-metadata.*` | `launchRadarWorker` |
| `spawn.list` (the caller's own children) | `registry.v1` | `spawn-list-params.*`, `spawn-list-page.*` | `runtime.children.list`, `observe`, `inspect` |
| `lifecycle.restart` by the recorded parent | `lifecycle.restart.v1` | — | `runtime.children.restart` |
| `observation.get` | `registry.v1` | `observation-get-response.*`, `observation-snapshot.*` | `client.observe()` |

The public process identifier is `registration_id`. Durable agent identity is `subject_id`, and `HandoffIdentity` carries both plus the execution it names.

### Sequences, leases and freshness

A publish requires a positive per-generation sequence. Lease defaults to 30
seconds and is bounded to 1–300 seconds. Strictly newer content replaces the
whole snapshot and the writer's stored sequence in one record write, before
acknowledgment. Equal-sequence identical content (snapshot, lease and observation
time) is returned unchanged and does **not** refresh the lease; equal sequence
with conflicting content and older sequence refuse. Heartbeat requires a newer
sequence. A failed write consumes no sequence, so an identical retry succeeds.

Producer `observed_at` is provenance, never authority for freshness. Daemon
receipt time and expiry control the lease. Every registry open creates a random
serving epoch, independent of the wall clock: persisted snapshots are stale
across daemon restart even if startup timestamps are equal or the clock moves
backwards. An unreplaced handle can reconnect with a newer sequence. Replaying
old content cannot make restored facts fresh.

Freshness is `fresh` only when the stored snapshot was accepted in the current
serving epoch, belongs to the current writer generation/handle, that writer is
not retired, and the lease has not expired. It is `stale` when restored, when the
writer was replaced, when the writer retired, or when the lease expired.

A fresh writer cannot be taken over. Explicit replacement requires retirement or
daemon-clock lease expiry, persists a new opaque handle/generation before
acknowledgment, and fences the old handle even if it submits a larger sequence.
An acquired writer that never reported has no live lease and may be replaced,
but only by naming its exact incumbent. A successor that has not reported is not
fresh merely because it sits beside the previous generation's unexpired snapshot. Stale and retired mean only stale and
retired: neither proves process exit, assignment completion or safe restart.
Each channel has its own writer, sequence, lease and freshness, so facts going
stale or being replaced never changes execution or assignment freshness, and the
reverse holds too.

### Persistence, ownership and bounds

Channel records and current-session context are bounded (64 KiB), versioned
private JSON files written by atomic sibling-temporary replacement, mode `0600`,
file and directory flushed before acknowledgment. Each snapshot has at most 16 actions, and each facts
`background_tasks` or assignment `awaited` list has at most 64 entries. Text
fields are at most 1 KiB. Corrupt records are errors, not successful empty reads. A local
transition mutex serializes writers within one `Registry`; the state root has one
owning daemon, with no cross-process locking or cross-user authorization system.

## Process identity verification

`agent.get` and `agent.list` read the immutable registration claim, then invoke
the verifier outside registry/store locks. A claimed birth identity is also
served as `process.claim` (`boot_id`, `pid`, `start_ticks`), present only when
claimed. It is same-UID process evidence for matching a live pane, not a
credential, and it never carries launch, argv, cwd or environment data. Verification is bounded by a worker
deadline; a timeout or daemon stop reports `unavailable`, never `absent`. The
daemon runs at most `MAX_VERIFICATION_JOBS` (4) verifier calls at once, shared by
every connection and page: exceeding the bound and waiting for a busy slot are
also `unavailable`. The managed-close fence and the creation-time
serving-process capture draw on that same budget and deadline: a capture that
misses its deadline records no instance rather than waiting, so a stalled
verifier cannot block a create indefinitely, and an unfenced edge refuses dead
cleanup later instead of guessing a generation. A call whose deadline expires or whose connection ends keeps
its slot until the verifier actually returns, so repeated requests cannot
accumulate blocked verifier threads, and a request arriving with no slot starts
no work at all. A verifier that blocks forever therefore holds its slot, and up
to that many uncancellable verifier threads can outlive a stop request; shutdown
does not join them, and each worker holds only the claimed process identity and
the verifier seam. The `Registry::verify_process(registration_id, verifier)` local API
remains available for
deterministic tests. No registry lock is held across procfs or an injected verifier. The result is
`verified` only when the full boot ID, PID and `/proc/<pid>/stat` start-tick tuple
matches; a known missing process is `absent`; a live PID with another boot ID,
PID or start time is `mismatched`; missing claim, unsupported procfs, permission
failure and other unreadable evidence are `unavailable`. No claim is not evidence
of absence. The production `LocalProcfsVerifier` reads local Linux procfs, using
the same stat parser as the process sampler; non-Linux platforms report
unavailable.

Process evidence is independent of publication freshness. A verified process may
have a stale publication lease, and an absent process may have a fresh publisher
snapshot. Neither value infers lifecycle eligibility or triggers a physical
operation. Tests inject a deterministic verifier; no live mux or process-control
operation is performed.

A consumer that maps registrations to panes joins on the backend-qualified
location plus the claimed birth identity, sampling the pane's own foreground
process for itself: a PID alone, a title, a label, an alias and a session UUID
never join one. Protection is separate from joinability — a pane carrying a
registration candidate is refused a generic unmanaged close whether its join is
exact, unresolved or ambiguous, and only a daemon-authored spawn edge makes a
managed close eligible. See
[dashboard consumption](daemon.md#dashboard-consumption).

The endpoint contract exposes explicit writer acquire and replace (with expected
generation/handle), channel publish/retire under the opaque handle, mutable context
publish/replace, plus bounded public channel/context reads/listing and separately
labelled process verification. Do
not overload immutable `agent.presence` with succession. Automatic replacement
policy, process-death inference from a stale lease, launch/stop/resume,
task-completion certification and physical mux controls remain out of scope.

## Reconnecting owner-publisher example

Run the example against an already-running daemon with a socket path that passes
the daemon's trust check:

```sh
python3 examples/agent-publisher/publisher.py /run/user/1000/agent-radar/control.sock --once
```

The script is dependency-free Python 3.10+ and demonstrates owner assignment
projection plus current-session context publication when its session changes. A production publisher stores its process registration and current session claim
outside the process, reuses the same `claim_id` after reconnect, reports the
current location, and calls `agent.acquire` without `replace` to recover the same
current writer. The example process generates an
incarnation once for its lifetime; it is illustrative, not durable publisher
state. It publishes a complete snapshot and uses a newer sequence for every
heartbeat. A lost response may be retried with exactly the same sequence,
snapshot, lease and `observed_at`; identical replay returns the accepted report
unchanged and does not extend expiry. Advance sequence to renew the lease.

The module's `publish_facts` helper shows the self-authored facts channel: a
session with no owner acquires `facts` under its own publisher identity and
publishes a complete snapshot.

The example accepts `--session <canonical-UUID>`; omitting it publishes explicit
null. It sends a context update only when its current session changes, using a
separate increasing context sequence under its context writer. A production
publisher obtains the current UUID from its host integration; it must never send
session-file paths, launch arguments, environment or raw provider error details.
Long leases remain allowed, but production publishers should keep a bounded lease
that reflects the expected update cadence and should republish changes promptly.

A different writer is never adopted implicitly. A publisher may replace only
when it has explicitly observed incumbent generation and handle and sends both
under `replace`; replacement is valid only after incumbent retirement or lease
expiry. A refusal that says the writer/handle changed means this producer has
been fenced: stop and reconcile with the owner, do not retry with `replace` or
new identity. This example contains no Herdr invocation, Herdr token parsing,
subprocess or physical lifecycle call. Herdsman's existing owner assignment
projection can map to `channel: "assignment"`; target process activity belongs
on the separate `execution` channel with its own publisher identity and sequence.

Run the full disposable-daemon contract smoke with:

```sh
cargo build --locked
python3 tests/agent_publisher.py
```

It uses a private temporary socket/state root, starts and bounds daemon children,
checks public launch/context redaction and unknown vocabulary, exercises self
facts without an owner and their pointers, the fixture's extended assignment and
independent facts replacement, assignment replay/heartbeat, context session switch and explicit null,
context privacy, channel lease-expiry replacement, old-writer fencing and restart
epoch freshness/reconnect, then stops the child in `finally`. It is not live
publisher or fleet integration. The pi-extensions port is a separate follow-up:
Herdsman must adopt `agent.presence` on session start/switch/fork under its existing
publisher/writer handling. Map its current owner projection to assignment and
its child execution facts to their separate channel, do not add
process execution or recovery policy here.

## Errors, trust and limits

Switch on protocol error codes, not message prose: `bad_params` is invalid shape
or bounds, `not_found` is missing registration/writer, `refused` is a fenced,
conflicting or invalid transition/store record, and `internal` means persistence
or serving failure (a lost reply may leave the caller uncertain). Retry register
with identical content; retry an uncertain publish only with identical
sequence/content. Never infer that an absent response did not persist.

The publisher validates a socket path before connecting: parent and socket must
be real non-symlinks, the parent must be owned by the effective uid with mode
exactly `0700`, and the socket must be owned by that uid. Socket trust is not
same-user authentication. Writer handles fence cooperating producers but are
not secrets against hostile local code; the state is private `0600`, yet another
process under the same uid can read it. Do not expose the socket remotely.

`reporting_owner` and the assignment owner fields are provenance the publisher
reports. The daemon does not compare `reporting_owner` with the registration's
`owner` or authenticate the publisher, so any process that can connect to the
socket can publish any channel under any identity. Owner authorship of assignment
fields is a publisher convention, not daemon authority.

Lease expiry and daemon epoch changes mean only that published snapshots are stale;
process verification is independent and none of registration, actions,
facts, retirement, replacement, freshness or process verification establishes lifecycle
eligibility, assignment completion or authority to launch/stop/resume. No
publisher API here executes physical controls.

The existing mux `report` method and its Herdr-forwarding behaviour are
unchanged.
