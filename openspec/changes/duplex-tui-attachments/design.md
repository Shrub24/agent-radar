# Design

## Context

See `proposal.md` for motivation and capability scope. The serving path currently upgrades JSON `ui.open` into either a request-only or a push-only `FrameStream`. `BinaryFrames::respond` occupies a blocking worker waiting for `UiHandle::ask`; the client serialises actions and content recovery behind a blocking `Link` mutex. `Subscription::next_frame` advances its known head when generating a frame, which must change before a writer can replace unsent candidates. Both TUI entry paths construct a timer-only current-thread runtime after creating the thread-based client.

The baseline is the implemented `shared-ui-session` plus `unified-tui-publications` branch, not an already-synced main spec. Existing MessagePack/XXH3-128 identities, 1 MiB payload and 4 MiB frame bounds, current-only client retention, domain admission and one synchronous actor remain the foundation.

## Goals / Non-Goals

**Goals:** one lifetime owner per attachment; independent I/O and request progress; honest uncertainty; finite aggregate resource bounds; fewer distributed client invariants; immediate optimistic input with explicit target safety.

**Non-goals:** converting the actor to async, a generic RPC/multiplexing framework, migrating other daemon lanes, adaptive/head-only delivery heuristics, interleaved content chunks, scheduling frameworks, guaranteed recovery of an unknown domain ID, row-ID changes or tmux restart/service coupling. Multiple TUI attachments are supported; the separate one-tmux-client model does not reduce TUI capacity requirements.

## Decisions

### 1. One opener and one connection own an attachment

Keep the bounded JSON `ui.open` upgrade on the existing trusted socket. Its payload carries a fresh attachment UUID, dashboard/sidebar/presenter role, and interaction ID where required. Role determines the presenter's reduced content needs; remove a separately configurable `session_only` flag. The actor establishes the attachment, its lease and primary publication receiver in one message and validates any presenter claim there.

The JSON upgrade acknowledgement is not client readiness. Send a binary authoritative baseline and make the client ready only when that head is coherently adopted. Registration and receiver acquisition are atomic in actor state, not transactional with socket delivery. The connection owner releases the lease on failure at any later step, including an opener whose actor answer arrives after disconnection.

**Reason:** attachment and subscription are one resource lifetime. Separate sockets provided independent progress but forced duplicate failure handling and a registration gap. Do not retain dual protocol modes as a fallback.

### 2. Typed correlation without a generic router framework

Use a request envelope struct `{id, request}`, not a single-variant client-message enum. Requests are Action, Content and Demand. Attachment identity is bound by the opener rather than repeated in every request. Request IDs correlate responses within a connection; action counters retain their independent actor ordering/deduplication meaning. A server frame distinguishes Reply from Publication. Keep the portable named-field/tag conventions, exact integer semantics and frame bounds documented with fixtures.

Preserve `content::Segments`: the tagged server envelope must encode small metadata around shared payload buffers rather than serialize the full content again. Content replies need the same segmented ownership treatment as publications. Extend the existing domain-agnostic envelope/framing helper only as needed; do not embed UI DTOs in `content.rs`.

The old Attach/Detach request exchange and Actions/Subscribe open roles disappear. Failure of either socket half ends the one attachment. Reconnect uses a fresh UUID and baseline; no resume protocol is introduced.

### 3. Persistent reading, ordered actions and one fair writer

The daemon reader owns the read half and preserves prefix/body progress. It routes completed envelopes without awaiting actor execution. Preserve buffered bytes across JSON/binary upgrade on both peers. A persistent decoder or dedicated read task is acceptable; do not recreate a partially consumed `read_exact` future in a publication-select loop. A standard length-delimited codec may be used if it preserves the existing framing bounds and upgrade buffers; no new custom general-purpose codec is needed.

One action-response slot is occupied before actor enqueue and released only after complete reply transmission or connection termination. Do not admit another action while that slot is occupied. Small bounded auxiliary content/demand work can progress while the action waits. Client actions remain serial; neither concurrent futures nor reply IDs define mailbox ordering. Extra requests at saturation receive a bounded refusal where capacity exists, otherwise the connection closes; never await a full action queue in the reader or accumulate unlimited busy replies.

The writer owns the sink. Replies are not coalesced. Publications come from a latest-value source rather than a FIFO. Initial fairness rule: while both sources are ready, write at most four reply frames before a publication; when a reply is ready, continuous publications cannot cause more than one intervening publication frame. No priority can interrupt a frame already writing. Terminate on partial write failure; never restart that frame or continue framing on the same connection.

Shared execution/handler permits cover processing only, not the lifetime of an unsent reply. One peer's slow reader must not retain global execution permits until its write deadline.

**Reason:** sequential actions and fixed slots are enough for interactive traffic. A general credit system, `FuturesOrdered` action execution, extra worker pools or RPC multiplexing dependency would add machinery without strengthening the contract.

### 4. Async reply handles bridge the synchronous actor

Add an async-facing enqueue operation returning a Tokio oneshot receiver. The existing actor thread completes the sender synchronously. Definite enqueue failure is distinct from receiver loss or timeout after enqueue. Avoid `spawn_blocking` merely to wait for the actor; keep real blocking domain work in its existing executor.

Cancellation of a wait does not retract an enqueued message. The actor checks lease validity when considering an action; admitted domain work retains its existing lifetime. Identical action retries can use receipts only under a valid attachment and existing retention. After reconnect, a known durable domain ID permits record lookup; if that ID was never learned, exact recovery is not promised. Never automatically repeat an uncertain command under a new ID.

**Rejected alternative:** report a post-enqueue timeout as Busy/non-admission. That makes a potentially applied action look safe to repeat.

### 5. Lease release does not depend on mailbox capacity

The connection owns the release capability of an attachment lease. The actor stores observable liveness, not another owner that keeps the connection alive. An explicitly released token or weak ownership works, provided release is nonblocking. A wake can accelerate reaping, but correctness cannot depend on successfully queuing ordinary Detach into a full mailbox. The actor checks liveness before admitting an attachment action and reaps resources on normal turns, with its existing periodic turn as a fallback.

Release removes only that UUID's demand, editing ownership and presentation claim. Check already-released leases during establishment as well. Already-admitted domain effects are not undone. Daemon shutdown releases connections and stops actor work through the existing bounded shutdown order.

**Reason:** `try_send(Detach)` can lose cleanup; blocking `send(Detach)` can park the transport executor. A lease makes lifetime observable without an unbounded cleanup queue or retry task per disconnect.

### 6. Coalesce against successful transmission

Keep immutable published content available to the writer without actor RPCs, projection work or serialization there. A writer holds one in-flight frame, the last fully transmitted head and the latest candidate. Generate candidate segments against the last transmitted head. Advance that cursor only after all segments write successfully. Replaced candidates must not establish assumed client content knowledge.

A completed write means bytes were accepted by the local transport, not decoded by the client. Ordered delivery plus complete-head validation makes per-head application ACKs unnecessary. Reset the cursor at reconnect. Retain explicit content-expiry recovery when required content is not locally available.

**Rejected alternative:** adaptive head-only delivery initially. It moves large bytes to replies on the same stream, adds recovery round trips and does not remove head-of-line blocking. Add chunking or adaptive delivery only against an established requirement.

### 7. Preserve optimistic navigation and displayed-row commands

Keep local immediate navigation plus one bounded FIFO of semantic actions. On rapid move-to-B then Delete, the frontend displays B, queues Move first, and captures B in the command. Action IDs and FIFO submission order are retained. Row identity is not a display index. The actor resolves explicit targets against its current canonical tree and freezes them; a missing row refuses, never retargets.

Extend configured Launch to carry the displayed row while still preparing command, shell and PATH in the frontend. It currently derives target from actor selection and must converge with Delete, Restart and Focus.

Observe a newer gate immediately even if content or replies are pending. Fence unsent old-gate input and key sequences. When its complete head is adopted, authoritative state may discard unaccepted speculative navigation. Do not promise to preserve speculation the actor rejected. Preserve ordinary accepted-action publication-floor semantics while simplifying their ownership; do not permit older heads to overwrite newer adopted state.

**Reason:** removing optimism changes rapid input meaning and puts cursor feedback behind socket writes. A deferred destructive-command barrier or separately advancing modal/navigation model adds more policy than this cutover needs. Retain the established UX and make exceptional reconciliation explicit. Optimism is not evidence of a domain effect or permission to invent projection rows.

### 8. One async client lifecycle on the existing runtime

Construct each dashboard/presenter current-thread runtime with I/O and timers before client startup. Enter one outer `block_on(run_tui(...))`; create client tasks within that scope and await explicit bounded shutdown before exit. Drop only requests cancellation. Do not synchronously join runtime tasks in Drop or create a dedicated client runtime.

One supervisor owns connection incarnation, readiness, outstanding reply correlation and cancellation. The reader updates observed gate/newest target immediately and routes replies. A bounded materialiser keeps the displayed payloads and one latest replacement, fencing late content by connection and head identity. One action sender owns the bounded input FIFO, action numbering and receipt floor. Demand uses a latest-desired-value notification, not the old 250 ms tick. Geometry and freshness judgement stay frontend-local.

Audit `pending`, `floor`, `resync`, `delivered`, `displayed_gate` and `observed_gate` by purpose: consolidate owners rather than copy the current atomics into an async struct or collapse distinct facts. Retain only state needed by the settled optimistic/gate policy.

### 9. Explicit finite connection and byte budgets

Use separate opening and established UI/ordinary permits, acquiring the destination permit before releasing opening capacity. Remove the shutdown registry's independent admission cap; it tracks sockets only. Initial connection defaults: 8 opening, 16 ordinary UI attachments, 1 reserved validated presenter, 32 ordinary connections. The reserve can be used only for an existing claimable interaction and is returned on failure. This yields at most 57 accepted connection lifetimes, not 57 file descriptors: retained socket clones and listeners also count toward FD use and must be included in the resource review. These are conservative initial limits, not measured saturation capacity.

Keep existing control/ingest/waiting execution-lane limits. Within a UI attachment use one action-response slot and at most two outstanding auxiliary requests. The client's input FIFO is bounded by count; it carries typed actions a person types, so a byte cap would guard text no keyboard produces. Frame and payload limits stay with the shared content layer. Count payload references that pin old content, not just queue metadata. No separate memory-credit framework is required.

Opening and partial-frame deadlines are distinct from ordinary idle and write timeouts. Established quiet UI connections do not time out for silence. Start a finite frame-completion deadline once a frame begins; do not extend it indefinitely for trickle traffic. Keep the existing two-second write deadline initially. Saturated classified openers get typed Busy where writable; unclassified opening floods may be closed immediately. Class budgets prevent established UI starvation of ordinary traffic, not absolute availability against every shared-listener flood.

## Risks / Trade-offs

- **Head-of-line blocking within a frame** → retain bounds, shared bytes, frame-boundary fairness and write deadlines. Measure readable-peer contention separately from stalled-peer teardown; no latency guarantee for a non-reader.
- **Actor stalls** → independent transport only delivers already-available heads. Keep actor transitions nonblocking and bound processing batches; acceptance must investigate consistently triggered stalls rather than normalise them as edge behaviour.
- **Current-thread render work delays sockets** → bounded event handling and normal async yields; measure sustained input and daemon contention. No unmeasured responsiveness claim from earlier median latency.
- **Released attachments await a normal actor turn for resource reaping** → check liveness at action admission and retain periodic fallback; never let cleanup depend on free ordinary mailbox space.
- **Breaking TUI opener** → daemon and TUI ship together, reject incompatible openers explicitly, update compatibility declaration/fixtures. Other lanes keep their codecs and semantics.

## Migration Plan

1. Land the combined DTOs, lease and async actor reply seam with focused boundary tests; freeze documented message and resource limits.
2. Replace the UI adapter with reader/action/auxiliary/writer ownership and class admission. Remove obsolete `FrameStream` responsibilities rather than wrap the new handler in legacy role switches.
3. Cut both frontend roots to the async supervisor in one coherent slice; preserve optimistic input and gate policy, and fix Launch target capture.
4. Remove obsolete role DTOs, socket links, blocking exchanges, thread joins and timed demand synchronization. Update maintained protocol/acceptance docs and record why in the existing context topic.
5. Validate focused tests, routine gate and representative socket-to-frame measurements. Owner performs live Kitty/tmux acceptance. Rollback is a coordinated daemon/TUI revision rollback, not permanent dual-mode support.
6. Sync `shared-ui-session`, `unified-tui-publications` and this change in dependency order after inherited sidebar prerequisites. Do not rewrite predecessor completion history to imply this cutover already exists.
