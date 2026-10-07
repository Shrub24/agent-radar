## MODIFIED Requirements

### Requirement: Details can receive keyboard focus
Tab and Shift-Tab SHALL switch between tree and visible details. In details, j/k or Up/Down SHALL scroll unless explicit process-row navigation is active; in that mode they SHALL move the process cursor. PgUp/PgDn SHALL scroll a viewport, Left/Right SHALL cycle pages and Escape SHALL return to the fleet tree. Home/End SHALL reach scroll limits in ordinary detail navigation and the first/last visible process row in process-row navigation. These keys SHALL NOT change fleet selection. Filter entry and confirmation modals SHALL retain precedence; lifecycle keys SHALL NOT initiate actions from details focus.

#### Scenario: Reading details preserves selection
- **WHEN** the user focuses details, changes pages and scrolls
- **THEN** the selected fleet row and its folds remain unchanged
- **AND** the focused panel and active page are visibly identified

#### Scenario: Focused details are hidden
- **WHEN** the user hides the details panel
- **THEN** focus returns to the tree

#### Scenario: Confirmation remains modal
- **WHEN** a confirmation is open
- **THEN** navigation keys operate the confirmation with existing Cancel-first behavior rather than moving detail pages

#### Scenario: Process navigation does not change the fleet
- **WHEN** the user moves between process rows in the Processes page
- **THEN** the fleet selection, fleet folds and lifecycle target remain unchanged

### Requirement: Long content has explicit disclosures
Radar SHALL initially collapse long assignment text and verbose task blocks behind labelled summaries. Outside process-row navigation, Space in details SHALL cycle disclosure targets and Enter SHALL toggle the highlighted target; inside process-row navigation these keys SHALL toggle the selected process branch. Clicking a disclosure SHALL toggle only that content. Identity, state, PID and resource summaries SHALL remain visible. Detail Enter SHALL NOT focus a pane or execute a lifecycle action.

#### Scenario: Expanding an assignment is not a focus action
- **WHEN** the user selects the assignment disclosure and presses Enter
- **THEN** its full sanitized text becomes reachable by scrolling
- **AND** no mux or owner action is issued

## ADDED Requirements

### Requirement: Processes offers a foldable process table
Processes SHALL offer an initially collapsed table of the current root and verified descendants beneath their observed parents. Rows SHALL align name, PID, CPU and RSS with branch indentation and fold markers. The selected sample SHALL supply compact process details. Root resources and qualified descendant totals SHALL remain separate; counts SHALL exclude the root. Narrow panels SHALL keep rows distinct, with omitted table facts reachable in selected-process details.

#### Scenario: Opening the table keeps fleet rows unchanged
- **WHEN** the user opens the process table
- **THEN** verified descendants appear only inside Processes, not as agent or task rows in the fleet

#### Scenario: Selecting a child updates process details
- **WHEN** the user selects a verified descendant
- **THEN** the compact process block describes that descendant using its own sample
- **AND** root binary-freshness facts are not attributed to the selected child

#### Scenario: A narrow panel cannot fit all columns
- **WHEN** the process table is drawn in a narrow panel
- **THEN** the name is shortened and lower-priority metric columns may be omitted without overlapping rows or hiding the PID
- **AND** the selected process's full name and available metrics remain reachable in its compact detail block

### Requirement: Process table navigation is explicit and identity-stable
With Processes focused and its table open, `t` SHALL toggle a visibly identified process-row navigation mode. In that mode, Up/Down or j/k SHALL select visible rows without wrapping, Home/End SHALL reach the first/last row, and Enter/Space SHALL toggle the selected branch; leaves SHALL do nothing. Page switching, Tab hand-off, Escape and viewport scrolling SHALL retain their meanings. Closing the table or leaving Processes SHALL exit this mode.

#### Scenario: Ordinary details still scroll
- **WHEN** process-row navigation is inactive
- **THEN** j/k and Up/Down scroll and Space cycles disclosure targets as before

### Requirement: Process selection follows birth identity
Process selection and folds SHALL use birth identity, not PID alone. Changing fleet selection or root identity SHALL reset both. Missing or replaced selections SHALL fall back to the current root; folding a selected descendant out of view SHALL select its ancestor branch. Refresh SHALL NOT preserve vanished metrics or transfer selection/folds to a recycled PID.

#### Scenario: A recycled PID is not the selected process
- **WHEN** a refresh replaces the selected descendant with another birth identity at the same PID
- **THEN** selection falls back to the current root and no old fold state is inherited by the replacement

#### Scenario: A selected child disappears
- **WHEN** the selected child is absent from the next verified observation
- **THEN** the compact process block returns to the current root rather than retaining the vanished child's metrics

#### Scenario: Folding hides the selected descendant
- **WHEN** a branch is folded while its selected descendant would become hidden
- **THEN** selection moves to that branch and remains visible

### Requirement: Process pointer targets are local
Clicking a process row SHALL select it and enter process-row navigation; clicking its fold marker SHALL only toggle that branch. Process interaction SHALL NOT focus a mux pane or issue lifecycle controls. Hit targets SHALL follow the rendered table after refresh, resize and scrolling, with every visible row reachable.

#### Scenario: Process navigation cannot execute controls
- **WHEN** Enter, Space or a mouse click operates a process row
- **THEN** no mux focus, close, restart or owner-control request is issued

#### Scenario: Refresh or resize moves table rows
- **WHEN** refresh or resize changes the table's rendered position
- **THEN** mouse targets and cursor visibility follow the new layout and all remaining rows stay reachable by scrolling
