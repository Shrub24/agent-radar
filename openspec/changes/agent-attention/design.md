# Design

## Context

- pi-radar opens one socket per call and closes it after the reply; an idle process holds none.
- The heartbeat is the publication path (`kick` every `RADAR_HEARTBEAT_MS` = 15 s; snapshot lease default 30 s, max 300 s). It already reaches the daemon for every live registration.
- The journal (`events::Log`) has a global sequence and epoch; each event carries an optional `subject_id`. `watchRadarEvents` treats pages as wake-ups only: callers reconcile from current reads.
- Handoff inbox counts come from `communication.pending(recipient)`.
- The TUI already has bounded MessagePack framing, class admission and connection-owned task sets that can be reused.

## Decisions

### 1. The heartbeat reply carries the hint; polling cost stays at zero connections

Add an optional `attention` object to the publish reply: `{ handoffs: <pending count>, events: { epoch, sequence } }`. It is additive; clients that ignore it behave as today. The hint is computed for the replying registration's subject; it never lists another subject's data.

Why: the round trip already exists and is authenticated by the writer handle, so the idle cost is unchanged and delivery latency is bounded by one heartbeat. Rejected: a connection per live process (one socket per idle agent), and a separate poll call (a second request per agent per interval).

The hint carries counts and a cursor, not content. The agent reads only what is flagged (`communication.inbox`) and stores `events.sequence` as its resume position.

### 2. The push tier is a wake subscription, not an event feed

The opener is JSON on the shared socket, then length-prefixed MessagePack (same upgrade as the TUI). The daemon pushes `{epoch, sequence}` whenever the journal position moves past the last position written to that connection. Positions coalesce: a slow reader receives the newest, never a queue. Epoch change is pushed as such.

Why: the only consumer treats events as a wake and reconciles from reads, so ordered pages and gaps would add replay, retention and acknowledgement machinery for no behaviour. Rejected: ordered page push now (needs per-subscriber cursors, backlog bounds and gap signalling; do it when a consumer needs bodies, e.g. an audit shipper). The opener carries a `mode` field defaulting to `wake` so ordered delivery can be added without a new opener.

### 3. Who holds the connection, and for how long

Only a process whose execution activity is working or blocked (waiting on a prompt, questionnaire or child) may hold the subscription. After it has been idle for `IDLE_HOLD_MS` (default 30 s, longer than one heartbeat so a brief pause does not churn) it closes the connection. It reopens when activity resumes and resumes from the stored hint cursor. A missed push is always recovered by the next heartbeat hint, so correctness never depends on the connection being open.

Why: only a working lead benefits from sub-heartbeat wake latency; idle agents are covered by the hint. Rejected: always-on subscription; per-process timers that outlive the process's activity state.

### 4. Admission and bounds

Wake subscriptions are a new connection class with a small fixed budget separate from ordinary and UI classes, so they cannot starve control calls or the TUI. Refusal is a typed `busy`; the agent falls back to the heartbeat hint and retries after a backoff. A subscription ends when the connection ends; the connection owns its tasks as in the duplex change. Writes have the existing write deadline; a stalled reader is closed. The pushed frame is a few bytes and has a hard size bound.

### 5. pi-radar behaviour

One attention loop per process, owned by the publisher: on each heartbeat reply, if `attention.handoffs > 0` or the sequence advanced past the stored cursor, call the existing wake handler; the handler reconciles from reads, as today. While the push tier is open, a pushed position triggers the same handler. The loop deduplicates wakes by cursor. All failures degrade to the heartbeat path; none is surfaced as an incident unless the daemon is present and refuses repeatedly.

### 6. Codec

pi-radar uses `radar-msgpack.ts` for the wake connection only. Integers beyond 2^53 arrive as `bigint`; the sequence is stored as a bigint-safe value. Every other call remains JSON.

### 7. Out of scope

Ordered event page push; observation push; subscriptions for non-Pi consumers (widgets need a JSON helper); changing the TUI path; assignment pending counts (no daemon state names a pending assignment for a registration today, so the hint omits it until a consumer defines one).

## Risks / Trade-offs

- Heartbeat latency for idle agents is up to 15 s → acceptable for background work; a working lead has the push tier.
- A clock-like hint can wake agents spuriously on unrelated journal activity → the sequence is global, so wakes are cheap reads; per-subject filtering is deferred until measured.
- The hint adds a field to a hot reply → additive, optional, bounded to two small values.
- Idle-hold tuning → one constant, documented; no adaptive logic.

## Validation

Routine gate. Focused evidence: hint present and scoped to the replying registration; unchanged clients unaffected; push reflects newest position under a stalled reader; slow reader closed without affecting others; budget exhausted yields `busy` and heartbeat still delivers; idle hold closes and activity reopens from the cursor; TypeScript and Rust agree on position integers beyond 2^53 using the shared fixtures.
