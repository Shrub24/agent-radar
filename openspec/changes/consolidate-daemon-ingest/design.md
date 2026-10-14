# Design

## Context

The daemon serves its control socket from `src/control_plane/server.rs`: one accept thread, one thread per connection, a global cap of 16, a 200 ms poll to notice a stop, and handlers that lock one shared `Core`. The TUI separately binds the bus socket from `src/bus.rs` with its own accept thread, its own cap of 16 and a near copy of the directory check and stale-socket handling. Bus state is connection-scoped and in memory by specification already, so moving it changes who owns it, not what it is.

Two upcoming changes put load on this layer: `shared-observation` adds waiting readers and revision wakes, and `tmux-control-mode` adds a persistent child stream. Both multiplex waits, timers and streams. This change rewrites the transport once, on a model that fits them, and leaves everything else alone.

## Decisions

### 1. Tokio in the transport module only

Async code lives in `src/control_plane/transport/` and nowhere else. The seam is one synchronous function: a line in, a reply out, called with the connection's lane. The existing per-line handlers become that function unchanged.

- The daemon owns a multi-thread runtime with two workers and a bounded blocking pool. `Daemon::start`, `run`, `stop` and `Drop` stay synchronous; stop uses `shutdown_timeout` and never drops the runtime from async context.
- Handlers run only on the blocking pool. The `Core` mutex is never taken on a runtime worker and never held across an await.
- The existing `stopping` flag stays the cancel signal for synchronous code. The transport itself shuts down through a `watch` channel, so `tokio-util` is not needed.
- Features: `rt-multi-thread`, `net`, `sync`, `time`, `io-util`. No macros, no `async-trait`.
- `RuntimeProvider`, `store`, `registry`, process verification, `service`, the CLI and the dashboard client are not converted.

Rejected: keeping threads now and adding Tokio with tmux control mode. The transport is being rewritten in this change; doing it twice costs more than the runtime plumbing.

### 2. Lanes admit work; they are not sockets

Each request takes a permit from one of three semaphores before it reaches the blocking pool: control, ingest and waiting. A method table maps each control method to a lane (publication methods are ingest; waiting reads are waiting; everything else is control). The compatibility bus socket is ingest. Capacity is reserved: total handler capacity exceeds the sum of the ingest and waiting limits, so control requests always have room. A request over its lane's limit is answered with a typed busy error and does not queue behind other lanes.

Starting limits, to be set from the existing constants and revisited in review: control 8, ingest 4, waiting 8, connections 32. The existing verification budget of 4 jobs stays separate and inside control.

### 3. Bounded framing is written once and tested at the edges

Tokio's line readers are unbounded, so the transport reads with an explicit bound of 1 MiB and closes a connection that exceeds it. Idle and write timeouts are explicit (120 s idle, as today, and a shorter write timeout). Tests cover 1 MiB minus one, exactly 1 MiB, one byte over, an unterminated line, an idle client and a client that never reads.

### 4. A disconnect does not cancel an admitted request

The handler runs to completion and its durable record is readable by request identifier, which is how lost replies are already designed to be recovered. Only the reply is dropped.

### 5. The bus moves to the daemon on its current path

The daemon binds the existing bus socket path with the existing bus protocol, through the transport's ingest lane. `src/bus.rs` keeps decoding and `BusState`; its listener, accept loop and directory check are deleted. The TUI stops binding it. State stays in memory and per connection and is never written to the event log or disk (the existing requirement, now stated for the daemon).

`tasks.list.v1` returns sessions ordered by session id with `connected`, the time of the last accepted list and its tasks, bounded with a `next_after` continuation. It is advertised only while the bus is bound, and added to the dashboard client's `KNOWN_CAPABILITIES`. A display that cannot read it says task detail is unavailable; it never shows an empty list for an unavailable source.

The TUI joins publisher task snapshots to agents only by the exact session the registry publishes for the agent (`agent.context`), because that is the only exact, Herdr-independent identity available. The `hello` pane field stays in the protocol and is ignored for joining. Existing `pi_bg_*` token facts remain a separate, lower-detail source on the row when no connected publisher matches; they never attach a publisher session or override its snapshot.

The end state, not in this change, is a `tasks` lane on the control socket with the same volatile semantics, `pi-bash-processes` migrated to the control protocol, and the compatibility socket deleted. Nothing here makes that harder: the lane, the volatile state and the read are already the final shape.

### 6. Waiting reads wait in the transport

`events.read` with a wait currently holds a worker for up to a second. The transport waits on a `watch` of the journal sequence instead, then calls the unchanged handler once. This is the single async-native consumer in this change. Revision wakes for `shared-observation` reuse it.

### 7. rustix replaces libc where it covers the need

In scope: `pidfd_open` and `pidfd_send_signal` (replacing raw `syscall` calls), `kill_process_group` (the `SIGKILL` of a probe's group in the herdr and tmux adapters), `getuid`, `poll` in the accept path (replaced by the runtime), `clock_ticks_per_second`, `page_size`, `tcgetattr` and the `O_NOFOLLOW | O_NONBLOCK` opens in `store` and `registry`. Out of scope: signal handler installation (`sigaction` is not in rustix) and anything in tests that is not testing a production primitive. The existing identity and process-verification tests are the guard; no guarantee changes, and the pidfd semantics stay authoritative.

## Risks

- **Runtime shutdown.** Dropping a runtime in an async context panics, and blocking tasks cannot be aborted. Mitigated by owning the runtime in `Daemon`, explicit `shutdown_timeout`, and a test that stops a daemon with a running handler.
- **Unbounded reads.** A forgotten bound is a memory exhaustion path on a socket any same-user process can reach. Mitigated by one reader and edge tests.
- **Shared `Core` lock.** The handler audit found no effectful `RuntimeProvider` operation, process-verifier wait, or await while the lock is held; a few paths call only the provider's read-only `identity()`/`capabilities()` accessors there (in-memory for the current adapters). Several paths do hold it across synchronous, filesystem-backed `Registry` transitions and operation-store reads/writes; a slow local filesystem can therefore delay a control handler despite lane reservations. Those writes also participate in request admission and registry transition ordering, so this change records the residual rather than broadening into a storage-serialization redesign. Revisit the lock boundary before adding more lanes or high-rate publishers.
- **Blocking pool exhaustion.** Lane permits are taken before the pool, and the pool is sized to the sum of the lane limits.
- **Live acceptance in progress.** The transport swap must not change behaviour the live runs rely on; the unchanged protocol suite and fixtures are the evidence, and acceptance evidence recorded before the swap does not transfer.
- **Dependency and build cost.** Tokio and rustix are new dependencies. The Nix package vendors from `Cargo.lock`; build time is unmeasured and is recorded in the verification note.

## Open choices

- Lane limits and the write timeout, above.
- Whether `tasks.list.v1` returns task detail inline or by session reference; start inline, bounded by the 1 MiB line cap and the existing per-list bounds.
- Whether the compatibility socket path moves under the daemon's runtime directory when the bus merges into the control socket.
