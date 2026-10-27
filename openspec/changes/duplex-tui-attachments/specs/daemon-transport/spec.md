# Spec Delta

## MODIFIED Requirements

### Requirement: UI subscriptions push typed frames independently of action RPCs
The existing trusted socket SHALL accept a bounded JSON `ui.open` attachment opener, answer in JSON, and upgrade that same connection to length-prefixed MessagePack requests, correlated replies and pushed publications. Each attachment SHALL use one duplex connection rather than separate action/subscription roles. Buffered bytes SHALL survive the upgrade. Request waiting SHALL NOT suppress available publications, and publication writing SHALL NOT suspend socket reading or actor transitions. Existing non-TUI codecs and handler semantics SHALL remain unchanged.

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

### Requirement: Lines are bounded and connections are timed
The transport SHALL read line-framed messages at most 1 MiB long without buffering beyond that bound, close overlong lines, and retain existing ordinary-connection idle and reply-write timeouts. Binary lengths SHALL be validated against an explicit bound before body allocation; truncated frames SHALL NOT be decoded. A quiet established duplex attachment SHALL NOT expire merely for sending no request. Opening, incomplete-frame completion and writes SHALL have separate finite deadlines. A partial write failure SHALL end the connection rather than continue at a new frame boundary. Disconnect SHALL NOT cancel domain work already admitted.

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

## ADDED Requirements

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
