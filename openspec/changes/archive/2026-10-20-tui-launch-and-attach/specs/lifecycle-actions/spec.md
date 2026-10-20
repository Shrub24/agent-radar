## ADDED Requirements

### Requirement: L launches the configured managed-agent command
Radar SHALL expose `[tui].launch_command` as an argv vector defaulting to `["pi"]`. Pressing `L` on a recorded workspace or pane SHALL immediately create a tab in that place's workspace running that command as a private recipe, without a command prompt or confirmation. A launch from a pane SHALL open its own tab rather than splitting the pane.

#### Scenario: Launch the default agent
- **WHEN** the user presses `L` on a recorded workspace or pane without a launch-command override
- **THEN** Radar starts `pi` in a new tab of that place's workspace

#### Scenario: Launch from a pane
- **WHEN** the user presses `L` on a pane
- **THEN** the agent gets a window of its own in that pane's workspace, and the pane it was launched from is left as it was

### Requirement: L uses the selected place's shell context
A launch SHALL use the pane's current directory, or the workspace's recorded directory when the selected place is the workspace itself. It SHALL start through the user's login/interactive shell and apply direnv in that target directory. It SHALL NOT copy the TUI environment wholesale or persist environment values in public records, reads, or events.

#### Scenario: Launch from a workspace
- **WHEN** the user presses `L` on a workspace
- **THEN** the new tab starts in the workspace's recorded directory

#### Scenario: Launch from a pane
- **WHEN** the user presses `L` on a pane
- **THEN** the new tab starts in that pane's current directory

#### Scenario: The TUI uses another project's environment
- **WHEN** the TUI runs with project-specific variables and `L` targets a different directory
- **THEN** the launched process uses shell startup and the target directory's direnv values without copying those TUI-only variables

### Requirement: Invalid managed launches are refused before creation
Radar SHALL refuse a launch without creating a pane or tab when its argv is empty, the user shell is missing or unsupported, or the selected place has no absolute working directory. The target shell SHALL resolve the configured executable after startup and target-directory direnv activation.

#### Scenario: The shell or directory is unavailable
- **WHEN** `SHELL` is unset or unsupported, or no absolute target directory is known
- **THEN** Radar reports the refusal and creates no tab or pane

#### Scenario: The target shell cannot find the configured executable
- **WHEN** the target shell cannot resolve the configured program after startup and direnv activation
- **THEN** the target shell reports the command failure in the launched place; the dashboard does not pre-resolve it using its own `PATH`

### Requirement: An empty shell remains available
The `a` action SHALL create an empty shell in the selected place for arbitrary interactive commands; it SHALL NOT require those commands to be configured as managed-agent launches.

#### Scenario: Run an arbitrary command
- **WHEN** the user wants an interactive command instead of the configured managed agent
- **THEN** the user can create an empty shell with `a` and run the command there

### Requirement: A launched or registered lead can be enrolled
Radar SHALL enroll a lead started with `L` through `lifecycle.enroll` once it has registered a joined pane and a fresh session, once per launched place, and SHALL offer `E` on any joined registered agent that is not a lifecycle subject through the default-Cancel confirmation. Enrollment SHALL revalidate the same registration and session before sending, SHALL record no spawn edge, and SHALL signal nothing.

#### Scenario: A launched lead registers
- **WHEN** a lead started with `L` registers a joined pane and a fresh session
- **THEN** one enrollment is sent for it and the row becomes a lifecycle subject

#### Scenario: A lead started elsewhere
- **WHEN** the user presses `E` on a joined registered agent that is not a subject
- **THEN** a confirmation names the session and Cancel is selected

#### Scenario: Nothing to enroll
- **WHEN** `E` is pressed on a pane that is not a registered agent
- **THEN** no dialog opens and a one-line message says so
