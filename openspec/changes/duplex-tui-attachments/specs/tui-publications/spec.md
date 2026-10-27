# Spec Delta

## MODIFIED Requirements

### Requirement: Gate safety survives delayed content
The input gate SHALL be invalidated promptly when a newer head announces a different generation, even if action replies or required content are pending. Gate observation SHALL NOT require adopting partial payload state. The previous coherent head MAY remain displayed while replacement materialisation completes, but stale ordinary input SHALL be fenced immediately. When the new coherent gate-changing head is adopted, its authoritative navigation MAY replace unaccepted optimistic navigation under the explicit reconciliation policy. Receipts, retained presentation requests and operation admission SHALL NOT be lost through publication coalescing.

#### Scenario: Modal opens while content is delayed
- **WHEN** a gate-changing head arrives while content recovery or an action acknowledgement is pending
- **THEN** ordinary input stamped against the old gate is stopped or refused before coherent rendering catches up

#### Scenario: Speculative selection differs
- **WHEN** pending optimistic navigation differs from the new gate-changing coherent head
- **THEN** that head may restore authoritative selection while retained action receipts and frozen command targets remain governed by actor ordering

### Requirement: TUI content is bounded and failures are explicit
Encoding, reception and retained content SHALL retain explicit size limits. Each attachment SHALL bound pending requests, replies and retained bytes, including old content pinned by pending transmissions. Immutable payload bytes SHALL be reused rather than re-encoded per subscriber. An oversized candidate SHALL retain the previous coherent head with a typed diagnostic, without partial adoption or endless reconnect. A slow or malformed peer SHALL NOT block actor transitions or other attachments.

#### Scenario: Oversized frame
- **WHEN** a peer advertises a frame beyond the frame bound
- **THEN** it is rejected before body allocation

#### Scenario: Oversized observation
- **WHEN** an observation cannot fit the content bound
- **THEN** the frontend retains its previous coherent head with an explicit size failure and retries only on a newer candidate or explicit reconnect

#### Scenario: Writer stalls
- **WHEN** one attachment stops reading within a publication
- **THEN** its deadline ends the connection, retained work remains bounded and other attachments and the actor continue

## ADDED Requirements

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
