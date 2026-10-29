# Addressed agent messages

## Context and correction

The previous draft used the global event journal as an attention clock. That conflated facts with messages and would wake unrelated agents. Even a per-subject raw-event cursor would leave pi-radar interpreting daemon internals to infer notification intent.

Today the durable handoff store already owns immutable content and separate receipts. Its public operations are `communication.publish/inbox/get/ack`; publishing is restricted to a managed worker's recorded parent. `watchRadarEvents` is an exported helper without a caller in the extension. These are foundations and a migration surface, not the shape of the new architecture.

## Goals / Non-goals

Goals: one addressed lane, durable pending work, execution-independent recipients, bounded delivery, no idle socket requirement, and explicit agent-side policy. Generalise the model, not just the old count field.

Non-goals: a general event bus, broker, topic wildcard language, arbitrary command execution by message, a second domain actor, raw-event subscriptions, observation push, or an async rewrite of domain logic. Assignment changes and child-exit notices are possible future message kinds, not automatically produced by this change.

## Architecture and ownership

```
typed producer                     consumer
  selects intent and recipients      pi-radar dispatches typed body
           |                         durable local capture
           v                                |
  message lane: publish / inbox / get / ack  |
  immutable message + recipient receipts <--+
           |
  per-subject attention {revision, pending}
           +---- heartbeat reply ------------+
           +---- optional push --------------+--> same mailbox drain
```

| Owner | Owns | Does not own |
|---|---|---|
| Domain or Pi producer | Message intent, body, allowed recipient selection | Delivery or recipient agent policy |
| Daemon message module | Validation, routing grants, persistence, inbox, receipts, retention, attention | Assignment settlement, LLM prompting, deciding every fact deserves a message |
| pi-radar | Verified process connection, inbox consumption, typed dispatch, retry and durable-capture integration | Inferring notification intent from raw events |
| Herdsman / extension consumer | Meaning of the body, durable local incorporation, whether/how to notify or awaken an agent | Daemon mailbox persistence |

The message module exposes a small typed interface: publish, pending-page, get, receive (ack), attention and watch-attention. JSON handlers and the binary subscription are adapters to it. Heartbeats do not scan journal pages, sockets do not own message lifetime, and handlers do not reimplement recipient checks.

### Three independent lanes

- **Observation:** current fleet facts; latest coherent snapshot can replace an older one.
- **Raw events:** ordered historical transitions for observers and audit; retention/gaps belong to that interface.
- **Messages:** producer-addressed intent/content; retained until each recipient records receipt. Only attention hints coalesce.

There is no journal-to-notification bridge by default. A future producer may deliberately turn a domain fact into a message. It must define the recipient and meaning; attaching a subject ID to an event is not enough.

## Domain model

- **Subject:** stable logical recipient across registrations and claims.
- **Execution:** current registration/session/claim evidence used to grant access and record provenance, not an inbox address.
- **Message:** immutable ID, body kind/version, producer provenance, canonical recipient set, optional correlation and creation time.
- **Delivery:** one message addressed to one subject. A multi-recipient message has independent deliveries.
- **Receipt:** durable capture by a recipient-side consumer, not read, understood, acted on, or domain completion.
- **Attention:** a small derived description of a subject's pending deliveries; not a replay cursor.

### Envelope and typed bodies

A message has one stable UUID and one immutable body. Use an explicit tagged body enum, initially `handoff` (summary, content and optional domain correlation). Reuse the current 384 KiB handoff-content ceiling. Producer provenance is either a verified agent execution or a typed daemon producer; callers cannot forge it.

The envelope records a sorted, deduplicated, nonempty set of at most 32 subject IDs. Recipient sets are resolved and authorised before acceptance and then frozen. New children or group members do not retrospectively receive an old message. Symbolic groups, selectors and automatic membership subscriptions are deferred; a producer can expand a permitted set to explicit subjects when that use case arrives.

There is no arbitrary `Value` notification bag or user-supplied event filter. Add a body only with its producer, routing policy, consumer and tests. The first producer remains worker-to-parent handoff. The common lane supports recipient sets, but that does not grant a worker arbitrary multicast rights.

### Identity and grants

The daemon derives the calling subject and execution from current verified registration context on publish, inbox, get, ack and subscribe. A registration ID supplied by a client is a lookup input, not authorisation. The daemon is a trusted local service, not a new hostile-user authentication system; preserve existing writer/context checks.

The current handoff producer can address only its daemon-recorded parent. Root, unrelated and unresolved targets refuse. Future kinds get explicit producer policies rather than inheriting a universal send permission.

A verified successor of a subject accesses the same deliveries and can retry the same receipt. Stale executions lose access, including existing subscriptions: revalidate on recipient-context changes/lease loss and before exposing a new attention update. A connection must not continue receiving because its opener was once valid. A receipt records the first accepted execution for provenance without turning a claim ID into recipient identity.

## Persistence, publication and recovery

Publish validates the entire body, frozen recipient set, authorisation and all capacity limits before accepting. Repeating an ID with identical immutable content returns the existing message; different content or recipients refuse.

One durable message record contains its canonical recipient set. Publish exposes that record atomically, then updates derived indexes and wakes. Recipient indexes rebuild from message and receipt records; a crash must not leave half a fanout published. Accepted means persisted according to the existing communication-store durability policy, not merely put on a socket.

Receipts are separate durable records keyed by message ID and recipient subject. Retrying ack returns the first receipt. Pending is derived from accepted messages minus receipts, not from an in-memory counter. Startup reconstructs pending attention before advertising readiness. A crash after message persistence but before notification still leaves inbox work; a crash after local capture but before ack permits redelivery and consumer deduplication.

For a future daemon producer coupled to a domain transition, do not promise atomicity between independent stores. That producer must persist a delivery intent with the authoritative transition and retry idempotent publication using a stable message ID. Its receipt is not required for the originating operation to complete. This change adds no generic outbox engine or lifecycle producer.

### Storage limits and retention

Initial bounds reuse the existing store ceilings: 128 pending deliveries per subject, 512 retained message bodies and 64 MiB aggregate content, with the existing summary/content bounds. Fanout adds bounded recipient/receipt metadata and is charged before admission, including a 32-recipient cap. Define and enforce a total metadata/record byte bound alongside these limits during DTO implementation.

Pending messages are never evicted or expired merely to admit another publish. Full storage returns a typed definite non-acceptance, so producers retain ownership and retry explicitly. Receipt alone does not delete content: retain fully received messages for the existing 30-day window, then prune the body and its receipt metadata together. A message pending for one recipient remains retained for that recipient; no hidden dead-letter or expiry policy.

Permanent abandonment can therefore fill capacity. Administrative discard/retirement policy is deliberately a separate product decision; expose capacity and pending diagnostics instead of quietly treating subject retirement as receipt.

## Inbox and consumer contract

Pending pages return bounded ordered message descriptors, not full bodies. Use a deterministic acceptance order with ID tie-break, stable page continuation and a fixed upper bound at implementation. New publications do not hide existing pending entries; receipt changes may shrink pages. Drain from the beginning after a completed pass; a page cursor is not a permanent consumption watermark.

Get returns addressed immutable content, also after receipt within retention. A consumer never has to put a 384 KiB payload in a push frame.

One pi-radar mailbox drain handles startup, heartbeat and push. For each descriptor:
1. fetch the typed message;
2. hand it to the registered body consumer;
3. wait for that consumer's durable-capture confirmation;
4. acknowledge the recipient delivery.

The consumer durably deduplicates by message ID. Two verified views of one subject may race; this is at-least-once delivery, not exactly-once execution or an exclusive consumer lease. Socket send, decoding and a callback return without durable capture are not receipts. No consumer or an unsupported body leaves the message pending and produces an actionable diagnostic. A poison message must not prevent later independent messages being offered.

Herdsman's handoff adapter is the first actual consumer, not a fabricated generic wake callback. It keeps existing ownership: capture before ack; assignment validity, wrap-up and worker closure remain Herdsman decisions. Receipt must not automatically send a user message or start an LLM turn.

## Attention and transport

Each verified recipient sees `{revision, pending}`. Revision is an opaque equality token for this subject's mailbox state, not a raw journal counter or a read position. Serialize it as a string in JSON and MessagePack; do not leak a u64 JSON rounding problem into the heartbeat contract. Subscription metadata also identifies the daemon instance so restart never compares unrelated revisions.

The module computes revision from recipient state and changes it on accepted delivery or receipt. Rebuild establishes a new baseline on startup. Unrelated recipients and raw journal activity do not change this subject's attention. Pending count, rather than revision inequality alone, drives recovery: a failed consumer must retry unchanged pending work. The token suppresses redundant hints, never suppresses a required redelivery.

Heartbeat reply: include recipient attention for a resolved managed subject using the same module; unmanaged callers have no mailbox hint. A successful heartbeat response already proves daemon liveness. Hint-read failure does not invalidate an accepted presence publication: return typed attention-unavailable, preserve the client's prior pending state and retry. Never fabricate `pending: 0`.

Subscription: JSON opener with the verified process context upgrades to length-prefixed MessagePack on the existing socket. Send an authoritative attention baseline then latest-wins attention updates. Establish watch and baseline without a gap (subscribe then read under the message module's sequencing, or equivalent). No raw-event resume cursor and no speculative `mode` field. Reconnect always gets current attention and drains pending work.

Use a separate subscription budget (initially 16), existing opening budget, hard frame/decode limits, supervised connection tasks, write deadline and connection-owned release. Final attention frames are small; set an explicit 4 KiB frame cap. A quiet established subscriber has no silence deadline. A slow reader loses only its own connection; no promise can deliver through a frame already blocked in the stream. Ordinary/UI established capacity remains separate, not an assurance against arbitrary opener floods.

The binary connection carries attention only. Bounded inbox/get/ack calls initially remain the existing JSON request/reply transport. Do not build duplex command multiplexing without a consumer requirement.

## pi-radar lifetime and delivery policy

One process-owned attention loop with one serialized drain and a remembered rerun flag handles races between triggers. Startup reconciles pending work before relying on hints. Heartbeats continue at the existing cadence whether subscribed or not.

The optional tier is opt-in for the message consumer, not automatic merely because a process exists. Use Herdsman's actual status model: only its literal idle status starts the 30-second continuous-idle hold. Waiting on user, tools, questionnaire or children does not start that timer. Re-entry into non-idle cancels the hold and opens/reuses the subscription. Before implementation, map the actual status enum exhaustively; do not substitute Pi streaming flags or generic presence labels.

After the hold closes the socket, an idle process still consumes pending work from heartbeat hints; delivery does not itself change Herdsman status. The consumer decides whether this warrants a model wake, scheduling a follow-up, local bookkeeping or display only.

Busy/disconnect falls back to heartbeats; reconnect uses bounded jittered backoff, cancelled on idle/shutdown/identity replacement. Failed drains retry pending work with bounded backoff even when the attention revision is unchanged. Concurrent hints coalesce to a rerun, not overlapping drains or an unbounded queue. Unsupported content remains visible without a tight retry loop.

## Concrete flows

### Worker result while parent is idle

Worker publishes handoff M to recorded parent P. M persists; P's pending count becomes one, nobody else's hint changes. On P's next successful heartbeat the same mailbox drain fetches M and calls the Herdsman handoff adapter. Durable local capture precedes ack. Herdsman chooses whether to awaken the parent; the daemon does not prompt it.

### Parent waiting for children

Waiting is not idle, so opted-in P retains the subscription. A handoff advances P's attention and prompts the drain immediately, without waiting for heartbeat. If the push is lost, the next successful heartbeat still reports pending delivery.

### Future child-exit notification

A lifecycle producer could emit `child_ended` addressed to the recorded parent with child identity and verified disposition. That requires a deliberate producer and recoverable delivery intent; neither a raw `ExitVerified` nor every child observation automatically becomes a message. Pi-radar dispatches the typed notice; the consumer decides its effect. This producer is deferred.

### Future addressed agent message

A Pi extension could publish a typed agent message to an authorised frozen recipient set. Each recipient captures and acknowledges independently. This needs a send grant and a real consumer contract; this design does not enable arbitrary cross-agent messaging as a side effect of generalising handoffs.

## Cutover

Replace the old handoff-specific envelope, store and handlers with the shared message module; switch the SDK and Herdsman adapter together. Remove the old handoff wire capability, duplicate receipt implementation and unused raw-event wake helper. The public Herdsman handoff port may retain its domain name, but internally uses only the message lane. No second mailbox, dual-write period or compatibility fallback.

The `durable-handoffs` spec continues to describe handoff policy, delegating shared delivery to `agent-messages`. Generated DTOs/schemas cover both JSON message calls and binary attention; Rust remains the contract owner. Old stored communication data is not silently migrated or dropped: fail startup with a clear diagnostic until the operator drains/exports/resets it explicitly.

Keep `events.read`, `observation.get`, TUI transport and their guarantees unchanged. Message attention replaces no observer feed.

## Alternatives and trade-offs

- Global or subject-filtered event wakes: rejected; recipient intent belongs to a message producer, not a consumer's interpretation of journal details.
- Just add a handoff count: rejected as the architecture; it preserves a special-case mailbox and gives the next message kind a second flow.
- Durable queue of push notifications: rejected; the mailbox already preserves work. Only attention may coalesce.
- Always-connected agents: rejected; heartbeats already provide bounded discovery while idle. Idle latency is one successful heartbeat, not a wall-clock guarantee under outage.
- ACK on socket delivery: rejected; loses content if the agent crashes before local capture.
- One new generic broker/actor: rejected; bounded store plus attention watch has the required ownership without duplicating domain execution.
- Opaque untyped message bags: rejected; generated closed body contracts keep Rust/TypeScript agreement.

## Verification and review points

Use focused message-module tests for routing, immutable retry, per-recipient receipts, atomic fanout visibility and rebuild. Exercise the real publisher/consumer join for handoff capture-before-ack. Controlled scheduler tests cover literal idle versus waiting, failed drains at unchanged revision and identity replacement. One real-socket check covers baseline-gap safety, slow-reader teardown and admission isolation; do not build a giant timing-sensitive end-to-end suite.

The common-lane direction and separation of policy are confirmed. The exact operation names, DTO layout, mailbox indexing and fixed page size are implementation decisions, not settled by this document. Child notifications, general send grants, group selectors and administrative discard remain explicitly deferred. Implementation must resolve the current Herdsman status and durable-capture seams before claiming the active tier works.
