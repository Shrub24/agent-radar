# Spec Delta

## Purpose

Make selected-row details readable through focused pages and bounded scrolling while preserving the fleet's selection and action safety.

## ADDED Requirements

### Requirement: Selected facts are organized into detail pages
Radar SHALL provide Overview, Processes, Tasks and Source pages. Overview SHALL put location, current PID, activity and model before long content. Previously exposed facts SHALL remain reachable, including labelled owner projection and observation freshness. Pages SHALL show facts applicable to the selected row without borrowing another row's process identity.

#### Scenario: Agent details do not bury the PID
- **WHEN** a current agent or subagent has foreground PID evidence and a long assignment
- **THEN** Overview shows the PID near its location before that assignment
- **AND** Source and Tasks expose its diagnostic and published task facts separately

#### Scenario: A selected task has no verified process identity
- **WHEN** a task publishes a PID but no birth identity
- **THEN** its Processes page shows the published PID with its source
- **AND** explains why resource metrics are unavailable rather than attributing a current process to it

### Requirement: Details can receive keyboard focus
Tab and Shift-Tab SHALL switch between tree and visible details. In details, j/k or Up/Down SHALL scroll, PgUp/PgDn SHALL scroll a viewport, Home/End SHALL reach limits, Left/Right SHALL cycle pages and Escape SHALL return to tree. These keys SHALL NOT change fleet selection. Filter entry and confirmation modals SHALL retain precedence; lifecycle keys SHALL NOT initiate actions from details focus.

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

### Requirement: Long content has explicit disclosures
Radar SHALL initially collapse long assignment text and verbose task blocks behind labelled summaries. Space in details SHALL cycle disclosure targets and Enter SHALL toggle the highlighted target; clicking a disclosure SHALL toggle only that content. Identity, state, PID and resource summaries SHALL remain visible. Detail Enter SHALL NOT focus a pane or execute a lifecycle action.

#### Scenario: Expanding an assignment is not a focus action
- **WHEN** the user selects the assignment disclosure and presses Enter
- **THEN** its full sanitized text becomes reachable by scrolling
- **AND** no mux or owner action is issued

### Requirement: Detail scroll follows rendered content
Mouse wheel SHALL continue scrolling the panel under the pointer and page headings SHALL be clickable. Scroll SHALL be clamped after resize or refresh. Selection change SHALL reset scroll and disclosures while preserving active page; page switching SHALL preserve each page's scroll for the same selection. Every page SHALL remain reachable on narrow terminals.

#### Scenario: A shorter refreshed page has no empty tail
- **WHEN** refreshed content becomes shorter or the terminal grows
- **THEN** its scroll offset is clamped to the rendered content

#### Scenario: Narrow details retain page access
- **WHEN** the details panel is narrow or stacked under the fleet
- **THEN** all four pages and their navigation remain accessible without drawing outside the panel
