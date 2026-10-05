# mouse-control Specification

## Purpose

Let the tree be used with the mouse as well as the keyboard, without moving the user's terminal by accident.

## Requirements

### Requirement: Mouse capture with the keyboard's meaning

Radar SHALL enable mouse capture while it runs and SHALL disable it on exit, and every mouse action SHALL do what its key equivalent does. Radar SHALL state in its documentation that capture takes the terminal's own text selection.

#### Scenario: Exit
- **WHEN** Radar quits, including on the failure path
- **THEN** mouse capture and raw mode are both released

### Requirement: Hit-testing the drawn tree

Radar SHALL map a mouse event to the row drawn at that position, using the area the tree was drawn into and the list's scroll offset, and SHALL take no action for a position that holds no row.

#### Scenario: A position with no row
- **WHEN** a click lands past the last row or in the details panel
- **THEN** the selection and the fold state are unchanged

#### Scenario: Folded rows
- **WHEN** a heading is folded and a click lands on a line below it
- **THEN** the row drawn on that line is the one selected, not a row hidden by the fold

### Requirement: Wheel scrolls what is under the pointer

Radar SHALL scroll the tree when the wheel turns over the tree and the details panel when it turns over the details panel.

#### Scenario: Over the tree
- **WHEN** the wheel turns over the tree
- **THEN** the tree scrolls without changing the selection or the fold state

### Requirement: A click selects, a second click focuses

Radar SHALL select the row a click lands on, SHALL fold or unfold a workspace heading a click lands on, and SHALL focus the pane of a row that is already selected when it is clicked again. A row with no observed pane SHALL send nothing.

#### Scenario: Selecting while browsing
- **WHEN** a row that is not selected is clicked
- **THEN** it becomes selected and no focus request is sent

#### Scenario: Acting on the selected row
- **WHEN** the selected agent row is clicked again and its pane is observed
- **THEN** Herdr is asked to focus that pane

#### Scenario: A heading
- **WHEN** a workspace heading is clicked
- **THEN** it folds or unfolds as `Space` would

#### Scenario: A stale inventory
- **WHEN** the selected row is clicked again while a failed collection is showing last-good rows
- **THEN** no request is sent and the reason is shown in one line
