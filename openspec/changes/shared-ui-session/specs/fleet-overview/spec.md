# Spec Delta

## MODIFIED Requirements

### Requirement: Keyboard selection and folding

Radar SHALL support keyboard selection and expanding/collapsing tree branches using Vim-style keys and arrow-key navigation through the shared UI session. Refreshes SHALL preserve selection and fold state for surviving row identities where possible. If a selected row disappears, selection SHALL remain on a valid visible row or become empty when none remain. Frontend viewport and details state SHALL remain local.

#### Scenario: Navigate and fold
- **WHEN** the user navigates to an owner and collapses its branch
- **THEN** its descendants are hidden and selection remains valid in every attached presentation

#### Scenario: Stable refresh
- **WHEN** a refresh updates facts without removing the selected row
- **THEN** selection remains on that row and existing fold state is preserved

#### Scenario: Independent viewport
- **WHEN** one frontend scrolls or changes terminal dimensions
- **THEN** its local geometry does not overwrite another frontend's viewport or canonical selection

### Requirement: Text filtering

Radar SHALL support a shared live case-insensitive fuzzy filter over displayed labels and available role/assignment text, edited by at most one attachment. Matching rows SHALL remain visible with the ancestors needed to understand their placement, retaining tree ordering rather than becoming a flat ranked result list. Clearing the filter SHALL restore the unfiltered tree and prior fold state.

#### Scenario: Match a nested assignment
- **WHEN** the filter matches a worker's reported assignment but not its owner's label
- **THEN** the worker and its owner/workspace ancestry are visible

#### Scenario: Clear filter
- **WHEN** the user clears the filter
- **THEN** all otherwise eligible rows return with the pre-filter fold state

#### Scenario: Live shared query
- **WHEN** the editor changes the query without pressing a commit key
- **THEN** all attached frontends receive the new canonical filter and resulting selection

### Requirement: Background-task rows can be toggled

Outside filter entry, b SHALL toggle task-row visibility for the current shared pane view without changing that view or the finished-session toggle. Each view's choice SHALL be retained for the daemon-hosted UI session. The footer SHALL name the effective action. A hidden selected task SHALL fall back to its surviving visible parent.

#### Scenario: Toggle task rows
- **WHEN** b is pressed outside filter entry
- **THEN** task children switch between visible and hidden in the current shared view, and each frontend's footer changes between showing and hiding background-task rows

#### Scenario: Mode choices survive cycling
- **WHEN** task rows are hidden with b in running view, and the user cycles through all and agents views back to running
- **THEN** running keeps its hidden choice without changing the choices for other views

#### Scenario: Hide a selected task
- **WHEN** the selected task is hidden by a view change or b
- **THEN** its surviving visible parent becomes selected, or another valid visible row if that parent is absent
- **AND** showing task rows again preserves existing branch folds and never automatically unfolds an agent

#### Scenario: Updates while hidden
- **WHEN** bus or inventory facts change while task rows are hidden
- **THEN** the parent badge and details reflect the latest normalized projection
- **AND** showing task rows later displays that projection rather than a cached earlier list

#### Scenario: b while typing a filter
- **WHEN** b is typed during filter entry
- **THEN** it is filter text rather than a visibility command

#### Scenario: A new Radar run
- **WHEN** a frontend restarts and attaches to the existing UI session
- **THEN** it receives that session's choices instead of resetting them
- **AND** a daemon restart starts a new volatile UI session with built-in defaults rather than restoring choices from disk
