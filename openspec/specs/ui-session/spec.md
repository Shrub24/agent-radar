# ui-session Specification

## Purpose
Provide one coherent navigation and command session to independently running Radar presentations without making terminal layout or domain execution shared UI state.

## Requirements

### Requirement: One canonical UI session serves all presentations
The daemon SHALL own one volatile UI session shared by attached dashboards and sidebars. The synchronized cursor path SHALL carry observed terminal focus rather than browsing selection. Folds, ordering, visibility, filter and interactions SHALL remain shared. Each frontend SHALL own selection, dimensions, viewport, split geometry, detail interaction and local informational overlays.

#### Scenario: Independent inspection
- **WHEN** two dashboards select different rows with different detail pages
- **THEN** they retain local selection and details while receiving the same shared focus and tree settings

#### Scenario: Two differently sized presentations
- **WHEN** one dashboard moves its selection and another has a different width, detail page and split
- **THEN** both show the same canonical tree and each keeps its own selection and presentation state

#### Scenario: Inspecting an error
- **WHEN** a frontend opens a local help or error-information overlay
- **THEN** another frontend can continue interacting with the shared session

### Requirement: Attachment establishes a fresh ordering baseline
Open and reconnect SHALL establish a fresh attachment UUID and action counter starting at zero, with first action one. Registration and its primary publication receiver SHALL be established together. Readiness SHALL require coherent baseline adoption. Old-attachment responses SHALL be discarded. Attachment SHALL NOT reset canonical state or the publication counter. The baseline SHALL convey shared focus, not a shared browsing selection.

#### Scenario: Daemon reconnect
- **WHEN** a frontend reconnects
- **THEN** it uses a fresh attachment and coherent baseline and rejects old responses without comparing publication counters across daemon restarts

#### Scenario: Another dashboard attaches
- **WHEN** another dashboard attaches
- **THEN** it receives current focus and shared settings without resetting another view's selection or the shared filter

### Requirement: Subscription starts from a gap-free current baseline
The combined attachment opener SHALL establish exactly one connection-owned primary publication receiver and an authoritative binary baseline, with no separate attach-to-subscribe operation. Transitions after establishment SHALL be available through the baseline or a subsequent newer head without needing unrelated input. Failed upgrade or baseline delivery SHALL release the attachment and its resources. Publication IDs SHALL be used for local stale-delivery rejection, not history replay.

#### Scenario: Session changes between attach and subscribe
- **WHEN** the actor changes after establishment and before baseline transmission
- **THEN** the baseline or a subsequent head conveys that transition or a newer coherent state

#### Scenario: Baseline write fails
- **WHEN** actor establishment succeeds but acknowledgement or baseline delivery fails
- **THEN** the connection releases its attachment, presenter claim and observation demand without awaiting a second client operation

#### Scenario: Second primary subscription
- **WHEN** another subscription is requested for an attachment that already has its connection
- **THEN** it is refused without replacing the existing one

### Requirement: Coalescing preserves non-snapshot delivery
Subscriptions MAY skip snapshots but SHALL preserve latest coherent publication order, not promise event history. Receipts SHALL remain on the ordered RPC path. Addressed effects SHALL remain retained/visible until acknowledged or superseded; delivery changes SHALL wake subscribers even with unchanged content hashes. Gate generations SHALL invalidate stale input despite skipped modal snapshots.

#### Scenario: Pending effect survives a skipped frame
- **WHEN** a publication containing a presentation request is superseded before its subscriber consumes it
- **THEN** the latest delivery still contains that unacknowledged request unless it has been superseded or cancelled

#### Scenario: Addressed delivery changes without new content
- **WHEN** a retained addressed delivery changes while tree/state hashes remain equal
- **THEN** its recipient is woken and receives the current pending delivery information

#### Scenario: Modal opens and closes before a frame
- **WHEN** a frontend skips the intermediate snapshots of a complete modal gate transition
- **THEN** the latest gate generation still invalidates input interpreted before that transition

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
Each published UI head SHALL have one increasing server update ID and identify its observation, tree and UI-session payloads by XXH3-128 hashes of their deterministic MessagePack bytes. Clients SHALL adopt complete heads in the current attachment's ordering scope, tolerate skipped snapshots and reject older updates. Content identity SHALL NOT substitute for action, interaction or domain-request identity.

#### Scenario: Selection-only update
- **WHEN** selection changes without changing observation or visible rows
- **THEN** the head retains the observation and tree hashes while naming updated session content

#### Scenario: Old content repeats
- **WHEN** a lane changes from content A to B and back to A
- **THEN** its return has a newer head update ID and can reuse retained A content

### Requirement: Content references are retrievable with bounded retention
The daemon SHALL make content retrievable before publishing its hash, pin the current head's payloads and bound historical retention by bytes and entries. Missing content SHALL return explicit expiry with the current head for recovery. The client SHALL keep the displayed head and bounded handover work, not historical content. A superseded fetch SHALL NOT roll the display back.

#### Scenario: Subscriber lacks a payload
- **WHEN** coalescing skipped inline content required by a head
- **THEN** the client obtains that content by hash or recovers from the current head after expiry

#### Scenario: A fetch finishes late
- **WHEN** a payload fetch finishes after a newer target head has arrived
- **THEN** it is fenced and cannot replace the newer intended display

### Requirement: Content identity covers every field
Every field of each published payload SHALL contribute to its content identity; publication metadata SHALL remain outside payloads.

#### Scenario: Content identity covers every field
- **WHEN** two payloads differ in any field
- **THEN** they have different content identities, and publication metadata changes neither

### Requirement: Navigation uses the canonical visible projection
The session SHALL determine visible ordering, ancestry and folds from existing fleet facts. Frontends SHALL render and navigate the canonical projection, reconciling their own selected identities against it rather than independently rebuilding membership or ordering. Runtime text SHALL retain sanitization and freshness meanings. Presentation changes SHALL NOT establish domain effects.

#### Scenario: Narrow and wide views navigate together
- **WHEN** two frontends navigate the same projection with different clipping and viewport sizes
- **THEN** their selected identities may differ but neither substitutes a local tree's ordering for the canonical rows

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
Each frontend SHALL preserve surviving visible selection, prefer a hidden or removed task's visible owner, then use a bounded positional fallback or empty selection. Reconciliation SHALL NOT execute focus or retarget a captured command. Stale evidence SHALL NOT prove domain removal. Location reuse SHALL NOT transfer frozen execution identity.

#### Scenario: Selected task disappears
- **WHEN** a current projection removes this view's selected task but keeps its owner
- **THEN** this view selects that visible owner without changing other views or tmux focus

#### Scenario: Target already captured
- **WHEN** local selection falls back after an invoked command's row disappears
- **THEN** the command still retains its original frozen identity and cannot use the fallback row

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

### Requirement: Attachment liveness gates actor admission
Connection loss SHALL release an attachment lease without blocking a transport executor or requiring free space in the ordinary actor mailbox. The actor SHALL check lease validity before admitting attachment actions and SHALL reap released attachment resources during normal processing. Cleanup SHALL be scoped to the released attachment identity. Release SHALL NOT cancel a domain operation already admitted.

#### Scenario: Queued action outlives its connection
- **WHEN** an action is queued, its lease is released, and the actor then considers it
- **THEN** it is not admitted as new work from a live attachment

#### Scenario: Mailbox full during disconnect
- **WHEN** the ordinary actor mailbox is full as a connection disappears
- **THEN** release is nonblocking and cleanup remains discoverable without a successful ordinary detach enqueue

#### Scenario: Old cleanup after reconnect
- **WHEN** cleanup for an old UUID is observed after a fresh attachment exists
- **THEN** it cannot release the new attachment or its resources

### Requirement: Optimistic navigation preserves ordered intent
Browsing selection SHALL be local and SHALL NOT require actor navigation submission or acknowledgement. Shared actions SHALL retain bounded FIFO submission and displayed-gate checks. Row-dependent TUI commands SHALL capture the local selected identity at invocation. New gates SHALL fence stale actions and key sequences without restoring a global browsing cursor.

#### Scenario: Rapid navigation and command
- **WHEN** a frontend selects B and immediately invokes a row-dependent command while publications are delayed
- **THEN** that command explicitly identifies B without needing a preceding shared move action

#### Scenario: Modal overtakes speculation
- **WHEN** a new gate arrives while old-gate actions are unsent or awaiting replies
- **THEN** stale input is fenced without copying shared focus into independent selection merely because the gate changed

### Requirement: Reply loss is distinct from admission refusal
The transport SHALL distinguish definite enqueue rejection from unobserved results after enqueue. Timeouts SHALL NOT prove non-execution. Identical retries under a valid attachment MAY use retained receipts. Reconnect SHALL NOT automatically repeat uncertain actions under fresh IDs. Recovery SHALL use known durable request IDs; exact recovery is not guaranteed when the ID was never learned.

#### Scenario: Mailbox rejects an action
- **WHEN** the actor mailbox refuses enqueue
- **THEN** the transport reports definite non-admission without claiming an action transition occurred

#### Scenario: Answer lost after enqueue
- **WHEN** the actor accepted a message but its reply becomes unavailable
- **THEN** the client treats the outcome as potentially applied, not safe to repeat as a new command

#### Scenario: Fresh attachment lacks durable correlation
- **WHEN** a connection is lost before the frontend learns the resulting domain request identity
- **THEN** reconnect adopts current state without claiming that the old action counter guarantees exact durable result recovery

### Requirement: Shared focus replaces shared browsing selection
The existing synchronized-selection contract SHALL become shared observed focus. Each frontend SHALL own its browsing selection. Filters, folds, ordering, canonical row membership and command interactions SHALL remain shared. There SHALL NOT be a second shared browsing cursor or legacy shared-selection fallback.

#### Scenario: Different inspection targets
- **WHEN** two Radar views select different visible rows
- **THEN** they retain independent selections while displaying the same available unified terminal focus

### Requirement: Following focus is local and event driven
The sidebar SHALL default to following and the dashboard to independent selection, with a local toggle. Enabling following and reconnect baseline adoption SHALL adopt available visible focus. A changed focused place SHALL update a following view's selection. Unchanged focus, unrelated publications and zoom-only changes SHALL NOT repeatedly reset browsing. Following SHALL NOT execute focus.

#### Scenario: Browse away between focus changes
- **WHEN** a following sidebar browses to B while focus remains A and an unrelated publication arrives
- **THEN** selection remains B without switching terminals

#### Scenario: Native focus change
- **WHEN** native tmux navigation changes focus to C
- **THEN** following views select C where representable and non-following views keep their selections

#### Scenario: Enable following
- **WHEN** following is enabled with current focus visible
- **THEN** the view selects it without issuing a focus request

#### Scenario: Hidden focus
- **WHEN** focus is outside the canonical visible listing
- **THEN** the view indicates that condition without rewriting shared filters or folds or falsely marking a neighbouring row as focused
