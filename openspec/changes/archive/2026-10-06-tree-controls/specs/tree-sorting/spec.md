# Spec Delta

## Purpose

Order the rows so that the ones needing a human come first, without breaking the structure of the tree.

## ADDED Requirements

### Requirement: Three row orders, cycled by one key

Radar SHALL order the rows by source order, by state or by name; SHALL start in source order; and SHALL cycle the three with `s`. Radar SHALL name the current order in the hint line.

#### Scenario: Cycling
- **WHEN** `s` is pressed three times from the default
- **THEN** the order passes through state and name and returns to source order, and the hint line names whichever is active

#### Scenario: The filter is being edited
- **WHEN** `s` is pressed while the filter is being edited
- **THEN** it is entered into the filter and the order is unchanged

### Requirement: Ordering stays inside a level

Radar SHALL order rows within each level of the tree and SHALL NOT move a row out of its parent, its group or its workspace. Under state order, a group SHALL rank by the highest-priority state among the rows beneath it.

#### Scenario: A child keeps its parent
- **WHEN** the order is by state and a worker's state outranks its owner's
- **THEN** the worker is still drawn beneath its owner

#### Scenario: A group follows its contents
- **WHEN** the order is by state and one workspace holds a blocked agent while another holds only idle ones
- **THEN** the workspace holding the blocked agent is drawn first

### Requirement: A sort never changes anything but position

Radar SHALL keep the selection on the same row when the order changes, SHALL preserve each fold, and SHALL NOT change the filter, the pane view, the details panel or which tasks are joined to which row.

#### Scenario: Selection follows the row
- **WHEN** the user is on a row low in the list and changes the order
- **THEN** that same row is selected wherever it now sits

#### Scenario: Fold state survives
- **WHEN** a workspace is folded and the order changes
- **THEN** it stays folded
