# daemon-transport Specification

## Purpose

The single transport under every daemon socket: one trusted bind and inherited-listener rule, bounded line framing, lane admission with capacity reserved for control requests, and synchronous handlers on a bounded blocking pool so a slow handler or a waiting read never starves the others while the wire contracts stay unchanged.

## Requirements

### Requirement: One transport serves every daemon socket
The daemon SHALL serve its control socket and any compatibility socket through one transport implementation that applies the same directory trust check, explicit framing bounds, timeouts, connection limits and shutdown to each. Existing RPC and compatibility traffic SHALL retain bounded line framing; an explicitly selected TUI attachment MAY use bounded binary framing.

#### Scenario: Untrusted directory
- **WHEN** a socket directory is a symlink, is owned by another user, or is group- or world-accessible
- **THEN** the daemon does not bind or serve there and names the directory and the failed check

#### Scenario: Inherited listener
- **WHEN** the daemon serves a listener passed by its service manager
- **THEN** it serves it without binding and leaves the socket path in place at shutdown

#### Scenario: Same protections on both sockets
- **WHEN** a client of the compatibility socket sends an over-long line
- **THEN** the connection is closed by the same bound that applies to other line-framed connections and other connections are unaffected

### Requirement: Socket directories are trusted or refused
A socket directory that is not a real directory owned by the daemon's user with mode `0700` SHALL be refused with no looser fallback, and an inherited listener SHALL never be unlinked.

#### Scenario: Socket directories are trusted or refused
- **WHEN** the socket directory is a symlink, owned by another user, or looser than `0700`
- **THEN** the daemon refuses to serve it, with no looser fallback

### Requirement: Lines are bounded and connections are timed
The transport SHALL read line-framed messages at most 1 MiB long without buffering beyond that bound, close overlong lines, and retain existing ordinary-connection idle and reply-write timeouts. Binary lengths SHALL be validated against an explicit bound before body allocation; truncated frames SHALL NOT be decoded.

#### Scenario: Unterminated line
- **WHEN** an ordinary client exceeds 1 MiB without a line terminator
- **THEN** the connection closes without buffering beyond the bound

#### Scenario: Quiet attachment
- **WHEN** an established TUI has no input and the fleet has no publication changes
- **THEN** the connection remains valid without polling, keepalive requests or a missing-reply timeout

#### Scenario: Incomplete binary frame
- **WHEN** a peer starts a frame but does not finish it within the frame-completion deadline
- **THEN** the connection closes without decoding partial content or indefinitely extending its lifetime for trickled bytes

#### Scenario: Oversized binary length
- **WHEN** a peer advertises a binary frame above the documented bound
- **THEN** it is rejected before allocating the advertised body

#### Scenario: Slow reader
- **WHEN** a peer stops reading a frame
- **THEN** its write deadline closes the connection while other connections continue

#### Scenario: Disconnect after admission
- **WHEN** an attachment disappears after its domain operation is admitted
- **THEN** that operation retains its existing execution and durable request record

#### Scenario: Partial binary frame
- **WHEN** a TUI connection ends partway through a binary frame
- **THEN** the client discards that frame and reconnects without adopting partial state

### Requirement: Connection deadlines are separate and finite
A quiet established duplex attachment SHALL NOT expire merely for sending no request. Opening, incomplete-frame completion and writes SHALL have separate finite deadlines. A partial write failure SHALL end the connection rather than continue at a new frame boundary. Disconnect SHALL NOT cancel domain work already admitted.

#### Scenario: Connection deadlines are separate and finite
- **WHEN** a peer opens a TUI attachment, or starts a frame, and then stops sending
- **THEN** the opening or frame-completion deadline closes the connection, and work it had already admitted continues

### Requirement: Work is admitted by lane with reserved capacity
The transport SHALL admit control requests, publication ingest and waiting reads against separate limits, and SHALL keep capacity for control requests that ingest and waiting reads cannot use. A request over its limit SHALL receive a typed busy answer rather than consume unbounded resources or delay another lane.

#### Scenario: Ingest saturated
- **WHEN** publication ingest has reached its limit
- **THEN** a lifecycle control request is still admitted

#### Scenario: Waiters saturated
- **WHEN** waiting reads have reached their limit
- **THEN** further waits are refused as busy and control requests are unaffected

#### Scenario: UI backlog saturated
- **WHEN** the UI actor cannot accept another action within its bound
- **THEN** submission receives an explicit retryable admission answer without being represented as an applied action

#### Scenario: Subscription capacity saturated
- **WHEN** the UI subscription limit is reached while reserved control connection capacity remains
- **THEN** a further subscription is refused as busy before acceptance and lifecycle controls remain admissible

### Requirement: UI attachments have their own limits
UI action submissions SHALL have bounded admission. UI attachments SHALL have an explicit separate limit and count toward total connection capacity; their lifetime SHALL NOT consume control or waiting-read permits.

#### Scenario: UI attachments have their own limits
- **WHEN** the UI attachment limit is reached while control requests are being served
- **THEN** a further attachment is refused as busy and control requests keep their capacity

### Requirement: Synchronous handlers never run on the transport's workers
Request handling SHALL run on a bounded pool separate from the transport's own workers, and the transport SHALL NOT hold daemon state locks while it awaits. Shutdown SHALL stop accepting, end connections, signal cancellation to running handlers and wait for them within a bound, as the daemon does today.

#### Scenario: Slow handler
- **WHEN** a handler takes seconds to return
- **THEN** other connections continue to be read and answered within their lane limits

#### Scenario: Shutdown with a running handler
- **WHEN** the daemon is stopped while a handler runs
- **THEN** the handler is signalled to cancel, shutdown waits for it within its bound, and no thread outlives the process

### Requirement: Waiting reads do not occupy handler capacity
A read waiting for journal events SHALL wait asynchronously in the transport and SHALL NOT hold synchronous handler capacity. Journal results and cursors SHALL retain their existing semantics. Awaiting a UI actor reply SHALL NOT block a transport worker thread. UI publication delivery SHALL use the dedicated subscription requirements below rather than a UI long-poll method.

#### Scenario: Event during a wait
- **WHEN** a fact commits while a journal reader waits
- **THEN** the reader receives that event before the wait ends and no handler capacity was held during the wait

#### Scenario: Wait ends empty
- **WHEN** a journal wait ends with no new event
- **THEN** the reader receives an empty list and the same cursor

### Requirement: The wire contracts are unchanged
The TUI publication/content protocol MAY change to the explicitly declared MessagePack contract. That replacement SHALL NOT change request, response, error or capability behavior for domain RPCs, the existing observation-read API, event feeds, presence, ingest or the compatibility bus.

#### Scenario: Existing consumers
- **WHEN** non-TUI protocol fixtures, the publisher fixture and existing non-TUI socket tests run against the transport
- **THEN** their wire fixtures pass without conversion to the TUI codec

#### Scenario: TUI binary connection selected
- **WHEN** a client explicitly opens the TUI publication/content mode
- **THEN** the connection uses that documented framing and codec without trying to interpret arbitrary binary content as JSON lines

### Requirement: UI subscriptions push typed frames independently of action RPCs
The existing trusted socket SHALL accept a bounded JSON `ui.open` attachment opener, answer in JSON, and upgrade that same connection to length-prefixed MessagePack requests, correlated replies and pushed publications. Each attachment SHALL use one duplex connection. Buffered bytes SHALL survive the upgrade. Existing non-TUI codecs and handler semantics SHALL remain unchanged.

#### Scenario: Action reply is delayed
- **WHEN** an action has been submitted but its reply is delayed while a newer publication is available
- **THEN** that publication can progress independently of the action wait, subject to an already-writing frame and the writer's bounded scheduling

#### Scenario: Buffered upgrade bytes
- **WHEN** a reader buffers bytes beyond the JSON upgrade message
- **THEN** the binary decoder receives those bytes exactly once without losing the first frame

#### Scenario: Partial input during publication
- **WHEN** a client request arrives in fragments while publications become ready
- **THEN** decoding retains its partial prefix and body until completion or connection failure

#### Scenario: Existing non-TUI traffic
- **WHEN** a domain, event, presence, ingest or compatibility client uses its existing protocol
- **THEN** its payload format and handler-lane semantics do not change to the TUI protocol

#### Scenario: Quiet subscription
- **WHEN** the fleet is quiet and an attachment has delivered its baseline
- **THEN** it remains valid without renewal or a missing-reply timeout while action requests still progress on it

#### Scenario: Ordered link capacity
- **WHEN** a TUI action or content request reaches the attachment's action lane at capacity
- **THEN** it is refused before actor admission without consuming an action number

#### Scenario: Action during an idle subscription
- **WHEN** an attachment is open with no new publication and the user navigates
- **THEN** the action is sent on the attachment's own connection without waiting for a publication frame

#### Scenario: Publication without input
- **WHEN** the session publishes while the frontend is idle
- **THEN** the server pushes a publication frame without another client read request or a periodic renewal

### Requirement: Publications and requests progress independently
Request waiting SHALL NOT suppress available publications, and publication writing SHALL NOT suspend socket reading or actor transitions.

#### Scenario: Publications and requests progress independently
- **WHEN** a publication becomes ready while the attachment is waiting on an action reply or reading a request
- **THEN** the publication is written without suspending socket reading or actor transitions

### Requirement: Subscription writers are bounded and asynchronous
Subscriptions SHALL use connection-owned async writers with bounded write deadlines and buffering, not a blocking cloned-writer mutex. Their lifetime SHALL NOT hold handler/control capacity or block the actor/other clients. They MAY skip intermediate publications. Ordinary RPC idle expiry SHALL NOT end a healthy subscription, and peer closure SHALL be detected independently of publications.

#### Scenario: Slow subscriber
- **WHEN** a frontend stops consuming publication frames
- **THEN** retained publication buffering stays bounded, the actor and other subscribers continue, and a blocked write exceeding its deadline ends that subscription

#### Scenario: Quiet live subscriber
- **WHEN** a subscription remains connected without publications beyond the ordinary RPC idle timeout
- **THEN** it remains subscribed without requiring idle read renewals

### Requirement: Subscription termination releases presentation resources
Peer closure, detach, write failure/deadline and shutdown SHALL release subscription capacity. Primary subscription loss SHALL invalidate its dashboard/sidebar attachment and release editor ownership, retaining the query. Recovery SHALL use a fresh attachment/baseline, without automatically replaying uncertain actions under new IDs. Separately claimed presenters SHALL remain independent. Closure SHALL NOT imply process exit or domain cancellation.

#### Scenario: Peer closes while idle
- **WHEN** a frontend closes its subscription while no publication is pending
- **THEN** the transport detects closure and releases subscription capacity and attachment ownership without waiting for the next publication

#### Scenario: Origin disconnects with a live presenter
- **WHEN** a sidebar loses its primary subscription after a separate presenter has claimed its interaction
- **THEN** the sidebar attachment is invalidated but the presenter can still resolve the interaction

#### Scenario: Shutdown with idle subscriptions
- **WHEN** the daemon shuts down while subscription tasks are waiting
- **THEN** those tasks terminate and release capacity without waiting for a publication or a long-poll timeout

### Requirement: Established connection classes retain independent capacity
Opening, established ordinary and established UI connections SHALL have separate finite budgets whose sum bounds total connections. UI usage SHALL NOT consume ordinary established capacity. Classification SHALL transfer permits without unaccounted lifetime. Valid presenters SHALL have bounded reserved headroom. Execution limits SHALL remain separate; tracking SHALL NOT impose a competing cap.

#### Scenario: Established UI capacity is full
- **WHEN** established ordinary UI attachment capacity is exhausted
- **THEN** another ordinary UI opener is refused as busy while ordinary control and ingest connections retain their own connection capacity

#### Scenario: Presenter needed at capacity
- **WHEN** ordinary UI attachment capacity is full and an active interaction requires a presenter
- **THEN** a valid presenter can use reserved headroom without accepting an arbitrary invalid presenter claim

#### Scenario: Opening capacity is exhausted
- **WHEN** unclassified peers fill the opening budget
- **THEN** further connections may be closed immediately and incomplete openers expire within the handshake deadline

#### Scenario: Classified overload
- **WHEN** enough of an opener is parsed to identify a full established class
- **THEN** the peer receives a typed busy refusal when it can be written within the deadline

### Requirement: Duplex request work is bounded before execution
Each attachment SHALL reserve one action-response slot until full reply transmission or disconnect. Auxiliary work and retained input/output bytes SHALL have explicit bounds. Accepted receipts SHALL NOT be silently dropped on queue saturation. Request order SHALL remain independent of correlation. Slow reply transmission SHALL NOT retain shared execution capacity.

#### Scenario: Second action while reply is unsent
- **WHEN** a peer sends another action while its first action-response slot remains occupied
- **THEN** the extra action is not admitted and overload is handled within bounded refusal capacity or by closing the connection

#### Scenario: Content while action waits
- **WHEN** an action reply is pending and the client needs content for an available head
- **THEN** bounded auxiliary request handling and publication delivery remain able to progress without awaiting that reply

#### Scenario: Reply backpressure
- **WHEN** replies cannot be transmitted as quickly as requests arrive
- **THEN** memory and outstanding work remain bounded without silently discarding an admitted action's receipt

### Requirement: Attention subscriptions are recipient scoped
The trusted socket SHALL accept a verified process opener that upgrades to bounded length-prefixed MessagePack and sends a gap-free recipient attention baseline followed by latest attention updates. Raw events, event cursors and message bodies SHALL NOT be sent on this connection. Loss of the recipient grant SHALL end access.

#### Scenario: Message arrives during subscription establishment
- **WHEN** a recipient message is accepted while the baseline watch is established
- **THEN** the baseline or a subsequent update reports it without a missed transition

#### Scenario: A stale execution retains a socket
- **WHEN** a subscriber loses its verified subject grant
- **THEN** it receives no further recipient information and its subscription ends

### Requirement: Attention delivery is bounded independently of messages
Only attention updates MAY coalesce. Established subscriptions SHALL use a separate budget of 16, a 4 KiB frame cap and bounded writes. Exhaustion SHALL return typed busy without consuming ordinary or UI established capacity. Quiet subscribers SHALL have no silence deadline.

#### Scenario: Slow subscriber
- **WHEN** a subscriber stalls beyond the write deadline
- **THEN** its tasks, connection and permit are released without affecting other subscribers or durable pending messages

#### Scenario: Quiet or capacity-limited subscriber
- **WHEN** all 16 established attention slots are occupied by quiet recipients
- **THEN** they remain attached, another attention opener is refused, and ordinary/UI established capacity remains separate
