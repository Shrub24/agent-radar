# Radar control-plane protocol v2

For the overall surface, lifecycle and adoption path, start with the
[daemon consumer guide](daemon.md). Backend implementers and migrating consumers
should also read the [mux integration contract](mux.md).

This is the wire reference for local clients, including extensions. The daemon
wraps physical mux operations and owns runtime edges only for children it spawns;
it does not infer ownership of arbitrary mux resources or grant assignment
authority. Herdsman retains assignment/run orchestration. Radar does not start or
supervise the daemon. Independent registry methods, managed spawn and verified
child-pane close are documented in [agent registration](agent-registration.md).
They do not provide process stop, resume or restart.

## Versions

Version 2 is a clean break: no version-1 method name is aliased. Several
version numbers are independent, and conflating them is the easiest way to
misread a record:

| Number | Where | Meaning |
|---|---|---|
| protocol version | request envelope `version`, handshake `protocol` | the wire scheme. This build serves `2`; a request naming `1` is refused `bad_version` and a removed method name is refused `unknown_method`, both without effect. |
| registration `version` | `agent.register` result, stored registration records | the registry record schema. This build writes and serves `1`. |
| spawn edge `version` | `spawn.create` result, `spawn.get`/`spawn.list` topology | `2`, which preserves the process parent and the logical session parent. A record of another version is refused. |
| close record `version` | stored managed-close records | the close record schema, `1`. |
| context record `version` | stored session-context records | the context record schema, `1`. |

Removed names include the flat reads (`observe`, `process_info`, `output`), the
old mutations (`focus`, `close`, `create`, `input`, `report`, `spawn`), the flat
operation readbacks (`request`, `requests`), `child.close`, and everything in the
`registry` and `resume` families. They return `unknown_method`; nothing is
dispatched and no record is written.

## Start and select

The daemon runs as a user service, activated by its socket unit, or can be run in the foreground for development. `radar` starts the daemon by default before opening the TUI. Its physical adapter is tmux on the server named by `[daemon] tmux_server` (default `radar`); Herdr is selected only with `RADAR_CONTROL_BACKEND=herdr`. The systemd daemon unit orders after and wants the matching `radar-tmux@<name>.service`; the daemon verifies the answering server's identity and refuses an unowned or unavailable instance. A foreground daemon requires that server unit to be started separately. Unknown backends and missing server names refuse startup.

Settings live in a `[daemon]` table. Environment variables override them:

```toml
[daemon]
backend = "tmux" # or "herdr"
tmux_server = "radar"
```

The former `[runtime]` table is removed. A configuration that still sets it is refused as containing unknown keys.

For example, start a daemon on a named tmux server with:

```sh
RADAR_CONTROL_BACKEND=tmux RADAR_TMUX_SERVER=work radar daemon
```

Under the systemd daemon unit, `radar-tmux@work.service` is started by the unit dependency. For a foreground daemon, start that server unit first.

An omitted socket uses `RADAR_CONTROL_SOCKET`, then `$XDG_RUNTIME_DIR/agent-radar/control.sock`, then `/tmp/agent-radar-<uid>/control.sock`. The daemon record root uses `RADAR_CONTROL_STATE`, `$XDG_STATE_HOME/agent-radar/control`, then `~/.local/state/agent-radar/control`, with `/tmp/agent-radar-<uid>` as last resort when neither state-home nor HOME exists.

Before dialing, Radar checks the socket path read-only: socket and parent must not be symlinks; the parent must exist, be this user's real directory with mode exactly `0700`; the socket must be a socket owned by this uid. This also applies to a configured override. The daemon uses the same directory rule when binding (and creates a missing directory as `0700`). If the daemon is unavailable, the TUI shows a blocking diagnostic with retry/quit; it never falls back to an in-process adapter.

## Transport and envelope

Unix-domain socket; UTF-8 JSON Lines, one request object and one response object per line. Protocol version is `2`. Request ID is a canonical UUID. Parameters are an object; omitted/null params mean `{}`. A request is:

```json
{"version":2,"id":"8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22","method":"daemon.ping","params":{}}
```

Success: `{"id":"…","result":{…}}`. Error: `{"id":"…","error":{"code":"bad_params","message":"…"}}`. Messages are for people; switch on codes and record outcomes, not message text. A decodable id is echoed on refusal; malformed/oversized lines may be answered with empty id. Unknown methods refuse and the connection remains usable.

The control socket and the temporary publisher-compatibility socket share one Tokio transport runtime and the same trust/framing code. Lines are capped at 1 MiB excluding newline; an oversized request is drained and refused. At most 32 connections are open across both entries. Active handler capacity is reserved by lane: control 8, ingest 4 and waiting reads 8, with handlers running synchronously on a bounded blocking pool. A connection idles at most 120 seconds; response writes have a 2-second timeout. `events.read` and revision-waiting `observation.get` use the reserved waiting lane. Their bounded waits occupy a waiting-pool worker rather than a control worker; a client that needs an immediate read sets `wait_ms` to zero. Shutdown closes streams and gives the runtime up to five seconds to stop. Radar's control client dials per request; each wait checks cancellation in 10 ms increments, with a 5-second exchange budget. Cancellation abandons the caller's wait, not accepted work. A `spawn.close` or registry request arriving while the daemon is stopping is refused `busy` with no record and no effect; other effectful requests are still admitted and recorded, and a stop that interrupts their backend call leaves the adapters' honest result — `refused` when nothing was dispatched, `unknown` when it may have been. No authentication beyond local filesystem socket-path trust is provided; this is not remote transport or same-user process authentication.

## Handshake

`daemon.ping`, params `{}`:

```json
{"protocol":2,"backend":"tmux","instance":"radar","revision":8,"contracts":["registry.v1","registry.facts.v1","events.v1","tasks.list.v1","spawn.v1","spawn.close.v1","spawn.launch_context.v1","spawn.exit_observation.v1","lifecycle.cancel.v1","lifecycle.stop.v1","topology.create.v1","topology.delete.v1","topology.reload.v1","topology.get.v1","topology.list.v1","topology.reconcile.v1","lifecycle.get.v1","lifecycle.list.v1","lifecycle.enroll.v1","lifecycle.restart.v1","lifecycle.abandon.v1","observation.get.v1","communication.handoff.v1"]}
```

`backend` is the wired physical backend: `herdr`, `tmux` or `none`, never a placeholder. `instance` names the tmux server for a tmux backend and is null otherwise. `revision` is the wire revision of the daemon's build, which increases whenever a method or field is added; Radar's own clients require at least their own revision and show a blocking diagnostic that names the restart when a daemon that outlived an install is older. `contracts` lists the daemon contracts the backend makes servable, which is what an external consumer pins. The backend's `mux.*` declaration stays inside the daemon and is not on the wire. A contract name your build does not know is tolerated; ignore it.

Inside the daemon, each backend returns a typed `Primitives` flag set, one bit per physical behavior. Diagnostic names use the internal `mux.*` vocabulary; that vocabulary is not sent to clients. The flags decide which contracts are advertised and which operations refuse before any effect:

| Internal primitive | Behavior |
|---|---|
| `mux.observe` | Normalized workspace/tab/pane inventory |
| `mux.process_info` | Current foreground process evidence for a pane |
| `mux.focus` | Focus a named location |
| `mux.close` | Close a verified unmanaged pane, tab or workspace target |
| `mux.create` | Create a workspace, tab or pane split |
| `mux.input` | Literal text or named keys |
| `mux.output` | Bounded, non-consuming output read |
| `mux.report` | Curated metadata forwarding |
| `mux.launch` | Launch a resolved command in a created pane |
| `mux.launch_context` | Carry cwd and an environment overlay into that launch |
| `mux.exit_observation` | Report a retained typed pane-command exit |
| `mux.lifecycle_tracking` | Capture the launched process identity for fenced lifecycle controls |

Daemon contracts are versioned promises that the daemon serves the methods its internal typed primitive set makes possible. The `mux.*` primitives are not part of the handshake; external consumers pin contracts. Contract needs are exhaustive in the implementation, and the advertised set is derived from them:

| Contract | Needs | Serves |
|---|---|---|
| `registry.v1` | — | All `agent.*` methods |
| `registry.facts.v1` | — | The `facts` channel and the assignment extension on `agent.acquire`/`agent.publish` |
| `events.v1` | — | `events.read` |
| `communication.handoff.v1` | — | `communication.publish`, `communication.inbox`, `communication.get`, `communication.ack` |
| `observation.get.v1` | `mux.observe` | `observation.get`: the daemon's shared observation snapshot |
| `tasks.list.v1` | task ingest configured | volatile publisher task snapshots via `tasks.list` |
| `spawn.v1` | `mux.create` + `mux.launch` | `spawn.create` |
| `spawn.close.v1` | `mux.observe` + `mux.close` | `spawn.close` |
| `spawn.launch_context.v1` | `mux.create` + `mux.launch` + `mux.launch_context` | cwd/environment-bearing `spawn.create` |
| `spawn.exit_observation.v1` | `mux.exit_observation` | typed `exit_observation` on current spawn edges |
| `topology.create.v1` | `mux.create` | `topology.create`: a labelled container, optionally with a private recipe. A recipe is carried by a pane split or a tab and runs in the pane that creation made; a workspace creation carries none |
| `topology.delete.v1` | `mux.close` | `topology.delete`: the guarded close of a container's place and its removal records |
| `topology.reload.v1` | `mux.launch` + `mux.exit_observation` | `topology.reload`: a pane container's recorded recipe, run again |
| `topology.get.v1`, `topology.list.v1` | — | container reads |
| `topology.reconcile.v1` | `mux.observe` | one bounded reconciliation pass over recorded containers |
| `lifecycle.cancel.v1`, `lifecycle.stop.v1` | `mux.lifecycle_tracking` | `lifecycle.cancel`, `lifecycle.stop` |
| `lifecycle.enroll.v1` | `mux.lifecycle_tracking` + `mux.observe` | `lifecycle.enroll` |
| `lifecycle.restart.v1` | `mux.lifecycle_tracking` + `mux.create` + `mux.launch` | `lifecycle.restart` |
| `lifecycle.get.v1`, `lifecycle.list.v1` | `mux.lifecycle_tracking` | managed launch subjects |
| `lifecycle.abandon.v1` | — | `lifecycle.abandon`: ends one unresolved record's recovery obligation |
| — | — | `lifecycle.reconcile`: one explicit reconciliation pass (no contract of its own, like the other daemon-owned reads) |

`registry.v1`, `registry.facts.v1`, `events.v1` and `communication.handoff.v1` need no backend and are advertised even for backend `none`. A contract that the daemon cannot serve is never advertised. Bundled Radar clients require their minimum `revision` and show a blocking restart diagnostic when a daemon outlived an install; external consumers check the contracts they need. Unknown contract names are tolerated. An absent required contract or `unknown_method` is the compatibility signal for external consumers.

## Methods

`launch.admit`, `launch.claim` and `lifecycle.wake` are internal methods of `radar run` and the launch chain. They are not advertised, have no contract and are not part of the consumer surface.

All method-specific parameter objects reject unknown keys unless noted; `daemon.ping`, `mux.request.get` and `mux.request.list` ignore unrecognized parameter keys. Identifiers are nonempty, at most 256 UTF-8 bytes, and contain no control characters. JSON serialization of normalized observation/evidence uses the Rust model's snake_case enums; unknown semantic/status values use `{"other":"value"}`. Durations are integer milliseconds; absent optional facts remain absent/null as defined by the normalized model.

### Durable handoffs

`communication.handoff.v1` is daemon-owned durable payload delivery. `communication.publish` uses the request UUID as the handoff ID and takes `sender_registration_id`, `recipient_subject_id`, optional `correlation_id`, a `summary`, and `content`. The first contract accepts only a managed sender targeting its exact resolved `parent_subject_id`; the daemon refuses root senders and any other target. `pi-radar` derives the sender from the current process registration and the recipient from Radar's injected parent context, so Herdsman callers do not choose either identity. The daemon resolves the sender from a fresh registry context and validates any Radar launch marker against the current durable execution. Retry an uncertain publication with the same request ID and identical content; a different payload under that ID is refused.

`communication.inbox` takes the registered `registration_id` and returns pending `handoff_ids` for its current managed subject; it does not include payloads, keeping the list response small. An unmanaged or unresolved process is refused. Fetch each addressed payload with `communication.get` by ID, including after acknowledgement. `communication.ack` records that the recipient consumer durably captured the payload; it does not attest to understanding or settle the correlated domain work. Persist the retrieved content locally before acknowledging it.

Content is capped at 384 KiB, summaries at 600 characters, pending obligations at 128 per recipient, aggregate storage at 512 records/64 MiB, and acknowledged records are retained for 30 days. Pending records are never age-pruned. Pointer events `communication.handoff_published` and `communication.handoff_acknowledged` carry IDs, not payloads; inbox scans remain the recovery source when a notification is missed.

Radar owns the immutable message and receipt. Herdsman retains authority over completion validity, assignment state and worker lifecycle disposition. Publishing or acknowledging a handoff does not authorize worker closure or assignment settlement.

### Reads

- `daemon.ping`, params `{}`: protocol/revision/backend/instance/contracts as above. No record.
- `mux.observe`, params `{}`: `{ "inventory": FleetObservation }`. Requires `mux.observe`. `Workspace.session_path` (session start directory) and `Pane.current_path` (current pane directory) are optional observed fields; `L` uses them as the launch cwd. They are not durable container metadata. No record.
- `mux.process_info`, params `{"pane_id":"wA:p1"}`: `{ "evidence": ForegroundEvidence }`. Requires `mux.process_info`. No record; inconclusive evidence is a valid result.
- `mux.output`, params `{"pane_id":"wA:p1","source":"recent","lines":100,"ansi":false}`. Requires `mux.output`. `source` is `visible`, `recent`, `recent_unwrapped`, or `detection`; `lines` is optional (runtime default), otherwise 1..=500; `ansi` defaults false. Result is `{ "output": {"text":"…","truncated":false,"revision":null} }`. Text is capped at 64 KiB on a UTF-8 boundary and `truncated` set. No record.
- `mux.request.get`, params `{"id":"<canonical UUID>"}`: `{ "request": RequestRecord }`, or `not_found`. No record for the lookup.
- `mux.request.list`, params `{}` or `{"limit":50}`: `{ "requests": [RequestRecord,…] }`, newest first; default 50, maximum 200 (larger counts clamp to 200). No record.
- `spawn.get`/`spawn.list`: durable child topology (exact attempt `claim_id`, durable `parent_subject_id`) and optional current backend location verification; see the [agent-registration topology contract](agent-registration.md). Requires `registry.v1`, and consults `mux.observe` for location freshness when the backend declares it. Current v2 edges carry an explicit `exit_observation`: `unknown`, or `exited` with code/signal and `observed_at`. Missing panes remain unresolved, not exited.
- `tasks.list`, params `{"after":"<session-uuid>","limit":100}`. Requires `tasks.list.v1`. Returns `{ "revision": 7, "sessions": [{"session":"<pi-session-uuid>","connected":true,"last_updated_at":1234,"tasks":[Task,…]}], "next_after":null }`. The task snapshot is volatile and in-memory. `after` is an exclusive session-ID cursor; `limit` defaults to 100 and must be 1..=256. Follow `next_after` immediately. Every page carries the same revision only if no publisher update intervenes; if it changes while paging, discard the partial snapshot and restart. Tasks join registry rows by exact Pi session UUID only, never by pane. See [publisher bridge](radar-bus.md) for task fields, volatility and the temporary compatibility socket.
- `observation.get`, params `{"epoch":"<last-seen-epoch>","since_revision":7,"wait_ms":1000,"classes":["fleet","processes"]}`. Requires `observation.get.v1`. The daemon samples once for every reader and publishes an immutable revision; this read returns the current one. `classes` names what the reader wants collected: `fleet`, `registry` and `tasks` are always collected, and `processes` — a whole-fleet process sweep — is sampled only while some reader has asked within the last few seconds. An unknown class name is refused `bad_params` rather than ignored. `wait_ms` defaults to 0 and must be at most 30000. The result is `{ "epoch":"…", "revision":7, "snapshot":{…} }`, where `snapshot` carries `collected_at`, `sources` (one entry per source with its `state` and `last_success_at`), `tasks` and the daemon's fleet projection. Naming the `epoch` and `since_revision` the caller already holds answers `{"not_modified":{"epoch":"…","revision":7}}`; a revision or epoch this daemon process does not hold answers `{"snapshot_expired":{…}}` and the reader restarts from the current revision rather than merging two; a snapshot too large for one response answers `{"snapshot_too_large":{…}}`. A daemon with no backend wired has nothing to observe and refuses `backend_unavailable` rather than publishing an empty fleet. Revisions are the sampler's own: they are never appended to the durable journal and are never returned by `events.read`. `revision` is only meaningful inside its `epoch`, because a restarted daemon counts from one again.
- `events.read`, params `{"after":12,"epoch":"<last-seen-epoch>","limit":100,"wait_ms":1000}`. Requires `events.v1`. `after` is a sequence number, default 0; `epoch` is optional. `limit` defaults to 100 and must be 1..=500. `wait_ms` defaults to 0 and must be 0..=1000. The normal result is `{ "epoch":"…", "events":[Event,…], "cursor":{"epoch":"…","sequence":12}, "tail":40, "more":true, "earliest":1 }`; `Event` has `seq`, `kind`, optional stable `subject_id` and exact `claim_id` for lifecycle events, plus event-specific references such as `registration_id`, `spawn`, `request_id`, and `at`. `cursor` is the position after the last delivered event — the request's `after` when nothing was delivered — so passing its `sequence` as `after` and its `epoch` as `epoch` resumes without skipping an undelivered event. `tail` is the journal head and `more` says whether events exist beyond `cursor`: while `more` is true, read again immediately instead of waiting. An empty `events` array means the bounded wait ended without a new event.

A lost-history result is still success, shaped as `{ "cursor_expired":{"epoch":"…","cursor":{"epoch":"…","sequence":12},"earliest":4} }`; it contains no partial event sequence. On expiry, re-read all current state: page through `agent.list` and `spawn.list` following each `next` cursor, and read ordinary operation records through `mux.request.list` (and exact `mux.request.get` lookups where their IDs are known). Then resume using the returned epoch and cursor. A first read with `after:0` can itself expire after retention has dropped older history.

Events are pointers, not state snapshots: fetch the exact referenced record with `agent.get`, `spawn.get`, or `mux.request.get` before acting. Absence of an event does not establish freshness or liveness. Facts are written before their events, so a crash can lose an event after its fact became durable; an unclean daemon restart changes the journal epoch and forces resynchronisation.

Read errors do not create operation records. A missing backend or capability returns `backend_unavailable`; backend read refusal for `mux.output` returns `refused` and uncertain output returns `backend_unavailable`.

### Durable operations

`mux.focus`, `mux.close`, `mux.create`, `mux.input`, `mux.report` and `spawn.create` are mutations. Accepted requests are atomically recorded before dispatch, and a `started` record is persisted before backend I/O. The caller should provide a fresh UUID per new operation. Same ID and same method/target/requester/JSON params returns the existing record without replay; same ID with different contents refuses. JSON object key order is immaterial, other values are significant. A target with pending, claimed, started, or unknown outcome suppresses a conflicting mutation across methods (`in_flight`); an unscoped request has its own lane. A backend lacking a capability is recorded as a completed/refused request with `backend_unavailable`, not started or dispatched. Invalid params refuse before recording. The shared transport admits at most 32 connections across the control and temporary ingest socket entries; active control, ingest and waiting handlers each have bounded reserved capacity. Excess work is closed or refused without an operation record according to its lane. `spawn.close` uses a separate durable record described below.

Records contain `id`, `method`, optional `requester`, optional `target`, stored `params`, `requested_at`, `expires_at`, `state`, optional `outcome`, `category`, `message`, `effects`, `created`, `completed_at`. Times are RFC3339 UTC with milliseconds. New operation requests expire after 30 seconds if never started; expired unstarted records derive `not_executed`. Claimed/started or completed-unknown records are uncertain and suppress replay indefinitely; clients must not infer non-execution from timeout, disconnect, cancellation, process disappearance or a missing reply.

Effectful backend refusals can settle `outcome: refused`; confirmed actions settle `completed`; possible dispatch without trustworthy outcome settles `unknown`. A record in `started` without outcome is also unknown. `internal` is not proof of non-dispatch. The client maps protocol pre-dispatch errors to refusal; `outcome_unknown`, `internal`, malformed/lost replies and post-write transport failure are unknown. A write failure before its first byte is refusal; after any byte it is unknown. Re-read with `mux.request.get`/`mux.request.list` to learn the durable record. Never automatically retry an uncertain mutation, including through the direct adapter. Retrying the same UUID only retrieves the record; it does not execute again.

- `mux.focus`: `{"target":"wA:p1","target_kind":"pane","requester":"extension-name"}`. Requires `mux.focus`. `target_kind` is `pane` or `workspace`. Completed effect is `focus`.
- `mux.close`: `{"request":{"target":{"pane":"wA:p1"},"identity":{"pane":{"pane_id":"wA:p1","occupant":null}}},"requester":"extension-name"}`. Requires `mux.close`. `target` is externally tagged `{"pane":"id"}`, `{"tab":"id"}` or `{"workspace":"id"}`. `identity` is a frozen `TargetIdentity`: pane form is `{"pane":{"pane_id":"…","occupant":null-or-AgentIdentity}}`; tab form is `{"tab":{"tab_id":"…","members":[PaneIdentity,…]}}`; workspace form is `{"workspace":{"workspace_id":"…","members":[PaneIdentity,…]}}` over every pane in it. AgentIdentity carries `name`, `session`, `lineage`, `managed`, `label`, `run`; PaneIdentity carries `pane_id`, `occupant`. Freeze from a fresh observation at confirmation time, then send that exact identity. The daemon re-observes and checks equality and positive-unmanaged containment, then reads the registry immediately before dispatch: any registration naming the pane, or a pane in the tab, refuses the close, and an unreadable registry refuses it too. Changed, managed, uncertain, mixed tabs, or a workspace whose membership changed since confirmation refuse. Completed means runtime confirmed close, not that removal was re-observed. Managed worker close/restart remains owner-routed, never this method; a daemon-created child pane is closed with `spawn.close` below.
- `mux.create`: `{"request":{"kind":"pane_split","pane_id":"wA:p1","direction":"right","focus":false},"requester":"extension-name"}`. Requires `mux.create`. Tagged request variants: `{"kind":"workspace","focus":false}`, `{"kind":"tab","workspace_id":"wA","focus":false}`, `{"kind":"pane_split","pane_id":"wA:p1","direction":"down","focus":false}`. Directions `right`/`down`. Creates a mux location only; it does not launch a process or restore an agent. Record `created` is `{ "kind":"workspace|tab|pane", "id":"runtime-reported-id" }` only when backend names it; do not infer it from request or effect prose.
- `mux.input`: `{"request":{"pane_id":"wA:p1","payload":{"kind":"text","text":"hello\n"}},"requester":"extension-name"}`. Requires `mux.input`. Exactly one tagged payload: text or keys. Text is 1..=4096 bytes; control characters refused except newline/tab. Keys are 1..=16 names, each 1..=32 ASCII letters/digits/`+`/`-`/`_`. Key names are passed to backend; use only names it supports. Text is literal input, not shell evaluation.
- `mux.report`: `{"request":{"kind":"state","pane_id":"wA:p1","source":"publisher","agent":"pi","state":"working","sequence":4},"requester":"extension-name"}`. Requires `mux.report`. Tagged variants `state`, `session`, `metadata`; see schemas below. This is a curated normalized schema, not a generic Herdr RPC passthrough. Unknown fields/kinds refuse. It forwards the caller's own values; it does not merge them into a registry or establish authority. `resume_argv`, `release_agent`, and `clear_agent_authority` are unsupported.
- `spawn.create`: `{"request":{"parent":"<agent-uuid>","parent_session_id":"<pi-session-uuid>","executable":"/usr/bin/pi","argv":["--child"],"cwd":"/work","env":{"NAME":"value"},"managed":{"owner":"herdr","run":"run-17","label":"reviewer","role":"worker","parent_subject_id":"<logical-subject-uuid>"}},"requester":"extension-name"}`. Requires `spawn.v1` (`mux.create` + `mux.launch`), and `spawn.launch_context.v1` when cwd/env are supplied. `managed` is optional and carries Radar-owned owner/run/label/role plus an optional stable parent subject. Text metadata is non-empty, control-free and at most 256 UTF-8 bytes. A supplied `parent_subject_id` must resolve to the exact registered parent agent before any effect. Accepted values are written durably before dispatch and inherited by successor executions; registry publication channels cannot replace them. Both UUIDs are required. Optional cwd must be absolute; env is an overlay of at most 256 conventional UTF-8 variable names/values. `PI_RADAR_REGISTRATION_ID` is reserved for Radar. `continue_subject_id` (optional) names the caller's own released child: the daemon refuses unless that subject is the caller's child with a recorded session and no live claim, replaces any session flag in the caller's `argv` with `--session <recorded session>`, and reserves the launch on the same subject. The selected backend must advertise `mux.launch_context` before context-bearing requests are accepted; Herdr refuses multiline/control values and unrepresentable command lines before pane creation. Environment values are private and omitted from public operation records. The daemon verifies fresh session context on that exact parent registration before any pane effect; it never searches registrations by session UUID. Before dispatching the create the daemon reads and verifies the process serving the backend, records it privately on the edge, and records nothing when it cannot be verified. Edges preserve both process parent and logical session parent. The result carries the operation record beside the edge: `{ "request": RequestRecord, "spawn": PublicSpawnEdge }`. The record and edge distinguish `created`, `launched` and `bound`; each step is `completed`, `refused` or `unknown` where it applies.

### Unified Pi presence and session claims

`agent.presence` is the single Pi recognition operation. It carries the verified process birth identity, current session UUID, per-session-load `claim_id`, mutable location and optional launch correlation. Radar resolves the process's `registration_id`, persists the claim decision, and returns `granted`, `duplicate` or `pending`, with a `subject_id` only when granted. Reasserting the same report recovers the same decision; it does not create a new admission opportunity. A fresh Pi `session_start` uses a new claim ID.

A duplicate decision stays rejected after incumbent release, process exit or daemon restart. A fresh session-load boundary is required to retry a duplicate. Verification/recovery failures stay pending. Process registration and session claim are separate internally but are created/reconciled by the same report operation. Location is mutable process context, not process identity.

`agent.release` releases exactly one registration/claim pair on graceful session shutdown. It is idempotent; delayed releases cannot release a newer claim. Releasing a session leaves its durable subject, children, inbox and history intact and does not deregister the process. The sweep only detects a verified process exit; pane loss and heartbeat expiry are not releases.

### Managed lifecycle identity

`lifecycle.get` and `lifecycle.list` identify a managed logical subject. `subject_id` is the agent and is stable across its attempts; each read reports the current `claim_id`, which names exactly one attempt. `lifecycle.list` pages by `subject_id`. `lifecycle.cancel`, `lifecycle.stop` and `lifecycle.restart` require that `subject_id` and the exact `claim_id` they were issued against: a control naming a claim that is no longer current is refused before anything is signalled. A restart keeps the `subject_id` and pre-mints a fresh claim, reserving it before the previous process is signalled. There is no execution-generation ordinal; an attempt is identified by its claim alone. Publisher `registration_id` remains a separate registration handle and is never reinterpreted as a subject ID. Omitted claims are bad parameters, and stale claims are refused rather than redirected to a successor. A parent restarting its own child names itself, the subject and the program to restart as, with no claim fence; its answer carries `launch_request_id`, the spawn edge to read for the replacement.

A launch whose process has not presented within 30 seconds of its edge's `recorded_at` is withdrawn by the daemon: the claim is revoked, any captured process is stopped through its scope, the launch's own pane is closed, and the edge records `withdrawn: {at, reason}`. The subject is then free to continue. A launch that does present is recorded as `granted_at` on the edge, which is what readers treat as ready. A reader treats `withdrawn` as a failed launch; it never infers failure from absence.

### Recovery, observation and reconciliation

Three separate mechanisms, named apart because they answer different questions:

- **Observation** is steady state. The daemon samples process evidence, bound-pane evidence and recorded containers continuously — every two seconds, whether or not any client is connected — because lifecycle coverage must not depend on a display being open. A `lifecycle.get`/`lifecycle.list` result reports it under `source.observation` as `{interval_ms, last_pass_at}`; `last_pass_at` is `null` until a pass has completed, never a claim of success. Weak evidence needs confirmation: one inventory listing that omits a bound pane starts a bounded window (three fresh consecutive absences across at least six seconds), a present read clears it, and a failed, stale or incomplete scan is never absence. A confirmed pane loss ends the launch *externally* — `ended_externally_at` with an evidence summary whose reason is `pane_lost` — which is distinct from `exited_at`, set only by a verified process exit. A pane's absence is not a process's death, so it never claims one.
- **Reconciliation** re-establishes an external observation baseline after continuity or confidence was lost. It runs at daemon start and on `lifecycle.reconcile`, dispatches no effect, never retries one, and settles a record only from independent evidence: a lifecycle operation from the captured process's own birth identity, a spawn from the outcomes its edge already recorded. Everything else is reported under `unresolved` with a reason, and nothing is settled by guesswork. A recorded container is re-observed on every pass, but the observation is edge-triggered like every other transition: looking again and seeing the same thing writes nothing, so `observed_at` on a container says when its binding was last observed *to change*, not when it was last read, and `unresolved` carries the reason while the binding is not trusted.
- **Abandonment** is the operator's way out of a record that can never resolve. `lifecycle.abandon` takes `{"request_id":"<the unresolved request>"}` — the record itself, not its target, because it is the record holding that target. It releases only the administrative block: the record keeps (or gains) `outcome: "unknown"` and gains `abandoned_at`, so the uncertainty stays recorded and the target becomes operable again. It preserves every independent process and pane safety check and never authorizes a replay. Nothing is dispatched, so a record it cannot release — one that was never claimed, or one with a definite outcome — is a settled refusal (`outcome: "refused"`, `category: "refused"`) rather than an unresolved record, and never leaves a record that blocks a later attempt. The contract needs no backend primitive, so a daemon with no backend wired both advertises and serves it: a registry-only daemon still owns the records an operator may need to release.

`lifecycle.reconcile` takes no parameters and answers `{"reconciled":{"trigger":"daemon_start"|"operator_request","examined":N,"resolved":N,"unresolved":[{"request_id":"…","method":"…","target":"…","reason":"…"}],"omitted":N}}`. `omitted` counts unresolved records the bounded report does not list. A record whose outcome is already `unknown` is never rewritten: it stays unresolved and is reported, not silently re-settled.

### Managed child close

`spawn.close` is a separate durable operation for the exact daemon-created child that registered with a spawn edge's private token. It requires `spawn.close.v1` (`mux.observe` + `mux.close`). It takes the edge's `spawn_request_id`, the exact bound `(source, incarnation)` and an audit intent (`complete` or `cancel`); the caller cannot select a pane. The exact request and response are in the [canonical registry fixture](agent-registration.fixture.jsonl). The edge must exist, be bound to that child and name a pane, and the child's registration must carry a process identity claim.

The live path, for a pane that still has an occupant: immediately before admission the daemon requires fresh inventory showing the pane in its recorded workspace/tab containment, then a positive non-shell foreground PID whose procfs birth identity exactly matches the registered `(pid, boot_id, start_ticks)` tuple. A missing, unbound or mismatched edge, pane or child; no process claim; shell, invalid PID or inconclusive foreground evidence; changed containment; unavailable verification; or a backend missing `mux.observe` or `mux.close` refuses before admission and before any backend effect. A different managed child, a foreign pane, a caller-chosen pane id, and targets named by label, title, session UUID or pane position are never eligible. Foreground evidence is an adapter-side process read, so a backend that cannot answer it refuses here rather than being trusted on pane presence alone.

The dead path, for a retained pane whose command has exited: a pane reported dead has no occupant to protect, so cleanup is defined without a foreground identity — but only for an edge whose pane is provably this edge's. The daemon requires, in addition to `mux.exit_observation`, (1) the edge's own recorded exit observation for that exact pane, (2) a fresh read showing the pane `exited` now, and (3) the verified birth identity of the process serving the backend to equal the one recorded privately on the edge before its create. That last check is the generation fence: a pane id outlives the server that answered the create, and a backend restarting under the same name hands the same ids to panes Radar never created. The serving process is a claim the backend makes and the daemon verifies through procfs (`pid`, `boot_id`, `start_ticks`) — a captured instance is never a guess, and an unverifiable capture records nothing.

An edge whose creating instance cannot be confirmed now is unfenced: `spawn.close` refuses them before admission rather than inferring a generation, so a dead pane of such an edge is left alone. A backend that cannot name its serving process (Herdr, or `none`) never fences an edge, so its dead panes are never cleaned up; that is the deliberate direction of failure. Both fence reads run on the daemon's shared verification budget: at most 4 verifier jobs daemon-wide, each abandoned at its 3-second deadline, polled every 50 ms so a stop request is honoured rather than waited out. A stalled backend or verifier therefore leaves an unverified capture rather than holding the worker that is answering a caller.

Verification is not atomic with dispatch: the daemon verifies the pane and then closes it, and a same-UID process could respawn the pane in that window. That residual sits inside the local same-UID trust boundary and is accepted; do not treat a close result as proof that no other process touched the pane.

The durable record stores the request ID, edge, bound child, recorded pane and intent before dispatch. The result carries the outcome, the intent and the pane. `completed` is a backend-confirmed pane closure. `already_exited` says the pane's command had exited, the edge recorded that exit, and the retained pane was now cleaned up: it confirms retained-pane cleanup, never that the registered process itself exited, and it never authorizes session relaunch. A positive backend refusal is `refused`; a possibly dispatched close without confirmation is `outcome_unknown`. A result is returned only for a close that happened; `refused` and `outcome_unknown` are error objects, so a consumer cannot mistake them for a completed close, and no consumer may read `outcome_unknown` as "nothing was dispatched". Identical same-ID replay returns the stored result without re-verification or redispatch; different content under the same ID refuses. An unknown result is never retried automatically or through another backend. The result reports the mux pane closure only: it never claims foreground-process or process-group exit and never writes assignment or execution facts. The spawn edge and binding remain; a pane that disappears after an unknown result stays unresolved, not confirmed closed. To reconcile a lost reply, resend the exact same `spawn.close` request ID and params; it returns the stored close outcome without another effect. `mux.request.get`/`mux.request.list` read ordinary mux-operation records, not these separate managed-close records.

## Vocabulary audit

The daemon uses one word for one concept across the surface. This is the
definitive table; the rules below it state the choices that are not obvious from
the names alone.

Error codes (`error.code`): a code says what the daemon could establish, not how
bad the news is.

| Code | Means | Sent when | Consumer action |
|---|---|---|---|
| `bad_version` | The envelope names a protocol version this build does not serve | Before any method dispatch | Negotiate; this daemon is not v2 |
| `unknown_method` | The method name is not served | Before any dispatch, with no record | Keep the previous path; do not synthesize an equivalent method |
| `bad_params` | The method exists and the parameters are unusable | Before any record, effect or verification | Fix the request |
| `bad_request` | The line is not a request object, or names no request id | Before any dispatch | Fix the client framing |
| `refused` | The daemon decided not to act, and says why | Before admission, or after a positive backend refusal | Do not retry automatically; the request provably did not run |
| `busy` | The daemon is stopping | Before any record, for `spawn.close` and the registry methods | Reconnect to a running daemon |
| `backend_unavailable` | Nothing was dispatched and no trustworthy answer exists: no backend is wired, a capability is absent, or a read-only method's evidence did not answer | Reads always; an effectful request that never reached dispatch, recorded with this word as its `category` | Nothing was dispatched; fix the backend or the capability, do not retry blindly |
| `outcome_unknown` | The daemon dispatched the request and cannot say what the backend did | `spawn.close` whose outcome is `unknown`: dispatch happened and no confirmation came back | The effect is unestablished: read the durable record, or resend the same request id for the stored answer. Never retry it as though nothing ran |
| `not_found` | The named record does not exist | Reads of records and edges | Treat as absent, not as a failure to read |
| `internal` | The daemon failed while serving | Any point after the request was read | The request may or may not have had an effect: treat it as unknown |

Result keys: one object takes the method's own noun, a page takes its plural plus
`next`, and the same noun names the same concept everywhere.

| Method | Result keys |
|---|---|
| `daemon.ping` | `protocol`, `revision`, `backend`, `instance`, `contracts`, `tasks` |
| `mux.observe` | `inventory` |
| `mux.process_info` | `evidence` |
| `mux.output` | `output` (`text`, `truncated`, `revision`) |
| `mux.focus`, `mux.close`, `mux.create`, `mux.input`, `mux.report` | `request` |
| `mux.request.get` | `request` |
| `mux.request.list` | `requests` |
| `agent.presence` | `registration_id`, `claim_id`, `standing`, optional `subject_id` |
| `agent.release` | released claim standing |
| `agent.acquire` | `writer` |
| `agent.publish`, `agent.retire` | `channel` |
| `agent.get` | `agent` (`registration`, mutable context, claim standing, process evidence and channels) |
| `agent.list` | `agents`, `next` |
| `spawn.create` | `request`, `spawn` |
| `spawn.get` | `spawn` (edge fields flattened, plus `state` and `freshness`) |
| `spawn.list` | `spawns`, `next` |
| `spawn.close` | `close` (`outcome`, `intent`, `pane`, `exit_observation` for `already_exited`) |
| `events.read` | `epoch`, `events`, `cursor`, `tail`, `more`, `earliest`, or `cursor_expired` |
| `observation.get` | `epoch`, `revision`, `snapshot`, or `not_modified`/`snapshot_expired`/`snapshot_too_large` |
| `communication.publish` | `handoff`, `created` |
| `communication.inbox` | `handoffs` |
| `communication.get` | `handoff`, `receipt` |
| `communication.ack` | `receipt`, `created` |

Durable request state, and what each word means:

| Word | Field | Meaning |
|---|---|---|
| `pending` | `state` | Recorded, nothing claimed it |
| `claimed` | `state` | The daemon is about to execute it; a same-id retry must never execute again |
| `started` | `state` | Dispatch has begun; only an outcome may be recorded from here |
| `completed` | `state` | Terminal: an outcome was recorded |
| `completed` | `outcome` | The operation ran and its effects are in the record |
| `refused` | `outcome` | The operation did not run, and the record says why |
| `unknown` | `outcome` | Execution began and the daemon cannot say what it did |
| `not_executed` | derived | An expired, never-claimed record: nothing ran and nothing will, and the record can no longer execute — a caller that still wants the work sends a new request rather than reusing this id |
| `in_flight` | `category` | An unresolved request for the same target and method already exists |
| `backend_unavailable` | `category` | The backend could not satisfy the operation: it lacks the capability, or it gave no usable answer (the record's `outcome` says which) |
| `backend_refused` | `category` | The backend gave an explicit refusal before reporting success |
| `agents_alive` | `category` | A container deletion was held because an agent inside it did not exit; the place stands until the operator repeats the deletion |

State and outcome are deliberately separate: `state` says how far the durable
record got, `outcome` says what the operation did. Neither is derived from the
other, and a `started` record with no outcome is uncertain, not failed.

Managed-close outcomes are the close's own vocabulary; no mux operation or spawn
step can carry `already_exited`:

| Word | Meaning |
|---|---|
| `completed` | Backend-confirmed pane closure |
| `already_exited` | The edge recorded the pane command's exit and the retained pane was cleaned up. Never proof that the registered process exited, and never relaunch authority |
| `refused` | The backend refused before applying the close, or the daemon refused before admission |
| `unknown` | Dispatch may have happened without confirmation; never retried |

Spawn and topology words reuse those outcomes where they mean the same thing:
`created` and `launched` on an edge are `completed`, `refused` or `unknown`, and
the request's own `outcome` is the step that stopped it. A read adds only what a
read can derive: `state` is `unbound`, `bound`, or `unresolved` (a recorded pane
absent from a successful inventory), and `freshness` is `fresh` or `stale` (or
absent when there is no pane to verify). `exit_observation.state` is `unknown` or
`exited`; it is the backend's retained pane-command status, never process exit.

Two rules complete the vocabulary. First, a cursor names a resumable position:
the event journal's `cursor` is an object `{epoch, sequence}` because a position
is epoch-scoped, while a page's `next` is the last returned key as a string; both
are passed back to the same request unchanged. Second, `outcome_unknown` is the
dedicated dispatch-uncertainty code: it says the daemon may have acted
and cannot say what happened, so a consumer reads it as an unestablished effect
and never as "nothing was dispatched". `backend_unavailable` is not that code —
it is the honest undispatched answer when no backend is wired, a capability is
absent, or a read could not answer.

## Report schemas and bounds

Every report requires a valid target and nonempty `source`; state/session also require nonempty `agent`. Every text value is at most 1024 bytes and contains no control characters. The `state` vocabulary is exactly `idle`, `working`, `blocked`, `unknown`; a future word is `bad_params` until the protocol enum is extended. Sequence values are unsigned 64-bit integers and are forwarded, not ordered here.

- `state`: required `pane_id`, `source`, `agent`, `state`; optional `message`, `sequence`.
- `session`: required `pane_id`, `source`, `agent`; optional `session_id`, `session_path`, `session_start_source`, `sequence`.
- `metadata`: required `target`, `source`; optional `tokens`, `agent`, `applies_to_source`, `title`, `display_agent`, `state_labels`, `clear_title`, `clear_display_agent`, `clear_state_labels`, `ttl_ms`, `sequence`. Target is `{"kind":"pane","pane_id":"…"}` or `{"kind":"workspace","workspace_id":"…"}`. Token/state-label maps have at most 16 entries; each key is 1..=32 ASCII alphanumeric, `-`, `_`. Token value is string or null (null withdraws a token); state-label values are text. TTL, when supplied, is 1..=86,400,000 ms. Workspace metadata permits tokens only and must contain at least one; pane-only display fields are rejected for workspace target.

## Security and privacy

Socket and state directories are user-owned mode `0700`, real directories, not symlinks. Request record files are atomically created mode `0600` under the private `requests` directory; each record is capped at 64 KiB. Records retain **unredacted** canonical params for exact duplicate comparison,
including literal terminal input, metadata and spawn environment overlays; do not
send secrets you do not want persisted. Public spawn operation projections redact
environment values and tokens; this is not general redaction of arbitrary input,
argv or shell/application output. Records are not automatically pruned; unresolved outcomes must remain inspectable. Local filesystem trust is not client authentication: another process running as the same user is not isolated by this protocol. Do not expose the socket remotely.

## Extension examples

The newline-delimited request/response examples are in [`control-plane.fixture.jsonl`](control-plane.fixture.jsonl); the records there are shape references, with generated timestamps and the rest of that session's records omitted. This direct Python client demonstrates one request per connection, matching IDs, and JSON-lines framing:

```python
import json, socket, uuid

def call(path, method, params):
    request = {"version": 2, "id": str(uuid.uuid4()), "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.connect(path)
        conn.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        answer = json.loads(conn.makefile("rb").readline())
    if answer.get("id") != request["id"]:
        raise RuntimeError("mismatched control-plane response")
    return answer
```

Set `backend = "daemon"` and optionally `socket` in Radar config to use the daemon for Radar; extensions may connect directly using the resolved socket path. Use `daemon.ping` to check the protocol version and the contracts you need. Bundled Radar clients additionally require the current wire `revision`; external consumers should ignore unfamiliar contract names and pin only the contracts they use. A contract absence is an explicit refusal, not a request to substitute a different method.

Typical calls (parameter objects are exactly the method shapes above):

```python
# Read inventory; focus a pane.
inventory = call(path, "mux.observe", {})
focused = call(path, "mux.focus", {"target": "wA:p1", "target_kind": "pane", "requester": "my-extension"})

# Create an empty pane; input literal text; read output.
created = call(path, "mux.create", {"request": {"kind": "pane_split", "pane_id": "wA:p1", "direction": "right", "focus": False}, "requester": "my-extension"})
sent = call(path, "mux.input", {"request": {"pane_id": "wA:p2", "payload": {"kind": "text", "text": "hello\n"}}, "requester": "my-extension"})
output = call(path, "mux.output", {"pane_id": "wA:p2", "source": "recent", "lines": 100, "ansi": False})

# Report only facts the publisher owns; this report is schema-checked and stored.
reported = call(path, "mux.report", {"request": {"kind": "state", "pane_id": "wA:p2", "source": "my-extension", "agent": "pi", "state": "working", "sequence": 4}, "requester": "my-extension"})

# Follow the journal: resume from the delivered position, never past it.
answer = call(path, "events.read", {"after": 12, "limit": 100, "wait_ms": 1000})
read = answer["result"]  # Check errors/cursor_expired before using the page.
# Process read["events"] before advancing the cursor.
while read.get("more"):
    answer = call(path, "events.read", {"after": read["cursor"]["sequence"], "epoch": read["epoch"], "limit": 100, "wait_ms": 0})
    read = answer["result"]
    # Process this page before advancing again.
```

For `mux.close`, use the identity frozen from the same fresh `mux.observe` shown to the operator, then pass it as `request.identity`; do not construct a bare-target close. The reference fixture has the serialized shape. A disconnect after sending a mutation is ambiguous: use the UUID you sent with `mux.request.get` to read it back (or `mux.request.list` to list recent operations). If it is unresolved or unknown, stop and surface that state; do not submit another operation to “make sure”, even with a new UUID. A same-UUID replay only returns the existing record.
