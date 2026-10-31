## MODIFIED Requirements

### Requirement: One canonical UI session serves all presentations
The daemon SHALL own one volatile UI session shared by attached dashboards and sidebars. The synchronized cursor path SHALL carry observed terminal focus rather than browsing selection. Folds, ordering, visibility, filter and interactions SHALL remain shared. Each frontend SHALL own selection, dimensions, viewport, split geometry, detail interaction and local informational overlays.

#### Scenario: Independent inspection
- **WHEN** two dashboards select different rows with different detail pages
- **THEN** they retain local selection and details while receiving the same shared focus and tree settings

### Requirement: Navigation uses the canonical visible projection
The session SHALL determine visible ordering, ancestry and folds from existing fleet facts. Frontends SHALL render and navigate the canonical projection, reconciling their own selected identities against it rather than independently rebuilding membership or ordering. Runtime text SHALL retain sanitization and freshness meanings. Presentation changes SHALL NOT establish domain effects.

#### Scenario: Different viewport sizes
- **WHEN** two frontends navigate the same projection with different clipping and viewport sizes
- **THEN** their selected identities may differ but neither substitutes a local tree's ordering for the canonical rows

### Requirement: Selection reconciliation preserves identity and freshness
Each frontend SHALL preserve surviving visible selection, prefer a hidden or removed task's visible owner, then use a bounded positional fallback or empty selection. Reconciliation SHALL NOT execute focus or retarget a captured command. Stale evidence SHALL NOT prove domain removal. Location reuse SHALL NOT transfer frozen execution identity.

#### Scenario: Task disappears
- **WHEN** a current projection removes this view's selected task but keeps its owner
- **THEN** this view selects that visible owner without changing other views or tmux focus

#### Scenario: Target already captured
- **WHEN** local selection falls back after an invoked command's row disappears
- **THEN** the command still retains its original frozen identity and cannot use the fallback row

### Requirement: Attachment establishes a fresh ordering baseline
Open and reconnect SHALL establish a fresh attachment UUID and action counter starting at zero, with first action one. Registration and its primary publication receiver SHALL be established together. Readiness SHALL require coherent baseline adoption. Old-attachment responses SHALL be discarded. Attachment SHALL NOT reset canonical state or the publication counter. The baseline SHALL convey shared focus, not a shared browsing selection.

#### Scenario: Reconnect
- **WHEN** a frontend reconnects
- **THEN** it uses a fresh attachment and coherent baseline and rejects old responses without comparing publication counters across daemon restarts

#### Scenario: Another dashboard attaches
- **WHEN** another dashboard attaches
- **THEN** it receives current focus and shared settings without resetting another view's selection or the shared filter

### Requirement: Optimistic navigation preserves ordered intent
Browsing selection SHALL be local and SHALL NOT require actor navigation submission or acknowledgement. Shared actions SHALL retain bounded FIFO submission and displayed-gate checks. Row-dependent TUI commands SHALL capture the local selected identity at invocation. New gates SHALL fence stale actions and key sequences without restoring a global browsing cursor.

#### Scenario: Rapid navigation and command
- **WHEN** a frontend selects B and immediately invokes a row-dependent command while publications are delayed
- **THEN** that command explicitly identifies B without needing a preceding shared move action

#### Scenario: Modal overtakes input
- **WHEN** a new gate arrives while old-gate actions are unsent or awaiting replies
- **THEN** stale input is fenced without copying shared focus into independent selection merely because the gate changed

## ADDED Requirements

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
