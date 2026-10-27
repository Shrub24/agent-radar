# Spec Delta

## MODIFIED Requirements

### Requirement: One transport serves every daemon socket
The daemon SHALL serve its control socket and any compatibility socket through one transport implementation that applies the same directory trust check, explicit framing bounds, timeouts, connection limits and shutdown to each. Existing RPC and compatibility traffic SHALL retain bounded line framing; an explicitly selected TUI publication/content connection MAY use bounded binary framing. A socket directory that is not a real directory owned by the daemon's user with mode `0700` SHALL be refused with no looser fallback, and an inherited listener SHALL never be unlinked.

#### Scenario: Untrusted directory
- **WHEN** a socket directory is a symlink, is owned by another user, or is group- or world-accessible
- **THEN** the daemon does not bind or serve there and names the directory and the failed check

#### Scenario: Inherited listener
- **WHEN** the daemon serves a listener passed by its service manager
- **THEN** it serves it without binding and leaves the socket path in place at shutdown

#### Scenario: Same protections on both sockets
- **WHEN** a client of the compatibility socket sends an over-long line
- **THEN** the connection is closed by the same bound that applies to other line-framed connections and other connections are unaffected

### Requirement: Lines are bounded and connections are timed
The transport SHALL read one line at most 1 MiB long without buffering beyond that bound, SHALL close a line-framed connection that exceeds it, and SHALL close a connection that is idle or whose reply cannot be written within its timeout. TUI binary frames SHALL declare a length checked against an explicit bound before body allocation, and truncated frames SHALL NOT be decoded or adopted. Subscription idle time while waiting for a publication SHALL NOT count as a missing reply. A closed connection SHALL NOT cancel a request the daemon already admitted.

#### Scenario: Unterminated line
- **WHEN** a client sends more than 1 MiB without a line terminator
- **THEN** the daemon closes that connection and holds no more than the bound for it

#### Scenario: Slow reader
- **WHEN** a client does not read its reply within the write timeout
- **THEN** the daemon closes the connection and keeps serving others

#### Scenario: Disconnect after admission
- **WHEN** a client disconnects after its operation was admitted
- **THEN** the operation completes and its durable record can be read back by its request identifier

#### Scenario: Oversized binary length
- **WHEN** a TUI peer supplies a frame length exceeding the binary frame bound
- **THEN** the transport rejects the frame before allocating its claimed body

#### Scenario: Partial binary frame
- **WHEN** a TUI connection ends partway through a binary frame
- **THEN** the client discards that frame and reconnects without adopting partial state

### Requirement: UI subscriptions push typed frames independently of action RPCs
The existing authenticated socket SHALL accept a bounded JSON `ui.open` request selecting a TUI action/content or subscription role, answer that opener in JSON, then switch that connection to length-prefixed MessagePack. The primary subscription SHALL provide an authoritative binary baseline followed by latest coherent heads without renewal reads. Actions, receipts and content retrieval SHALL use an independent reusable ordered binary request/reply connection. Subscription writers SHALL share immutable payload bytes, have bounded pending state and preserve existing write deadlines, capacity and shutdown protections. Existing domain reads SHALL remain unchanged; no generic subscription framework is required.

#### Scenario: Quiet subscription
- **WHEN** the fleet is quiet and a subscription has delivered its baseline
- **THEN** it remains valid without renewal or a missing-reply timeout while action requests can progress on their own connection

#### Scenario: Ordered link capacity
- **WHEN** a TUI action/content request reaches the control lane at capacity
- **THEN** it is refused before actor admission without consuming an action number

### Requirement: The wire contracts are unchanged
The TUI publication/content protocol MAY change to the explicitly declared MessagePack contract. That replacement SHALL NOT change request, response, error or capability behavior for domain RPCs, the existing observation-read API, event feeds, presence, ingest or the compatibility bus.

#### Scenario: Existing consumers
- **WHEN** non-TUI protocol fixtures, the publisher fixture and existing non-TUI socket tests run against the transport
- **THEN** their wire fixtures pass without conversion to the TUI codec

#### Scenario: TUI binary connection selected
- **WHEN** a client explicitly opens the TUI publication/content mode
- **THEN** the connection uses that documented framing and codec without trying to interpret arbitrary binary content as JSON lines
