# tui-publications Specification

## Purpose
Deliver one coherent TUI view from independently content-addressed observation, visible-tree and UI-session payloads, without frontend history or independent observation fetching.

## Requirements

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
Input gates SHALL be invalidated promptly when a newer head announces a different generation, even while replies or required content are pending. Gate observation SHALL NOT adopt partial payload state. The previous coherent head MAY remain displayed during materialisation, but stale ordinary input SHALL be fenced immediately.

#### Scenario: Modal opens while observation content is delayed
- **WHEN** a gate-changing head arrives during content recovery or an outstanding acknowledgement
- **THEN** old-gate ordinary input is stopped or refused before rendering catches up

#### Scenario: Speculative selection differs
- **WHEN** a non-following view selects B while a gate-changing coherent head reports focus A
- **THEN** B remains selected if still valid, while receipts and frozen command targets retain their actor ordering

### Requirement: Adopting a head keeps the view's selection
Coherent adoption SHALL NOT overwrite local browsing selection with a global cursor. Local selection changes SHALL follow projection validity or the explicit follow policy. Receipts, retained presentation requests and admitted operations SHALL survive publication coalescing.

#### Scenario: Adopting a head keeps the view's selection
- **WHEN** a coherent head arrives while a view browses a row other than the focused one
- **THEN** the view keeps its own selection while the row stays valid, and its receipts and admitted operations survive coalescing

### Requirement: One observation delivery workflow serves the TUI
The TUI SHALL obtain observation through the head/content workflow rather than a parallel observation-fetch thread or long-read connection. The actor SHALL publish the observation input used to derive the accompanying tree and session transition. Sampling authority SHALL remain in the daemon observation service; freshness judgement and geometry SHALL remain local to the frontend. Optional process demand SHALL follow active TUI requirements and be released when attachments leave.

#### Scenario: Projected row arrives with its observation
- **WHEN** a head introducing rows becomes displayable
- **THEN** the frontend has the accompanying observation payload and joins facts by row identity, not local row position

#### Scenario: Last process view closes
- **WHEN** no active TUI attachment requests process data
- **THEN** the TUI path releases that sampling demand without changing non-TUI readers' demand

### Requirement: TUI content is bounded and failures are explicit
Encoding, reception and retained content SHALL retain explicit size limits. Each attachment SHALL bound pending requests, replies and retained bytes, including old content pinned by pending transmissions. Immutable payload bytes SHALL be reused rather than re-encoded per subscriber. An oversized candidate SHALL retain the previous coherent head with a typed diagnostic, without partial adoption or endless reconnect. A slow or malformed peer SHALL NOT block actor transitions or other attachments.

#### Scenario: Advertised frame exceeds the limit
- **WHEN** a peer advertises a frame beyond the frame bound
- **THEN** it is rejected before body allocation

#### Scenario: Observation exceeds the content bound
- **WHEN** an observation cannot fit the content bound
- **THEN** the frontend retains its previous coherent head with an explicit size failure and retries only on a newer candidate or explicit reconnect

#### Scenario: Writer stalls
- **WHEN** one attachment stops reading within a publication
- **THEN** its deadline ends the connection, retained work remains bounded and other attachments and the actor continue

### Requirement: Other daemon lanes remain unchanged
This change SHALL preserve existing domain RPC, observation-read, event, ingest, presence and pi-radar behavior. Reusable content and framing machinery SHALL NOT require UI-domain types, but other lanes SHALL NOT be migrated or wired to it speculatively.

#### Scenario: Existing non-TUI consumer
- **WHEN** a pi-radar or observation-read client uses its existing daemon contract
- **THEN** its encoding, admission, acknowledgement and recovery semantics remain unchanged

### Requirement: Coalesced delivery uses the last transmitted head
A writer SHALL keep its active publication and latest candidate, not history. Inline content SHALL be chosen against the last fully transmitted head; the cursor SHALL advance only after full transmission. Replacing candidates SHALL NOT assume skipped content was delivered. Reconnect SHALL reset the cursor and send a baseline. Per-head application acknowledgements are not required.

#### Scenario: Burst replaces an unsent candidate
- **WHEN** B is superseded by C before B is transmitted after A
- **THEN** C supplies the inline content needed relative to A, or uses the explicit bounded content-recovery contract, without assuming B was delivered

#### Scenario: Publication write fails halfway
- **WHEN** a writer fails during a publication frame
- **THEN** it closes the connection rather than advancing the cursor and continuing with another frame

### Requirement: Replies and latest publications receive bounded fair service
One writer SHALL own frame transmission and provide explicit finite scheduling fairness between reliable replies and available publications at frame boundaries. A receipt SHALL NOT be coalesced as a snapshot. A publication SHALL NOT be starved by continuous replies, nor replies by continuous observation changes. An in-progress frame need not be interruptible; the protocol SHALL NOT promise timely delivery to a peer that stops reading.

#### Scenario: Continuous reply traffic
- **WHEN** a readable attachment has a newer publication available while replies keep arriving
- **THEN** the publication is selected after a bounded number of reply frames

#### Scenario: Continuous observation traffic
- **WHEN** publications continue changing while a reply is ready
- **THEN** the reply is served without waiting for the publication source to become quiet

### Requirement: Client lifetime is reactive and nonblocking
Both dashboard and detached presenter SHALL process connection updates, input and completion wakeups without periodic wire polling. Process demand SHALL communicate the latest desired value and release on attachment loss. Normal shutdown SHALL await client task completion within a bound; drop fallback SHALL request cancellation without blocking the terminal executor. Socket progress SHALL NOT wait for blocking request/reply exchanges or synchronous thread joins on that executor.

#### Scenario: Quiet process-demand change
- **WHEN** local detail visibility changes process-data demand with no keyboard or publication activity afterward
- **THEN** desired demand is communicated by notification rather than a periodic synchronization tick

#### Scenario: Normal exit and early drop
- **WHEN** a dashboard or presenter exits normally or is dropped during a handshake
- **THEN** cancellation releases connection resources and normal exit awaits bounded shutdown without blocking the runtime thread in a synchronous join
