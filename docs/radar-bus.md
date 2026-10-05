# Radar bus

The radar bus is how an extension that owns a background task pushes the detail
behind the `pi_bg_*` pane tokens — per task its id, state, run time, output
activity and exit code. The extension dials Radar; Radar only listens and never
connects back. Today's publisher is `pi-bash-processes`.

Nothing in the protocol asks Radar to act. There is no acknowledgement, no
receipt, no generation token and no lifecycle operation: reading a task's
detail through the bus leaves that task and its result exactly as they were.
`pi-bg get` stays the only way an agent consumes a task.

## Socket

Radar binds, in this order:

1. `$RADAR_SOCKET` when set (tests, nested runs; there is no settings key),
2. `$XDG_RUNTIME_DIR/agent-radar/radar.sock`,
3. `/tmp/agent-radar-<uid>/radar.sock` when no runtime directory is set.

The directory is created `0700` and checked before binding: it must be owned by
the current uid, have mode `0700`, and not be a symlink. A directory that fails
the check means Radar runs without the bus; it never falls back to a looser
location. A publisher checks the same three properties before it connects.

Radar is a listener for the whole local user: any process of that user can
connect, and bus data is advisory like the tokens. If the socket already exists,
Radar connects to it once — a refusal means a dead Radar left it behind and the
file is removed, a success means another Radar owns it and this one runs without
the bus. Radar does not read peer credentials.

## Framing

- One JSON object per line, UTF-8, terminated by `\n`.
- A line is at most **1 MiB** (`1048576` bytes, excluding the terminator). A
  longer line closes the connection.
- The connection closes on: a line that is not valid JSON or lacks a required
  field, a `hello` naming a version other than `1`, and a `tasks` message before
  that connection's `hello`.
- At most **16** connections at once. Further connections are closed until one
  ends.
- A line that Radar cannot act on — an unknown `type`, or a field this version
  does not model — is ignored and the connection stays open, so a newer
  publisher does not break an older Radar.

## Messages

### `hello` — first message on a connection, once

```json
{"type":"hello","v":1,"session":"c1a2b3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d","pane":"8c7d:p2","ops":[]}
```

| field     | meaning                                                                                                            |
| --------- | ------------------------------------------------------------------------------------------------------------------ |
| `v`       | protocol version; `1` is the only version this Radar accepts                                                        |
| `session` | the exact Pi session UUID the tasks belong to, as published (a `hello` is per Pi session, not per pane or process)   |
| `pane`    | optional; the Herdr pane id (`HERDR_PANE_ID`) where the publisher has one. Used only as a join fallback              |
| `ops`     | operations the client supports. Version 1 advertises none, so it is always `[]`                                      |

The session UUID changes on `/new` and `/resume` in the same pane: a new session
means a new connection with its own `hello`, and the old connection is closed. A
second `hello` on one connection closes it.

### `tasks` — the complete current list

```json
{"type":"tasks","tasks":[{"id":"bg-1","state":"running","pid":48213,"started_at":1759218000123,"last_output_at":1759218074567,"output_bytes":18244}]}
```

`tasks` replaces the previous list for that session. There are no deltas: a task
that is no longer unresolved is simply absent from the next list, and it is
never reported in a final state. An empty list is a real message and means "no
unresolved tasks"; it is not the same as going silent.

### Task object

| field            | meaning                                                                                        |
| ---------------- | ---------------------------------------------------------------------------------------------- |
| `id`             | required; the publisher's task id, `bg-<n>`                                                     |
| `state`          | required; see below                                                                             |
| `command`        | optional; the raw command line the task was spawned with                                         |
| `cwd`            | optional; the task's working directory                                                           |
| `pid`            | optional; the process id while the task is alive                                                |
| `started_at`     | optional; Unix milliseconds when the task started                                               |
| `last_output_at` | optional; Unix milliseconds of the last output written                                          |
| `output_bytes`   | optional; bytes captured so far                                                                 |
| `exit_code`      | optional; present once the process has exited. Missing is not the same as `0`                    |

`state` uses the publisher's vocabulary:

- `running` — the process is alive.
- `flushing` — it has exited and its capture is not yet certified.
- `review` — it has exited, the capture is ready, and the agent has not yet
  retrieved it.

A state word Radar does not know is kept and shown as published. It is a newer
publisher's word, not a rejection.

## Publisher obligations

These are the properties Radar relies on; they are the publisher's to keep.

1. **One connection per session, one writer.** A session has one ordered
   connection and one writer on it. Messages are never interleaved, and a
   reconnect starts with a fresh `hello` and a full list.
2. **One full list per state change.** Every change to the set of unresolved
   tasks is followed by a `tasks` message carrying every unresolved task.
3. **An explicit empty list when the last task resolves.** A live connection
   sends `{"type":"tasks","tasks":[]}` rather than going silent. Radar clears
   the rows on that message; closing the connection clears them too, but the
   empty list is what distinguishes "nothing unresolved" from "no publisher".
4. **Throttle `tasks` to about once per second.** `output_bytes` and
   `last_output_at` change continuously; a state change is published promptly,
   and the byte counters ride along on that same schedule.
5. **Check the socket directory before connecting** — owned by the current user,
   mode `0700`, not a symlink — and do not connect when the check fails.
6. **Send `command` and `cwd` on every task, bounded to 256 characters.** Both
   ride on every task in every list, a task in `review` included, so a row keeps
   its label. A field that is genuinely absent is omitted — never `null` and
   never an empty string — and the bound is plain truncation. Both are ordinary
   display data: the same command line and directory a background-task dashboard
   or a fleet view already shows a same-user operator, with no redaction policy
   to keep. No log path is ever sent; `last_output_at` and `output_bytes`
   already show activity, and a path is an invitation to read the file.

Radar keeps `command` and `cwd` in memory for the running process and writes
them only to the terminal: never to a file, the configuration or a log.

Radar holds a session's tasks only while its connection is open; a disconnect
removes that session's data immediately, and a gone publisher is never evidence
that its tasks ended. Radar joins the list to an agent row by exact `session`,
falling back to the row's Herdr pane id when the row publishes no session.
`pi_bg_running` counts only `running` tasks, so Radar compares the two on
running tasks alone and states a disagreement rather than resolving it.

## Fixture

`docs/radar-bus.fixture.json` holds example lines, one JSON object per line, and
is what both Radar's tests and the publisher's tests run against. In file order
it shows: a `hello` with `pane`; a `hello` without `pane`; a `hello` carrying an
unknown field from a newer publisher; a `tasks` list with a `running`, a
`flushing` and a `review` task with an `exit_code`; a task with only `id` and
`state`, every optional field absent; a task with an unknown state word; an
explicit empty list; and an unknown message type.

## Version

Version 1 is this document. `log_file` is deliberately not a field: a path to
output invites reading it, and `last_output_at` with `output_bytes` already show
activity. A later version can add a field or a message type without breaking
this one, since unknown ones are ignored.
