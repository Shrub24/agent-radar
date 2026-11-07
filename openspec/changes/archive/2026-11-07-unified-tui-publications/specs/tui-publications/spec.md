# Spec Delta

## Purpose

Deliver one coherent TUI view from independently content-addressed observation, visible-tree and UI-session payloads, without frontend history or independent observation fetching.

## ADDED Requirements

### Requirement: One head orders three payloads
A publication SHALL name observation, tree-projection and UI-session payload hashes under one increasing head update ID within the current attachment's daemon lifetime. Hashes SHALL identify lane content without per-lane ordering counters. A changed head SHALL be made available after a state-changing actor transition; unchanged heads need not be republished. Action receipts SHALL remain independent of whether a head changes.

#### Scenario: Navigation alone changes
- **WHEN** selection changes without changing observation or visible rows
- **THEN** the next head names the previous observation and tree hashes and the new session hash

#### Scenario: Retried action changes nothing
- **WHEN** an identical action retry returns its original receipt
- **THEN** no fresh content identity or head update is required merely to answer the retry

#### Scenario: Daemon restarts
- **WHEN** a consumer reconnects to a restarted daemon
- **THEN** a fresh attachment and baseline establish its ordering scope and old in-flight responses cannot replace the new head

### Requirement: Payload identity follows stable portable bytes
Content SHALL use a documented deterministic MessagePack encoding and XXH3-128 hash of the exact encoded payload bytes served. Named fields, enum tags, integer handling, map ordering and hash representation SHALL be specified. Publication sequence, publication time and delivery bookkeeping SHALL NOT enter lane hashes. Genuine domain identities, timestamps and source-freshness facts SHALL retain their meaning; a freshly computed elapsed age SHALL NOT cause content churn.

#### Scenario: Only delivery metadata changes
- **WHEN** the same payload is delivered under different publication or connection bookkeeping
- **THEN** its encoded bytes and hash remain identical

#### Scenario: Source becomes stale
- **WHEN** collection fails while last-good fleet facts remain unchanged
- **THEN** the observation payload conveys the failure and freshness change without treating failure as disappearance

#### Scenario: Equivalent map construction
- **WHEN** equivalent payload maps were built with different insertion orders
- **THEN** they encode identically and have the same hash

### Requirement: Content exists before reference publication
Each referenced payload SHALL be available before its head is published. Unchanged payloads SHALL reuse immutable encoded content without being serialized for each subscriber or unrelated action. Current content SHALL be pinned; historical retention SHALL be bounded by bytes and entries. Fetching absent content SHALL return an explicit expired-content result and permit recovery from the current head, never silently substitute different bytes for the requested hash.

#### Scenario: Coalescing skips an inline payload
- **WHEN** a consumer receives a hash without having received its earlier inline payload
- **THEN** it can fetch the referenced content or recover from the current head if that content has expired

#### Scenario: Content returns to an earlier value
- **WHEN** a lane changes from A to B and then back to A
- **THEN** its content hash returns to A's identity while the overall head ordering advances

### Requirement: Consumers adopt coherent heads without history
A TUI SHALL display only a complete head whose required payloads match its hashes, and SHALL NOT combine payloads from unrelated heads. It SHALL retain only current content and a bounded in-progress replacement, release replaced or superseded content, and keep the last coherent display visibly stale during recovery rather than invent missing facts. A newer head SHALL supersede older fetch work without allowing fetch completion order to determine display order.

#### Scenario: One replacement payload is delayed
- **WHEN** a newer head references an observation payload not yet available locally
- **THEN** the previous coherent head remains displayed until the required replacement content is available

#### Scenario: A newer head overtakes a fetch
- **WHEN** content for an older pending head arrives after a newer head was adopted
- **THEN** it does not roll the display backward and unneeded bytes are released

### Requirement: Gate safety survives delayed content
The input gate SHALL be invalidated promptly when a newer head announces a different gate generation, even if its observation content is still being obtained. This SHALL NOT permit mixed-head rendering or accept stale ordinary input. Receipts, retained presentation requests and operation admission SHALL NOT be lost through snapshot coalescing.

#### Scenario: Modal opens while observation content is delayed
- **WHEN** a gate-changing head arrives while content recovery or a local action acknowledgement is pending
- **THEN** ordinary input stamped against the previous gate is stopped or refused before coherent rendering catches up

### Requirement: One observation delivery workflow serves the TUI
The TUI SHALL obtain observation through the head/content workflow rather than a parallel observation-fetch thread or long-read connection. The actor SHALL publish the observation input used to derive the accompanying tree and session transition. Sampling authority SHALL remain in the daemon observation service; freshness judgement and geometry SHALL remain local to the frontend. Optional process demand SHALL follow active TUI requirements and be released when attachments leave.

#### Scenario: Projected row arrives with its observation
- **WHEN** a head introducing rows becomes displayable
- **THEN** the frontend has the accompanying observation payload and joins facts by row identity, not local row position

#### Scenario: Last process view closes
- **WHEN** no active TUI attachment requests process data
- **THEN** the TUI path releases that sampling demand without changing non-TUI readers' demand

### Requirement: TUI content is bounded and failures are explicit
Encoding, frame reception and retained content SHALL have explicit size limits, and subscriptions SHALL have bounded pending state and write deadlines. Oversized content SHALL produce a typed size failure without partial adoption or endless reconnect to the same unusable baseline. A slow or malformed peer SHALL NOT block actor transitions or other subscribers.

#### Scenario: Advertised frame exceeds the limit
- **WHEN** a peer advertises a payload length above the configured frame bound
- **THEN** the reader rejects it before allocating that advertised amount

#### Scenario: Observation exceeds the content bound
- **WHEN** the next observation cannot fit the documented content limit
- **THEN** the consumer retains its last coherent head with an explicit size failure and retries only on a newer candidate or explicit reconnect

#### Scenario: Writer stalls
- **WHEN** a subscriber stops reading within a publication
- **THEN** the writer deadline closes that subscription while actor processing and other subscribers continue

### Requirement: Other daemon lanes remain unchanged
This change SHALL preserve existing domain RPC, observation-read, event, ingest, presence and pi-radar behavior. Reusable content and framing machinery SHALL NOT require UI-domain types, but other lanes SHALL NOT be migrated or wired to it speculatively.

#### Scenario: Existing non-TUI consumer
- **WHEN** a pi-radar or observation-read client uses its existing daemon contract
- **THEN** its encoding, admission, acknowledgement and recovery semantics remain unchanged
