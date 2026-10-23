# lifecycle-actions Specification

## Purpose
Let an operator close panes or restart managed workers through the authority that owns them, without confusing a request with an applied effect.

## Requirements

### Requirement: Lifecycle actions require separate confirmation
Radar SHALL open confirmation for x (selected pane/agent close), X (its tab close) and r (managed-worker restart). Opening confirmation SHALL send nothing. It SHALL name the target and possible losses, default to Cancel, and require activation of Confirm. Task/workspace rows SHALL not redirect destructive actions to their parents.

#### Scenario: Close confirmation
- **WHEN** x is pressed on an eligible row
- **THEN** a confirmation names the pane and any process, assignment or outstanding work that may be lost, without sending a request

#### Scenario: Cancel or accidental Enter
- **WHEN** Escape is pressed or the initially selected Cancel is activated
- **THEN** no lifecycle action is submitted

#### Scenario: Mouse confirmation
- **WHEN** the operator clicks Confirm in the drawn dialog
- **THEN** the same checks as keyboard confirmation apply
- **AND** tree second-click focus never serves as lifecycle confirmation

#### Scenario: Filter entry
- **WHEN** a lifecycle key is typed while editing the filter
- **THEN** it is filter text and opens no confirmation

### Requirement: Confirmation is tied to observed identity
Radar SHALL refuse lifecycle submission from stale inventory or absent targets. Confirmation SHALL freeze operation and target identities and SHALL be revalidated before submission. A changed identity or containment SHALL cancel the action with a reason, not redirect it to the current selection.

#### Scenario: Replacement during confirmation
- **WHEN** the pane's session or managed run changes before Confirm
- **THEN** Radar sends nothing and reports the target changed

#### Scenario: Collection fails
- **WHEN** inventory is no longer current at confirmation
- **THEN** no action is submitted

### Requirement: Direct runtime close requires positive unmanaged evidence
Radar SHALL route unmanaged pane/tab close through its runtime provider only with current positive unmanaged evidence. Pi panes without ownership metadata are uncertain. Managed and continuity-retained managed associations SHALL never use direct runtime close. Unsupported providers SHALL refuse explicitly.

#### Scenario: Ordinary unmanaged pane
- **WHEN** current evidence reports no agent and no retained or uncertain managed association
- **THEN** confirmed close can reach the runtime provider

#### Scenario: Unverified Pi pane
- **WHEN** a Pi pane publishes no ownership metadata
- **THEN** direct close is refused, not treated as unmanaged

#### Scenario: Retained managed association
- **WHEN** a managed association is locally retained while its pane still exists
- **THEN** direct runtime close remains forbidden

### Requirement: Container close is all or nothing at admission
Radar SHALL refuse an entire tab close when any observed member is managed or uncertain. It SHALL perform no partial pane closure and SHALL recheck containment before direct tab close.

#### Scenario: Mixed tab
- **WHEN** a tab contains an ordinary pane and a managed or unverified Pi pane
- **THEN** tab close is refused and neither pane is closed

### Requirement: Managed operations are owner-routed
Radar SHALL send confirmed managed close/restart only to the exact parent-session owner using herdsman-control/v1, with exact label/run identity, available cross-checks and matching confirmation. Missing identity or owner SHALL refuse. Restart SHALL be limited to idle retained managed workers; leads, standalone sessions and busy targets SHALL not be restarted.

#### Scenario: Idle managed worker restart
- **WHEN** an eligible idle managed worker is confirmed
- **THEN** the request echoes its operation, label and run UUID and asks its exact owner to restart it

#### Scenario: Busy or parentless target
- **WHEN** restart targets a working/waiting/blocked worker or a lead/standalone session
- **THEN** Radar refuses without a mux launch fallback

#### Scenario: Close abandoned assignment
- **WHEN** a managed close is confirmed for active work
- **THEN** Radar warns that the assignment may be abandoned and the owner decides eligibility

### Requirement: Owner file transport preserves trust and identity
Requests SHALL use trusted owner-created user-owned 0700 non-symlink directories, unique UUID filenames, atomic 0600 publication, an 8 KiB limit and absolute expiry. Radar SHALL not create owner directories or overwrite requests. Results SHALL be bounded and validated against request id, version and operation; unknown fields SHALL be tolerated.

#### Scenario: Untrusted or missing directory
- **WHEN** an owner directory is absent, symlinked, foreign-owned or has loose permissions
- **THEN** Radar refuses and writes nothing

#### Scenario: Mismatched result
- **WHEN** a result names a different request or operation
- **THEN** Radar reports invalid evidence and does not claim completion

### Requirement: Outcomes come from files, never local timeout
Radar SHALL derive result, started, not_executed and pending in that precedence. Claimed requests SHALL never be retried; local timeout and shutdown SHALL not imply cancellation. Pending or unknown actions SHALL block duplicate submission for that target until authoritative resolution.

#### Scenario: Claim without result
- **WHEN** a claim exists but no result exists
- **THEN** Radar reports started with unknown outcome even after expiry and does not retry
- **AND** a later valid result can refine the display

#### Scenario: Expired unclaimed request
- **WHEN** neither result nor claim exists and absolute expiry passed
- **THEN** Radar reports not_executed

#### Scenario: Owner refusal
- **WHEN** a valid refused result exists
- **THEN** Radar shows the owner's category and message without removing or changing the row optimistically

#### Scenario: Quit after submitting
- **WHEN** Radar quits with a published request
- **THEN** quitting remains responsive and Radar does not delete or cancel the request

### Requirement: Lifecycle results do not replace observation
Lifecycle work SHALL not block input, collection or quitting. Outcomes SHALL be shown separately from source diagnostics, with no optimistic row removal, whether read from owner files or control-plane records. Managed controls SHALL remain unavailable until owner implementation acceptance is confirmed and SHALL refuse when their transport is unavailable. An uncertain control-plane outcome SHALL not trigger direct retry.

#### Scenario: Owner control unavailable
- **WHEN** the owner implementation is unconfirmed or no trusted transport exists
- **THEN** managed actions explain their unavailability while observation and focus continue

#### Scenario: Applied operation
- **WHEN** a valid closed or restarted result arrives
- **THEN** Radar reports it and reconciles rows through subsequent inventory rather than fabricating runtime state

#### Scenario: Control-plane outcome unknown
- **WHEN** a control-plane response is lost after possible dispatch
- **THEN** Radar reports uncertainty, issues no direct retry and fabricates no row removal

### Requirement: A new workspace starts in a chosen project directory
Radar SHALL, on `A`, offer a fuzzy picker of project directories and create a workspace in the chosen one, named for the folder of the git root containing it, or for the directory's own folder when it is in no repository. Cancelling the picker SHALL create nothing. Radar SHALL NOT follow a later change of directory in the workspace.

#### Scenario: A project is chosen
- **WHEN** the user presses `A` and chooses a directory inside a git repository
- **THEN** a workspace is created in that directory and named for the repository's folder

#### Scenario: The picker is cancelled
- **WHEN** the user escapes from the picker
- **THEN** no workspace is created and the dashboard is drawn again

### Requirement: Project candidates come from a configured source
Radar SHALL offer the directory it was started in first, then the directories printed by `[tui] project_source` (default `zoxide query -l`) that are absolute, existing and accepted by `[tui] project_filter` (default: containing `.git`), each once. Radar SHALL NOT run anything the source prints. A source that is missing, fails or exceeds its time limit SHALL leave only the start directory and say so in one line.

#### Scenario: No source is installed
- **WHEN** the project source program is missing
- **THEN** the picker offers only the start directory and a one-line message names the failure

### Requirement: A workspace directory is checked by the runtime
The runtime SHALL refuse a workspace creation whose directory is relative or is not an existing directory before creating anything, and SHALL start a created tmux session in the directory it was given. A runtime that cannot start a workspace in a directory SHALL refuse it.

#### Scenario: A missing directory
- **WHEN** a workspace creation names a directory that does not exist
- **THEN** it is refused and no session is created

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

### Requirement: The standing of a Pi session is readable in place
Radar SHALL answer `E` on a selected Pi session with a one-line message naming its claim standing (granted, duplicate, reserved, pending or released), or stating that it is waiting for the next presence report when none is recorded. It SHALL send no request, open no dialog and change nothing.

#### Scenario: A granted session
- **WHEN** `E` is pressed on a Pi session whose claim is granted
- **THEN** a message says it already has a Radar subject and nothing is sent

#### Scenario: A rejected duplicate
- **WHEN** `E` is pressed on a session whose claim was rejected as a duplicate
- **THEN** the message says to reload or resume from its last live state

#### Scenario: Not a Pi session
- **WHEN** `E` is pressed on a pane that is not a Pi session
- **THEN** a one-line message says only a Pi session can be managed and nothing else happens
