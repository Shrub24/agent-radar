# Design

## Context

Radar observes Herdr by polling `herdr api snapshot` off the UI thread (`src/collector.rs`) and reconciles the result into `ObservationState`. Herdsman's facts arrive as pane tokens (`src/herdr.rs`), including `pi_bg_running`, `pi_bg_tasks` and `pi_bg_started` from `pi-bash-processes`, which `HerdsmanFacts` already carries.

The tokens are a pointer. The owning extension has the detail: for each task its id, command, working directory, pid, state, start, last output, output size, exit code and log path.

## Goals / Non-Goals

**Goals:** let the extension that owns background tasks push their detail to Radar without giving an agent anything to call or consume; keep the tokens as the baseline when no bus client is connected.

**Non-Goals:** lifecycle controls; a general plugin or RPC framework; durable history; any dependency beyond what Radar already has. See the proposal for the rest.

## Decisions

### 1. Extensions dial Radar; Radar never dials an extension

Radar binds `$XDG_RUNTIME_DIR/agent-radar/radar.sock`, or `RADAR_SOCKET` when set (for tests and nested runs; there is no settings key). With no runtime directory it uses `/tmp/agent-radar-<uid>/radar.sock`. The directory is created `0700` and verified before binding: owned by the current uid, mode `0700`, and not a symlink. A directory that fails the check means no bus, never a looser fallback, because the extension dials this path and a squatter on a shared directory would receive what it sends. The same checks are the contract for the publisher before it connects. Radar does not read the peer's credentials: that is unstable in the standard library, and a `0700` directory already admits no other user. Bus data is advisory, like tokens: any process of the same user can connect. Radar being a listener is the whole mechanism: an extension has no inbound endpoint for an agent to find, and the protocol has no receipt, acknowledgement or generation token, so nothing about it can settle a completion. `pi-bg get` stays the only way an agent consumes one.

*Rejected:* an `observe` operation on `pi-bg`'s per-session socket. It would add an inbound endpoint on the agent's side, and the same-user boundary means it could not be closed to the agent; the discipline would be as behavioural as `pi-bg`'s own.

*Rejected:* more tokens. The 32-key limit leaves no room, and a per-pane publisher would have to send every task on every refresh.

### 2. A line protocol with one state-carrying message

One JSON object per line, UTF-8. Three message types:

- `hello {v, session, pane?, ops}`: sent first, once per Pi session. `v` is the protocol version (`1`), `session` the exact Pi session UUID, `pane` the Herdr pane id (`HERDR_PANE_ID`) where the publisher has one, `ops` the operations the client supports (always `[]` in v1; it exists so a later control operation can be advertised without a protocol break). A session UUID changes on `/new` and `/resume` in the same pane, so a new session means a new `hello` on a new connection and the old connection is closed.
- `tasks {tasks: [...]}`: the session's complete current list of **unresolved** tasks. It replaces the previous list; there are no deltas, and a task that resolves is simply absent from the next list.
- A task is `{id, state, command, cwd, pid, started_at, last_output_at, output_bytes, exit_code}`. Every field except `id` and `state` is optional. Times are Unix milliseconds. `state` uses the publisher's own three-word vocabulary: `running` (the process is alive), `flushing` (it has exited and its capture is not yet certified) and `review` (it has exited, the capture is ready, and the agent has not yet retrieved it). A word Radar does not know is shown as published. `id` is `bg-<n>`.

Two publisher properties keep that true, and the contract document states both: one connection per session carries one full list per state change, with a single writer and no interleaving; and when the last task resolves the publisher sends an explicit empty list rather than going silent, so Radar clears the rows on a live connection (and the connection closing clears them otherwise).

A full replace over an ordered stream has no gap to detect and nothing to resynchronise: a reconnect starts with a fresh `hello` and a full list, and liveness is the connection, not a timer. A task list is at most a few dozen entries. The publisher throttles `tasks` to roughly once a second, because `output_bytes` changes continuously.

`command` and `cwd` are the sensitive fields: secrets turn up in command lines, and the token contract carries neither. They are optional and the publisher's redaction or omission policy is not yet decided, so the fixture treats both as absent by default. Where sent they are bounded to 256 characters by the publisher, and Radar bounds them again and writes them only to the terminal — never to a file, the configuration or a log. `log_file` is not in v1: a path to output invites reading it, and `last_output_at` with `output_bytes` already show activity.

A line is capped at 1 MiB. A line that is not valid JSON, has an unknown `v`, or arrives before `hello` closes the connection. Unknown fields and unknown message types on a valid connection are ignored, so a newer publisher does not break an older Radar.

*Rejected:* a snapshot-plus-deltas protocol. It is the smaller wire format, and the only one that needs a resync story.

### 3. Bus data is connection state, held beside the observation, not inside it

`ObservationState` reconciles Herdr's inventory and its continuity rules. Bus data is a different kind of fact: it is true only while its connection is open. A `BusState` holds `session -> tasks` and is rendered next to the observation, joined at render time. A disconnect drops that session's entry immediately. Nothing is retained: a vanished extension is not evidence that the tasks ended. Ages are measured at render from the published timestamps, not stamped once when a list arrives, because a publisher sends a full list about once a second while the panel redraws on every key press. *Rejected:* stamping the age into `BusState` at receive time — it would put a clock inside the listener's state machine to no visible benefit.

### 4. Threads, not async

The listener is one accept thread and one thread per connection, with a cap on concurrent connections (16). They send decoded messages to the app loop over an `mpsc` channel, as the collector does. No runtime and no new dependency: `std::os::unix::net`, `serde_json` and the threading Radar already has.

A stale socket file from a dead Radar is detected by trying to connect: a refusal means it is stale and is removed, success means another Radar owns it. In that case this Radar runs without the bus and says so in the fleet heading's diagnostic, rather than failing to start.

### 5. The join is the exact session UUID, with the pane as a fallback

A session's tasks are shown on the agent row whose `pi_herdsman_session` equals the `hello` session. Where a row publishes no `pi_herdsman_session` (Herdsman not loaded, `pi-bash-processes` is), the `hello` `pane` is matched against the row's Herdr pane id instead. Nothing else joins them: not the human session name, not tab position, not the working directory. A session with no matching row is held but not shown. A row with no connected session shows the token facts exactly as it does now.

When both exist, the bus list is shown and the token count is not trusted over it. `pi_bg_running` counts only `running` tasks, while the list holds every unresolved one, so the two are compared on running tasks alone. A disagreement is stated in the details, not resolved silently.

### 6. The contract is a document with a fixture

`docs/radar-bus.md` states the protocol, the throttle and the field meanings, with `docs/radar-bus.fixture.json` as a set of example lines. Both Radar's tests and the `pi-bash-processes` publisher's tests run against the fixture, as Herdsman's `pane-metadata.fixture.json` is used today.

## Risks / Trade-offs

- A different process of the same user can pose as an extension and feed Radar false rows. This is no worse than today: any local process can write a Herdr token with `herdr pane report-metadata`.
- The publisher's `pi_bg_tasks` token grammar (`<id>:<phase>`) is not final, so this change neither reads it nor puts it in the fixture; the bus list is the source of task identity and phase.
- Radar becomes a server for the first time. The failure modes are bounded by the line cap, the connection cap and the drop-on-disconnect rule.
- A second Radar on the same machine runs without the bus. Accepted for now; one Radar per user is the expected case.
