# lifecycle-actions Specification

## Purpose
Let an operator close panes or restart managed workers through the authority that owns them, without confusing a request with an applied effect.

## Requirements

### Requirement: Lifecycle actions require separate confirmation
Radar SHALL offer one semantic Close command on `D` and restart on `r`. Destructive confirmation SHALL name the selected resource and known running work, default to Cancel and send nothing until accepted. Keyboard, mouse and palette SHALL reach the same daemon-owned target resolution; unsupported rows SHALL NOT redirect to parents.

#### Scenario: Close confirmation
- **WHEN** the operator confirms `D` on an eligible agent
- **THEN** one operation terminates its managed execution and removes its pane without a second `D`

#### Scenario: Mouse confirmation
- **WHEN** the operator accepts the drawn close confirmation with the mouse
- **THEN** it uses the same target and checks as keyboard acceptance, never a focus second-click

#### Scenario: Filter entry
- **WHEN** a close key is typed while editing a filter
- **THEN** it changes filter text rather than dispatching close

#### Scenario: Cancel or accidental Enter
- **WHEN** the operator cancels a close dialog
- **THEN** no termination or removal is submitted

### Requirement: Confirmation is tied to observed identity
Radar SHALL refuse lifecycle submission from stale inventory or absent targets. Confirmation SHALL freeze operation and target identities and SHALL be revalidated before submission. A changed identity or containment SHALL cancel the action with a reason, not redirect it to the current selection.

#### Scenario: Replacement during confirmation
- **WHEN** the pane's session or managed run changes before Confirm
- **THEN** Radar sends nothing and reports the target changed

#### Scenario: Collection fails
- **WHEN** inventory is no longer current at confirmation
- **THEN** no action is submitted

### Requirement: Direct runtime close requires positive unmanaged evidence
Close SHALL terminate known managed executions through the common lifecycle implementation before terminal removal. Ordinary unmanaged workloads SHALL use best-effort terminal teardown. Uncertain agent identity SHALL NOT authorize signalling a guessed process; a confirmed eligible enclosing-container close MAY still remove its verified resource with unresolved execution outcomes retained.

#### Scenario: Ordinary unmanaged pane
- **WHEN** the operator confirms Close on an ordinary shell
- **THEN** its pane is removed without inventing a managed execution

#### Scenario: Unverified Pi pane
- **WHEN** a narrow agent Close cannot establish its exact execution association
- **THEN** it refuses to signal a guessed process; separately confirmed enclosing-container closure retains its broader removal intent

#### Scenario: Retained managed association
- **WHEN** a known managed association remains on an eligible retained pane
- **THEN** the common close resolves its execution state rather than using an independent mux-only bypass

#### Scenario: Incomplete managed discovery
- **WHEN** a confirmed container close cannot identify every hosted agent
- **THEN** it may remove the verified root without signalling guessed identities or claiming unverified process exits

### Requirement: Container close is all or nothing at admission
A confirmed container Close SHALL recursively handle its hosted managed executions and remove its panes through the common close implementation. Managed children SHALL NOT alone cause refusal. Graceful shutdown SHALL share a deadline and incomplete child handling SHALL NOT alone block bounded enclosing-resource removal; verified identity changes SHALL still fence destruction.

#### Scenario: Mixed tab
- **WHEN** a tab contains an ordinary shell and a managed agent
- **THEN** Close requests managed termination and removes the selected tab without requiring separate operator actions for each child

### Requirement: Managed operations are owner-routed
Radar SHALL route managed Close and restart to the daemon's common lifecycle implementation using exact subject/claim and placement identities. A spawn-parent relationship SHALL authorize parent requests, not select a separate termination implementation. Assignment and message-consumption authority SHALL remain with their existing owners.

#### Scenario: Idle managed worker restart
- **WHEN** an eligible managed worker is restarted
- **THEN** the daemon replaces its execution through the common lifecycle implementation

#### Scenario: Busy or parentless target
- **WHEN** a managed agent has no spawn parent or is busy but has eligible verified execution and resume context
- **THEN** those facts alone do not force an owner-file route or a second lifecycle implementation; normal restart admission and confirmation apply

#### Scenario: Close abandoned assignment
- **WHEN** Close will interrupt active work
- **THEN** confirmation names that loss and closure does not settle the assignment

#### Scenario: Different entrypoints
- **WHEN** an authorized parent and the operator each request equivalent managed closure through their respective interfaces
- **THEN** both reach the same managed termination and pane-removal semantics after authorization

### Requirement: Lifecycle results do not replace observation
Lifecycle work SHALL NOT block input or collection. Confirmed resource removal SHALL be presented normally and rows SHALL reconcile from observation. Confirmed survivors or failed removal SHALL be actionable; unavailable exit code alone SHALL remain diagnostic. Uncertain outcomes SHALL remain readable without being presented as proof that no effect occurred.

#### Scenario: Owner control unavailable
- **WHEN** retired owner-file transport is absent but daemon lifecycle authority is available
- **THEN** managed close uses the daemon and does not require that retired transport

#### Scenario: Applied operation
- **WHEN** Close or restart returns a valid outcome
- **THEN** the TUI reconciles rows through subsequent observation, without fabricating topology

#### Scenario: Control-plane outcome unknown
- **WHEN** a request reply is lost after possible dispatch
- **THEN** the view reconciles the existing operation rather than submitting a fresh destructive request

#### Scenario: Missing code only
- **WHEN** a close verifies the requested removal but cannot capture an exit code
- **THEN** the TUI does not leave the branch pending or require another confirmation solely for that missing code

#### Scenario: Confirmed surviving agent
- **WHEN** a managed execution is verified to survive after its terminal was removed
- **THEN** the outcome exposes that actionable failure without changing it into a verified exit

### Requirement: A new workspace starts in a chosen project directory
Radar SHALL, on `A`, offer a fuzzy picker of project directories and create a workspace in the chosen one, named for the folder of the git root containing it, or for the directory's own folder when it is in no repository. Cancelling the picker SHALL create nothing. Radar SHALL NOT follow a later change of directory in the workspace.

#### Scenario: A project is chosen
- **WHEN** the user presses `A` and chooses a directory inside a git repository
- **THEN** a workspace is created in that directory and named for the repository's folder

#### Scenario: The picker is cancelled
- **WHEN** the user escapes from the picker
- **THEN** no workspace is created and the dashboard is drawn again

### Requirement: Project candidates come from a configured source
Radar SHALL offer the directories printed by `[tui] project_source` (default `zoxide query -l`) that are absolute, existing and accepted by `[tui] project_filter` (default: containing `.git`), each once, in the order the source printed them. The candidate list SHALL NOT depend on the working directory of the process that requested it. Radar SHALL NOT run anything the source prints.

#### Scenario: No source is installed
- **WHEN** the project source program is missing
- **THEN** the picker offers only the base directory and a one-line message names the failure

#### Scenario: Requested from different directories
- **WHEN** the picker is opened from two frontends whose working directories differ
- **THEN** both are offered the same candidates in the same order

#### Scenario: Source accepted nothing
- **WHEN** the source succeeds but every directory it prints is rejected by the filter
- **THEN** the picker offers the base directory and says that no listed project was accepted

#### Scenario: Base directory unusable
- **WHEN** the configured base directory is absent or not an existing directory
- **THEN** the picker offers no candidate and says so rather than substituting another directory

### Requirement: No accepted project directory
When the source yields no accepted directory, including when it is missing, fails or exceeds its time limit, Radar SHALL offer one configured base directory, defaulting to the user's home directory, and SHALL say why in one line.

#### Scenario: No accepted project directory
- **WHEN** the project source is missing, fails, times out or yields no accepted directory
- **THEN** Radar offers the configured base directory, defaulting to the home directory, and says why in one line

### Requirement: A workspace directory is checked by the runtime
The runtime SHALL refuse a workspace creation whose directory is relative or is not an existing directory before creating anything, and SHALL start a created tmux session in the directory it was given. A runtime that cannot start a workspace in a directory SHALL refuse it.

#### Scenario: A missing directory
- **WHEN** a workspace creation names a directory that does not exist
- **THEN** it is refused and no session is created

### Requirement: L launches the configured managed-agent command
`L` SHALL create dedicated placement in the selected eligible workspace and run `[agents].launch_command` through common managed admission and supervision. It SHALL NOT leave the program as an untracked container recipe. Existing target-shell startup, directory and direnv behaviour SHALL remain; adding a launch SHALL require no destructive confirmation.

#### Scenario: Launch the default agent
- **WHEN** the operator presses `L` on an eligible workspace with the default launch command
- **THEN** a new tab starts the default agent with verified placement, scope, claim and completion reporting

#### Scenario: Launch from a pane
- **WHEN** the operator presses `L` on an eligible pane
- **THEN** its new tab receives managed supervision and target-shell context while the source pane is left untouched

#### Scenario: Scope fails during a TUI launch
- **WHEN** the scope cannot be established for an `L` launch
- **THEN** the command does not run, its reserved claim is withdrawn, and Radar reports the refusal rather than starting it unscoped

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
The `a` action SHALL create an empty shell in a new tab of the selected row's workspace, starting in that workspace's directory, for arbitrary interactive commands. It SHALL create a tab whichever row is selected, never a split, and SHALL ask for no confirmation, since an addition loses no work. It SHALL NOT require those commands to be configured as managed-agent launches.

#### Scenario: Run an arbitrary command
- **WHEN** the user wants an interactive command instead of the configured managed agent
- **THEN** the user can create an empty shell with `a` and run the command there

#### Scenario: Adding from a pane row
- **WHEN** the user presses `a` with a pane row selected
- **THEN** a new tab is created in that pane's workspace, in the workspace directory, with no confirmation

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

### Requirement: Externally started agents are observed, not promoted
Radar SHALL show an agent started outside its managed launch path as observed: its facts, session identity, children and inbox identity remain visible and durable. Radar SHALL NOT offer it execution controls that require a managed launch, and SHALL NOT imply its resource grouping, startup admission or exit result are tracked.

#### Scenario: Observed agent selected
- **WHEN** the operator selects an agent Radar did not launch
- **THEN** Radar shows its facts and relationships and states that it is not a managed execution

#### Scenario: Transition through restart
- **WHEN** the operator chooses to place an observed agent under Radar's management
- **THEN** Radar confirms a restart, verifies the previous process has exited, and starts its replacement through the managed launch path

### Requirement: Lifecycle controls name the claim they were issued against
Every lifecycle control that affects a process SHALL name the claim that was current when the operation was requested. Radar SHALL refuse the control when that claim is no longer the subject's current claim, and SHALL NOT retarget a response or a signal onto a successor attempt.

#### Scenario: Stale control after a restart
- **WHEN** an operation naming an earlier claim reaches the daemon after the subject's successor became current
- **THEN** it is refused as stale and no signal targets the successor

#### Scenario: Replayed control
- **WHEN** a control is replayed under its own request id for the claim it named
- **THEN** the recorded response is returned and no second effect occurs

### Requirement: Restart pre-mints its successor before stopping
A managed restart SHALL pre-mint the successor claim and record it as the subject's current claim before it signals the previous process, so the subject is never without a current attempt between the stop and the replacement starting. A restart SHALL refuse before signalling when it cannot pre-mint.

#### Scenario: Continuity during restart
- **WHEN** a managed restart is dispatched
- **THEN** the successor claim exists before the previous process is signalled, and a competing launch cannot claim that subject in the gap

#### Scenario: Successor cannot be admitted
- **WHEN** the successor claim cannot be made durable
- **THEN** no signal is sent and the previous process is left as it was
