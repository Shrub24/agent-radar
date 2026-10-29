# Tasks

Implemented through the handoff producer first; the common lane exists, the
policies and consumers for further body kinds do not. Tests target owned
delivery and identity invariants rather than a generic broker.

## 1. Contract and consumer seam

- [x] 1.1 Rust-owned `Message` (tagged `handoff` body), `AgentIdentity`, frozen
  `recipients`, `MessageReceipt`, bounded `MessagePage` and `Attention`, with
  generated JSON and binary contracts. Message kinds are added with their
  producer and consumer, never as an open payload.
- [x] 1.2 Herdsman's handoff port keeps its durable-capture-then-ack seam; the
  SDK exposes `RadarMailboxPort` and `createRadarAttention` for the consumer to
  register with. Gap: Herdsman's own adapter lives outside this repository and
  has not been exercised against the new lane.

## 2. Shared message module and handoff cutover

- [x] 2.1 One typed message module: immutable idempotent publish, atomic frozen
  fanout (one record holding the whole recipient set), bounded page/get,
  per-recipient receipt, restart rebuild and retention. Verified: whole-set
  write, per-recipient receipt independence, unchanged pending after rebuild,
  capacity refusal without eviction, page continuation.
- [x] 2.2 Access derives from verified execution context; the handoff producer
  policy is parent-only; unmanaged or unresolved callers are refused.
- [x] 2.3 The daemon, SDK and handoff capability switched in one step and the
  old handoff store, envelope and capability were deleted. Incompatible stored
  records now fail startup with the directory named, rather than migrating.

## 3. Heartbeat and inbox consumer

- [x] 3.1 Attention is derived from recipient mailbox state only, exposed on an
  accepted `agent.publish` reply, with a typed unavailable outcome that leaves
  presence accepted. Verified: an addressed message moves that recipient's
  attention and not another subject's; receipt moves it back.
- [x] 3.2 One startup/heartbeat/push drain with a rerun flag, receipt only after
  durable capture. Verified in the SDK: overlapping triggers run one drain, a
  failed capture stays pending, and an unchanged revision still retries.

## 4. Optional push tier

- [x] 4.1 Gap-free baseline and latest-wins attention over bounded MessagePack
  framing on its own 16-connection budget, with a 4 KiB frame cap and the
  existing write deadline. Verified over a real socket: baseline reflects the
  mailbox, acceptance and receipt each push, and the subscription never carries
  content. Gap: a deliberately stalled subscriber's teardown is covered by the
  write deadline path, not by a dedicated test.
- [x] 4.2 Subscription lifetime follows the agent's activity: continuous
  `idle` starts the 30 s hold, `waiting` does not, activity cancels it, and a
  refused subscription falls back to heartbeats with backoff. Verified with a
  controlled scheduler.
  Gap: pi-radar exposes the activity input; reading Herdsman's status into it
  belongs to the Herdsman adapter and is not wired here.

## 5. Convergence and closeout

- [x] 5.1 Replaced paths deleted (`communication.rs`, the handoff server module,
  the `communication.handoff.v1` capability, the handoff schemas), docs and
  plan updated.
- [ ] 5.2 Gate: fmt, strict Clippy, Rust targets, SDK tests, schema/type checks,
  strict OpenSpec validation, rationale lint and whitespace — see the change's
  final commit message. Live acceptance of the Herdsman consumer and of an
  actual idle Pi process dropping its socket is not covered here.
