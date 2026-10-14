# Background-task publisher bridge

The publisher protocol carries the detail behind `pi_bg_*` task tokens: task id,
state, command, working directory, process id, output activity and exit code.
`pi-bash-processes` is the current publisher. This is an advisory snapshot feed,
not a control channel: Radar sends no acknowledgement or command, and reading a
task does not consume its output. `pi-bg get` remains the way to retrieve a
background task's output.

Radar's daemon now owns ingest and task state. The TUI is only a client of
`tasks.list.v1`; it does not bind a publisher socket or apply publisher messages.
For compatibility, the daemon temporarily accepts publishers on the existing
bus socket path as a second socket entry into the same bounded Tokio transport.
The versioned control socket and this compatibility entry share transport,
trust checks, runtime, shutdown and lane admission. This temporary entry can be
removed after the publisher migrates to the control socket's ingest lane.

Task state is volatile, in-memory and connection-scoped. It is not written to
the durable event log or operation store. A daemon restart loses the current
snapshot; a publisher must reconnect and send a full list.

## Socket and trust

The compatibility socket path resolves in this order:

1. `$RADAR_SOCKET` when set (tests and nested runs),
2. `$XDG_RUNTIME_DIR/agent-radar/radar.sock`,
3. `/tmp/agent-radar-<uid>/radar.sock` when no runtime directory is set.

The daemon creates a missing parent with mode `0700` and checks that it is a
real, non-symlink directory owned by the current uid with exactly that mode.
An existing live socket or an untrusted path prevents startup; Radar does not
fall back to a looser path. Filesystem trust is not authentication between
processes running as the same user. The publisher performs the same directory
check before connecting.

## Framing and ownership

- UTF-8 JSON Lines, one object per line, terminated by `\n`.
- A line is at most **1 MiB** excluding the terminator. Longer input closes the
  connection.
- Unknown message types and unknown fields are ignored; malformed required
  fields, an unsupported protocol version, or `tasks` before `hello` closes
  that connection.
- Shared transport admits at most 32 connections across its socket entries,
  with reserved handler capacity for control (8), ingest (4) and waiting reads
  (8). Excess work is bounded; it cannot consume the control lane's reserved
  capacity.
- A session has one current publisher connection. A new connection greeting
  the same session replaces and closes the previous connection. Disconnecting
  the current owner clears that session's volatile task snapshot.

## Messages

### `hello`

The first message on a connection identifies the Pi session:

```json
{"type":"hello","v":1,"session":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d","pane":"8c7d:p2","ops":[]}
```

`session` is the exact Pi session UUID; tasks are keyed by it, not by pane or
process. `pane` is an optional legacy hint and is never used as a task identity
or join fallback. Version 1 advertises no operations, so `ops` is `[]`.

### `tasks`

A message replaces the complete current list for the greeted session:

```json
{"type":"tasks","tasks":[{"id":"bg-1","state":"running","pid":48213,"started_at":1759218000123,"last_output_at":1759218074567,"output_bytes":18244}]}
```

There are no deltas. An empty list means no unresolved tasks; a disconnect means
there is no current publisher and removes the volatile session detail. Neither
case proves that an underlying process exited.

Each task has a required publisher `id` and `state`, and optional `command`,
`cwd`, `pid`, `started_at`, `last_output_at`, `output_bytes` and `exit_code`.
Optional fields are omitted when absent. `state` values currently used are:

- `running` — the process is alive.
- `flushing` — it exited, but output capture is not yet certified.
- `review` — capture is ready and has not yet been retrieved.

Unknown state words are preserved and displayed as published.

## Read contract and display join

Negotiate `tasks.list.v1` in `daemon.ping`, then call `tasks.list` with optional
`after` and `limit` (default 100, maximum 256). A page returns a daemon-local
`revision`, `sessions`, and `next_after`. Each session item includes its exact
`session` UUID, `connected`, `last_updated_at` and complete `tasks` list. Follow
`next_after` immediately to drain pages. If the revision changes while paging,
discard the partial result and read again; do not combine different snapshots.

Radar joins task detail to a registry row only by exact session UUID. It never
falls back to `pane`, aliases or process identity. Unmatched task sessions
remain task detail, not fabricated agent rows. The revision is a snapshot
version, not a durable cursor and not comparable across daemon restarts.

`command` and `cwd` are held in daemon memory and shown as ordinary same-user
display data. They are not written to a file, configuration, log or durable
lifecycle event. No output path is sent or opened by Radar.

## Publisher obligations

1. Keep one ordered connection per session. Reconnect with a fresh `hello` and
   immediately send the full current task list.
2. Send a full `tasks` list after changes, including an explicit empty list when
   the last unresolved task goes away. Throttle continuously changing output
   counters to about once per second while publishing state changes promptly.
3. Send bounded `command` and `cwd` strings on each task when available; do not
   send an output-file path.
4. Check the socket directory before connecting. Treat connection refusal as
   the daemon being unavailable, not as evidence that a task finished.

## Fixture

`docs/radar-bus.fixture.json` holds example protocol lines. Unit tests validate
decoding; the publisher fixture runs against a disposable daemon and checks the
served task read and registry-session join.

## Version

Version 1 is the publisher wire format. Unknown types and fields are ignored so
a newer publisher does not break an older reader. The separate compatibility
socket is temporary; migrating it does not change task authority or the
volatile snapshot semantics.
