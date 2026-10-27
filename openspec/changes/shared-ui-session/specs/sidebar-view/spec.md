# Spec Delta

## MODIFIED Requirements

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
