# sidebar-view Specification

## Purpose
Defines the sidebar: a compressed view of the dashboard that keeps its management actions, floats its dialogs over the workspace, focuses the view's own client, and expands into the dashboard on request.

## Requirements

### Requirement: The sidebar is a compressed projection of the dashboard

`radar sidebar` SHALL run as its own presentation of the default shared UI session and draw its canonical tree with compressed rows and no details panel. Tree, folds, selection and filter SHALL be shared with dashboards; terminal geometry, viewport and expanded/compressed presentation SHALL remain local.

#### Scenario: Narrow rows
- **WHEN** the sidebar is drawn in a narrow terminal
- **THEN** each row shows its mark, its name and its state, the name giving way first when the row is wider than the terminal, and no details panel is drawn

#### Scenario: Local state
- **WHEN** a sidebar and a full dashboard run together and one folds a branch or sets a filter
- **THEN** the other's canonical tree, folds, selection and filter reflect that change, while its details and geometry remain local

#### Scenario: Same keys
- **WHEN** a navigation, jump, fold, filter or pane-view key is pressed in the sidebar
- **THEN** it invokes the same semantic session action as the full dashboard

### Requirement: The sidebar retains dashboard management

The sidebar SHALL offer the dashboard's launch and lifecycle actions. A narrow
presentation SHALL NOT remove authority or bypass confirmation and revalidation.

#### Scenario: Management key
- **WHEN** a management key is pressed in the sidebar
- **THEN** it reaches the same action and confirmation path as the dashboard

### Requirement: Modals float over the whole workspace where the host can

Where the host supports full-workspace popups, the sidebar SHALL request one detached presenter for the session's captured interaction rather than draw it in the narrow pane or reconstruct another App. The presenter SHALL submit its result directly to the session. Missing or changed targets SHALL refuse without selecting a default replacement.

#### Scenario: Confirmation over both areas
- **WHEN** a lifecycle command is invoked for the displayed agent row in a workspace host
- **THEN** the single confirmation presenter is centred over sidebar and agent area for that captured identity, while other dashboards show the shared gate

#### Scenario: Row gone
- **WHEN** the captured target no longer passes validation before confirmation/submission
- **THEN** the interaction reports that reason and no operation is redirected to another row

#### Scenario: Answered
- **WHEN** the session acknowledges the modal's typed result
- **THEN** the popup can close without waiting for admitted domain work, and attached views receive its subsequent authoritative outcome

#### Scenario: Sidebar exits
- **WHEN** the originating sidebar exits after its popup has claimed the interaction
- **THEN** the popup can still resolve it without a parent callback or interpreting tmux's exit status as the chosen value

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
only if the host command succeeded.

#### Scenario: Expanded
- **WHEN** the zoom key is pressed and `zoom_command` succeeds
- **THEN** the same process draws the whole dashboard on the same selection and filter

#### Scenario: Returned
- **WHEN** the zoom key is pressed again
- **THEN** `unzoom_command` runs and the compressed tree is drawn again

#### Scenario: No host command
- **WHEN** the zoom key is pressed with the command unset, failing to start, or exiting unsuccessfully
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
