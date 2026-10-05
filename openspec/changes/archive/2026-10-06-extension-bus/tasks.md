# Tasks

## 1. Contract

- [x] 1.1 Write `docs/radar-bus.md` (socket location, messages, field meanings, throttle, limits, and the publisher's obligations: one writer and one full list per change on one ordered connection per session, an explicit empty list when the last task resolves) and `docs/radar-bus.fixture.json` with example lines covering a `running` task, a `review` task with an exit code, a `flushing` task, optional fields absent, an explicit empty list, an unknown field, an unknown state word, no `command` or `cwd` on the default examples, a `hello` with and without `pane`, and an unknown message type; verify the fixture parses as one JSON object per line.

## 2. Listener and protocol

- [x] 2.1 Decode the line protocol into typed messages in a new `src/bus.rs`, applying the line cap, version check, `hello`-first rule and unknown-field tolerance; verify decode tests run against the fixture and cover each malformed-connection case.
- [x] 2.2 Add the listener: bind in a verified `0700` directory owned by the current user (`RADAR_SOCKET` overrides the path), detect a stale or owned socket, accept with the connection cap, and deliver decoded messages over a channel; drop a session's data on disconnect. Verify with a stub client: connect, hello, tasks, replace, disconnect, a second Radar owning the socket, a stale socket, a directory owned by another user or with loose permissions, a session change in the same pane, and the connection cap.
- [x] 2.3 Hold `session -> tasks` in a `BusState` beside the observation and wire the listener into the app loop and `main`; verify Radar still starts and quits promptly with no client, and when the socket cannot be bound.

## 3. Presentation

- [x] 3.1 Join tasks to the agent row by exact `pi_herdsman_session`, falling back to the `hello` pane only where the row publishes no session, and draw them in the details with id, state, run time, last-output age, command and, where published, working directory and exit code; verify rendering tests cover a `running` task, a `review` task with an exit code, an unknown state word, absent optional fields, control sequences, an over-long command, no matching agent, a different session UUID and the pane fallback.
- [x] 3.2 Show the bus list over the token count and note a disagreement; compare the count to `running` tasks only; verify tests cover agreement, disagreement, a `review` task with `pi_bg_running` of zero, and no bus connection.

## 4. Verification

- [x] 4.1 Update the README and `plan.md`; verify `cargo fmt --check`, Clippy with `-D warnings`, `cargo test --locked` and the PTY smoke pass, and that the PTY smoke still exits promptly with a client connected and then killed.
