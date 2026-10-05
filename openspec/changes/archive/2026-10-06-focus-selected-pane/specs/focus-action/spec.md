# Spec Delta

## Purpose

Let the user move from the overview to a pane without leaving Radar or changing anything about an agent.

## ADDED Requirements

### Requirement: Focus the selected row

Radar SHALL, on `Enter`, ask Herdr to focus the selected row's location: the pane of an agent or pane row, the workspace of a workspace row. Radar SHALL keep running and keep its selection. Focus SHALL be the only change Radar asks Herdr to make. A retained row is focusable: continuity keeps it only while its pane is still observed.

#### Scenario: An agent row
- **WHEN** the user presses `Enter` on an agent row with a live pane
- **THEN** Herdr is asked to focus that pane's workspace, tab and pane, and Radar remains open with the same row selected

#### Scenario: A workspace row
- **WHEN** the user presses `Enter` on a workspace row
- **THEN** Herdr is asked to focus that workspace

### Requirement: No focus on a location Radar cannot prove

Radar SHALL ask Herdr to focus a row's location whenever the current observation holds that location. Radar SHALL NOT ask when the displayed inventory is not current (a failed collection showing last-good data) or when the row's pane is absent from the observation, and SHALL say why in one line.

A retained row is not a refusal: continuity keeps a row only while its pane is still observed, so a retained agent's row focuses the pane it was last seen on.

#### Scenario: A retained row
- **WHEN** the user presses `Enter` on a retained row
- **THEN** Herdr is asked to focus the pane that row was last observed on

#### Scenario: A stale inventory
- **WHEN** collection has failed and the displayed rows are last-good data
- **THEN** no request is sent and a one-line message states that the inventory is not current

#### Scenario: A row whose pane is not in the observation
- **WHEN** the selected row's pane is absent from the current observation
- **THEN** no request is sent and a one-line message states that the pane is not observed

### Requirement: A failed focus is stated and harmless

Radar SHALL NOT block input or quitting while a focus request is outstanding, and SHALL state a failure in one line that does not replace the source diagnostic.

#### Scenario: Herdr does not answer
- **WHEN** a focus request exceeds the command timeout
- **THEN** the interface remains responsive, the request is abandoned and a failure line is shown

#### Scenario: Filter entry
- **WHEN** the filter is being edited and the user presses `Enter`
- **THEN** the filter is kept as before and no focus request is sent
