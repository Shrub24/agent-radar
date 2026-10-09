# Radar control-plane protocol v1

For the overall surface, lifecycle and adoption path, start with the
[daemon consumer guide](daemon.md).

This is the wire reference for local clients, including extensions. The daemon is a backend-neutral **physical mux wrapper**. It does not grant agent assignment authority or register mux topology. Herdsman remains the owner of managed-worker lifecycle and assignment authority; Radar does not start or supervise the daemon. The daemon also serves additive direct agent-registry methods independently of the mux backend; see the versioned [agent-registration reference](agent-registration.md) for their full contract and reconnecting publisher example. These registry methods do not add restart/resume or physical lifecycle controls.

## Start and select

Run `radar daemon` separately. It prints the socket and record directory, serves until SIGINT/SIGTERM, and removes only its own socket on shutdown. It does not start a TUI. Radar's `[runtime]` config selects its adapter:

```toml
[runtime]
backend = "daemon" # or direct (the default)
# socket = "/run/user/1000/agent-radar/control.sock"
```

An omitted socket uses `RADAR_CONTROL_SOCKET`, then `$XDG_RUNTIME_DIR/agent-radar/control.sock`, then `/tmp/agent-radar-<uid>/control.sock`. The daemon record root uses `RADAR_CONTROL_STATE`, `$XDG_STATE_HOME/agent-radar/control`, then `~/.local/state/agent-radar/control`, with `/tmp/agent-radar-<uid>` as last resort when neither state-home nor HOME exists.

Before dialing, Radar checks the socket path read-only: socket and parent must not be symlinks; the parent must exist, be this user's real directory with mode exactly `0700`; the socket must be a socket owned by this uid. This also applies to a configured override. The daemon uses the same directory rule when binding (and creates a missing directory as `0700`); paths it could not bind are rejected before connection. A failed trust check or handshake yields one diagnostic and selects direct Herdr before observations/actions. Radar never starts the daemon and never changes adapter after selection.

## Transport and envelope

Unix-domain socket; UTF-8 JSON Lines, one request object and one response object per line. Protocol version is `1`. Request ID is a canonical UUID. Parameters are an object; omitted/null params mean `{}`. A request is:

```json
{"version":1,"id":"8a1f5c30-6f4b-4c58-9c7b-2d0e1a9f4b22","method":"ping","params":{}}
```

Success: `{"id":"…","result":{…}}`. Error: `{"id":"…","error":{"code":"bad_params","message":"…"}}`. Codes: `bad_version`, `unknown_method`, `bad_params`, `bad_request`, `refused`, `busy`, `backend_unavailable`, `not_found`, `internal`. Messages are for people; switch on codes and record outcomes, not message text. A decodable id is echoed on refusal; malformed/oversized lines may be answered with empty id. Unknown methods refuse and the connection remains usable.

Lines are capped at 1 MiB excluding newline; an oversized line is drained and refused. At most 16 connection workers serve simultaneously; excess connections are closed without a protocol response and are not accepted. This connection ceiling is the daemon's capacity limit; `busy` is a reserved code, not emitted by this version. A connection idles at most 120 seconds, response writes have a 2-second timeout. Backend calls run synchronously on these bounded workers. Stop closes active streams and joins workers. Radar's client dials per request; each wait checks cancellation in 10 ms increments, with a 5-second exchange budget. Cancellation abandons the caller's wait, not accepted work. No authentication beyond local filesystem socket-path trust is provided; this is not remote transport or same-user process authentication.

## Capabilities

`ping` returns `{"protocol":1,"backend":"runtime","capabilities":[...]}` (or backend `none`). Mux capabilities come from the backend; `agent_registry` is additive and advertised regardless of backend. Known capabilities:

| Method(s) | Capability |
|---|---|
| `observe` | `observe` |
| `process_info` | `process_info` |
| `focus` | `focus` |
| `close` | `close` |
| `create` | `creation` |
| `input` | `input` |
| `output` | `output` |
| `report` | `reporting` |
| Agent registry methods | `agent_registry` |

`agent_registry` is advertised independently of backend capabilities, including when the backend is `none`. Radar requires the first four at startup; other missing capabilities do not prevent handshake and refuse only their method. Current production backend is Herdr. There is no production tmux backend. `registry` and `resume` (and other unknown names) return `unknown_method`. Registry request schemas are documented once in the [agent-registration contract](agent-registration.md); methods are `agent.register`, `agent.acquire`, `agent.publish`, `agent.retire`, `agent.context`, `agent.get` and `agent.list`.

## Methods

All method-specific parameter objects reject unknown keys unless noted. `ping`,
`request` and `requests` currently ignore unrecognized parameter keys. Identifiers are
nonempty, at most 256 UTF-8 bytes, and contain no control characters. JSON serialization of normalized observation/evidence uses the Rust model's snake_case enums; unknown semantic/status values use `{"other":"value"}`. Durations are integer milliseconds; absent optional facts remain absent/null as defined by the normalized model.

### Reads

- `ping`, params `{}`: protocol/backend/capabilities as above.
- `observe`, params `{}`: `{ "inventory": FleetObservation }`. No record.
- `process_info`, params `{"pane_id":"wA:p1"}`: `{ "evidence": ForegroundEvidence }`. No record; inconclusive evidence is a valid result.
- `output`, params `{"pane_id":"wA:p1","source":"recent","lines":100,"ansi":false}`. `source` is `visible`, `recent`, `recent_unwrapped`, or `detection`; `lines` is optional (runtime default), otherwise 1..=500; `ansi` defaults false. Result is `{ "output": {"text":"…","truncated":false,"revision":null} }`. Text is capped at 64 KiB on a UTF-8 boundary and `truncated` set. No record.
- `request`, params `{"id":"<canonical UUID>"}`: `{ "request": RequestRecord }`, or `not_found`. No record for the lookup.
- `requests`, params `{}` or `{"limit":50}`: `{ "requests": [RequestRecord,…] }`, newest first; default 50, maximum 200 (larger counts clamp to 200). No record.

Read errors do not create operation records. Missing backend/capability returns `backend_unavailable`; backend read refusal for `output` returns `refused` and uncertain output returns `backend_unavailable`.

### Durable operations

`focus`, `close`, `create`, `input`, and `report` are mutations. Accepted requests are atomically recorded before dispatch, and a `started` record is persisted before backend I/O. The caller should provide a fresh UUID per new operation. Same ID and same method/target/requester/JSON params returns the existing record without replay; same ID with different contents refuses. JSON object key order is immaterial, other values are significant. A target with pending, claimed, started, or unknown outcome suppresses a conflicting mutation across methods (`in_flight`); an unscoped request has its own lane. A backend lacking a capability is recorded as a completed/refused request with `backend_unavailable`, not started or dispatched. Invalid params refuse before recording. When all 16 connection workers are occupied, an excess connection is closed before it is accepted; no protocol response or record is produced.

Records contain `id`, `method`, optional `requester`, optional `target`, stored `params`, `requested_at`, `expires_at`, `state`, optional `outcome`, `category`, `message`, `effects`, `created`, `completed_at`. (`fingerprint` is a legacy duplicate-comparison field; a record without `params` refuses same-id replay because it cannot prove content equality.) Times are RFC3339 UTC with milliseconds. States: `pending`, `claimed`, `started`, `completed`; terminal outcomes: `completed`, `refused`, `unknown`. New operation requests expire after 30 seconds if never started; expired unstarted records derive `not_executed`. Claimed/started or completed-unknown records are uncertain and suppress replay indefinitely; clients must not infer non-execution from timeout, disconnect, cancellation, process disappearance or a missing reply.

Effectful backend refusals can settle `outcome: refused`; confirmed actions settle `completed`; possible dispatch without trustworthy outcome settles `unknown`. A record in `started` without outcome is also unknown. `internal` is not proof of non-dispatch. The client maps protocol pre-dispatch errors to refusal; `internal`, malformed/lost replies and post-write transport failure are unknown. A write failure before its first byte is refusal; after any byte it is unknown. Re-read with `request`/`requests` to learn the durable record. Never automatically retry an uncertain mutation, including through the direct adapter. Retrying the same UUID only retrieves the record; it does not execute again.

- `focus`: `{"target":"wA:p1","target_kind":"pane","requester":"extension-name"}`. `target_kind` is `pane` or `workspace`. Completed effect is `focus`.
- `close`: `{"request":{"target":{"pane":"wA:p1"},"identity":{"pane":{"pane_id":"wA:p1","occupant":null}}},"requester":"extension-name"}`. `target` is externally tagged `{"pane":"id"}` or `{"tab":"id"}`. `identity` is a frozen `TargetIdentity`: pane form is `{"pane":{"pane_id":"…","occupant":null-or-AgentIdentity}}`; tab form is `{"tab":{"tab_id":"…","members":[PaneIdentity,…]}}`. AgentIdentity carries `name`, `session`, `lineage`, `managed`, `label`, `run`; PaneIdentity carries `pane_id`, `occupant`. Freeze from a fresh observation at confirmation time, then send that exact identity. The daemon re-observes and checks equality and positive-unmanaged containment; changed, managed, uncertain, or mixed tabs refuse. Completed means runtime confirmed close, not that removal was re-observed. Managed worker close/restart remains owner-routed, never this method.
- `create`: `{"request":{"kind":"pane_split","pane_id":"wA:p1","direction":"right","focus":false},"requester":"extension-name"}`. Tagged request variants: `{"kind":"workspace","focus":false}`, `{"kind":"tab","workspace_id":"wA","focus":false}`, `{"kind":"pane_split","pane_id":"wA:p1","direction":"down","focus":false}`. Directions `right`/`down`. Creates a mux location only; it does not launch a process or restore an agent. Record `created` is `{ "kind":"workspace|tab|pane", "id":"runtime-reported-id" }` only when backend names it; do not infer it from request or effect prose.
- `input`: `{"request":{"pane_id":"wA:p1","payload":{"kind":"text","text":"hello\n"}},"requester":"extension-name"}`. Exactly one tagged payload: text or keys. Text is 1..=4096 bytes; control characters refused except newline/tab. Keys are 1..=16 names, each 1..=32 ASCII letters/digits/`+`/`-`/`_`. Key names are passed to backend; use only names it supports. Text is literal input, not shell evaluation.
- `report`: `{"request":{"kind":"state","pane_id":"wA:p1","source":"publisher","agent":"pi","state":"working","sequence":4},"requester":"extension-name"}`. Tagged variants `state`, `session`, `metadata`; see schemas below. This is a curated normalized schema, not a generic Herdr RPC passthrough. Unknown fields/kinds refuse. It forwards the caller's own values; it does not merge them into a registry or establish authority. `resume_argv`, `release_agent`, and `clear_agent_authority` are unsupported.

### Report schemas and bounds

Every report requires a valid target and nonempty `source`; state/session also require nonempty `agent`. Every text value is at most 1024 bytes and contains no control characters. The `state` vocabulary is exactly `idle`, `working`, `blocked`, `unknown`; a future word is `bad_params` until the protocol enum is extended. Sequence values are unsigned 64-bit integers and are forwarded, not ordered here.

- `state`: required `pane_id`, `source`, `agent`, `state`; optional `message`, `sequence`.
- `session`: required `pane_id`, `source`, `agent`; optional `session_id`, `session_path`, `session_start_source`, `sequence`.
- `metadata`: required `target`, `source`; optional `tokens`, `agent`, `applies_to_source`, `title`, `display_agent`, `state_labels`, `clear_title`, `clear_display_agent`, `clear_state_labels`, `ttl_ms`, `sequence`. Target is `{"kind":"pane","pane_id":"…"}` or `{"kind":"workspace","workspace_id":"…"}`. Token/state-label maps have at most 16 entries; each key is 1..=32 ASCII alphanumeric, `-`, `_`. Token value is string or null (null withdraws a token); state-label values are text. TTL, when supplied, is 1..=86,400,000 ms. Workspace metadata permits tokens only and must contain at least one; pane-only display fields are rejected for workspace target.

## Security and privacy

Socket and state directories are user-owned mode `0700`, real directories, not symlinks. Request record files are atomically created mode `0600` under the private `requests` directory; each record is capped at 64 KiB. Records retain **unredacted** canonical params for exact duplicate comparison, including literal terminal input and metadata; do not send secrets you do not want persisted. Records are not automatically pruned; unresolved outcomes must remain inspectable. Local filesystem trust is not client authentication: another process running as the same user is not isolated by this protocol. Do not expose the socket remotely.

## Extension examples

The complete newline-delimited request/response examples are in [`control-plane.fixture.jsonl`](control-plane.fixture.jsonl). This direct Python client demonstrates one request per connection, matching IDs, and JSON-lines framing:

```python
import json, socket, uuid

def call(path, method, params):
    request = {"version": 1, "id": str(uuid.uuid4()), "method": method, "params": params}
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as conn:
        conn.connect(path)
        conn.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        answer = json.loads(conn.makefile("rb").readline())
    if answer.get("id") != request["id"]:
        raise RuntimeError("mismatched control-plane response")
    return answer
```

Set `backend = "daemon"` and optionally `socket` in Radar config to use the daemon for Radar; extensions may connect directly using the resolved socket path. Use `ping` to negotiate protocol and capabilities. A capability absence is an explicit refusal, not a request to substitute a different method.

Typical calls (parameter objects are exactly the method shapes above):

```python
# Read inventory; focus a pane.
inventory = call(path, "observe", {})
focused = call(path, "focus", {"target": "wA:p1", "target_kind": "pane", "requester": "my-extension"})

# Create an empty pane; input literal text; read output.
created = call(path, "create", {"request": {"kind": "pane_split", "pane_id": "wA:p1", "direction": "right", "focus": False}, "requester": "my-extension"})
sent = call(path, "input", {"request": {"pane_id": "wA:p2", "payload": {"kind": "text", "text": "hello\n"}}, "requester": "my-extension"})
output = call(path, "output", {"pane_id": "wA:p2", "source": "recent", "lines": 100, "ansi": False})

# Report only facts the publisher owns; this report is schema-checked and stored.
reported = call(path, "report", {"request": {"kind": "state", "pane_id": "wA:p2", "source": "my-extension", "agent": "pi", "state": "working", "sequence": 4}, "requester": "my-extension"})
```

For close, use the identity frozen from the same fresh `observe` shown to the operator, then pass it as `request.identity`; do not construct a bare-target close. The reference fixture has the serialized shape. A disconnect after sending a mutation is ambiguous: use the UUID you sent with `request` to read it back (or `requests` to list recent operations). If it is unresolved or unknown, stop and surface that state; do not submit another operation to “make sure”, even with a new UUID. A same-UUID replay only returns the existing record.
