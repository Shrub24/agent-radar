## MODIFIED Requirements

### Requirement: Focus the selected row

Radar SHALL, on ordinary Enter outside confirmation, ask the runtime provider to focus the selected row's location: the pane of an agent or pane row, the workspace of a workspace row. Radar SHALL keep running and keep its selection. Enter on the tree SHALL not submit lifecycle actions. A retained row is focusable while its pane is observed.

#### Scenario: An agent row
- **WHEN** the user presses Enter on an agent row with a live pane outside confirmation
- **THEN** the runtime provider is asked to focus that pane and Radar remains open with the same row selected

#### Scenario: A workspace row
- **WHEN** the user presses Enter on a workspace row outside confirmation
- **THEN** the runtime provider is asked to focus that workspace
