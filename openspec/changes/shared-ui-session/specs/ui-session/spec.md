# Spec Delta

## Purpose

Provide one coherent navigation and command session to independently running Radar presentations without making terminal layout or domain execution shared UI state.

## ADDED Requirements

### Requirement: One canonical UI session serves all presentations
The daemon SHALL own one volatile UI session shared by attached dashboards and sidebars. It SHALL own main-tree selection, folds, ordering, visibility and filter. Terminal dimensions, split geometry, viewport, details visibility/focus/page and detail interaction SHALL remain local. Local informational overlays SHALL NOT block other presentations.

#### Scenario: Two differently sized presentations
- **WHEN** one dashboard changes main-tree selection and another has a different width, detail page and split
- **THEN** both show the same canonical selection and tree state while retaining their local presentation state

#### Scenario: Inspecting an error
- **WHEN** a frontend opens a local help or error-information overlay
- **THEN** another frontend can continue interacting with the shared session

### Requirement: Attachment establishes a fresh ordering baseline
Attach and reconnect SHALL establish a fresh attachment UUID, an authoritative current snapshot and its server update ID. The client action counter SHALL start at zero and the first action SHALL be one. Clients SHALL discard old-attachment responses. Attaching SHALL NOT reset canonical session state or the actor's publication counter.

#### Scenario: Daemon reconnect
- **WHEN** a frontend reconnects after losing the daemon
- **THEN** it adopts the new attachment and current snapshot rather than comparing the new stream with its old ordering baseline

#### Scenario: Another dashboard attaches
- **WHEN** a second frontend attaches to an existing session
- **THEN** it receives the current selection and filter without resetting them

### Requirement: Client action IDs preserve ordered application
Actions SHALL be identified by attachment UUID and increasing client counter. The session SHALL apply the next expected action once, refuse conflicting reuse, and not execute an already applied ID again. A gap SHALL report the expected ID and permit explicit retry or reattachment. Frontends SHALL submit through one ordered path and SHALL NOT replay uncertain actions under fresh IDs automatically.

#### Scenario: Duplicate submission
- **WHEN** an accepted action is resent with the same ID and contents
- **THEN** its original retained receipt is returned without repeating the transition, or explicit receipt expiry requires reconciliation

#### Scenario: Missing action
- **WHEN** action 12 arrives while action 11 is expected
- **THEN** the session reports 11 without applying 12, and the client can recover instead of remaining silently frozen

### Requirement: Bounded receipts preserve acceptance identity
The session SHALL retain bounded per-attachment receipts containing action identity, disposition, associated server update ID and interaction/domain-request IDs established at acceptance. Identical retries SHALL return that receipt; conflicting retained payload reuse SHALL be refused. Expired receipts SHALL require reconciliation without re-execution. Later workflow status SHALL remain distinct from the original receipt.

#### Scenario: Lost interaction acceptance
- **WHEN** an invocation created an interaction but its reply was lost and the same action is retried within retention
- **THEN** the original interaction ID is returned without creating another interaction

#### Scenario: Receipt no longer retained
- **WHEN** an old processed action is retried after its receipt expires
- **THEN** the response explicitly requires reconciliation and does not treat it as a fresh command

### Requirement: Publications carry ordered content identities
Each published UI update SHALL have one increasing server update ID and identify its complete tree and session-state payloads by content hash. Clients SHALL apply publication order within their current attachment, tolerate skipped snapshots and ignore older updates. Content identity SHALL NOT substitute for action identity or interaction identity.

#### Scenario: Selection-only update
- **WHEN** selection changes without changing the projected tree
- **THEN** the publication references the same tree hash and the updated session-state hash without requiring retransmission of the tree

#### Scenario: Old content repeats
- **WHEN** the tree changes from content A to B and back to A
- **THEN** the return to A has a newer update ID and can reuse the retained A payload

### Requirement: Content references are retrievable with bounded retention
The daemon SHALL make a payload retrievable before publishing its hash, retain current payloads, and bound historical retention. A missing or evicted hash SHALL return an explicit answer permitting current-snapshot recovery. Clients SHALL NOT apply an older fetch result over a newer intended publication. Every field of a published payload SHALL contribute to its content identity.

#### Scenario: Subscriber lacks a payload
- **WHEN** an update references a hash the frontend has not cached
- **THEN** it obtains that payload by hash or recovers from the current snapshot if retention has expired

#### Scenario: A fetch finishes late
- **WHEN** a payload fetch finishes after a newer publication has been received
- **THEN** the frontend does not roll its displayed state back to the older publication

### Requirement: Navigation uses the canonical visible projection
The session SHALL determine visible ordering, ancestry, folds and selection reconciliation from existing fleet facts. Frontends SHALL render the projection rather than independently reproduce navigation decisions. Runtime text SHALL retain existing sanitization and freshness meanings. Presentation changes SHALL NOT become evidence of domain effects.

#### Scenario: Narrow and wide views navigate together
- **WHEN** a sidebar requests the next visible row
- **THEN** all presentations receive the same selected identity despite different clipping and viewport sizes

### Requirement: One editor updates a shared live fuzzy filter
At most one attachment SHALL own shared filter editing. Its local editor SHALL submit complete query strings as ordered semantic updates, not character/cursor edits. Accepted queries SHALL update fuzzy matching live for all views, preserving tree order, ancestry and prior folds. Other views SHALL mirror rather than concurrently edit. Ownership release SHALL retain the latest accepted query; explicit clearing SHALL restore eligible rows.

#### Scenario: Live matching
- **WHEN** the owning editor changes the query
- **THEN** all presentations receive the resulting canonical filtered tree without waiting for a commit key

#### Scenario: Local editing mechanics
- **WHEN** the editor moves its caret or changes query text
- **THEN** caret mechanics stay local and each submitted filter update contains the complete current query

#### Scenario: A second editor
- **WHEN** another frontend tries to acquire filter editing while an owner remains active
- **THEN** it is refused with a readable ownership indication and does not overwrite the owner's query

#### Scenario: Editor disconnects
- **WHEN** the owning attachment is detached or expires
- **THEN** ownership is released without erasing the shared filter

### Requirement: Selection reconciliation preserves identity and freshness
The session SHALL retain surviving selection, prefer a removed task's visible owner, then use a deterministic bounded positional fallback or no selection when empty. Stale/unavailable observation SHALL NOT prove removal. A replacement occupant SHALL NOT inherit execution identity or frozen action targets solely from a reused location.

#### Scenario: Selected task disappears
- **WHEN** a current authoritative projection removes the selected task but retains its visible owner
- **THEN** the owner becomes selected in every presentation

#### Scenario: Source unavailable
- **WHEN** observation becomes unavailable after a valid fleet
- **THEN** retained facts remain visibly stale rather than being treated as an authoritative empty fleet

### Requirement: Frontends wake promptly and render the latest state
Input, received session updates and relevant completions SHALL wake frontend processing without waiting for a periodic idle poll. Frontends SHALL coalesce available snapshots into the newest coherent render while independently preserving ordered actions, receipts, addressed effects and gate invalidation. There SHALL be no default input debounce or delay waiting to fill a batch.

#### Scenario: Several updates before a frame
- **WHEN** multiple snapshots arrive before drawing
- **THEN** the frontend renders the newest coherent snapshot without drawing each intermediate frame or losing acknowledgements and modal input invalidation

#### Scenario: Quiet frontend receives a change
- **WHEN** the session changes while no terminal key is pressed
- **THEN** the frontend is woken by the update rather than waiting for its former idle polling interval

### Requirement: UI lifetime does not change domain authority
UI state SHALL remain in memory and SHALL NOT journal selection, folds or typing into lifecycle records. The session SHALL consume existing observation and operation results, not collect another fleet. Restart SHALL invalidate volatile attachments/interactions without fabricating their outcomes or replaying commands. Admitted domain operations SHALL retain their existing durability.

#### Scenario: Restart during an interaction
- **WHEN** the daemon restarts with a picker unresolved
- **THEN** its old attachment and interaction are rejected rather than resumed or reported as successful

#### Scenario: No active presentation demand
- **WHEN** presentations and UI work no longer require observation
- **THEN** the UI subsystem does not keep otherwise demand-driven sampling active merely because its session exists
