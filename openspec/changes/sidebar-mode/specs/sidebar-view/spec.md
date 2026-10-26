## ADDED Requirements

### Requirement: The sidebar is a compressed projection of the dashboard

`radar sidebar` SHALL run the dashboard as its own process on the shared observation and draw the session tree with compressed rows and no details panel. Its tree, fold, selection and filter state SHALL be local to that process.

#### Scenario: Narrow rows
- **WHEN** the sidebar is drawn in a narrow terminal
- **THEN** each row shows its mark, its name and its state, the name giving way first when the row is wider than the terminal, and no details panel is drawn

#### Scenario: Local state
- **WHEN** a sidebar and a full dashboard run together and one folds a branch or sets a filter
- **THEN** the other's tree, fold, selection and filter are unchanged

#### Scenario: Same keys
- **WHEN** a navigation, jump, fold, filter or pane-view key is pressed in the sidebar
- **THEN** it does what it does in the full dashboard

### Requirement: The sidebar retains dashboard management

The sidebar SHALL offer the dashboard's launch and lifecycle actions. A narrow
presentation SHALL NOT remove authority or bypass confirmation and revalidation.

#### Scenario: Management key
- **WHEN** a management key is pressed in the sidebar
- **THEN** it reaches the same action and confirmation path as the dashboard

### Requirement: Modals float over the whole workspace where the host can

Where the host can show a popup over the whole workspace, the sidebar SHALL open
its pickers and confirmations there, over the row that was selected, instead of
drawing them in its own pane. The popup SHALL select that row by identity and
SHALL refuse, changing nothing, if it no longer exists.

#### Scenario: Confirmation over both areas
- **WHEN** a lifecycle key is pressed on a selected agent row in a workspace host
- **THEN** its confirmation is drawn centred over the sidebar and the agent area, for that row

#### Scenario: Row gone
- **WHEN** the row selected in the sidebar has gone by the time the popup reads the fleet
- **THEN** the popup says so, no confirmation opens and nothing is requested

#### Scenario: Answered
- **WHEN** the modal is answered and any work it started has finished
- **THEN** the popup closes and the sidebar shows the result through the observation

### Requirement: Focusing an agent moves the view's client

Pressing the focus key on an agent in the sidebar SHALL send the existing focus action for that agent's pane. The sidebar SHALL NOT name a client or session in it, because one view is supported.

#### Scenario: Focus
- **WHEN** an agent row is selected and the focus key is pressed, and one tmux client is attached
- **THEN** the daemon's focus action runs for that pane and the attached client shows it

#### Scenario: Stale fleet
- **WHEN** the observation is stale
- **THEN** the sidebar does not send a focus and says why, as the full dashboard does

### Requirement: The sidebar expands into the dashboard as an explicit mode

The sidebar is the dashboard process and SHALL have an explicit expanded mode,
toggled by its zoom key, that draws the whole dashboard with the details panel and
every key. The mode SHALL NOT depend on the terminal's width. Toggling SHALL ask
the host with `[sidebar] zoom_command` or `unzoom_command` and change the mode
only if the host command started.

#### Scenario: Expanded
- **WHEN** the zoom key is pressed and `zoom_command` starts
- **THEN** the same process draws the whole dashboard on the same selection and filter

#### Scenario: Returned
- **WHEN** the zoom key is pressed again
- **THEN** `unzoom_command` runs and the compressed tree is drawn again

#### Scenario: No host command
- **WHEN** the zoom key is pressed with the command unset or failing to start
- **THEN** the mode is unchanged and the footer names the command to set

### Requirement: Hide and unhide are host commands

The sidebar SHALL run `[sidebar] hide_command` or `show_command` on its toggle key, each an argv with no shell parsing. Below a minimum width it SHALL draw a collapsed rail instead of the tree. When the command for the requested direction is not configured, it SHALL say so and change nothing.

#### Scenario: Hide
- **WHEN** the toggle key is pressed while the tree is drawn and `hide_command` is configured
- **THEN** that command is run and the sidebar's own state is unchanged until the terminal resizes it

#### Scenario: Rail
- **WHEN** the terminal gives the sidebar less than the minimum width
- **THEN** a collapsed rail is drawn and the tree is not

#### Scenario: Unconfigured
- **WHEN** the toggle key is pressed and the command for that direction is unset
- **THEN** the footer names the missing setting and no command runs

#### Scenario: Command failure
- **WHEN** a configured command cannot be started or exits unsuccessfully
- **THEN** the footer reports it and the sidebar continues unchanged
